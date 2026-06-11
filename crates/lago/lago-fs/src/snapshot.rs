use std::fs;
use std::path::Path;
use tracing::warn;
use walkdir::WalkDir;

use crate::manifest::Manifest;
use lago_core::LagoResult;
use lago_store::BlobStore;

/// Default ceiling on the number of files a bounded snapshot will scan.
pub const DEFAULT_MAX_FILES: usize = 10_000;

/// Default ceiling on the size of a single file a bounded snapshot will
/// blob-store. Matches the 16 MiB cap used elsewhere in the tracking path.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Bounds for a workspace snapshot walk.
///
/// The exec-path reconciliation walks the entire workspace after a shell
/// command runs, so it must be bounded: a runaway command could create an
/// arbitrary number of files (or one enormous file). Tracking is best-effort
/// observability — exceeding a bound logs a warning and skips, never errors.
#[derive(Debug, Clone, Copy)]
pub struct SnapshotLimits {
    /// Maximum number of regular files to scan before stopping the walk.
    pub max_files: usize,
    /// Maximum size (bytes) of a single file to read + blob-store. Larger
    /// files are skipped (the manifest will not track them this pass).
    pub max_file_bytes: u64,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_MAX_FILES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

impl SnapshotLimits {
    /// Limits that impose no practical ceiling (preserves legacy
    /// [`snapshot`] behavior for callers that opt out of bounding).
    pub fn unbounded() -> Self {
        Self {
            max_files: usize::MAX,
            max_file_bytes: u64::MAX,
        }
    }
}

/// Builds a new manifest by scanning a physical directory.
///
/// Uses the `previous_manifest` to optimize hashing. If a file's size and
/// last modified time (mtime) match the previous entry, its hash is reused
/// instead of recalculating and re-storing the blob.
///
/// This is the unbounded variant, kept for callers (snapshots, diffs) that
/// already operate over trusted, fixed inputs. The exec-path reconciliation
/// uses [`snapshot_bounded`] instead.
pub fn snapshot(
    root: &Path,
    previous_manifest: &Manifest,
    blob_store: &BlobStore,
) -> LagoResult<Manifest> {
    snapshot_bounded(
        root,
        previous_manifest,
        blob_store,
        SnapshotLimits::unbounded(),
    )
}

/// Bounded variant of [`snapshot`]: caps the number of files scanned and the
/// per-file size that is read + blob-stored.
///
/// Used by the exec-path reconciliation, where the workspace may have been
/// mutated by an arbitrary shell command. Exceeding either bound logs a
/// `warn!` and skips (the walk stops once `max_files` regular files have been
/// processed); it never returns an error for a bound violation. Genuine I/O
/// errors (unreadable metadata/content) still propagate.
pub fn snapshot_bounded(
    root: &Path,
    previous_manifest: &Manifest,
    blob_store: &BlobStore,
    limits: SnapshotLimits,
) -> LagoResult<Manifest> {
    let mut new_manifest = Manifest::new();

    // Prune entire directory trees early so WalkDir never descends into them.
    // This also keeps the journal/blob dirs out of the manifest.
    let walker = WalkDir::new(root).into_iter().filter_entry(|entry| {
        let name = entry.file_name().to_string_lossy();
        !matches!(
            name.as_ref(),
            ".git"
                | ".lago"
                | ".lake"
                | ".arcan"
                | ".target"
                | "target"
                | "node_modules"
                | ".DS_Store"
        )
    });

    let mut scanned: usize = 0;

    for entry in walker.filter_map(Result::ok) {
        let path = entry.path();

        // Ignore symlinks and directories in this pass
        if !path.is_file() {
            continue;
        }

        if scanned >= limits.max_files {
            warn!(
                max_files = limits.max_files,
                root = %root.display(),
                "snapshot_bounded: file-count cap reached, remaining files not tracked this pass"
            );
            break;
        }
        scanned += 1;

        let rel_path = path.strip_prefix(root).unwrap_or(path);
        let rel_str = rel_path.to_string_lossy().to_string();

        let virtual_path = format!("/{}", rel_str);

        let metadata = fs::metadata(path)?;
        let size = metadata.len();
        let mtime = metadata
            .modified()
            .unwrap_or_else(|_| std::time::SystemTime::now())
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if size > limits.max_file_bytes {
            warn!(
                path = %virtual_path,
                size_bytes = size,
                max_file_bytes = limits.max_file_bytes,
                "snapshot_bounded: file exceeds per-file size cap, not tracked this pass"
            );
            continue;
        }

        // Check if we can reuse the previous hash (fast path)
        let mut reused = false;
        if let Some(prev_entry) = previous_manifest.get(&virtual_path)
            && prev_entry.size_bytes == size
            && prev_entry.updated_at == mtime
        {
            // Match: assume content is identical to skip IO + hashing
            new_manifest.apply_write(
                virtual_path.clone(),
                prev_entry.blob_hash.clone(),
                size,
                prev_entry.content_type.clone(),
                mtime,
            );
            reused = true;
        }

        if !reused {
            // Slow path: Read file, hash, and store in the blob store
            let data = fs::read(path)?;
            let new_hash = blob_store.put(&data)?;

            new_manifest.apply_write(
                virtual_path,
                // The blob store returns a lego_core::BlobHash
                lago_core::BlobHash::from_hex(new_hash.as_str()),
                size,
                None,
                mtime,
            );
        }
    }

    Ok(new_manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn snapshot_creates_new_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let blob_store = BlobStore::open(temp.path().join("blobs")).unwrap();
        let workspace = temp.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();

        // Write a file to real disk
        let file_path = workspace.join("hello.txt");
        let mut file = fs::File::create(&file_path).unwrap();
        file.write_all(b"world").unwrap();
        file.sync_all().unwrap();

        let prev = Manifest::new();
        let next = snapshot(&workspace, &prev, &blob_store).unwrap();

        assert!(next.exists("/hello.txt"));
        let entry = next.get("/hello.txt").unwrap();
        assert_eq!(entry.size_bytes, 5);
        assert!(blob_store.exists(&entry.blob_hash));
    }

    #[test]
    fn snapshot_reuses_unchanged_files() {
        let temp = tempfile::tempdir().unwrap();
        let blob_store = BlobStore::open(temp.path().join("blobs")).unwrap();
        let workspace = temp.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();

        let file_path = workspace.join("hello.txt");
        fs::write(&file_path, "world").unwrap();

        let prev = snapshot(&workspace, &Manifest::new(), &blob_store).unwrap();

        let prev_entry = prev.get("/hello.txt").unwrap().clone();

        // Take snapshot again without changes
        let next = snapshot(&workspace, &prev, &blob_store).unwrap();
        let next_entry = next.get("/hello.txt").unwrap();

        assert_eq!(prev_entry.blob_hash, next_entry.blob_hash);
        assert_eq!(prev_entry.updated_at, next_entry.updated_at);
    }

    #[test]
    fn bounded_snapshot_skips_oversized_file() {
        let temp = tempfile::tempdir().unwrap();
        let blob_store = BlobStore::open(temp.path().join("blobs")).unwrap();
        let workspace = temp.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();

        fs::write(workspace.join("small.txt"), "tiny").unwrap();
        fs::write(workspace.join("big.bin"), vec![0u8; 4096]).unwrap();

        let limits = SnapshotLimits {
            max_files: 10_000,
            max_file_bytes: 1024,
        };
        let manifest = snapshot_bounded(&workspace, &Manifest::new(), &blob_store, limits).unwrap();

        // Small file tracked, oversized file skipped — no panic, no error.
        assert!(manifest.exists("/small.txt"));
        assert!(!manifest.exists("/big.bin"));
    }

    #[test]
    fn bounded_snapshot_honors_file_count_cap() {
        let temp = tempfile::tempdir().unwrap();
        let blob_store = BlobStore::open(temp.path().join("blobs")).unwrap();
        let workspace = temp.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();

        for i in 0..10 {
            fs::write(workspace.join(format!("f{i}.txt")), format!("c{i}")).unwrap();
        }

        let limits = SnapshotLimits {
            max_files: 3,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        };
        let manifest = snapshot_bounded(&workspace, &Manifest::new(), &blob_store, limits).unwrap();

        // Only file entries count toward the cap; directory sentinels are
        // created implicitly by apply_write and are not regular files.
        let file_entries = manifest
            .entries()
            .values()
            .filter(|e| e.content_type.as_deref() != Some("inode/directory"))
            .count();
        assert_eq!(file_entries, 3, "file-count cap should bound scanned files");
    }

    #[test]
    fn bounded_snapshot_skips_git_dir() {
        let temp = tempfile::tempdir().unwrap();
        let blob_store = BlobStore::open(temp.path().join("blobs")).unwrap();
        let workspace = temp.path().join("ws");
        fs::create_dir_all(workspace.join(".git/objects")).unwrap();
        fs::write(workspace.join(".git/config"), "[core]").unwrap();
        fs::write(workspace.join(".git/objects/blob"), "data").unwrap();
        fs::write(workspace.join("real.txt"), "tracked").unwrap();

        let manifest = snapshot_bounded(
            &workspace,
            &Manifest::new(),
            &blob_store,
            SnapshotLimits::default(),
        )
        .unwrap();

        assert!(manifest.exists("/real.txt"));
        assert!(!manifest.exists("/.git"));
        assert!(!manifest.exists("/.git/config"));
        assert!(!manifest.exists("/.git/objects/blob"));
    }

    #[test]
    fn unbounded_limits_track_everything() {
        let temp = tempfile::tempdir().unwrap();
        let blob_store = BlobStore::open(temp.path().join("blobs")).unwrap();
        let workspace = temp.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("a.txt"), vec![1u8; 100_000]).unwrap();

        // Unbounded limits must behave like the legacy snapshot.
        let bounded = snapshot_bounded(
            &workspace,
            &Manifest::new(),
            &blob_store,
            SnapshotLimits::unbounded(),
        )
        .unwrap();
        let legacy = snapshot(&workspace, &Manifest::new(), &blob_store).unwrap();

        assert!(bounded.exists("/a.txt"));
        assert_eq!(bounded.get("/a.txt").unwrap().size_bytes, 100_000);
        assert_eq!(
            bounded.get("/a.txt").unwrap().blob_hash,
            legacy.get("/a.txt").unwrap().blob_hash
        );
    }
}
