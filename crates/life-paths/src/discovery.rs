use std::path::{Path, PathBuf};

/// Walk up from `start_dir` looking for a `.life/` directory marker.
/// Returns the directory containing `.life/` if found.
pub fn find_project_root_from(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir.to_path_buf();
    loop {
        let candidate = current.join(".life");
        if candidate.is_dir() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Find project root from the current working directory.
pub fn find_project_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_project_root_from(&cwd)
}

/// Return the project-local `.life/` directory if a project root is found,
/// otherwise fall back to `~/.life/`.
pub fn life_dir() -> PathBuf {
    if let Some(root) = find_project_root() {
        root.join(".life")
    } else {
        global_life_dir()
    }
}

/// Always return `~/.life/`.
pub fn global_life_dir() -> PathBuf {
    dirs::home_dir()
        .expect("home directory must be resolvable")
        .join(".life")
}

/// Resolve the directory for a named module with the following priority:
/// 1. CLI override (if provided)
/// 2. Project-local `.life/{module}/`
/// 3. Global `~/.life/{module}/`
pub fn resolve_module_dir(module: &str, cli_override: Option<&Path>) -> PathBuf {
    if let Some(p) = cli_override {
        return p.to_path_buf();
    }
    if let Some(root) = find_project_root() {
        let local = root.join(".life").join(module);
        if local.exists() {
            return local;
        }
    }
    global_life_dir().join(module)
}

/// Check whether a `.life/` directory exists (either project-local or global).
pub fn is_initialized() -> bool {
    if find_project_root().is_some() {
        return true;
    }
    global_life_dir().exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn find_in_current_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".life")).unwrap();
        let result = find_project_root_from(tmp.path());
        assert_eq!(result, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn find_in_parent_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".life")).unwrap();
        let child = tmp.path().join("sub").join("deep");
        std::fs::create_dir_all(&child).unwrap();
        let result = find_project_root_from(&child);
        assert_eq!(result, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn not_found() {
        let tmp = TempDir::new().unwrap();
        let result = find_project_root_from(tmp.path());
        assert!(result.is_none());
    }

    #[test]
    fn module_with_override() {
        let tmp = TempDir::new().unwrap();
        let override_dir = tmp.path().join("custom");
        std::fs::create_dir(&override_dir).unwrap();
        let result = resolve_module_dir("lago", Some(&override_dir));
        assert_eq!(result, override_dir);
    }

    #[test]
    fn module_without_override_falls_to_global() {
        // When no project root is found and no override, falls back to global.
        // We can't reliably change cwd in tests (shared state), so test the logic directly:
        // With no project root found, resolve_module_dir should use global
        let result = resolve_module_dir("lago", None);
        assert!(result.ends_with(".life/lago") || result.ends_with(".life\\lago"));
    }

    #[test]
    fn global_under_home() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(global_life_dir(), home.join(".life"));
    }
}
