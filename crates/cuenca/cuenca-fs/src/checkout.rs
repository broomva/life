//! Checkout: materialise a tree into a directory, cloning where the disk can.
//!
//! Nothing is reached by a path that could be re-resolved. The destination is opened once
//! without following a link, every directory under it is opened one component at a time
//! from its parent without following a link, and each file is created by its single name
//! in its directory. So a link that appears inside the destination while the checkout runs
//! fails the checkout instead of leading it out, whatever the destination's permissions or
//! ACLs allow others. Content is cloned from an open source file, never from a path, so
//! what is cloned is the file whose stat was checked.

use crate::cache::{Stamp, StatCache};
use crate::error::{FsError, Result};
use crate::scan::MAX_FILE_BYTES;
use cuenca_tree::{
    Blob, Entry, GitOid, MAX_GITATTRIBUTES_BYTES, MAX_SYMLINK_TARGET, Mode, RepoPath, Tree,
    TreeError, check_content, has_content_rule,
};
use rayon::prelude::*;
use rustix::fs::{AtFlags, CWD, Dir, OFlags, mkdirat, openat, statat, symlinkat, unlinkat};
use rustix::io::Errno;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{CString, OsStr};
use std::fs::{self, File};
use std::io::{self, Read, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// How many files a checkout writes at once. Clones on one APFS volume contend: on a
/// 10,844-file tree, 1 to 4 threads took 0.9 to 1.0 s and 14 threads 2.8 s (a volume
/// holding earlier forks; 0.48 s with 4 threads on a fresh one).
const WRITERS: usize = 4;

/// Where blob content comes from during a checkout.
pub trait BlobSource: Sync {
    /// The content of `oid`, or `None` if this source does not have it.
    fn fetch(&self, oid: GitOid) -> io::Result<Option<Fetched>>;
}

/// A blob's content, as a source hands it over.
#[derive(Debug)]
pub enum Fetched {
    /// An open regular file holding the blob, which a checkout hashes and then clones (or
    /// copies, where the disk cannot clone), failing if the file changed in between.
    File(File),
    /// The content in memory. A checkout hashes it before writing it.
    Bytes(Vec<u8>),
    /// A file a checkout's own stat cache vouches for, cloned without hashing. Only this
    /// crate makes one ([`CheckoutSource`]): another source cannot vouch for content.
    Vouched(Vouched),
}

/// An open file, the blob a [`StatCache`] recorded it as, and the stat it was recorded
/// under. A checkout takes it only for that blob.
#[derive(Debug)]
pub struct Vouched {
    oid: GitOid,
    file: File,
    stamp: Stamp,
}

impl<T: BlobSource + ?Sized> BlobSource for &T {
    fn fetch(&self, oid: GitOid) -> io::Result<Option<Fetched>> {
        (**self).fetch(oid)
    }
}

/// The first source that has the blob.
impl<A: BlobSource, B: BlobSource> BlobSource for (A, B) {
    fn fetch(&self, oid: GitOid) -> io::Result<Option<Fetched>> {
        match self.0.fetch(oid)? {
            Some(found) => Ok(Some(found)),
            None => self.1.fetch(oid),
        }
    }
}

/// A directory laid out as the authority's blob store: `<dir>/<2 hex>/<38 hex>`. Its
/// files are offered without a stamp, so a checkout hashes each one before cloning it: a
/// damaged object fails the checkout instead of reaching a checkout.
pub struct ObjectDir {
    dir: PathBuf,
}

impl ObjectDir {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

impl BlobSource for ObjectDir {
    fn fetch(&self, oid: GitOid) -> io::Result<Option<Fetched>> {
        let hex = oid.to_hex();
        let Some(file) = open_regular(&self.dir.join(&hex[..2]).join(&hex[2..]))? else {
            return Ok(None);
        };
        Ok(Some(Fetched::File(file)))
    }
}

/// An existing checkout of `tree`, with its stat cache. A file whose stat is still the
/// one the cache recorded for its content is offered for cloning; anything else of the
/// tree (a file written too recently to be cached, a symlink) is read here and handed over
/// only if it hashes to the blob asked for.
pub struct CheckoutSource {
    root: PathBuf,
    by_oid: HashMap<GitOid, Vec<(String, Entry)>>,
    stamps: HashMap<String, Stamp>,
}

impl CheckoutSource {
    pub fn new(root: impl Into<PathBuf>, tree: &Tree, cache: &StatCache) -> Self {
        let mut by_oid: HashMap<GitOid, Vec<(String, Entry)>> = HashMap::new();
        for (path, entry) in tree.entries() {
            by_oid
                .entry(entry.oid())
                .or_default()
                .push((path.to_string(), *entry));
        }
        let stamps = cache
            .iter()
            .filter(|(path, cached)| tree.get(path) == Some(&cached.entry))
            .map(|(path, cached)| (path.clone(), cached.stamp))
            .collect();
        Self {
            root: root.into(),
            by_oid,
            stamps,
        }
    }
}

impl BlobSource for CheckoutSource {
    fn fetch(&self, oid: GitOid) -> io::Result<Option<Fetched>> {
        let Some(paths) = self.by_oid.get(&oid) else {
            return Ok(None);
        };
        for (path, _) in paths {
            let Some(stamp) = self.stamps.get(path) else {
                continue;
            };
            if let Some(file) = open_regular(&self.root.join(path))?
                && Stamp::of(&file.metadata()?) == *stamp
            {
                return Ok(Some(Fetched::Vouched(Vouched {
                    oid,
                    file,
                    stamp: *stamp,
                })));
            }
        }
        for (path, entry) in paths {
            let abs = self.root.join(path);
            let content = match entry {
                Entry::Symlink(_) => match fs::read_link(&abs) {
                    Ok(target) => target.into_os_string().into_vec(),
                    Err(e) if gone(&e) => continue,
                    Err(e) => return Err(e),
                },
                _ => match open_regular(&abs)? {
                    Some(file) => {
                        let mut bytes = Vec::new();
                        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
                        bytes
                    }
                    None => continue,
                },
            };
            if Blob::of(&content).map(|b| b.oid()) == Some(oid) {
                return Ok(Some(Fetched::Bytes(content)));
            }
        }
        Ok(None)
    }
}

/// Whether an error means the path no longer holds what the source expected there.
fn gone(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
    ) || e.raw_os_error() == Some(libc::ELOOP)
}

/// Open `path` if it is a regular file, without following a link at the end.
fn open_regular(path: &Path) -> io::Result<Option<File>> {
    match File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) if file.metadata()?.is_file() => Ok(Some(file)),
        Ok(_) => Ok(None),
        Err(e) if gone(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// What a checkout wrote.
#[derive(Debug)]
pub struct Checkout {
    /// Files cloned copy-on-write.
    pub cloned: usize,
    /// Files copied, because the disk could not clone them or the content came in memory.
    pub copied: usize,
}

/// Materialise `tree` into `dest`: a missing directory, which is created, or an empty
/// one. `dest` is the caller's path and is resolved as given (on macOS `/tmp` is itself a
/// link); everything under it is reached without following a link. Others who can write
/// `dest` can still put content in it while the checkout runs, which its first snapshot
/// sees (a checkout leaves no stat cache, so that snapshot reads every file), but they
/// cannot redirect a write outside it. Files are
/// cloned from `source` where the disk supports it (APFS `clonefile`, a Linux reflink)
/// and copied otherwise; modes are git's (`0644`, `0755`), whatever the source's are.
/// Symlinks are created last. On error the destination is left partly written and should
/// be discarded.
pub fn checkout(tree: &Tree, dest: &Path, source: &dyn BlobSource) -> Result<Checkout> {
    let root = prepare(dest)?;
    let mut dirs = BTreeSet::new();
    for (path, _) in tree.entries() {
        let mut end = 0;
        let s = path.as_str();
        while let Some(i) = s[end..].find('/') {
            end += i;
            dirs.insert(&s[..end]);
            end += 1;
        }
    }
    // Sorted, so a parent comes before its children.
    for dir in dirs {
        let (parent, name) = split(dir);
        let at = open_under(&root, dest, parent)?;
        mkdirat(&at, name, dir_mode()).map_err(errno(dest.join(dir)))?;
    }
    // Grouped by directory, so each worker opens a directory once for all its files.
    let mut files: BTreeMap<&str, Vec<(&RepoPath, Entry)>> = BTreeMap::new();
    let mut links = Vec::new();
    for (path, entry) in tree.entries() {
        match entry {
            Entry::File(_) | Entry::Executable(_) => {
                files
                    .entry(split(path.as_str()).0)
                    .or_default()
                    .push((path, *entry));
            }
            Entry::Symlink(oid) => links.push((path, *oid)),
            other => {
                return Err(TreeError::UnsupportedMode(other.mode().as_str().into()).into());
            }
        }
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(WRITERS)
        .build()
        .map_err(|e| FsError::io(dest)(io::Error::other(e)))?;
    let written: Vec<Vec<Written>> = pool.install(|| {
        files
            .par_iter()
            .map(|(dir, entries)| {
                let at = open_under(&root, dest, dir)?;
                entries
                    .iter()
                    .map(|(path, entry)| {
                        let (cloned, id) = write_file(&at, dest, path, *entry, source)?;
                        Ok((*path, cloned, id))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<_>>()
    })?;
    let mut link_ids = Vec::new();
    for (path, oid) in links {
        let abs = dest.join(path.as_str());
        let target = match source.fetch(oid).map_err(FsError::io(&abs))? {
            None => return Err(FsError::MissingBlob(oid)),
            Some(Fetched::Bytes(bytes)) => bytes,
            Some(Fetched::File(file) | Fetched::Vouched(Vouched { file, .. })) => {
                // A target longer than any checkout can hold is refused unread.
                let mut bytes = Vec::new();
                file.take(MAX_SYMLINK_TARGET as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(FsError::io(&abs))?;
                bytes
            }
        };
        check_content(path, Mode::Symlink, &target)?;
        verify(oid, &target)?;
        let (parent, name) = split(path.as_str());
        let at = open_under(&root, dest, parent)?;
        symlinkat(OsStr::from_bytes(&target), &at, name).map_err(errno(abs.clone()))?;
        let st = statat(&at, name, AtFlags::SYMLINK_NOFOLLOW).map_err(errno(abs))?;
        link_ids.push((path, dev_ino(&st)));
    }
    // Every name still holds what this checkout put there, or the checkout fails: a file
    // another writer swapped in meanwhile is reported, not passed off as the tree's.
    let file_ids = written.iter().flatten().map(|(path, _, id)| (*path, *id));
    verify_written(&root, dest, file_ids.chain(link_ids))?;
    let cloned = written.iter().flatten().filter(|(_, c, _)| *c).count();
    let copied = written.iter().flatten().count() - cloned;
    Ok(Checkout { cloned, copied })
}

#[allow(clippy::unnecessary_cast)] // the field types differ between platforms
fn dev_ino(st: &rustix::fs::Stat) -> (u64, u64) {
    (st.st_dev as u64, st.st_ino as u64)
}

/// Check that each path names the inode the checkout wrote there.
fn verify_written<'a>(
    root: &OwnedFd,
    dest: &Path,
    written: impl Iterator<Item = (&'a RepoPath, (u64, u64))>,
) -> Result<()> {
    type Names<'a> = Vec<(&'a str, (u64, u64))>;
    let mut by_dir: BTreeMap<&str, Names> = BTreeMap::new();
    for (path, id) in written {
        let (dir, name) = split(path.as_str());
        by_dir.entry(dir).or_default().push((name, id));
    }
    for (dir, names) in by_dir {
        let at = open_under(root, dest, dir)?;
        for (name, id) in names {
            let now = match statat(&at, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(st) => Some(dev_ino(&st)),
                Err(Errno::NOENT) => None,
                Err(e) => return Err(FsError::io(dest.join(dir).join(name))(e.into())),
            };
            if now != Some(id) {
                return Err(FsError::Destination {
                    path: dest.join(dir).join(name),
                    why: "another file was put in place of one the checkout wrote",
                });
            }
        }
    }
    Ok(())
}

/// A file written: its path, whether it was cloned, and its device and inode.
type Written<'a> = (&'a RepoPath, bool, (u64, u64));

/// `a/b/c` → (`a/b`, `c`); `c` → (``, `c`).
fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

/// The directory `rel` under `root`, opened one component at a time without following a
/// link, so a link put anywhere on the way fails the open.
fn open_under(root: &OwnedFd, dest: &Path, rel: &str) -> Result<OwnedFd> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut at = openat(root, ".", flags, rustix::fs::Mode::empty()).map_err(errno(dest.into()))?;
    if rel.is_empty() {
        return Ok(at);
    }
    for name in rel.split('/') {
        at = match openat(&at, name, flags, rustix::fs::Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::LOOP | Errno::NOTDIR) => {
                return Err(FsError::Destination {
                    path: dest.join(rel),
                    why: "a symlink appeared inside it during the checkout",
                });
            }
            Err(e) => return Err(FsError::io(dest.join(rel))(e.into())),
        };
    }
    Ok(at)
}

fn dir_mode() -> rustix::fs::Mode {
    use rustix::fs::Mode as M;
    M::RWXU | M::RGRP | M::XGRP | M::ROTH | M::XOTH
}

fn errno(path: PathBuf) -> impl Fn(Errno) -> FsError {
    move |e| FsError::io(path.clone())(e.into())
}

/// The destination, open, after checking it is a directory, not a link, that holds
/// nothing.
fn prepare(dest: &Path) -> Result<OwnedFd> {
    let refuse = |why| FsError::Destination {
        path: dest.into(),
        why,
    };
    match fs::symlink_metadata(dest) {
        // A symlink is refused by the open below, which does not follow one.
        Ok(meta) if !meta.is_dir() && !meta.file_type().is_symlink() => {
            return Err(refuse("it is not a directory"));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(dest)
            .map_err(FsError::io(dest))?,
        Err(e) => return Err(FsError::io(dest)(e)),
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let fd = match openat(CWD, dest, flags, rustix::fs::Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::LOOP | Errno::NOTDIR) => return Err(refuse("it is a symlink")),
        Err(e) => return Err(FsError::io(dest)(e.into())),
    };
    for item in Dir::read_from(&fd).map_err(errno(dest.into()))? {
        let item = item.map_err(errno(dest.into()))?;
        if !matches!(item.file_name().to_bytes(), b"." | b"..") {
            return Err(refuse("it is not empty"));
        }
    }
    Ok(fd)
}

fn verify(oid: GitOid, content: &[u8]) -> Result<()> {
    let actual = Blob::of(content)
        .map(|b| b.oid())
        .unwrap_or_else(|| GitOid::for_blob(content));
    if actual != oid {
        return Err(FsError::BlobMismatch {
            expected: oid,
            actual,
        });
    }
    Ok(())
}

/// Write one regular file by its name in its directory `root`; true if it was cloned.
/// The mode is set through a descriptor for the file this call created, never by name.
fn write_file(
    root: &OwnedFd,
    dest: &Path,
    path: &RepoPath,
    entry: Entry,
    source: &dyn BlobSource,
) -> Result<(bool, (u64, u64))> {
    let abs = dest.join(path.as_str());
    let name = CString::new(path.file_name()).expect("a repository path has no NUL");
    let oid = entry.oid();
    let (cloned, written) = match source.fetch(oid).map_err(FsError::io(&abs))? {
        None => return Err(FsError::MissingBlob(oid)),
        Some(fetched @ (Fetched::File(_) | Fetched::Vouched(_))) => {
            let (file, stamp) = match fetched {
                Fetched::Vouched(v) if v.oid == oid => (v.file, v.stamp),
                Fetched::Vouched(v) => {
                    return Err(FsError::BlobMismatch {
                        expected: oid,
                        actual: v.oid,
                    });
                }
                Fetched::File(file) => {
                    let stamp = verify_file(oid, &file, &abs)?;
                    (file, stamp)
                }
                Fetched::Bytes(_) => unreachable!("matched above"),
            };
            if has_content_rule(path, entry.mode()) {
                check_content(path, entry.mode(), &read_ruled(&file, &abs)?)?;
            }
            let (cloned, written) = clone_or_copy(&file, root, &name).map_err(FsError::io(&abs))?;
            let now = Stamp::of(&file.metadata().map_err(FsError::io(&abs))?);
            if now != stamp {
                unlinkat(root, &*name, AtFlags::empty()).map_err(errno(abs.clone()))?;
                return Err(FsError::SourceChanged(abs));
            }
            (cloned, written)
        }
        Some(Fetched::Bytes(bytes)) => {
            verify(oid, &bytes)?;
            check_content(path, entry.mode(), &bytes)?;
            let mut file = create(root, &name).map_err(FsError::io(&abs))?;
            file.write_all(&bytes).map_err(FsError::io(&abs))?;
            (false, Some(file))
        }
    };
    let file = match written {
        Some(file) => file,
        None => reopen_alone(root, &name, &abs)?,
    };
    let mode = if matches!(entry, Entry::Executable(_)) {
        0o755
    } else {
        0o644
    };
    let meta = file.metadata().map_err(FsError::io(&abs))?;
    if Stamp::of(&meta).permissions() != mode {
        file.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(FsError::io(&abs))?;
    }
    Ok((cloned, (meta.dev(), meta.ino())))
}

/// A file a clone just created by `name` (macOS gives back no descriptor), opened again
/// and refused unless it has no other name: a hard link put in its place, to a file
/// outside the destination, is not ours to change.
fn reopen_alone(root: &OwnedFd, name: &CString, abs: &Path) -> Result<File> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file = File::from(
        openat(root, &**name, flags, rustix::fs::Mode::empty()).map_err(errno(abs.into()))?,
    );
    let meta = file.metadata().map_err(FsError::io(abs))?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(FsError::Destination {
            path: abs.into(),
            why: "another file was put in its place during the checkout",
        });
    }
    Ok(file)
}

/// The content of a file a content rule applies to (`.gitattributes`), read up to one
/// byte past the largest such file git accepts.
fn read_ruled(file: &File, abs: &Path) -> Result<Vec<u8>> {
    let limit = MAX_GITATTRIBUTES_BYTES as u64 + 1;
    let mut bytes = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    while (bytes.len() as u64) < limit {
        match file.read_at(&mut buf, bytes.len() as u64) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FsError::io(abs)(e)),
        }
    }
    Ok(bytes)
}

/// Hash an open file that came without a stamp and return the stamp it was hashed under,
/// or fail if it is not the blob `oid`.
fn verify_file(oid: GitOid, file: &File, abs: &Path) -> Result<Stamp> {
    let stamp = Stamp::of(&file.metadata().map_err(FsError::io(abs))?);
    let mut hasher = cuenca_tree::BlobHasher::new(stamp.size());
    let mut buf = vec![0u8; 64 * 1024];
    let mut at = 0u64;
    // Positional reads: the file's shared offset is never relied on.
    while at <= stamp.size() {
        match file.read_at(&mut buf, at) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buf[..n]);
                at += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FsError::io(abs)(e)),
        }
    }
    let actual = match hasher.finish() {
        Ok(blob) => blob.oid(),
        Err(cuenca_tree::HashFailure::Collision) => return Err(FsError::Collision(abs.into())),
        // Not the length its stat gave: it changed while it was read.
        Err(cuenca_tree::HashFailure::WrongLength { .. }) => {
            return Err(FsError::SourceChanged(abs.into()));
        }
    };
    if actual != oid {
        return Err(FsError::BlobMismatch {
            expected: oid,
            actual,
        });
    }
    Ok(stamp)
}

