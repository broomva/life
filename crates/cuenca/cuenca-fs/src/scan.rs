//! Snapshot: a full-tree scan of a directory into a tree and a changeset.
//!
//! The walk holds each directory open and reaches everything under it relative to that
//! descriptor (`openat`, `fstatat`, `readlinkat`), never by re-resolving a path. A file
//! is opened without following a link, and a directory only if it is the inode the listing
//! saw. So a directory swapped for a symlink while the scan runs cannot lead it outside
//! the root: the snapshot reports the path as unstable instead.

use crate::cache::{Stamp, StatCache};
use crate::error::{FsError, Result};
use crate::ignore::{FORKIGNORE, ForkIgnore, open_dir};
use cuenca_tree::{
    Blob, BlobHasher, Changeset, Entry, GitOid, HashFailure, Mode, PathError, RepoPath, Tree,
    check_content, has_content_rule,
};
use rayon::prelude::*;
use rustix::fs::{AtFlags, Dir, FileType, OFlags, fstat, openat, readlinkat, statat};
use rustix::io::Errno;
use std::collections::HashSet;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// The largest file a snapshot takes, as the authority's blob store accepts.
pub const MAX_FILE_BYTES: u64 = cuenca_tree::git::DEFAULT_MAX_BLOB_BYTES;

/// How many times a file that changes while it is read is read again before the
/// snapshot gives up on it.
const READ_ATTEMPTS: usize = 3;
const CHUNK: usize = 64 * 1024;
/// How many files to read are queued before they are read, in parallel. Each queued
/// file keeps its directory open, so this bounds the descriptors a scan holds.
const BATCH: usize = 64;

/// What one scan found.
#[derive(Debug)]
pub struct Snapshot {
    pub tree: Tree,
    /// From the base tree given to [`snapshot`] to [`Snapshot::tree`].
    pub changeset: Changeset,
    /// One per blob the tree names and the base does not, each once.
    pub new_blobs: Vec<NewBlob>,
    /// Entries left out that are not content (sockets, FIFOs, devices) or whose name git
    /// reserves, other than `.git` itself, which is always left out silently.
    pub skipped: Vec<Skipped>,
    /// How many files were read, as opposed to taken from the stat cache.
    pub hashed: usize,
}

impl Snapshot {
    /// Whether the directory holds exactly the base tree.
    pub fn is_clean(&self) -> bool {
        self.changeset.ops.is_empty()
    }
}

/// A blob the base tree does not have, and where its content is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBlob {
    pub oid: GitOid,
    pub size: u64,
    pub content: Content,
}

/// Where a new blob's content is. A file can change after the scan, so whoever uploads
/// it hashes what it sends (the authority's ingest does).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    File(PathBuf),
    /// A symlink's target.
    Inline(Vec<u8>),
}

/// An entry a snapshot left out, by its path relative to the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: String,
    pub reason: &'static str,
}

/// Snapshot `root` against `base`, with the rules of `root/.forkignore` (or the defaults
/// when there is none). The rules must be the `.forkignore` the snapshot holds: if the
/// file changes during the scan, the snapshot fails as unstable. On success `cache` holds
/// this scan's stats; on failure it is left as it was.
///
/// A path of `base` that the rules leave out is carried over unchanged: the snapshot did
/// not look at it, so it does not claim it changed.
pub fn snapshot(root: &Path, base: &Tree, cache: &mut StatCache) -> Result<Snapshot> {
    let dir = open_dir(root)?;
    let (rules, parsed) = ForkIgnore::load(&dir, root)?;
    let (snap, next) = scan(dir, root, base, cache, &rules)?;
    rules_held(&rules, parsed.as_deref(), &snap.tree, root)?;
    *cache = next;
    Ok(snap)
}

