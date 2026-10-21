//! Snapshot: a full-tree scan of a directory into a tree and a changeset.

use crate::cache::{Stamp, StatCache};
use crate::error::{FsError, Result};
use crate::ignore::ForkIgnore;
use cuenca_tree::{
    Blob, BlobHasher, Changeset, Entry, GitOid, HashFailure, Mode, PathError, RepoPath, Tree,
    check_content, has_content_rule,
};
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The largest file a snapshot takes, as the authority's blob store accepts.
pub const MAX_FILE_BYTES: u64 = cuenca_tree::git::DEFAULT_MAX_BLOB_BYTES;

/// How many times a file that changes while it is read is read again before the
/// snapshot gives up on it.
const READ_ATTEMPTS: usize = 3;
const CHUNK: usize = 64 * 1024;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    pub reason: &'static str,
}

/// Snapshot `root` against `base`, with the rules of `root/.forkignore` (or the defaults
/// when there is none). On success `cache` holds this scan's stats; on failure it is
/// left as it was.
pub fn snapshot(root: &Path, base: &Tree, cache: &mut StatCache) -> Result<Snapshot> {
    snapshot_with(root, base, cache, &ForkIgnore::for_root(root)?)
}

/// [`snapshot`] with explicit ignore rules.
pub fn snapshot_with(
    root: &Path,
    base: &Tree,
    cache: &mut StatCache,
    ignore: &ForkIgnore,
) -> Result<Snapshot> {
    let started = SystemTime::now();
    let mut found = Vec::new();
    let mut skipped = Vec::new();
    walk(root, ignore, &mut found, &mut skipped)?;

    let mut entries = Vec::with_capacity(found.len());
    let mut to_hash = Vec::new();
    for (path, abs) in found {
        let meta = fs::symlink_metadata(&abs).map_err(FsError::io(&abs))?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            let target = fs::read_link(&abs)
                .map_err(FsError::io(&abs))?
                .into_os_string()
                .into_vec();
            check_content(&path, Mode::Symlink, &target)?;
            let blob = Blob::of(&target).ok_or_else(|| FsError::Collision(abs.clone()))?;
            entries.push(Found {
                path,
                entry: Entry::Symlink(blob.oid()),
                stamp: None,
                content: Content::Inline(target),
            });
        } else if kind.is_file() {
            let stamp = Stamp::of(&meta);
            match cache.lookup(path.as_str(), &stamp) {
                Some(entry) => entries.push(Found {
                    path,
                    entry,
                    stamp: Some(stamp),
                    content: Content::File(abs),
                }),
                None => to_hash.push((path, abs, stamp)),
            }
        } else if kind.is_dir() {
            // It was a file when the directory was listed.
            return Err(FsError::Unstable(abs));
        } else {
            skipped.push(Skipped {
                path: abs,
                reason: "not a regular file or symlink",
            });
        }
    }
    let hashed = to_hash.len();
    let read: Vec<Found> = to_hash
        .into_par_iter()
        .map(|(path, abs, stamp)| hash_file(path, abs, stamp))
        .collect::<Result<_>>()?;
    entries.extend(read);

    let mut next = StatCache::new();
    for found in &entries {
        if let Some(stamp) = found.stamp {
            next.record(found.path.to_string(), stamp, found.entry, started);
        }
    }
    let tree = Tree::from_entries(entries.iter().map(|f| (f.path.clone(), f.entry)))?;
    let changeset = Changeset::between(base, &tree);
    let mut known: HashSet<GitOid> = base.entries().map(|(_, e)| e.oid()).collect();
    let mut new_blobs = Vec::new();
    for found in entries {
        let oid = found.entry.oid();
        if known.insert(oid) {
            let size = match (&found.content, found.stamp) {
                (Content::Inline(bytes), _) => bytes.len() as u64,
                (Content::File(_), Some(stamp)) => stamp.size(),
                (Content::File(_), None) => unreachable!("every file carries its stamp"),
            };
            new_blobs.push(NewBlob {
                oid,
                size,
                content: found.content,
            });
        }
    }
    *cache = next;
    Ok(Snapshot {
        tree,
        changeset,
        new_blobs,
        skipped,
        hashed,
    })
}

struct Found {
    path: RepoPath,
    entry: Entry,
    /// The stat the entry was read under; `None` for a symlink, which is never cached.
    stamp: Option<Stamp>,
    content: Content,
}

