//! Checkout: materialise a tree into a directory, cloning where the disk can.

use crate::cache::{Stamp, StatCache};
use crate::error::{FsError, Result};
use cuenca_tree::{Blob, Entry, GitOid, Mode, Tree, TreeError, check_content};
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How far before the checkout started the files it writes are dated, so that every
/// one of them is settled (see [`crate::RACY_MARGIN`]) and the checkout's first snapshot
/// reads nothing.
const BACKDATE: Duration = Duration::from_secs(10);

/// How many files a checkout writes at once. Clones on one APFS volume contend: measured
/// on a 10,844-file tree, 1 to 4 threads took 0.9 to 1.0 s and 14 threads 2.8 s.
const WRITERS: usize = 4;

/// Where blob content comes from during a checkout.
pub trait BlobSource: Sync {
    /// Where `oid`'s content is, or `None` if this source does not have it.
    fn fetch(&self, oid: GitOid) -> io::Result<Option<Fetched>>;
}

/// A blob's content, as a source hands it over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// A file holding exactly the blob, which a checkout clones (or copies, where the
    /// disk cannot clone). If `stamp` is given, the checkout checks the file still has
    /// it after cloning, and fails if it does not.
    File { path: PathBuf, stamp: Option<Stamp> },
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
/// file exactly the blob with that git id, as the store verified it at ingest.
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
        let path = self.dir.join(&hex[..2]).join(&hex[2..]);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_file() => Ok(Some(Fetched::File { path, stamp: None })),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// An existing checkout of `tree`, as its stat cache describes it. A file is offered only
/// while its stat is still the one the cache recorded for that content, so a file edited
/// since the last snapshot is never cloned as the old content. A symlink's target is read
/// from the link at its path, and the checkout hashes it before using it.
pub struct CheckoutSource {
    root: PathBuf,
    files: HashMap<GitOid, (String, Stamp)>,
    links: HashMap<GitOid, String>,
}

impl CheckoutSource {
    pub fn new(root: impl Into<PathBuf>, tree: &Tree, cache: &StatCache) -> Self {
        let mut files = HashMap::new();
        for (path, cached) in cache.iter() {
            files
                .entry(cached.entry.oid())
                .or_insert_with(|| (path.clone(), cached.stamp));
        }
        let mut links = HashMap::new();
        for (path, entry) in tree.entries() {
            if let Entry::Symlink(oid) = entry {
                links.entry(*oid).or_insert_with(|| path.to_string());
            }
        }
        Self {
            root: root.into(),
            files,
            links,
        }
    }
}

