//! Default local filesystem implementation of [`FsPort`].
//!
//! [`LocalFs`] wraps `std::fs` with workspace boundary enforcement
//! via [`FsPolicy`]. It auto-creates parent directories on writes.

use crate::error::PraxisResult;
use crate::fs_port::{FsDirEntry, FsMetadata, FsPort};
use crate::workspace::FsPolicy;
use std::path::{Path, PathBuf};

/// Local filesystem backed by `std::fs` with [`FsPolicy`] enforcement.
#[derive(Debug, Clone)]
pub struct LocalFs {
    policy: FsPolicy,
}

impl LocalFs {
    /// Create a new local filesystem bound to a workspace root.
    pub fn new(policy: FsPolicy) -> Self {
        Self { policy }
    }

    /// Access the underlying policy.
    pub fn policy(&self) -> &FsPolicy {
        &self.policy
    }
}

impl FsPort for LocalFs {
    fn workspace_root(&self) -> &Path {
        self.policy.workspace_root()
    }

    fn resolve(&self, path: &Path) -> PraxisResult<PathBuf> {
        self.policy.resolve_existing(path)
    }

    fn resolve_for_write(&self, path: &Path) -> PraxisResult<PathBuf> {
        self.policy.resolve_for_write(path)
    }

    fn read_to_string(&self, path: &Path) -> PraxisResult<String> {
        let resolved = self.policy.resolve_existing(path)?;
        Ok(std::fs::read_to_string(resolved)?)
    }

    fn read_bytes(&self, path: &Path) -> PraxisResult<Vec<u8>> {
        let resolved = self.policy.resolve_existing(path)?;
        Ok(std::fs::read(resolved)?)
    }

    fn write(&self, path: &Path, content: &[u8]) -> PraxisResult<()> {
        // Resolve (and boundary-check) BEFORE creating anything on disk —
        // resolve_for_write tolerates missing parents, so creating them
        // first would only manufacture directories for a path that may
        // then be rejected as outside the workspace.
        let resolved = self.policy.resolve_for_write(path)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(std::fs::write(resolved, content)?)
    }

    fn exists(&self, path: &Path) -> bool {
        self.policy.resolve_existing(path).is_ok()
    }

    fn metadata(&self, path: &Path) -> PraxisResult<FsMetadata> {
        let resolved = self.policy.resolve_existing(path)?;
        let meta = std::fs::metadata(resolved)?;
        Ok(FsMetadata {
            is_file: meta.is_file(),
            is_dir: meta.is_dir(),
            size_bytes: meta.len(),
        })
    }

    fn read_dir(&self, path: &Path) -> PraxisResult<Vec<FsDirEntry>> {
        let resolved = self.policy.resolve_existing(path)?;
        let entries = std::fs::read_dir(resolved)?
            .filter_map(|entry| {
                entry.ok().map(|e| {
                    let is_dir = e.path().is_dir();
                    FsDirEntry {
                        name: e.file_name().to_string_lossy().to_string(),
                        is_file: !is_dir,
                        is_dir,
                    }
                })
            })
            .collect();
        Ok(entries)
    }

    fn create_dir_all(&self, path: &Path) -> PraxisResult<()> {
        let resolved = self.policy.resolve_for_write(path)?;
        Ok(std::fs::create_dir_all(resolved)?)
    }

    fn relative(&self, absolute_path: &Path) -> Option<PathBuf> {
        self.policy.relative(absolute_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fs() -> (tempfile::TempDir, LocalFs) {
        let tmp = tempfile::tempdir().unwrap();
        let policy = FsPolicy::new(tmp.path());
        (tmp, LocalFs::new(policy))
    }

    #[test]
    fn write_and_read_roundtrip() {
        let (tmp, fs) = make_fs();
        let path = tmp.path().join("hello.txt");
        fs.write(&path, b"world").unwrap();
        let content = fs.read_to_string(&path).unwrap();
        assert_eq!(content, "world");
    }

    #[test]
    fn write_creates_parent_directories() {
        let (tmp, fs) = make_fs();
        let path = tmp.path().join("sub/dir/file.txt");
        fs.write(&path, b"nested").unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested");
    }

    #[test]
    fn resolve_for_write_then_write_into_missing_relative_dir() {
        // The exact WriteFileTool flow that hit BRO-1490 in prod: pre-flight
        // resolve_for_write on a workspace-relative path whose parent does
        // not exist yet, then write to the resolved path.
        let (tmp, fs) = make_fs();

        let resolved = fs
            .resolve_for_write(Path::new("artifacts/receipt.txt"))
            .expect("resolve_for_write must tolerate a missing parent dir");
        fs.write(&resolved, b"BRO-1490").unwrap();

        let on_disk = tmp.path().join("artifacts/receipt.txt");
        assert!(on_disk.exists());
        assert_eq!(std::fs::read_to_string(on_disk).unwrap(), "BRO-1490");
    }

    #[test]
    fn write_outside_workspace_creates_nothing() {
        // The boundary check must run BEFORE any directory creation: a
        // rejected write must not leave stray directories behind.
        let (_tmp, fs) = make_fs();
        let outside = std::env::temp_dir().join("praxis-localfs-escape-check/sub/file.txt");

        let err = fs.write(&outside, b"nope").unwrap_err();
        assert!(matches!(
            err,
            crate::error::PraxisError::PathOutsideWorkspace { .. }
        ));
        assert!(
            !std::env::temp_dir()
                .join("praxis-localfs-escape-check")
                .exists()
        );
    }

    #[test]
    fn read_bytes_roundtrip() {
        let (tmp, fs) = make_fs();
        let path = tmp.path().join("data.bin");
        let data = vec![0u8, 1, 2, 255];
        fs.write(&path, &data).unwrap();
        let read = fs.read_bytes(&path).unwrap();
        assert_eq!(read, data);
    }

    #[test]
    fn exists_returns_true_for_existing_file() {
        let (tmp, fs) = make_fs();
        let path = tmp.path().join("exists.txt");
        std::fs::write(&path, "x").unwrap();
        assert!(fs.exists(&path));
    }

    #[test]
    fn exists_returns_false_for_missing_file() {
        let (tmp, fs) = make_fs();
        let path = tmp.path().join("nope.txt");
        assert!(!fs.exists(&path));
    }

    #[test]
    fn metadata_reports_correct_size() {
        let (tmp, fs) = make_fs();
        let path = tmp.path().join("sized.txt");
        std::fs::write(&path, "12345").unwrap();
        let meta = fs.metadata(&path).unwrap();
        assert!(meta.is_file);
        assert!(!meta.is_dir);
        assert_eq!(meta.size_bytes, 5);
    }

    #[test]
    fn read_dir_lists_entries() {
        let (tmp, fs) = make_fs();
        std::fs::write(tmp.path().join("a.txt"), "").unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();

        let entries = fs.read_dir(tmp.path()).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"a.txt"));
        assert!(names.contains(&"sub"));
    }

    #[test]
    fn resolve_outside_workspace_fails() {
        let (_tmp, fs) = make_fs();
        let result = fs.resolve(Path::new("/etc/passwd"));
        assert!(result.is_err());
    }

    #[test]
    fn relative_path_computation() {
        let (tmp, fs) = make_fs();
        let file = tmp.path().join("sub/test.txt");
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        std::fs::write(&file, "x").unwrap();

        let rel = fs.relative(&file).unwrap();
        assert_eq!(rel, PathBuf::from("sub/test.txt"));
    }
}
