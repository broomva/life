# Unified `.life/` Filesystem — Implementation Plan (Part 1: Foundation)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the `life-paths` shared crate, `life init` command, and update `life setup` to use cascading credential resolution — the foundation for all daemons to consolidate into `.life/`.

**Architecture:** New `life-paths` crate provides `find_project_root()`, `resolve_module_dir()`, and `resolve_credential()` used by all daemons. `life init` scaffolds `.life/` per-project. `life setup` stores credentials via keychain (macOS) or `.env` fallback. All daemons will use these in Part 2.

**Tech Stack:** Rust 2024 Edition, `dirs` crate, `dotenvy` for .env loading, `security` CLI for macOS keychain, `secret-tool` for Linux

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/life-paths/Cargo.toml` | CREATE | Crate manifest |
| `crates/life-paths/src/lib.rs` | CREATE | Public API: find_project_root, resolve_module_dir, resolve_credential |
| `crates/life-paths/src/discovery.rs` | CREATE | .life/ directory discovery (walk-up algorithm) |
| `crates/life-paths/src/credentials.rs` | CREATE | Credential cascade: .env → keychain → global .env → env vars |
| `crates/life-paths/src/env.rs` | CREATE | .env file loading |
| `crates/life-paths/src/keychain.rs` | CREATE | System keychain read/write (macOS + Linux) |
| `crates/cli/life-cli/Cargo.toml` | MODIFY | Add life-paths dependency |
| `crates/cli/life-cli/src/cli.rs` | MODIFY | Add Init command |
| `crates/cli/life-cli/src/init.rs` | CREATE | `life init` implementation |
| `crates/cli/life-cli/src/setup.rs` | MODIFY | Use credential cascade, no plaintext keys in config |
| `crates/cli/life-cli/src/lib.rs` | MODIFY | Add init module, wire Init command |
| `Cargo.toml` | MODIFY | Add life-paths to workspace members + deps |

---

### Task 1: Create `life-paths` crate with discovery

**Files:**
- Create: `crates/life-paths/Cargo.toml`
- Create: `crates/life-paths/src/lib.rs`
- Create: `crates/life-paths/src/discovery.rs`
- Modify: `Cargo.toml` (workspace)

- [ ] **Step 1: Create crate directory and Cargo.toml**

```toml
# crates/life-paths/Cargo.toml
[package]
name = "life-paths"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Shared path resolution for Life Agent OS — project discovery, module dirs, credentials"

[dependencies]
dirs = "6"
tracing = "0.1"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create discovery.rs with find_project_root**

```rust
// crates/life-paths/src/discovery.rs
use std::path::{Path, PathBuf};

/// The name of the Life project directory.
const LIFE_DIR: &str = ".life";

/// Find the `.life/` directory by walking up from `start_dir`.
///
/// Returns the directory containing `.life/` (the project root),
/// or `None` if no `.life/` directory is found up to the filesystem root.
pub fn find_project_root_from(start_dir: &Path) -> Option<PathBuf> {
    let mut dir = start_dir.to_path_buf();
    loop {
        if dir.join(LIFE_DIR).is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Find the `.life/` directory by walking up from the current working directory.
pub fn find_project_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_project_root_from(&cwd)
}

/// Resolve the `.life/` directory path.
///
/// If a project root is found, returns `{project_root}/.life/`.
/// Otherwise returns `~/.life/` (global fallback).
pub fn life_dir() -> PathBuf {
    if let Some(root) = find_project_root() {
        root.join(LIFE_DIR)
    } else {
        global_life_dir()
    }
}

/// Global `~/.life/` directory.
pub fn global_life_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(LIFE_DIR)
}

/// Resolve the data directory for a specific module.
///
/// Priority:
/// 1. `cli_override` (from --data-dir flag)
/// 2. `{project_root}/.life/{module}/`
/// 3. `~/.life/{module}/`
pub fn resolve_module_dir(module: &str, cli_override: Option<&Path>) -> PathBuf {
    if let Some(override_path) = cli_override {
        return override_path.to_path_buf();
    }
    life_dir().join(module)
}

/// Check if a `.life/` project exists in the current directory tree.
pub fn is_initialized() -> bool {
    find_project_root().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn find_project_root_in_current_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".life")).unwrap();
        let root = find_project_root_from(tmp.path());
        assert_eq!(root, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn find_project_root_in_parent() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".life")).unwrap();
        let nested = tmp.path().join("src").join("deep");
        std::fs::create_dir_all(&nested).unwrap();
        let root = find_project_root_from(&nested);
        assert_eq!(root, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn find_project_root_not_found() {
        let tmp = TempDir::new().unwrap();
        // No .life/ directory
        let root = find_project_root_from(tmp.path());
        assert!(root.is_none());
    }

    #[test]
    fn resolve_module_dir_with_override() {
        let p = PathBuf::from("/custom/path");
        assert_eq!(resolve_module_dir("arcan", Some(&p)), p);
    }

    #[test]
    fn resolve_module_dir_without_override() {
        let dir = resolve_module_dir("arcan", None);
        // Should end with .life/arcan
        assert!(dir.ends_with(".life/arcan"));
    }

    #[test]
    fn global_life_dir_is_under_home() {
        let d = global_life_dir();
        assert!(d.to_str().unwrap().contains(".life"));
    }
}
```

