//! Blob content on disk, one file per git id. A file only counts once the log database
//! indexes it (see `Store::ingest_blob`); a file without an index row is an interrupted
//! ingest and is overwritten by the next one.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use cuenca_tree::{Blob, BlobHasher, GitOid};

use crate::error::{CoreError, Result};

/// The largest blob the store accepts, as the git bridge's default.
pub const MAX_BLOB_BYTES: u64 = cuenca_tree::git::DEFAULT_MAX_BLOB_BYTES;

const CHUNK: usize = 64 * 1024;

pub(crate) struct BlobFiles {
    objects: PathBuf,
    tmp: PathBuf,
}

impl BlobFiles {
    pub(crate) fn open(root: &Path) -> Result<Self> {
        let objects = root.join("objects");
        let tmp = root.join("tmp");
        fs::create_dir_all(&objects)?;
        fs::create_dir_all(&tmp)?;
        // Make the directories' own entries durable before any blob is indexed.
        for dir in [root.parent().unwrap_or(root), root, &objects] {
            File::open(dir)?.sync_all()?;
        }
        // Leftovers of interrupted ingests: never indexed, so never referenced.
        for leftover in fs::read_dir(&tmp)? {
            let path = leftover?.path();
            if path.is_file() {
                fs::remove_file(path)?;
            }
        }
        Ok(Self { objects, tmp })
    }

    fn path(&self, oid: GitOid) -> PathBuf {
        let hex = oid.to_hex();
        self.objects.join(&hex[..2]).join(&hex[2..])
    }

    /// Stream `size` bytes from `content` into a temporary file, hashing as they pass,
    /// and move the file into place only if they hash to `claimed`. On any failure the
    /// temporary file is removed, so nothing is stored.
    pub(crate) fn write(&self, claimed: GitOid, size: u64, content: impl Read) -> Result<Blob> {
        if size > MAX_BLOB_BYTES {
            return Err(CoreError::TooLarge {
                size,
                limit: MAX_BLOB_BYTES,
            });
        }
        let mut tmp = tempfile::NamedTempFile::new_in(&self.tmp)?;
        let mut hasher = BlobHasher::new(size);
        // Read at most one byte past the announced size, so an endless stream ends.
        let mut limited = content.take(size + 1);
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = match limited.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            hasher.update(&buf[..n]);
            tmp.write_all(&buf[..n])?;
        }
        let blob = hasher.finish().map_err(CoreError::Hash)?;
        if blob.oid() != claimed {
            return Err(CoreError::OidMismatch {
                claimed,
                actual: blob.oid(),
            });
        }
        tmp.as_file().sync_all()?;
        let dest = self.path(claimed);
        let dir = dest.parent().expect("objects/<xx>");
        match fs::create_dir(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        // The shard's own entry must be durable before the index names the blob. Done
        // on every ingest: a concurrent ingest may have created the shard and not yet
        // made it durable.
        File::open(&self.objects)?.sync_all()?;
        tmp.persist(&dest).map_err(|e| e.error)?;
        File::open(dir)?.sync_all()?;
        Ok(blob)
    }

    /// The content of `oid`, re-hashed on the way out so disk corruption surfaces as an
    /// error rather than as wrong bytes.
    pub(crate) fn read(&self, oid: GitOid) -> Result<Vec<u8>> {
        let bytes = match fs::read(self.path(oid)) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(CoreError::CorruptBlob(oid));
            }
            Err(e) => return Err(e.into()),
        };
        match Blob::of(&bytes) {
            Some(blob) if blob.oid() == oid => Ok(bytes),
            _ => Err(CoreError::CorruptBlob(oid)),
        }
    }
}
