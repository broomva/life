//! `life init` — create a `.life/` directory in the current project.
//!
//! Initializes a project-local `.life/` configuration directory with sensible
//! defaults.  Secrets are never written to config files — they live in the
//! system keychain or `~/.life/credentials/.env` (managed by `life setup`).

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

// ── ANSI helpers ──────────────────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";

fn use_color() -> bool {
    io::stdout().is_terminal()
}

fn c(code: &str, text: &str) -> String {
    if use_color() {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

// ── Default content ───────────────────────────────────────────────────────

const DEFAULT_CONFIG_TOML: &str = r#"# Life Agent OS — project configuration
# Secrets are stored in the system keychain (life setup).

[provider]
name = "anthropic"
model = "claude-sonnet-4-5-20250929"

[consciousness]
enabled = true

[arcan]
port = 3000
"#;

const DEFAULT_POLICY_YAML: &str = r#"# Life Agent OS — control policy
# Profiles define escalating levels of autonomy.
# Gates are sequential quality checks.

profiles:
  baseline:
    description: "Default profile — manual approval required"
    auto_approve: false
  governed:
    description: "CI-governed — auto-approve if gates pass"
    auto_approve: true
    require_gates: [smoke, check]
  autonomous:
    description: "Full autonomy — all gates must pass"
    auto_approve: true
    require_gates: [smoke, check, test, audit]

gates:
  smoke:
    description: "Quick format/syntax/build check"
    command: "cargo fmt --check && cargo check"
    timeout_secs: 30
  check:
    description: "Format + clippy + test"
    command: "cargo fmt --check && cargo clippy --workspace && cargo test --workspace"
    timeout_secs: 120
  test:
    description: "Full test suite"
    command: "cargo test --workspace"
    timeout_secs: 300
  audit:
    description: "Governance compliance audit"
    command: "make audit"
    timeout_secs: 60
"#;

const GITIGNORE_PATTERNS: &[&str] = &[
    "# Life Agent OS",
    ".life/*",
    "!.life/config.toml",
    "!.life/control/",
    ".life/credentials/",
];

// ── Core logic ────────────────────────────────────────────────────────────

/// Create the `.life/` directory tree in `root`.
fn create_life_dir(root: &Path) -> Result<PathBuf> {
    let life_dir = root.join(".life");
    std::fs::create_dir_all(&life_dir).context("failed to create .life/ directory")?;
    std::fs::create_dir_all(life_dir.join("control")).context("failed to create .life/control/")?;
    Ok(life_dir)
}

/// Write `.life/config.toml` with default provider settings.
fn write_config(life_dir: &Path) -> Result<()> {
    let path = life_dir.join("config.toml");
    std::fs::write(&path, DEFAULT_CONFIG_TOML)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Write `.life/control/policy.yaml` with default governance rules.
fn write_policy(life_dir: &Path) -> Result<()> {
    let path = life_dir.join("control").join("policy.yaml");
    std::fs::write(&path, DEFAULT_POLICY_YAML)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Append `.life/` gitignore patterns to the project `.gitignore`.
/// Idempotent — skips if the sentinel pattern is already present.
fn update_gitignore(root: &Path) -> Result<()> {
    let gitignore_path = root.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();

    if existing.contains(".life/*") {
        return Ok(());
    }

    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push('\n');
    for pattern in GITIGNORE_PATTERNS {
        content.push_str(pattern);
        content.push('\n');
    }

    std::fs::write(&gitignore_path, &content)
        .with_context(|| format!("failed to write {}", gitignore_path.display()))?;
    Ok(())
}

/// Print a colored check line to stderr.
fn check(msg: &str) {
    eprintln!("  {} {msg}", c(GREEN, "✓"));
}

// ── Public entry point ────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let root = std::env::current_dir().context("failed to determine current directory")?;

    // Guard: already initialized
    if root.join(".life").join("config.toml").is_file() {
        eprintln!(
            "  {} .life/ already initialized in {}",
            c(CYAN, "●"),
            c(DIM, &root.display().to_string()),
        );
        eprintln!(
            "  {} Run {} to reconfigure.",
            c(DIM, "→"),
            c(CYAN, "life setup"),
        );
        return Ok(());
    }

    eprintln!();
    eprintln!(
        "  Initializing .life/ in {}",
        c(DIM, &root.display().to_string())
    );
    eprintln!();

    let life_dir = create_life_dir(&root)?;
    check("Created .life/ directory");

    write_config(&life_dir)?;
    check("Wrote .life/config.toml");

    write_policy(&life_dir)?;
    check("Wrote .life/control/policy.yaml");

    update_gitignore(&root)?;
    check("Updated .gitignore");

    eprintln!();
    eprintln!("  {} Project initialized.", c(GREEN, "✓"));
    eprintln!(
        "  Run {} to configure providers & credentials.",
        c(CYAN, "life setup")
    );
    eprintln!();

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        let life_dir = create_life_dir(root).unwrap();
        write_config(&life_dir).unwrap();
        write_policy(&life_dir).unwrap();

        let config = std::fs::read_to_string(root.join(".life/config.toml")).unwrap();
        assert!(config.contains("[provider]"));
        assert!(config.contains("name = \"anthropic\""));
        assert!(config.contains("[consciousness]"));
        assert!(config.contains("enabled = true"));
        assert!(config.contains("[arcan]"));
        assert!(config.contains("port = 3000"));
        // Must NOT contain api_key
        assert!(!config.contains("api_key"));

        let policy = std::fs::read_to_string(root.join(".life/control/policy.yaml")).unwrap();
        assert!(policy.contains("profiles:"));
        assert!(policy.contains("baseline:"));
        assert!(policy.contains("governed:"));
        assert!(policy.contains("autonomous:"));
        assert!(policy.contains("gates:"));
        assert!(policy.contains("smoke:"));
    }

    #[test]
    fn update_gitignore_adds_patterns() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // Start with an existing .gitignore
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        update_gitignore(root).unwrap();

        let content = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(content.contains("target/"));
        assert!(content.contains(".life/*"));
        assert!(content.contains("!.life/config.toml"));
        assert!(content.contains("!.life/control/"));
        assert!(content.contains(".life/credentials/"));
    }

    #[test]
    fn update_gitignore_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        std::fs::write(root.join(".gitignore"), "").unwrap();
        update_gitignore(root).unwrap();
        let first = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        let first_count = first.matches(".life/*").count();

        update_gitignore(root).unwrap();
        let second = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        let second_count = second.matches(".life/*").count();

        assert_eq!(first_count, 1);
        assert_eq!(second_count, 1);
    }
}