impl BlobSource for CheckoutSource {
    fn fetch(&self, oid: GitOid) -> io::Result<Option<Fetched>> {
        if let Some((path, stamp)) = self.files.get(&oid) {
            let path = self.root.join(path);
            match fs::symlink_metadata(&path) {
                Ok(meta) if Stamp::of(&meta) == *stamp => {
                    return Ok(Some(Fetched::File {
                        path,
                        stamp: Some(*stamp),
                    }));
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(path) = self.links.get(&oid) {
            match fs::read_link(self.root.join(path)) {
                Ok(target) => return Ok(Some(Fetched::Bytes(target.into_os_string().into_vec()))),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
                    ) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }
}

/// What a checkout wrote.
#[derive(Debug)]
pub struct Checkout {
    /// The new checkout's stat cache, so its first snapshot reads nothing.
    pub cache: StatCache,
    /// Files cloned copy-on-write.
    pub cloned: usize,
    /// Files copied, because the disk could not clone them or the content came in memory.
    pub copied: usize,
}

/// Materialise `tree` into `dest`, which must be missing or an empty directory. Files are
/// cloned from `source` where the disk supports it (APFS `clonefile`, a Linux reflink)
/// and copied otherwise; modes are git's (`0644`, `0755`), whatever the source's are.
/// Symlinks are created last, so no write goes through one. On error the destination is
/// left partly written and should be discarded.
pub fn checkout(tree: &Tree, dest: &Path, source: &dyn BlobSource) -> Result<Checkout> {
    let started = SystemTime::now();
    prepare(dest)?;
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
        let abs = dest.join(dir);
        fs::create_dir(&abs).map_err(FsError::io(&abs))?;
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
    let written: Vec<(String, Stamp, Entry, bool)> = pool.install(|| {
        files
            .par_iter()
            .map(|(path, entry)| {
                let abs = dest.join(path.as_str());
                let (cloned, stamp) = write_file(&abs, *entry, source, started)?;
                Ok((path.to_string(), stamp, *entry, cloned))
            })
            .collect::<Result<_>>()
    })?;
    for (path, oid) in links {
        let abs = dest.join(path.as_str());
        let target = match source.fetch(oid).map_err(FsError::io(&abs))? {
            None => return Err(FsError::MissingBlob(oid)),
            Some(Fetched::Bytes(bytes)) => bytes,
            Some(Fetched::File { path, .. }) => fs::read(&path).map_err(FsError::io(&path))?,
        };
        verify(oid, &target)?;
        check_content(path, Mode::Symlink, &target)?;
        std::os::unix::fs::symlink(OsStr::from_bytes(&target), &abs).map_err(FsError::io(&abs))?;
    }
    let now = SystemTime::now();
    let mut cache = StatCache::new();
    let (mut cloned, mut copied) = (0, 0);
    for (path, stamp, entry, was_cloned) in written {
        if was_cloned {
            cloned += 1;
        } else {
            copied += 1;
        }
        cache.record(path, stamp, entry, now);
    }
    Ok(Checkout {
        cache,
        cloned,
        copied,
    })
}

fn prepare(dest: &Path) -> Result<()> {
    match fs::read_dir(dest) {
        Ok(mut items) => {
            if items.next().is_some() {
                return Err(FsError::DestinationNotEmpty(dest.into()));
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(dest).map_err(FsError::io(dest))
        }
        Err(e) => Err(FsError::Io {
            path: dest.into(),
            source: e,
        }),
    }
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

/// Write one regular file: whether it was cloned rather than copied, and its stamp. A
/// clone carries the source's mode and modification time, so the mode is set and the
/// file backdated only where they are not already right.
fn write_file(
    abs: &Path,
    entry: Entry,
    source: &dyn BlobSource,
    started: SystemTime,
) -> Result<(bool, Stamp)> {
    let oid = entry.oid();
    let cloned = match source.fetch(oid).map_err(FsError::io(abs))? {
        None => return Err(FsError::MissingBlob(oid)),
        Some(Fetched::File { path, stamp }) => {
            let copied = reflink_copy::reflink_or_copy(&path, abs).map_err(FsError::io(abs))?;
            if let Some(stamp) = stamp {
                let now = fs::symlink_metadata(&path).map_err(FsError::io(&path))?;
                if Stamp::of(&now) != stamp {
                    fs::remove_file(abs).map_err(FsError::io(abs))?;
                    return Err(FsError::SourceChanged(path));
                }
            }
            copied.is_none()
        }
        Some(Fetched::Bytes(bytes)) => {
            verify(oid, &bytes)?;
            let mut file = File::options()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(abs)
                .map_err(FsError::io(abs))?;
            file.write_all(&bytes).map_err(FsError::io(abs))?;
            false
        }
    };
    let mode = if matches!(entry, Entry::Executable(_)) {
        0o755
    } else {
        0o644
    };
    let lstat = || -> Result<Stamp> {
        Ok(Stamp::of(
            &fs::symlink_metadata(abs).map_err(FsError::io(abs))?,
        ))
    };
    let mut stamp = lstat()?;
    let mut touched = false;
    if stamp.permissions() != mode {
        fs::set_permissions(abs, fs::Permissions::from_mode(mode)).map_err(FsError::io(abs))?;
        touched = true;
    }
    if !stamp.settled(started) {
        let dated = started
            .checked_sub(BACKDATE)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        File::open(abs)
            .and_then(|f| f.set_modified(dated))
            .map_err(FsError::io(abs))?;
        touched = true;
    }
    if touched {
        stamp = lstat()?;
    }
    Ok((cloned, stamp))
}
