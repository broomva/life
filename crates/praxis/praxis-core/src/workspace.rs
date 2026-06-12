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

    /// Resolve a path for writing. Neither the file nor its parent
    /// directories need exist yet (BRO-1490: `write_file artifacts/x.txt`
    /// into a fresh workspace must resolve, not ENOENT): the nearest
    /// EXISTING ancestor is canonicalized and boundary-checked, then the
    /// not-yet-existing components are re-appended. Those must be plain
    /// segments — a `..` (or stray `.`) past the verified prefix cannot be
    /// canonicalized (nothing to stat) and could escape the workspace once
    /// the directories are created, so it is rejected.
    pub fn resolve_for_write(&self, candidate: &Path) -> PraxisResult<PathBuf> {
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.workspace_root.join(candidate)
        };
        let outside = || PraxisError::PathOutsideWorkspace {
            path: joined.display().to_string(),
        };
        // `file_name()` is None for paths ending in `..` — reject those
        // outright rather than guessing what the caller meant to write.
        let file_name = joined.file_name().ok_or_else(outside)?.to_os_string();

        // Walk up to the nearest existing ancestor, collecting the
        // not-yet-existing directory components in between.
        let mut missing: Vec<std::ffi::OsString> = Vec::new();
        let mut ancestor = joined.parent().ok_or_else(outside)?;
        while !ancestor.exists() {
            // None ⇒ the ancestor ends in `..` (traversal through a
            // non-existing directory) — nothing trustworthy to anchor on.
            missing.push(ancestor.file_name().ok_or_else(outside)?.to_os_string());
            ancestor = ancestor.parent().ok_or_else(outside)?;
        }

        // Canonicalize the existing prefix and enforce the boundary THERE —
        // before any caller creates the missing directories under it.
        let canonical = ancestor.canonicalize()?;
        self.ensure_within_root(&canonical)?;

        let mut resolved = canonical;
        for component in missing.iter().rev() {
            if component == ".." || component == "." {
                return Err(outside());
            }
            resolved.push(component);
        }
        resolved.push(&file_name);
        Ok(resolved)
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

    // ── resolve_for_write with missing parents (BRO-1490) ───────────────

    #[test]
    fn resolve_for_write_accepts_missing_parent() {
        // The prod regression: `write_file artifacts/receipt.txt` into a
        // fresh workspace whose `artifacts/` does not exist yet.
        let dir = tempfile::tempdir().unwrap();
        let policy = FsPolicy::new(dir.path());

        let resolved = policy
            .resolve_for_write(Path::new("artifacts/receipt.txt"))
            .unwrap();

        let root = dir.path().canonicalize().unwrap();
        assert_eq!(resolved, root.join("artifacts/receipt.txt"));
        // Resolution must not create anything — that's the writer's job.
        assert!(!dir.path().join("artifacts").exists());
    }

    #[test]
    fn resolve_for_write_accepts_nested_missing_parents() {
        let dir = tempfile::tempdir().unwrap();
        let policy = FsPolicy::new(dir.path());

        let resolved = policy
            .resolve_for_write(Path::new("a/b/c/receipt.txt"))
            .unwrap();

        let root = dir.path().canonicalize().unwrap();
        assert_eq!(resolved, root.join("a/b/c/receipt.txt"));
    }

    #[test]
    fn resolve_for_write_existing_parent_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("artifacts")).unwrap();
        let policy = FsPolicy::new(dir.path());

        let resolved = policy
            .resolve_for_write(Path::new("artifacts/receipt.txt"))
            .unwrap();

        let root = dir.path().canonicalize().unwrap();
        assert_eq!(resolved, root.join("artifacts/receipt.txt"));
    }

    #[test]
    fn resolve_for_write_rejects_traversal_through_missing_dir() {
        // `ghost/` does not exist, so the `..` segments after it cannot be
        // canonicalized — allowing them could escape the workspace once the
        // directories are created.
        let dir = tempfile::tempdir().unwrap();
        let policy = FsPolicy::new(dir.path());

        let err = policy
            .resolve_for_write(Path::new("ghost/../../escape.txt"))
            .unwrap_err();
        assert!(matches!(err, PraxisError::PathOutsideWorkspace { .. }));
    }

    #[test]
    fn resolve_for_write_rejects_absolute_outside_with_missing_parents() {
        let dir = tempfile::tempdir().unwrap();
        let policy = FsPolicy::new(dir.path());

        // Nearest existing ancestor is the system temp dir — outside the
        // workspace — so the boundary check must reject it.
        let outside = std::env::temp_dir().join("praxis-resolve-for-write-nope/sub/file.txt");
        let err = policy.resolve_for_write(&outside).unwrap_err();
        assert!(matches!(err, PraxisError::PathOutsideWorkspace { .. }));
    }

    #[test]
    fn resolve_for_write_rejects_trailing_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let policy = FsPolicy::new(dir.path());

        let err = policy
            .resolve_for_write(Path::new("artifacts/.."))
            .unwrap_err();
        assert!(matches!(err, PraxisError::PathOutsideWorkspace { .. }));
    }

    #[test]
    fn resolve_for_write_traversal_within_existing_dirs_still_works() {
        // `..` through EXISTING directories canonicalizes safely — the
        // missing-suffix restriction must not break this.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        let policy = FsPolicy::new(dir.path());

        let resolved = policy
            .resolve_for_write(Path::new("sub/../newdir/file.txt"))
            .unwrap();

        let root = dir.path().canonicalize().unwrap();
        assert_eq!(resolved, root.join("newdir/file.txt"));
    }
}
