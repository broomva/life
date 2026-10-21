//! Checkout: materialise a tree into a directory, cloning where the disk can.
//!
//! Everything is written relative to one descriptor for the destination, opened without
//! following a link, so no path above it is resolved again. The destination must belong
//! to the caller and be writable by no one else, and every directory the checkout makes is
//! `0755`, so no other user can put a link inside it while the checkout runs. Content is
//! cloned from an open source file, never from a path, so what is cloned is the file whose
//! stat was checked.

use crate::cache::{Stamp, StatCache};
use crate::error::{FsError, Result};
use crate::scan::MAX_FILE_BYTES;
use cuenca_tree::{Blob, Entry, GitOid, Mode, RepoPath, Tree, TreeError, check_content};
use rayon::prelude::*;
use rustix::fs::{AtFlags, CWD, Dir, OFlags, fstat, mkdirat, openat, symlinkat, unlinkat};
use rustix::io::Errno;
use std::collections::{BTreeSet, HashMap};
use std::ffi::{CString, OsStr};
use std::fs::{self, File};
use std::io::{self, Read, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How far before the checkout started the files it writes are dated, so that every
/// one of them is settled (see [`crate::RACY_MARGIN`]) and the checkout's first snapshot
/// reads nothing.
const BACKDATE: Duration = Duration::from_secs(10);

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
    /// An open regular file holding exactly the blob, which a checkout clones (or copies,
    /// where the disk cannot clone). With a `stamp`, the source vouches for the content by
    /// that stat: the checkout re-checks it after cloning, fails if it moved, and lets its
    /// new stat cache vouch for the copy. Without one, the content is taken on trust and
    /// the copy is left out of the new cache, so the next snapshot reads it.
    File { file: File, stamp: Option<Stamp> },
    /// The content in memory. A checkout hashes it before writing it.
    Bytes(Vec<u8>),
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

/// A directory laid out as the authority's blob store: `<dir>/<2 hex>/<38 hex>`, each
/// file the blob with that git id as the store verified it at ingest. Its files are not
/// re-hashed here, so a checkout from it does not vouch for them in its cache.
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
        Ok(Some(Fetched::File { file, stamp: None }))
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
                return Ok(Some(Fetched::File {
                    file,
                    stamp: Some(*stamp),
                }));
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
    /// The new checkout's stat cache. It vouches for every file whose content was
    /// verified (hashed bytes, or a clone of a file the source vouched for), so the
    /// checkout's first snapshot reads only the rest.
    pub cache: StatCache,
    /// Files cloned copy-on-write.
    pub cloned: usize,
    /// Files copied, because the disk could not clone them or the content came in memory.
    pub copied: usize,
}