/// Whether the snapshot holds the `.forkignore` its rules were parsed from (`parsed`,
/// `None` for the defaults). If the file changed between parsing and the scan, the
/// snapshot followed one rule set and records another, so it is refused as unstable.
fn rules_held(rules: &ForkIgnore, parsed: Option<&[u8]>, tree: &Tree, root: &Path) -> Result<()> {
    if rules.excludes(FORKIGNORE) {
        return Ok(());
    }
    let held = tree.get(FORKIGNORE).map(Entry::oid);
    if held != parsed.map(GitOid::for_blob) {
        return Err(FsError::Unstable(root.join(FORKIGNORE)));
    }
    Ok(())
}

/// [`snapshot`] with explicit ignore rules, whatever `root/.forkignore` says.
pub fn snapshot_with(
    root: &Path,
    base: &Tree,
    cache: &mut StatCache,
    ignore: &ForkIgnore,
) -> Result<Snapshot> {
    let (snap, next) = scan(open_dir(root)?, root, base, cache, ignore)?;
    *cache = next;
    Ok(snap)
}

struct Found {
    path: RepoPath,
    entry: Entry,
    /// The stat the entry was read under; `None` for a symlink, which is never cached,
    /// and for an entry carried over from the base.
    stamp: Option<Stamp>,
    /// `None` for an entry carried over from the base, whose blob the base has.
    content: Option<Content>,
}

/// A directory the walk listed and has still to open, from its parent's descriptor.
struct Pending {
    parent: Arc<OwnedFd>,
    name: CString,
    dev_ino: (u64, u64),
    prefix: String,
    rel: PathBuf,
    abs: PathBuf,
}

/// A file to read, by its directory's descriptor and its name in it.
struct ToRead {
    dir: Arc<OwnedFd>,
    name: CString,
    path: RepoPath,
    abs: PathBuf,
    stamp: Stamp,
}

