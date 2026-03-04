//! Filesystem port abstraction for Praxis tools.
//!
//! [`FsPort`] defines the filesystem operations that tools depend on.
//! The default [`LocalFs`] implementation wraps `std::fs` with workspace
//! boundary enforcement via [`FsPolicy`]. Alternative implementations
//! (e.g. tracked filesystems with event notification) can be injected.

use crate::error::PraxisResult;
use std::path::{Path, PathBuf};

/// Metadata about a filesystem entry.
#[derive(Debug, Clone)]
pub struct FsMetadata {
    pub is_file: bool,
    pub is_dir: bool,
    pub size_bytes: u64,
}

/// A single directory entry.
#[derive(Debug, Clone)]
pub struct FsDirEntry {
    pub name: String,
    pub is_file: bool,
    pub is_dir: bool,
}

/// Filesystem abstraction for Praxis tools.
///
/// Implementations must enforce workspace boundaries. All paths
/// passed to methods are either absolute or relative to the
/// workspace root.
pub trait FsPort: Send + Sync {
    /// The workspace root directory.
    fn workspace_root(&self) -> &Path;

    /// Resolve an existing path within the workspace.
    fn resolve(&self, path: &Path) -> PraxisResult<PathBuf>;

    /// Resolve a path for writing (file may not exist yet).
    fn resolve_for_write(&self, path: &Path) -> PraxisResult<PathBuf>;

    /// Read a file as UTF-8 text.
    fn read_to_string(&self, path: &Path) -> PraxisResult<String>;

    /// Read a file as raw bytes.
    fn read_bytes(&self, path: &Path) -> PraxisResult<Vec<u8>>;

    /// Write content to a file. Creates parent directories as needed.
    fn write(&self, path: &Path, content: &[u8]) -> PraxisResult<()>;

    /// Check whether a path exists.
    fn exists(&self, path: &Path) -> bool;

    /// Get metadata for a path.
    fn metadata(&self, path: &Path) -> PraxisResult<FsMetadata>;

    /// List entries in a directory.
    fn read_dir(&self, path: &Path) -> PraxisResult<Vec<FsDirEntry>>;

    /// Create a directory and all parent directories.
    fn create_dir_all(&self, path: &Path) -> PraxisResult<()>;

    /// Return a path relative to the workspace root, if within bounds.
    fn relative(&self, absolute_path: &Path) -> Option<PathBuf>;
}
