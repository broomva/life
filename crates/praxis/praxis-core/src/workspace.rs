//! Workspace boundary enforcement (FsPolicy).
//!
//! All filesystem operations in Praxis are confined to a workspace root.
//! `FsPolicy` validates paths and prevents directory traversal attacks.

use crate::error::{PraxisError, PraxisResult};
use std::path::{Path, PathBuf};

/// Filesystem policy that enforces workspace boundaries.
///
/// All paths must resolve to locations within the workspace root.
/// Symlinks are resolved before validation to prevent traversal.
#[derive(Debug, Clone)]
pub struct FsPolicy {
    workspace_root: PathBuf,
}

impl FsPolicy {
    /// Create a new policy rooted at the given directory.
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    /// Return the workspace root.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Resolve an existing path, ensuring it's within the workspace.
    ///
    /// The path must exist. Returns the canonicalized absolute path.
    pub fn resolve_existing(&self, candidate: &Path) -> PraxisResult<PathBuf> {
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.workspace_root.join(candidate)
        };
        let canonical = joined.canonicalize()?;
        self.ensure_within_root(&canonical)?;
        Ok(canonical)
    }

    /// Resolve a path for writing. The parent must exist and be within workspace,
    /// but the file itself may not exist yet.
    pub fn resolve_for_write(&self, candidate: &Path) -> PraxisResult<PathBuf> {
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.workspace_root.join(candidate)
        };
        let parent = joined
            .parent()
            .ok_or_else(|| PraxisError::PathOutsideWorkspace {
                path: joined.display().to_string(),
            })?;
        let canonical_parent = parent.canonicalize()?;
        self.ensure_within_root(&canonical_parent)?;
        Ok(canonical_parent.join(joined.file_name().unwrap()))
    }

    /// Validate that a canonical path is within the workspace root.
    fn ensure_within_root(&self, candidate: &Path) -> PraxisResult<()> {
        let canonical_root = self.workspace_root.canonicalize().map_err(|e| {
            PraxisError::WorkspaceViolation(format!("cannot resolve workspace root: {e}"))
        })?;
        if candidate.starts_with(&canonical_root) {
            Ok(())
        } else {
            Err(PraxisError::PathOutsideWorkspace {
                path: candidate.display().to_string(),
            })
        }
    }

    /// Validate and resolve a path, ensuring it's within the workspace.
    ///
    /// Returns the canonicalized absolute path on success.
    pub fn resolve(&self, path: &str) -> PraxisResult<PathBuf> {
        let target = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.workspace_root.join(path)
        };

        // For paths that don't exist yet, validate the parent
        if target.exists() {
            let canonical = target.canonicalize()?;
            let canonical_root = self.workspace_root.canonicalize().map_err(|e| {
                PraxisError::WorkspaceViolation(format!("cannot resolve workspace root: {e}"))
            })?;
            if !canonical.starts_with(&canonical_root) {
                return Err(PraxisError::PathOutsideWorkspace {
                    path: path.to_string(),
                });
            }
            Ok(canonical)
        } else {
            // Path doesn't exist — validate the parent
            let parent = target
                .parent()
                .ok_or_else(|| PraxisError::PathOutsideWorkspace {
                    path: path.to_string(),
                })?;
            if parent.exists() {
                let canonical_parent = parent.canonicalize()?;
                let canonical_root = self.workspace_root.canonicalize().map_err(|e| {
                    PraxisError::WorkspaceViolation(format!("cannot resolve workspace root: {e}"))
                })?;
                if !canonical_parent.starts_with(&canonical_root) {
                    return Err(PraxisError::PathOutsideWorkspace {
                        path: path.to_string(),
                    });
                }
                Ok(target)
            } else {
                Err(PraxisError::PathOutsideWorkspace {
                    path: path.to_string(),
                })
            }
        }
    }

    /// Return a path relative to the workspace root, if it's within bounds.
    pub fn relative(&self, absolute_path: &Path) -> Option<PathBuf> {
        let canonical_root = self.workspace_root.canonicalize().ok()?;
        let canonical_path = absolute_path.canonicalize().ok()?;
        canonical_path
            .strip_prefix(&canonical_root)
            .ok()
            .map(|p| p.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_existing_file_within_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let policy = FsPolicy::new(dir.path());
        let resolved = policy.resolve("test.txt").unwrap();
        assert!(resolved.exists());
        assert!(resolved.starts_with(dir.path().canonicalize().unwrap()));
    }

    #[test]
    fn resolve_absolute_path_within_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let policy = FsPolicy::new(dir.path());
        let resolved = policy.resolve(file_path.to_str().unwrap()).unwrap();
        assert!(resolved.exists());
    }

    #[test]
    fn resolve_path_outside_workspace_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let policy = FsPolicy::new(dir.path());
        let err = policy.resolve("/etc/passwd").unwrap_err();
        assert!(matches!(err, PraxisError::PathOutsideWorkspace { .. }));
    }

    #[test]
    fn resolve_traversal_attack_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();

        let policy = FsPolicy::new(dir.path());
        let err = policy.resolve("../../../etc/passwd").unwrap_err();
        assert!(matches!(err, PraxisError::PathOutsideWorkspace { .. }));
    }

    #[test]
    fn resolve_new_file_within_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let policy = FsPolicy::new(dir.path());
        let resolved = policy.resolve("new_file.txt").unwrap();
        assert!(!resolved.exists());
        assert!(resolved.ends_with("new_file.txt"));
    }

    #[test]
    fn relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("sub/test.txt");
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(&file_path, "hello").unwrap();

        let policy = FsPolicy::new(dir.path());
        let rel = policy.relative(&file_path).unwrap();
        assert_eq!(rel, PathBuf::from("sub/test.txt"));
    }
}