fn scan(
    root_fd: OwnedFd,
    root: &Path,
    base: &Tree,
    cache: &StatCache,
    ignore: &ForkIgnore,
) -> Result<(Snapshot, StatCache)> {
    let started = SystemTime::now();
    let mut found = Vec::new();
    let mut skipped = Vec::new();
    let mut queue: Vec<ToRead> = Vec::new();
    let mut hashed = 0;
    // Depth first, so the directories held open are about one chain of ancestors.
    let mut pending: Vec<Pending> = Vec::new();
    let mut next_dir = Some((
        Arc::new(root_fd),
        String::new(),
        PathBuf::new(),
        root.to_path_buf(),
    ));
    loop {
        let (dir, prefix, rel_dir, abs_dir) = match next_dir.take() {
            Some(dir) => dir,
            None => match pending.pop() {
                Some(p) => {
                    let fd = open_child(&p)?;
                    (fd, p.prefix, p.rel, p.abs)
                }
                None => break,
            },
        };
        let listing = Dir::read_from(&*dir).map_err(errno(&abs_dir))?;
        for item in listing {
            let item = item.map_err(errno(&abs_dir))?;
            let raw = item.file_name();
            let bytes = raw.to_bytes();
            if bytes == b"." || bytes == b".." || bytes == b".git" {
                continue;
            }
            let os = OsStr::from_bytes(bytes);
            let abs = abs_dir.join(os);
            let rel = rel_dir.join(os);
            let st = match statat(&*dir, raw, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(st) => st,
                // Removed since the listing: not part of this snapshot.
                Err(Errno::NOENT) => continue,
                Err(e) => return Err(FsError::io(&abs)(e.into())),
            };
            let kind = FileType::from_raw_mode(st.st_mode);
            // The rules come first, so they can leave out a name no tree could hold.
            if ignore.is_ignored(&rel, kind == FileType::Directory) {
                continue;
            }
            let Some(name) = os.to_str() else {
                return Err(FsError::NotUtf8(abs));
            };
            let rel_str = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            };
            let path = match RepoPath::new(rel_str.as_str()) {
                Ok(path) => path,
                Err(PathError::GitDir(_)) => {
                    skipped.push(Skipped {
                        path: rel_str,
                        reason: "git reserves the name",
                    });
                    continue;
                }
                Err(source) => return Err(FsError::Name { path: abs, source }),
            };
            match kind {
                FileType::Directory => pending.push(Pending {
                    parent: Arc::clone(&dir),
                    name: raw.to_owned(),
                    dev_ino: dev_ino(&st),
                    prefix: rel_str,
                    rel,
                    abs,
                }),
                FileType::Symlink => {
                    let target = match readlinkat(&*dir, raw, Vec::new()) {
                        Ok(target) => target.into_bytes(),
                        // No longer a link.
                        Err(Errno::INVAL) => return Err(FsError::Unstable(abs)),
                        Err(e) => return Err(FsError::io(&abs)(e.into())),
                    };
                    check_content(&path, Mode::Symlink, &target)?;
                    let blob = Blob::of(&target).ok_or_else(|| FsError::Collision(abs.clone()))?;
                    found.push(Found {
                        path,
                        entry: Entry::Symlink(blob.oid()),
                        stamp: None,
                        content: Some(Content::Inline(target)),
                    });
                }
                FileType::RegularFile => {
                    let stamp = Stamp::from_stat(&st);
                    match cache.lookup(path.as_str(), &stamp) {
                        Some(entry) => found.push(Found {
                            path,
                            entry,
                            stamp: Some(stamp),
                            content: Some(Content::File(abs)),
                        }),
                        None => queue.push(ToRead {
                            dir: Arc::clone(&dir),
                            name: raw.to_owned(),
                            path,
                            abs,
                            stamp,
                        }),
                    }
                    if queue.len() >= BATCH {
                        hashed += read_all(&mut queue, &mut found)?;
                    }
                }
                _ => skipped.push(Skipped {
                    path: rel_str,
                    reason: "not a regular file or symlink",
                }),
            }
        }
    }
    hashed += read_all(&mut queue, &mut found)?;

    // What the rules leave out of the walk keeps its base entry.
    let walked: HashSet<&str> = found.iter().map(|f| f.path.as_str()).collect();
    let carried: Vec<Found> = base
        .entries()
        .filter(|(path, _)| ignore.excludes(path.as_str()) && !walked.contains(path.as_str()))
        .map(|(path, entry)| Found {
            path: path.clone(),
            entry: *entry,
            stamp: None,
            content: None,
        })
        .collect();
    found.extend(carried);

    let mut next = StatCache::new();
    for f in &found {
        if let Some(stamp) = f.stamp {
            next.record(f.path.to_string(), stamp, f.entry, started);
        }
    }
    let tree = Tree::from_entries(found.iter().map(|f| (f.path.clone(), f.entry)))?;
    let changeset = Changeset::between(base, &tree);
    let mut known: HashSet<GitOid> = base.entries().map(|(_, e)| e.oid()).collect();
    let mut new_blobs = Vec::new();
    for f in found {
        let oid = f.entry.oid();
        if !known.insert(oid) {
            continue;
        }
        let content = f
            .content
            .expect("only base entries are carried, and the base has their blobs");
        let size = match (&content, f.stamp) {
            (Content::Inline(bytes), _) => bytes.len() as u64,
            (Content::File(_), Some(stamp)) => stamp.size(),
            (Content::File(_), None) => unreachable!("every file carries its stamp"),
        };
        new_blobs.push(NewBlob { oid, size, content });
    }
    let snap = Snapshot {
        tree,
        changeset,
        new_blobs,
        skipped,
        hashed,
    };
    Ok((snap, next))
}

#[allow(clippy::unnecessary_cast)] // the field types differ between platforms
fn dev_ino(st: &rustix::fs::Stat) -> (u64, u64) {
    (st.st_dev as u64, st.st_ino as u64)
}

/// Open a directory the walk listed, refusing anything but that same directory: whatever
/// was put in its place, a link to another directory included, has another inode.
fn open_child(p: &Pending) -> Result<Arc<OwnedFd>> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let fd = match openat(&*p.parent, &p.name, flags, rustix::fs::Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::NOTDIR) => return Err(FsError::Unstable(p.abs.clone())),
        Err(e) => return Err(FsError::io(&p.abs)(e.into())),
    };
    if dev_ino(&fstat(&fd).map_err(errno(&p.abs))?) != p.dev_ino {
        return Err(FsError::Unstable(p.abs.clone()));
    }
    Ok(Arc::new(fd))
}