/// A new, empty file `name` under `root`, readable and writable by the owner only until
/// its mode is set.
fn create(root: &OwnedFd, name: &CString) -> io::Result<File> {
    use rustix::fs::Mode as M;
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    Ok(File::from(openat(root, &**name, flags, M::RUSR | M::WUSR)?))
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn copy(src: &File, root: &OwnedFd, name: &CString) -> io::Result<(bool, Option<File>)> {
    let mut dst = create(root, name)?;
    copy_from_start(src, &mut dst)?;
    Ok((false, Some(dst)))
}

/// Copy all of `src` with positional reads, whatever its shared offset.
fn copy_from_start(src: &File, dst: &mut File) -> io::Result<u64> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut at = 0u64;
    loop {
        match src.read_at(&mut buf, at) {
            Ok(0) => return Ok(at),
            Ok(n) => {
                dst.write_all(&buf[..n])?;
                at += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// Clone `src` to `name` under `root`: true if cloned, false if the disk cannot clone and
/// the content was copied, with the descriptor of the new file where there is one.
#[cfg(target_os = "macos")]
fn clone_or_copy(src: &File, root: &OwnedFd, name: &CString) -> io::Result<(bool, Option<File>)> {
    use std::os::fd::AsRawFd;
    // SAFETY: both descriptors are open for the duration of the call and `name` is a
    // NUL-terminated string that outlives it; `fclonefileat` reads only these.
    let rc = unsafe { libc::fclonefileat(src.as_raw_fd(), root.as_raw_fd(), name.as_ptr(), 0) };
    if rc == 0 {
        return Ok((true, None));
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ENOTSUP | libc::EXDEV | libc::ENOSYS) => copy(src, root, name),
        _ => Err(err),
    }
}

#[cfg(target_os = "linux")]
fn clone_or_copy(src: &File, root: &OwnedFd, name: &CString) -> io::Result<(bool, Option<File>)> {
    let mut dst = create(root, name)?;
    match rustix::fs::ioctl_ficlone(&dst, src) {
        Ok(()) => Ok((true, Some(dst))),
        Err(Errno::OPNOTSUPP | Errno::XDEV | Errno::INVAL | Errno::NOTTY | Errno::BADF) => {
            copy_from_start(src, &mut dst)?;
            Ok((false, Some(dst)))
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clone_or_copy(src: &File, root: &OwnedFd, name: &CString) -> io::Result<(bool, Option<File>)> {
    copy(src, root, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_no_longer_holds_what_was_written_fails_the_checkout() {
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("dest");
        fs::create_dir_all(dest.join("d")).unwrap();
        fs::write(dest.join("d/f"), b"ours\n").unwrap();
        let root = prepare_for_test(&dest);
        let path = RepoPath::new("d/f").unwrap();
        let id = |p: &Path| {
            let m = fs::symlink_metadata(p).unwrap();
            (m.dev(), m.ino())
        };
        let ours = id(&dest.join("d/f"));
        assert!(verify_written(&root, &dest, [(&path, ours)].into_iter()).is_ok());
        // Swapped for another file (made first, so it cannot reuse the inode number).
        fs::write(dest.join("d/g"), b"theirs\n").unwrap();
        fs::rename(dest.join("d/g"), dest.join("d/f")).unwrap();
        assert!(matches!(
            verify_written(&root, &dest, [(&path, ours)].into_iter()),
            Err(FsError::Destination { .. })
        ));
        fs::remove_file(dest.join("d/f")).unwrap();
        assert!(matches!(
            verify_written(&root, &dest, [(&path, ours)].into_iter()),
            Err(FsError::Destination { .. })
        ));
    }

    #[test]
    fn a_hard_link_put_in_a_clones_place_is_refused() {
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let secret = out.path().join("secret");
        fs::write(&secret, b"key\n").unwrap();
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&secret, dest.join("f")).unwrap();
        fs::write(dest.join("g"), b"ours\n").unwrap();
        let root = prepare_for_test(&dest);
        let name = CString::new("f").unwrap();
        assert!(matches!(
            reopen_alone(&root, &name, &dest.join("f")),
            Err(FsError::Destination { .. })
        ));
        let mode = fs::metadata(&secret).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the outside file is untouched");
        assert!(reopen_alone(&root, &CString::new("g").unwrap(), &dest.join("g")).is_ok());
    }

    #[test]
    fn a_vouched_file_that_changed_while_cloned_is_refused() {
        let out = tempfile::tempdir().unwrap();
        let src = out.path().join("src");
        let other = out.path().join("other");
        fs::write(&src, b"content\n").unwrap();
        fs::write(&other, b"other\n").unwrap();
        // The stamp the cache recorded is not the file's any more.
        struct Stale(PathBuf, Stamp);
        impl BlobSource for Stale {
            fn fetch(&self, _: GitOid) -> io::Result<Option<Fetched>> {
                Ok(Some(Fetched::Vouched(Vouched {
                    oid: GitOid::for_blob(b"content\n"),
                    file: File::open(&self.0)?,
                    stamp: self.1,
                })))
            }
        }
        let source = Stale(src, Stamp::of(&fs::metadata(&other).unwrap()));
        let tree =
            Tree::from_entries([(RepoPath::new("a.txt").unwrap(), Entry::file(b"content\n"))])
                .unwrap();
        let dest = out.path().join("fork");
        assert!(matches!(
            checkout(&tree, &dest, &source),
            Err(FsError::SourceChanged(_))
        ));
        assert!(!dest.join("a.txt").exists(), "the clone is removed");
    }

    #[test]
    fn hashing_and_copying_ignore_the_shared_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        fs::write(&path, b"0123456789").unwrap();
        let mut file = File::open(&path).unwrap();
        let mut skip = [0u8; 4];
        file.read_exact(&mut skip).unwrap();
        assert!(verify_file(GitOid::for_blob(b"0123456789"), &file, &path).is_ok());
        let mut out = File::create(dir.path().join("g")).unwrap();
        copy_from_start(&file, &mut out).unwrap();
        assert_eq!(fs::read(dir.path().join("g")).unwrap(), b"0123456789");
    }

    fn prepare_for_test(dest: &Path) -> OwnedFd {
        openat(
            CWD,
            dest,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .unwrap()
    }

    #[test]
    fn a_link_inside_the_destination_is_never_followed() {
        let out = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let dest = out.path().join("dest");
        fs::create_dir_all(dest.join("a")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dest.join("a/b")).unwrap();
        let root = openat(
            CWD,
            &dest,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .unwrap();
        assert!(open_under(&root, &dest, "a").is_ok());
        assert!(matches!(
            open_under(&root, &dest, "a/b"),
            Err(FsError::Destination { .. })
        ));
        assert!(matches!(
            open_under(&root, &dest, "a/b/c"),
            Err(FsError::Destination { .. })
        ));
    }
}