/// Every entry under `root` that is not a directory and that the rules keep, as
/// (repository path, absolute path). Ignored directories are not descended into.
fn walk(
    root: &Path,
    ignore: &ForkIgnore,
    found: &mut Vec<(RepoPath, PathBuf)>,
    skipped: &mut Vec<Skipped>,
) -> Result<()> {
    let mut dirs = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = dirs.pop() {
        for item in fs::read_dir(&dir).map_err(FsError::io(&dir))? {
            let item = item.map_err(FsError::io(&dir))?;
            let abs = item.path();
            let name = item.file_name();
            let Some(name) = name.to_str() else {
                return Err(FsError::NotUtf8(abs));
            };
            if name == ".git" {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            };
            let path = match RepoPath::new(rel.as_str()) {
                Ok(path) => path,
                Err(PathError::GitDir(_)) => {
                    skipped.push(Skipped {
                        path: abs,
                        reason: "git reserves the name",
                    });
                    continue;
                }
                Err(source) => return Err(FsError::Name { path: abs, source }),
            };
            let kind = item.file_type().map_err(FsError::io(&abs))?;
            if ignore.is_ignored(&rel, kind.is_dir()) {
                continue;
            }
            // What is not a directory is classified once, by the `lstat` that also
            // gives its stamp.
            if kind.is_dir() {
                dirs.push((abs, rel));
            } else {
                found.push((path, abs));
            }
        }
    }
    Ok(())
}

/// Read a regular file whose `lstat` gave `stamp`, and accept what was read only if it
/// was opened without following a link, held the announced number of bytes, and the open
/// file's `fstat` is still `stamp` at the end. Otherwise read it again, up to
/// [`READ_ATTEMPTS`] times.
fn hash_file(path: RepoPath, abs: PathBuf, mut stamp: Stamp) -> Result<Found> {
    let rule = has_content_rule(&path, Mode::File);
    for _ in 0..READ_ATTEMPTS {
        if stamp.size() > MAX_FILE_BYTES {
            return Err(FsError::TooLarge {
                path: abs,
                size: stamp.size(),
                limit: MAX_FILE_BYTES,
            });
        }
        match read_once(&abs, stamp, rule)? {
            Some((blob, kept)) => {
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
                    content: Content::File(abs),
                });
            }
            None => {
                let meta = fs::symlink_metadata(&abs).map_err(FsError::io(&abs))?;
                if !meta.file_type().is_file() {
                    return Err(FsError::Unstable(abs));
                }
                stamp = Stamp::of(&meta);
            }
        }
    }
    Err(FsError::Unstable(abs))
}

/// One read of the file; `None` if the file read was not, at the end, the one `stamp`
/// describes.
fn read_once(abs: &Path, stamp: Stamp, keep: bool) -> Result<Option<(Blob, Option<Vec<u8>>)>> {
    let mut file = match File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(abs)
    {
        Ok(file) => file,
        // Replaced by a symlink since the lstat (`O_NOFOLLOW` refuses it with ELOOP).
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok(None),
        Err(e) => {
            return Err(FsError::Io {
                path: abs.into(),
                source: e,
            });
        }
    };
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
    // The open file is the one `stamp` describes, and it did not change while it was read:
    // a replaced file has another inode, a write moves the times. If the path is replaced
    // during the read, the snapshot holds what was there when it was read, and the next
    // scan sees the new file by its inode.
    let after = Stamp::of(&file.metadata().map_err(FsError::io(abs))?);
    if after != stamp {
        return Ok(None);
    }
    Ok(Some((blob, kept)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn stamp(path: &Path) -> Stamp {
        Stamp::of(&fs::metadata(path).unwrap())
    }

    #[test]
    fn a_read_under_a_stale_stamp_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        fs::write(&path, b"one\n").unwrap();
        let before = stamp(&path);
        assert!(read_once(&path, before, false).unwrap().is_some());
        fs::write(&path, b"two\n").unwrap();
        assert!(
            read_once(&path, before, false).unwrap().is_none(),
            "same size, new content"
        );
        let now = stamp(&path);
        let (blob, _) = read_once(&path, now, false).unwrap().unwrap();
        assert_eq!(blob.oid(), GitOid::for_blob(b"two\n"));
    }

    #[test]
    fn a_file_replaced_by_a_symlink_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"elsewhere\n").unwrap();
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();
        // The stamp of what the link points at: following it would match.
        assert!(read_once(&link, stamp(&target), false).unwrap().is_none());
    }
}