fn errno(path: &Path) -> impl Fn(Errno) -> FsError + '_ {
    move |e| FsError::io(path)(e.into())
}

/// Read every queued file, in parallel, into `found`; how many were read.
fn read_all(queue: &mut Vec<ToRead>, found: &mut Vec<Found>) -> Result<usize> {
    let n = queue.len();
    let read: Vec<Found> = std::mem::take(queue)
        .into_par_iter()
        .map(read_file)
        .collect::<Result<_>>()?;
    found.extend(read);
    Ok(n)
}

/// Read a regular file whose `fstatat` gave `stamp`, and accept what was read only if it
/// was opened without following a link, held the announced number of bytes, and the open
/// file's `fstat` is still `stamp` at the end. Otherwise read it again, up to
/// [`READ_ATTEMPTS`] times.
fn read_file(item: ToRead) -> Result<Found> {
    let ToRead {
        dir,
        name,
        path,
        abs,
        mut stamp,
    } = item;
    let rule = has_content_rule(&path, Mode::File);
    for _ in 0..READ_ATTEMPTS {
        if stamp.size() > MAX_FILE_BYTES {
            return Err(FsError::TooLarge {
                path: abs,
                size: stamp.size(),
                limit: MAX_FILE_BYTES,
            });
        }
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let read = match openat(&*dir, &name, flags, rustix::fs::Mode::empty()) {
            Ok(fd) => read_once(File::from(fd), stamp, rule, &abs)?,
            // Replaced by a symlink since it was listed.
            Err(Errno::LOOP) => None,
            Err(e) => return Err(FsError::io(&abs)(e.into())),
        };
        if let Some((blob, kept)) = read {
            if let Some(content) = kept {
                check_content(&path, Mode::File, &content)?;
            }
            let entry = if stamp.is_executable() {
                Entry::Executable(blob.oid())
            } else {
                Entry::File(blob.oid())
            };
            return Ok(Found {
                path,
                entry,
                stamp: Some(stamp),
                content: Some(Content::File(abs)),
            });
        }
        let st = statat(&*dir, &name, AtFlags::SYMLINK_NOFOLLOW).map_err(errno(&abs))?;
        if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
            return Err(FsError::Unstable(abs));
        }
        stamp = Stamp::from_stat(&st);
    }
    Err(FsError::Unstable(abs))
}