/// Materialise `tree` into `dest`: a missing directory, which is created, or an empty one
/// that belongs to the caller and that neither group nor others can write. Files are
/// cloned from `source` where the disk supports it (APFS `clonefile`, a Linux reflink)
/// and copied otherwise; modes are git's (`0644`, `0755`), whatever the source's are.
/// Symlinks are created last. On error the destination is left partly written and should
/// be discarded.
pub fn checkout(tree: &Tree, dest: &Path, source: &dyn BlobSource) -> Result<Checkout> {
    let started = SystemTime::now();
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
        mkdirat(&root, dir, dir_mode()).map_err(errno(dest.join(dir)))?;
    }
    let mut files = Vec::new();
    let mut links = Vec::new();
    for (path, entry) in tree.entries() {
        match entry {
            Entry::File(_) | Entry::Executable(_) => files.push((path, *entry)),
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
    let written: Vec<Written> = pool.install(|| {
        files
            .par_iter()
            .map(|(path, entry)| write_file(&root, dest, path, *entry, source, started))
            .collect::<Result<_>>()
    })?;
    for (path, oid) in links {
        let abs = dest.join(path.as_str());
        let target = match source.fetch(oid).map_err(FsError::io(&abs))? {
            None => return Err(FsError::MissingBlob(oid)),
            Some(Fetched::Bytes(bytes)) => bytes,
            Some(Fetched::File { file, .. }) => {
                let mut bytes = Vec::new();
                file.take(MAX_FILE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(FsError::io(&abs))?;
                bytes
            }
        };
        verify(oid, &target)?;
        check_content(path, Mode::Symlink, &target)?;
        symlinkat(OsStr::from_bytes(&target), &root, path.as_str()).map_err(errno(abs))?;
    }
    let now = SystemTime::now();
    let mut cache = StatCache::new();
    let (mut cloned, mut copied) = (0, 0);
    for w in written {
        if w.cloned {
            cloned += 1;
        } else {
            copied += 1;
        }
        if w.vouched {
            cache.record(w.path, w.stamp, w.entry, now);
        }
    }
    Ok(Checkout {
        cache,
        cloned,
        copied,
    })
}

struct Written {
    path: String,
    stamp: Stamp,
    entry: Entry,
    cloned: bool,
    vouched: bool,
}

fn dir_mode() -> rustix::fs::Mode {
    use rustix::fs::Mode as M;
    M::RWXU | M::RGRP | M::XGRP | M::ROTH | M::XOTH
}

fn errno(path: PathBuf) -> impl Fn(Errno) -> FsError {
    move |e| FsError::io(path.clone())(e.into())
}

/// The destination, open, after checking it is a directory of the caller's own that no
/// one else can write and that holds nothing.
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
    let st = fstat(&fd).map_err(errno(dest.into()))?;
    if st.st_uid != rustix::process::geteuid().as_raw() {
        return Err(refuse("it belongs to another user"));
    }
    #[allow(clippy::unnecessary_cast)] // the field type differs between platforms
    if st.st_mode as u32 & 0o022 != 0 {
        return Err(refuse("group or others can write it"));
    }
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

/// Write one regular file under `root`. A clone carries the source's mode and
/// modification time, so the mode is set and the file backdated only where they are not
/// already right.
fn write_file(
    root: &OwnedFd,
    dest: &Path,
    path: &RepoPath,
    entry: Entry,
    source: &dyn BlobSource,
    started: SystemTime,
) -> Result<Written> {
    let abs = dest.join(path.as_str());
    let name = CString::new(path.as_str()).expect("a repository path has no NUL");
    let oid = entry.oid();
    let (cloned, vouched) = match source.fetch(oid).map_err(FsError::io(&abs))? {
        None => return Err(FsError::MissingBlob(oid)),
        Some(Fetched::File { file, stamp }) => {
            let cloned = clone_or_copy(&file, root, &name).map_err(FsError::io(&abs))?;
            if let Some(stamp) = stamp {
                let now = Stamp::of(&file.metadata().map_err(FsError::io(&abs))?);
                if now != stamp {
                    unlinkat(root, &*name, AtFlags::empty()).map_err(errno(abs.clone()))?;
                    return Err(FsError::SourceChanged(abs));
                }
            }
            (cloned, stamp.is_some())
        }
        Some(Fetched::Bytes(bytes)) => {
            verify(oid, &bytes)?;
            let mut file = create(root, &name).map_err(FsError::io(&abs))?;
            file.write_all(&bytes).map_err(FsError::io(&abs))?;
            (false, true)
        }
    };
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file = File::from(
        openat(root, &*name, flags, rustix::fs::Mode::empty()).map_err(errno(abs.clone()))?,
    );
    let mode = if matches!(entry, Entry::Executable(_)) {
        0o755
    } else {
        0o644
    };
    let mut stamp = Stamp::of(&file.metadata().map_err(FsError::io(&abs))?);
    let mut touched = false;
    if stamp.permissions() != mode {
        file.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(FsError::io(&abs))?;
        touched = true;
    }
    if !stamp.settled(started) {
        let dated = started
            .checked_sub(BACKDATE)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        file.set_modified(dated).map_err(FsError::io(&abs))?;
        touched = true;
    }
    if touched {
        stamp = Stamp::of(&file.metadata().map_err(FsError::io(&abs))?);
    }
    Ok(Written {
        path: path.to_string(),
        stamp,
        entry,
        cloned,
        vouched,
    })
}

/// A new, empty file `name` under `root`, readable and writable by the owner only until
/// its mode is set.
fn create(root: &OwnedFd, name: &CString) -> io::Result<File> {
    use rustix::fs::Mode as M;
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    Ok(File::from(openat(root, &**name, flags, M::RUSR | M::WUSR)?))
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn copy(src: &File, root: &OwnedFd, name: &CString) -> io::Result<bool> {
    let mut dst = create(root, name)?;
    io::copy(&mut &*src, &mut dst)?;
    Ok(false)
}

/// Clone `src` to `name` under `root`; true if cloned, false if the disk cannot clone and
/// the content was copied.
#[cfg(target_os = "macos")]
fn clone_or_copy(src: &File, root: &OwnedFd, name: &CString) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: both descriptors are open for the duration of the call and `name` is a
    // NUL-terminated string that outlives it; `fclonefileat` reads only these.
    let rc = unsafe { libc::fclonefileat(src.as_raw_fd(), root.as_raw_fd(), name.as_ptr(), 0) };
    if rc == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ENOTSUP | libc::EXDEV | libc::ENOSYS) => copy(src, root, name),
        _ => Err(err),
    }
}

#[cfg(target_os = "linux")]
fn clone_or_copy(src: &File, root: &OwnedFd, name: &CString) -> io::Result<bool> {
    let mut dst = create(root, name)?;
    match rustix::fs::ioctl_ficlone(&dst, src) {
        Ok(()) => Ok(true),
        Err(Errno::OPNOTSUPP | Errno::XDEV | Errno::INVAL | Errno::NOTTY | Errno::BADF) => {
            io::copy(&mut &*src, &mut dst)?;
            Ok(false)
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clone_or_copy(src: &File, root: &OwnedFd, name: &CString) -> io::Result<bool> {
    copy(src, root, name)
}