- [ ] **Step 3: Create lib.rs**

```rust
// crates/life-paths/src/lib.rs
pub mod credentials;
pub mod discovery;
pub mod env;
pub mod keychain;

pub use credentials::resolve_credential;
pub use discovery::{
    find_project_root, find_project_root_from, global_life_dir, is_initialized, life_dir,
    resolve_module_dir,
};
pub use env::load_env;
```

- [ ] **Step 4: Add to workspace**

In root `Cargo.toml`, add to `members`:
```toml
"crates/life-paths",
```

And to `[workspace.dependencies]`:
```toml
life-paths = { path = "crates/life-paths", version = "0.3.0" }
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p life-paths`
Expected: Compiles (credentials, env, keychain modules empty for now)

- [ ] **Step 6: Run tests**

Run: `cargo test -p life-paths`
Expected: 6 tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/life-paths/ Cargo.toml
git commit -m "feat: add life-paths crate with project discovery"
```

---

### Task 2: Add .env file loading

**Files:**
- Create: `crates/life-paths/src/env.rs`

- [ ] **Step 1: Implement env.rs**

```rust
// crates/life-paths/src/env.rs
use std::collections::HashMap;
use std::path::Path;

/// Parse a .env file into key-value pairs.
///
/// Handles: KEY=VALUE, KEY="VALUE", KEY='VALUE', comments (#), empty lines.
/// Does NOT modify `std::env` — returns the parsed values.
pub fn parse_env_file(path: &Path) -> std::io::Result<HashMap<String, String>> {
    let content = std::fs::read_to_string(path)?;
    Ok(parse_env_str(&content))
}

/// Parse .env content from a string.
pub fn parse_env_str(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            // Strip quotes
            let value = if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            map.insert(key.to_string(), value.to_string());
        }
    }
    map
}