/// One read of an open file; `None` if, at the end, the file read was not the one
/// `stamp` describes. A replaced file has another inode, and a write moves the times.
fn read_once(
    mut file: File,
    stamp: Stamp,
    keep: bool,
    abs: &Path,
) -> Result<Option<(Blob, Option<Vec<u8>>)>> {
    let mut hasher = BlobHasher::new(stamp.size());
    let mut kept = keep.then(Vec::new);
    let mut buf = vec![0u8; CHUNK];
    // Read at most one byte past the announced size, so a growing file ends.
    let mut limited = (&mut file).take(stamp.size() + 1);
    loop {
        let n = match limited.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(FsError::Io {
                    path: abs.into(),
                    source: e,
                });
            }
        };
        hasher.update(&buf[..n]);
        if let Some(kept) = kept.as_mut() {
            kept.extend_from_slice(&buf[..n]);
        }
    }
    let blob = match hasher.finish() {
        Ok(blob) => blob,
        Err(HashFailure::WrongLength { .. }) => return Ok(None),
        Err(HashFailure::Collision) => return Err(FsError::Collision(abs.into())),
    };
    let after = Stamp::from_stat(&fstat(&file).map_err(errno(abs))?);
    if after != stamp {
        return Ok(None);
    }
    Ok(Some((blob, kept)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn stamp(path: &Path) -> Stamp {
        Stamp::of(&fs::symlink_metadata(path).unwrap())
    }

    fn read(path: &Path, stamp: Stamp) -> Option<GitOid> {
        let file = File::open(path).unwrap();
        read_once(file, stamp, false, path)
            .unwrap()
            .map(|(b, _)| b.oid())
    }

    #[test]
    fn rules_that_changed_during_the_scan_are_refused() {
        let root = Path::new("/r");
        let old = ForkIgnore::parse("").unwrap();
        let tree_with = |content: &[u8]| {
            Tree::from_entries([(RepoPath::new(FORKIGNORE).unwrap(), Entry::file(content))])
                .unwrap()
        };
        // Parsed as empty; by the time the walk read it, it said `.env`.
        assert!(matches!(
            rules_held(&old, Some(b""), &tree_with(b".env\n"), root),
            Err(FsError::Unstable(_))
        ));
        // Defaults parsed (no file); a file appeared before the walk.
        assert!(matches!(
            rules_held(&old, None, &tree_with(b""), root),
            Err(FsError::Unstable(_))
        ));
        // A file parsed, gone by the walk.
        assert!(matches!(
            rules_held(&old, Some(b""), &Tree::new(), root),
            Err(FsError::Unstable(_))
        ));
        assert!(rules_held(&old, Some(b""), &tree_with(b""), root).is_ok());
        assert!(rules_held(&old, None, &Tree::new(), root).is_ok());
    }

    #[test]
    fn a_read_under_a_stale_stamp_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        fs::write(&path, b"one\n").unwrap();
        // Settled, as a cached stamp is: on a coarse clock (ext4, overlayfs) a second write
        // within the same tick would otherwise leave the stamp as it was, which is why
        // only settled stamps are cached.
        let old = SystemTime::now() - std::time::Duration::from_secs(60);
        File::open(&path).unwrap().set_modified(old).unwrap();
        let before = stamp(&path);
        assert!(read(&path, before).is_some());
        fs::write(&path, b"two\n").unwrap();
        assert_eq!(read(&path, before), None, "same size, new content");
        assert_eq!(read(&path, stamp(&path)), Some(GitOid::for_blob(b"two\n")));
    }

    #[test]
    fn a_file_replaced_by_a_symlink_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target"), b"elsewhere\n").unwrap();
        std::os::unix::fs::symlink("target", dir.path().join("link")).unwrap();
        let item = ToRead {
            dir: Arc::new(open_dir(dir.path()).unwrap()),
            name: CString::new("link").unwrap(),
            path: RepoPath::new("link").unwrap(),
            abs: dir.path().join("link"),
            // The stamp of what the link points at: following it would match.
            stamp: stamp(&dir.path().join("target")),
        };
        assert!(matches!(read_file(item), Err(FsError::Unstable(_))));
    }

    #[test]
    fn a_directory_swapped_for_a_symlink_is_not_walked() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), b"host secret\n").unwrap();
        fs::create_dir(dir.path().join("a")).unwrap();
        let st = statat(
            rustix::fs::CWD,
            dir.path().join("a"),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();
        // The replacement is made while `a` still exists, so it cannot reuse its inode.
        fs::create_dir(dir.path().join("b")).unwrap();
        // The walk listed `a` as a directory; then it became a link out of the root.
        fs::remove_dir(dir.path().join("a")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("a")).unwrap();
        let mut pending = Pending {
            parent: Arc::new(open_dir(dir.path()).unwrap()),
            name: CString::new("a").unwrap(),
            dev_ino: dev_ino(&st),
            prefix: "a".into(),
            rel: "a".into(),
            abs: dir.path().join("a"),
        };
        assert!(matches!(open_child(&pending), Err(FsError::Unstable(_))));
        // Another directory put in its place is refused too.
        fs::remove_file(dir.path().join("a")).unwrap();
        fs::rename(dir.path().join("b"), dir.path().join("a")).unwrap();
        assert!(matches!(open_child(&pending), Err(FsError::Unstable(_))));
        pending.dev_ino = dev_ino(
            &statat(
                rustix::fs::CWD,
                dir.path().join("a"),
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .unwrap(),
        );
        assert!(open_child(&pending).is_ok());
    }
}