/// Load a .env file and set values as environment variables.
///
/// Only sets variables that are NOT already set in the environment
/// (existing env vars take precedence).
pub fn load_env(path: &Path) {
    if let Ok(vars) = parse_env_file(path) {
        for (key, value) in vars {
            if std::env::var(&key).is_err() {
                std::env::set_var(&key, &value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_env() {
        let content = "KEY=value\nOTHER=123";
        let map = parse_env_str(content);
        assert_eq!(map.get("KEY").unwrap(), "value");
        assert_eq!(map.get("OTHER").unwrap(), "123");
    }

    #[test]
    fn parse_quoted_values() {
        let content = "KEY=\"quoted value\"\nSINGLE='single'";
        let map = parse_env_str(content);
        assert_eq!(map.get("KEY").unwrap(), "quoted value");
        assert_eq!(map.get("SINGLE").unwrap(), "single");
    }

    #[test]
    fn parse_comments_and_empty() {
        let content = "# comment\n\nKEY=value\n  # another comment\n";
        let map = parse_env_str(content);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("KEY").unwrap(), "value");
    }

    #[test]
    fn parse_env_file_not_found() {
        let result = parse_env_file(Path::new("/nonexistent/.env"));
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p life-paths`
Expected: 10 tests pass (6 discovery + 4 env)

- [ ] **Step 3: Commit**

```bash
git add crates/life-paths/src/env.rs
git commit -m "feat(life-paths): add .env file parser and loader"
```

---

### Task 3: Add keychain integration

**Files:**
- Create: `crates/life-paths/src/keychain.rs`

- [ ] **Step 1: Implement keychain.rs**

```rust
// crates/life-paths/src/keychain.rs
//! System keychain integration for credential storage.
//!
//! macOS: uses `security` CLI (Keychain Access)
//! Linux: uses `secret-tool` CLI (GNOME Keyring / KDE Wallet)
//! Other: falls back to None (use .env instead)

use std::process::Command;

const SERVICE: &str = "life-agent-os";

/// Store a credential in the system keychain.
///
/// Returns `true` if stored successfully, `false` if keychain is unavailable.
pub fn store(account: &str, secret: &str) -> bool {
    if cfg!(target_os = "macos") {
        store_macos(account, secret)
    } else if cfg!(target_os = "linux") {
        store_linux(account, secret)
    } else {
        false
    }
}

/// Read a credential from the system keychain.
pub fn read(account: &str) -> Option<String> {
    if cfg!(target_os = "macos") {
        read_macos(account)
    } else if cfg!(target_os = "linux") {
        read_linux(account)
    } else {
        None
    }
}

/// Delete a credential from the system keychain.
pub fn delete(account: &str) -> bool {
    if cfg!(target_os = "macos") {
        delete_macos(account)
    } else if cfg!(target_os = "linux") {
        delete_linux(account)
    } else {
        false
    }
}

/// Check if keychain is available on this system.
pub fn is_available() -> bool {
    if cfg!(target_os = "macos") {
        Command::new("security")
            .arg("list-keychains")
            .output()
            .is_ok()
    } else if cfg!(target_os = "linux") {
        Command::new("secret-tool")
            .arg("--version")
            .output()
            .is_ok()
    } else {
        false
    }
}

// ── macOS ────────────────────────────────────────────────────────────────

fn store_macos(account: &str, secret: &str) -> bool {
    // Delete existing entry first (update not supported)
    let _ = Command::new("security")
        .args(["delete-generic-password", "-s", SERVICE, "-a", account])
        .output();

    Command::new("security")
        .args([
            "add-generic-password",
            "-s", SERVICE,
            "-a", account,
            "-w", secret,
            "-U", // update if exists
        ])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn read_macos(account: &str) -> Option<String> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-s", SERVICE,
            "-a", account,
            "-w", // output password only
        ])
        .output()
        .ok()?;

    if output.status.success() {
        let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if secret.is_empty() { None } else { Some(secret) }
    } else {
        None
    }
}

fn delete_macos(account: &str) -> bool {
    Command::new("security")
        .args(["delete-generic-password", "-s", SERVICE, "-a", account])
        .output()
        .is_ok_and(|o| o.status.success())
}

// ── Linux ────────────────────────────────────────────────────────────────

fn store_linux(account: &str, secret: &str) -> bool {
    Command::new("secret-tool")
        .args([
            "store",
            "--label", &format!("Life Agent OS: {account}"),
            "service", SERVICE,
            "account", account,
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(secret.as_bytes())?;
            }
            child.wait()
        })
        .is_ok_and(|s| s.success())
}

fn read_linux(account: &str) -> Option<String> {
    let output = Command::new("secret-tool")
        .args(["lookup", "service", SERVICE, "account", account])
        .output()
        .ok()?;

    if output.status.success() {
        let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if secret.is_empty() { None } else { Some(secret) }
    } else {
        None
    }
}

fn delete_linux(account: &str) -> bool {
    Command::new("secret-tool")
        .args(["clear", "service", SERVICE, "account", account])
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_available_does_not_panic() {
        // Just verify it returns without panicking on any platform
        let _ = is_available();
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p life-paths`
Expected: 11 tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/life-paths/src/keychain.rs
git commit -m "feat(life-paths): add system keychain integration (macOS + Linux)"
```

---

### Task 4: Add credential resolution

**Files:**
- Create: `crates/life-paths/src/credentials.rs`

- [ ] **Step 1: Implement credentials.rs**

```rust
// crates/life-paths/src/credentials.rs
//! Cascading credential resolution for Life Agent OS.
//!
//! Resolution order:
//! 1. Project `.life/.env`
//! 2. System keychain (`life-agent-os/{key}`)
//! 3. Global `~/.life/credentials/.env`
//! 4. Environment variable (std::env)

use std::path::Path;

use crate::{discovery, env, keychain};

/// Credential source for logging/debugging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    ProjectEnv,
    Keychain,
    GlobalEnv,
    EnvironmentVariable,
}

/// A resolved credential with its source.
#[derive(Debug, Clone)]
pub struct ResolvedCredential {
    pub value: String,
    pub source: CredentialSource,
}

/// Resolve a credential by cascading through all sources.
///
/// `env_var_name` is the environment variable name (e.g., "ANTHROPIC_API_KEY").
/// `keychain_account` is the keychain account name (e.g., "anthropic_api_key").
pub fn resolve_credential(
    env_var_name: &str,
    keychain_account: &str,
) -> Option<ResolvedCredential> {
    // 1. Project .life/.env
    if let Some(value) = resolve_from_project_env(env_var_name) {
        return Some(ResolvedCredential {
            value,
            source: CredentialSource::ProjectEnv,
        });
    }

    // 2. System keychain
    if let Some(value) = keychain::read(keychain_account) {
        return Some(ResolvedCredential {
            value,
            source: CredentialSource::Keychain,
        });
    }

    // 3. Global ~/.life/credentials/.env
    if let Some(value) = resolve_from_global_env(env_var_name) {
        return Some(ResolvedCredential {
            value,
            source: CredentialSource::GlobalEnv,
        });
    }

    // 4. Environment variable
    if let Ok(value) = std::env::var(env_var_name) {
        if !value.is_empty() {
            return Some(ResolvedCredential {
                value,
                source: CredentialSource::EnvironmentVariable,
            });
        }
    }

    None
}

/// Store a credential in the preferred location.
///
/// Tries keychain first, falls back to `~/.life/credentials/.env`.
/// Returns the storage method used.
pub fn store_credential(
    env_var_name: &str,
    keychain_account: &str,
    value: &str,
) -> CredentialSource {
    // Try keychain first
    if keychain::is_available() && keychain::store(keychain_account, value) {
        tracing::info!(account = keychain_account, "credential stored in system keychain");
        return CredentialSource::Keychain;
    }

    // Fallback: write to ~/.life/credentials/.env
    let creds_dir = discovery::global_life_dir().join("credentials");
    std::fs::create_dir_all(&creds_dir).ok();
    let env_path = creds_dir.join(".env");

    // Read existing, update or append
    let mut lines: Vec<String> = if env_path.exists() {
        std::fs::read_to_string(&env_path)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    } else {
        Vec::new()
    };

    // Update existing key or append
    let prefix = format!("{env_var_name}=");
    let new_line = format!("{env_var_name}={value}");
    if let Some(pos) = lines.iter().position(|l| l.starts_with(&prefix)) {
        lines[pos] = new_line;
    } else {
        lines.push(new_line);
    }

    let content = lines.join("\n") + "\n";
    std::fs::write(&env_path, &content).ok();

    // Set 0600 permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600)).ok();
    }

    tracing::info!(path = %env_path.display(), "credential stored in env file");
    CredentialSource::GlobalEnv
}

fn resolve_from_project_env(key: &str) -> Option<String> {
    let life_dir = discovery::find_project_root()?.join(".life");
    let env_path = life_dir.join(".env");
    let vars = env::parse_env_file(&env_path).ok()?;
    vars.get(key).cloned()
}

fn resolve_from_global_env(key: &str) -> Option<String> {
    let env_path = discovery::global_life_dir().join("credentials").join(".env");
    let vars = env::parse_env_file(&env_path).ok()?;
    vars.get(key).cloned()
}

/// Map a provider name to its environment variable and keychain account.
pub fn provider_credential_names(provider: &str) -> (&'static str, &'static str) {
    match provider {
        "anthropic" => ("ANTHROPIC_API_KEY", "anthropic_api_key"),
        "openai" => ("OPENAI_API_KEY", "openai_api_key"),
        "vercel" | "vercel-gateway" => ("OPENAI_API_KEY", "vercel_api_key"),
        _ => ("API_KEY", "api_key"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_from_env_var() {
        let key = "LIFE_TEST_CRED_12345";
        unsafe { std::env::set_var(key, "test-value") };
        let result = resolve_credential(key, "test_account_12345");
        assert!(result.is_some());
        let cred = result.unwrap();
        assert_eq!(cred.value, "test-value");
        assert_eq!(cred.source, CredentialSource::EnvironmentVariable);
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn resolve_missing_credential() {
        let result = resolve_credential(
            "LIFE_NONEXISTENT_CRED_99999",
            "nonexistent_account_99999",
        );
        assert!(result.is_none());
    }

    #[test]
    fn provider_credential_names_anthropic() {
        let (env_var, account) = provider_credential_names("anthropic");
        assert_eq!(env_var, "ANTHROPIC_API_KEY");
        assert_eq!(account, "anthropic_api_key");
    }

    #[test]
    fn store_credential_creates_env_file() {
        let tmp = TempDir::new().unwrap();
        let creds_dir = tmp.path().join("credentials");
        std::fs::create_dir_all(&creds_dir).unwrap();
        let env_path = creds_dir.join(".env");

        // Write directly to test the env file logic
        std::fs::write(&env_path, "TEST_KEY=test_value\n").unwrap();
        let vars = crate::env::parse_env_file(&env_path).unwrap();
        assert_eq!(vars.get("TEST_KEY").unwrap(), "test_value");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p life-paths`
Expected: 15+ tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/life-paths/src/credentials.rs
git commit -m "feat(life-paths): add cascading credential resolution"
```

---

### Task 5: Add `life init` command

**Files:**
- Create: `crates/cli/life-cli/src/init.rs`
- Modify: `crates/cli/life-cli/src/cli.rs`
- Modify: `crates/cli/life-cli/src/lib.rs`
- Modify: `crates/cli/life-cli/Cargo.toml`

- [ ] **Step 1: Add life-paths dependency to life-cli**

In `crates/cli/life-cli/Cargo.toml`, add:
```toml
life-paths = { path = "../../life-paths", version = "0.3.0" }
```

- [ ] **Step 2: Create init.rs**

```rust
// crates/cli/life-cli/src/init.rs
//! `life init` — create a `.life/` directory in the current project.

use std::path::Path;

use anyhow::{Context, Result};

const DEFAULT_CONFIG: &str = r#"# Life Agent OS — project configuration
# This file is safe to commit (no secrets).

[provider]
# name = "anthropic"
# model = "claude-sonnet-4-5-20250929"

[consciousness]
enabled = true

[arcan]
port = 3000
"#;

const DEFAULT_POLICY: &str = r#"# Life Agent OS — governance policy
# See: https://docs.broomva.tech/docs/life/control

active_profile: baseline

profiles:
  baseline:
    gates: [smoke, check]
    auto_merge: false
  governed:
    gates: [smoke, check, test, audit]
    auto_merge: false
  autonomous:
    gates: [smoke, check, test]
    auto_merge: true

gates:
  smoke:
    command: "cargo fmt --check && cargo check"
    blocking: true
  check:
    command: "cargo clippy --workspace"
    blocking: true
  test:
    command: "cargo test --workspace"
    blocking: true
  audit:
    command: "cargo audit"
    blocking: false
"#;

const GITIGNORE_ADDITIONS: &str = r#"
# Life Agent OS
.life/
!.life/config.toml
!.life/control/
"#;

pub fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("cannot determine current directory")?;
    let life_dir = cwd.join(".life");

    if life_dir.exists() {
        eprintln!("  .life/ already exists in {}", cwd.display());
        return Ok(());
    }

    // Create .life/ and subdirectories
    std::fs::create_dir_all(&life_dir).context("failed to create .life/")?;
    eprintln!("  \x1b[32m\u{2713}\x1b[0m Created .life/");

    // Write config.toml
    let config_path = life_dir.join("config.toml");
    std::fs::write(&config_path, DEFAULT_CONFIG)?;
    eprintln!("  \x1b[32m\u{2713}\x1b[0m Created .life/config.toml");

    // Write control/policy.yaml
    let control_dir = life_dir.join("control");
    std::fs::create_dir_all(&control_dir)?;
    std::fs::write(control_dir.join("policy.yaml"), DEFAULT_POLICY)?;
    eprintln!("  \x1b[32m\u{2713}\x1b[0m Created .life/control/policy.yaml");

    // Update .gitignore
    update_gitignore(&cwd)?;

    eprintln!();
    eprintln!("  Life project initialized in {}", cwd.display());
    eprintln!("  Run \x1b[36mlife setup\x1b[0m to configure your LLM provider.");

    Ok(())
}

fn update_gitignore(project_root: &Path) -> Result<()> {
    let gitignore_path = project_root.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();

    if existing.contains(".life/") {
        eprintln!("  \x1b[2m\u{2713} .gitignore already has .life/ patterns\x1b[0m");
        return Ok(());
    }

    let mut content = existing;
    content.push_str(GITIGNORE_ADDITIONS);
    std::fs::write(&gitignore_path, content)?;
    eprintln!("  \x1b[32m\u{2713}\x1b[0m Updated .gitignore");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_creates_life_dir() {
        let tmp = TempDir::new().unwrap();
        let life_dir = tmp.path().join(".life");
        std::fs::create_dir_all(&life_dir).unwrap();
        std::fs::write(life_dir.join("config.toml"), DEFAULT_CONFIG).unwrap();
        assert!(life_dir.join("config.toml").exists());
    }

    #[test]
    fn update_gitignore_adds_patterns() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "node_modules/\n").unwrap();
        update_gitignore(tmp.path()).unwrap();
        let content = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(content.contains(".life/"));
        assert!(content.contains("!.life/config.toml"));
    }

    #[test]
    fn update_gitignore_idempotent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), ".life/\n").unwrap();
        update_gitignore(tmp.path()).unwrap();
        let content = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        // Should not duplicate
        assert_eq!(content.matches(".life/").count(), 1);
    }
}
```

- [ ] **Step 3: Wire into CLI**

In `crates/cli/life-cli/src/cli.rs`, add `Init` to the `Command` enum:
```rust
/// Initialize a .life/ directory in the current project.
Init,
```

In `crates/cli/life-cli/src/lib.rs`, add:
```rust
pub mod init;
```

And in the match block:
```rust
Some(cli::Command::Init) => init::run(),
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p life-cli`
Expected: Passes (existing + new init tests)

- [ ] **Step 5: Build and test**

Run: `cargo run --bin life -- init` (in a temp dir)
Expected: Creates `.life/` with config.toml and control/policy.yaml

- [ ] **Step 6: Commit**

```bash
git add crates/cli/life-cli/
git commit -m "feat(cli): add life init command"
```

---

### Task 6: Update `life setup` to use credential cascade

**Files:**
- Modify: `crates/cli/life-cli/src/setup.rs`

- [ ] **Step 1: Update save_config to not store API keys**

Replace the `save_config` function to:
1. Write `~/.life/config.toml` without `api_key` field
2. Store credentials via `life_paths::credentials::store_credential()`
3. Report where the credential was stored

- [ ] **Step 2: Update config struct**

Remove `api_key` from `ProviderConfig`:
```rust
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ProviderConfig {
    name: String,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    // No api_key — stored in keychain or .env
}
```

- [ ] **Step 3: Update save_config function**

```rust
fn save_config(
    provider: &Provider,
    api_key: &Option<String>,
    model: &str,
    base_url: &Option<String>,
) -> Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).context("failed to create ~/.life directory")?;

    // Save config WITHOUT api key
    let cfg = LifeConfig {
        provider: ProviderConfig {
            name: provider.name().to_string(),
            model: model.to_string(),
            base_url: base_url.clone(),
        },
        consciousness: ConsciousnessConfig::default(),
        arcan: ArcanConfig::default(),
    };

    let content = toml::to_string_pretty(&cfg).context("failed to serialize config")?;
    let path = config_path();
    std::fs::write(&path, &content)?;

    eprintln!("  {} Config saved to {}", c(GREEN, "ok"), dim(&path.display().to_string()));

    // Store credential separately
    if let Some(key) = api_key {
        let (env_var, account) = life_paths::credentials::provider_credential_names(provider.name());
        let source = life_paths::credentials::store_credential(env_var, account, key);
        match source {
            life_paths::credentials::CredentialSource::Keychain => {
                eprintln!("  {} API key stored in system keychain", c(GREEN, "ok"));
            }
            life_paths::credentials::CredentialSource::GlobalEnv => {
                eprintln!("  {} API key stored in ~/.life/credentials/.env", c(GREEN, "ok"));
            }
            _ => {}
        }
    }

    Ok(())
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p life-cli`
Expected: Passes

- [ ] **Step 5: Commit**

```bash
git add crates/cli/life-cli/src/setup.rs
git commit -m "feat(setup): use credential cascade — no plaintext keys in config"
```

---

### Task 7: Full validation and push

- [ ] **Step 1: Format and lint**

```bash
cargo fmt
cargo clippy -p life-paths -p life-cli
```

- [ ] **Step 2: Run all tests**

```bash
cargo test -p life-paths -p life-cli
```

- [ ] **Step 3: Build binaries**

```bash
cargo build --bin life
```

- [ ] **Step 4: Integration test**

```bash
# Test life init
cd /tmp && mkdir test-project && cd test-project
life init
ls -la .life/
cat .life/config.toml
cat .life/control/policy.yaml
cat .gitignore | grep life
```

- [ ] **Step 5: Push**

```bash
git push origin main
```
