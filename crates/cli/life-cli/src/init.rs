//! `life init` — bootstrap a Life agent in the current project.
//!
//! Creates a project-local `.life/` directory with sensible defaults AND
//! instantiates the agent's foundational substrates so the project is
//! immediately runnable. Today that means:
//!
//! 1. **Project config** — `.life/config.toml` (provider, consciousness, arcan)
//! 2. **Control policy** — `.life/control/policy.yaml`
//! 3. **Anima identity** — `.life/identity/soul.json` + `.life/identity/seed.local.bin`
//!    via `InProcessAnima` (Spec D D-Sub-A). This produces:
//!      - a `did:key:zDn…` (P-256 multicodec `0x1200`) auth identity, and
//!      - a derived secp256k1 EVM wallet address on Base (Haima/x402-compatible).
//! 4. **`.gitignore` patches** — never commits the seed, always commits soul.json.
//!
//! Analogous to `zero init`, but routed through the `AnimaCustody` trait so
//! production deployments can later swap to Vault / TPM / WebCrypto / Soma /
//! HardwareWallet without touching agent code.
//!
//! Secrets discipline:
//!   - `.life/identity/seed.local.bin` is the raw 32-byte master seed, written
//!     with `0o600` on Unix. Never committed (see `.gitignore`).
//!   - `.life/credentials/` (managed by `life setup`) stores LLM API keys
//!     in the system keychain with `.env` fallback. Never written by `life init`.
//!   - At-rest encryption of the seed (via `EncryptedSeed` + keychain
//!     passphrase) is a Phase C upgrade; for InProcess dev mode `0o600` matches
//!     the protection Zero's `~/.zero/config.json` provides.

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use anima_core::soul::{AgentSoul, SoulBuilder};
use anima_identity::{InProcessAnima, MasterSeed};
use anyhow::{Context, Result};

// ── ANSI helpers ──────────────────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";

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

/// `.gitignore` patterns appended by `life init`.
///
/// Layered rules:
/// - Ignore everything in `.life/` by default.
/// - Allow `config.toml`, `control/`, and `identity/soul.json` (public agent
///   identity descriptor — committable so collaborators see the same agent).
/// - Explicitly ignore the seed file even though `.life/*` already excludes
///   it (defense in depth — surface the rule next to the identity file).
const GITIGNORE_PATTERNS: &[&str] = &[
    "# Life Agent OS",
    ".life/*",
    "!.life/config.toml",
    "!.life/control/",
    "!.life/identity/",
    ".life/identity/seed.local.bin",
    ".life/identity/*.local.*",
    ".life/credentials/",
];

/// Sentinel pattern used to detect whether `.life/` gitignore entries have
/// already been appended. Must remain stable across versions; bumping it would
/// re-append the block on every existing project.
const GITIGNORE_SENTINEL: &str = "!.life/identity/";

// ── Public types ──────────────────────────────────────────────────────────

/// Result of a `life init` run. Returned so callers (tests, library
/// consumers) can introspect what happened without reparsing files.
#[derive(Debug, Clone)]
pub struct InitSummary {
    /// Absolute path of the `.life/` directory.
    pub life_dir: PathBuf,
    /// Identity bootstrap result.
    pub identity: IdentitySummary,
    /// Whether identity was created in this run (vs. already existed).
    pub identity_created: bool,
}

/// Identity bootstrap result — surfaced both in tests and on the CLI.
#[derive(Debug, Clone)]
pub struct IdentitySummary {
    /// User DID — `did:key:zDn…` (P-256 multicodec `0x1200`).
    pub did: String,
    /// EVM wallet address (Base mainnet, eip155:8453).
    pub wallet_address: String,
    /// Blake3 soul hash.
    pub soul_hash: String,
    /// Custody backend kind. Currently always `"in_process"`; future work
    /// adds `vault` / `tpm` / `web_crypto` / `soma` / `hardware_wallet`.
    pub custody_kind: String,
    /// Absolute path of `.life/identity/`.
    pub identity_dir: PathBuf,
}

// ── Soul-on-disk schema ──────────────────────────────────────────────────

/// On-disk shape of `.life/identity/soul.json`.
///
/// Schema-versioned so future migrations can read the file without breaking.
/// The full `AgentSoul` is nested under `soul` for round-trip fidelity; the
/// flat fields (`did`, `wallet`, `soul_hash`, `custody`) exist for quick
/// inspection from shell scripts and other tools that don't pull in
/// `anima-core`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SoulDocument {
    schema_version: u32,
    did: String,
    wallet: WalletDescriptor,
    soul_hash: String,
    custody: CustodyDescriptor,
    soul: AgentSoul,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct WalletDescriptor {
    address: String,
    chain: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CustodyDescriptor {
    kind: String,
    seed_file: String,
}

// ── Filesystem helpers ────────────────────────────────────────────────────

/// Create the `.life/` directory tree in `root`.
fn create_life_dir(root: &Path) -> Result<PathBuf> {
    let life_dir = root.join(".life");
    std::fs::create_dir_all(&life_dir).context("failed to create .life/ directory")?;
    std::fs::create_dir_all(life_dir.join("control")).context("failed to create .life/control/")?;
    std::fs::create_dir_all(life_dir.join("identity"))
        .context("failed to create .life/identity/")?;
    Ok(life_dir)
}

/// Write `.life/config.toml` with default provider settings.
/// Idempotent — skips if the file already exists so user edits aren't lost.
fn write_config(life_dir: &Path) -> Result<bool> {
    let path = life_dir.join("config.toml");
    if path.exists() {
        return Ok(false);
    }
    std::fs::write(&path, DEFAULT_CONFIG_TOML)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

/// Write `.life/control/policy.yaml` with default governance rules.
/// Idempotent — skips if the file already exists so user edits aren't lost.
fn write_policy(life_dir: &Path) -> Result<bool> {
    let path = life_dir.join("control").join("policy.yaml");
    if path.exists() {
        return Ok(false);
    }
    std::fs::write(&path, DEFAULT_POLICY_YAML)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

/// Append `.life/` gitignore patterns to the project `.gitignore`.
/// Idempotent — skips if the sentinel pattern is already present.
fn update_gitignore(root: &Path) -> Result<()> {
    let gitignore_path = root.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();

    if existing.contains(GITIGNORE_SENTINEL) {
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

/// Write the raw 32-byte master seed to `.life/identity/seed.local.bin` with
/// `0o600` permissions on Unix (best-effort on other platforms).
fn write_seed(identity_dir: &Path, seed_bytes: &[u8; 32]) -> Result<()> {
    let path = identity_dir.join("seed.local.bin");
    std::fs::write(&path, seed_bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&path, perms)
            .with_context(|| format!("failed to chmod 0o600 {}", path.display()))?;
    }

    Ok(())
}

/// Read the master seed from `.life/identity/seed.local.bin`, if present.
/// Returns `Ok(None)` if the file doesn't exist; `Err` if it exists but is
/// malformed (wrong length).
fn read_seed(identity_dir: &Path) -> Result<Option<[u8; 32]>> {
    let path = identity_dir.join("seed.local.bin");
    if !path.exists() {
        return Ok(None);
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let arr: [u8; 32] = bytes.try_into().map_err(|v: Vec<u8>| {
        anyhow::anyhow!(
            "{} has wrong length: {} bytes (expected 32)",
            path.display(),
            v.len()
        )
    })?;
    Ok(Some(arr))
}

/// Read and parse `.life/identity/soul.json`, if present.
fn read_soul_document(identity_dir: &Path) -> Result<Option<SoulDocument>> {
    let path = identity_dir.join("soul.json");
    if !path.exists() {
        return Ok(None);
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let doc: SoulDocument = serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not a valid soul document", path.display()))?;
    Ok(Some(doc))
}

/// Persist `.life/identity/soul.json` with the on-disk schema.
fn write_soul_document(identity_dir: &Path, doc: &SoulDocument) -> Result<()> {
    let path = identity_dir.join("soul.json");
    let json = serde_json::to_string_pretty(doc).context("failed to serialize soul document")?;
    std::fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

// ── Anima identity bootstrap ─────────────────────────────────────────────

/// Default agent name used for fresh InProcess identities.
///
/// Includes a 6-char suffix of the wallet address so two `life init` runs in
/// neighbouring projects don't collide visually. The full provenance lives in
/// the soul itself (which is hashed for tamper-evidence).
fn default_agent_name(wallet_address: &str) -> String {
    let stripped = wallet_address.trim_start_matches("0x");
    let len = stripped.len();
    let tail: String = stripped.chars().skip(len.saturating_sub(6)).collect();
    format!("life-agent-{tail}")
}

/// Bootstrap an Anima identity inside `.life/identity/`.
///
/// Idempotent: if `soul.json` already exists, returns the existing identity
/// without regenerating. Re-keying is an explicit operation (Phase C —
/// `life identity rotate`), not an accidental side-effect of re-running init.
pub fn bootstrap_anima_identity(life_dir: &Path) -> Result<(IdentitySummary, bool)> {
    let identity_dir = life_dir.join("identity");
    std::fs::create_dir_all(&identity_dir).context("failed to create .life/identity/")?;

    // ── Reload existing identity if present (idempotency) ────────────────
    if let Some(doc) = read_soul_document(&identity_dir)? {
        return Ok((
            IdentitySummary {
                did: doc.did,
                wallet_address: doc.wallet.address,
                soul_hash: doc.soul_hash,
                custody_kind: doc.custody.kind,
                identity_dir,
            },
            false,
        ));
    }

    // ── Reuse a leftover seed if the operator ran reset half-way through ─
    let seed_bytes = match read_seed(&identity_dir)? {
        Some(bytes) => bytes,
        None => {
            let seed = MasterSeed::generate();
            let bytes = *seed.as_bytes();
            // Seed handed to InProcessAnima below; bytes is a copy held here
            // for at-rest persistence before the seed gets zeroized on drop.
            write_seed(&identity_dir, &bytes)?;
            bytes
        }
    };

    let seed = MasterSeed::from_bytes(seed_bytes);
    let custody = InProcessAnima::from_seed_arc(seed)
        .context("failed to derive identity from master seed")?;

    let did = custody.user_did().to_string();
    let wallet = custody
        .wallet_address()
        .context("InProcess custody must expose a wallet address")?
        .clone();
    let auth_pubkey = custody.auth_pubkey().to_vec();

    // Build the soul: name derives from wallet tail (unique per identity);
    // mission is the canonical bootstrap mission; creator is the system
    // because `life init` is a non-interactive bootstrap.
    let soul: AgentSoul = SoulBuilder::new(
        default_agent_name(&wallet.address),
        "instantiated by life init",
        auth_pubkey,
    )
    .build();

    let doc = SoulDocument {
        schema_version: 1,
        did: did.clone(),
        wallet: WalletDescriptor {
            address: wallet.address.clone(),
            chain: wallet.chain.to_string(),
        },
        soul_hash: soul.soul_hash().to_string(),
        custody: CustodyDescriptor {
            kind: "in_process".into(),
            seed_file: "seed.local.bin".into(),
        },
        soul,
    };
    write_soul_document(&identity_dir, &doc)?;

    Ok((
        IdentitySummary {
            did,
            wallet_address: wallet.address,
            soul_hash: doc.soul_hash,
            custody_kind: doc.custody.kind,
            identity_dir,
        },
        true,
    ))
}

// ── Pretty printing ──────────────────────────────────────────────────────

/// Print a colored check line to stderr.
fn check(msg: &str) {
    eprintln!("  {} {msg}", c(GREEN, "✓"));
}

fn print_identity_block(summary: &IdentitySummary, created: bool) {
    eprintln!();
    if created {
        eprintln!("  {}", c(GREEN, "✓ Identity created"));
    } else {
        eprintln!("  {}", c(CYAN, "● Identity already configured"));
    }
    eprintln!("    {} {}", c(DIM, "DID"), c(GREEN, &summary.did));
    eprintln!(
        "    {} {}",
        c(DIM, "Wallet"),
        c(GREEN, &summary.wallet_address),
    );
    eprintln!(
        "    {} {}",
        c(DIM, "Custody"),
        c(DIM, &summary.custody_kind),
    );
}

// ── Public entry point ────────────────────────────────────────────────────

/// Run `life init` against the current working directory.
pub fn run() -> Result<()> {
    let root = std::env::current_dir().context("failed to determine current directory")?;
    let summary = run_in(&root)?;

    eprintln!();
    if summary.identity_created {
        eprintln!("  {} Project initialized.", c(GREEN, "✓"));
    } else {
        eprintln!(
            "  {} .life/ refreshed in {}",
            c(CYAN, "●"),
            c(DIM, &summary.life_dir.display().to_string()),
        );
    }
    eprintln!(
        "  Run {} to configure your LLM provider, then {} to chat.",
        c(CYAN, "life setup"),
        c(CYAN, "arcan chat"),
    );
    eprintln!(
        "  Fund the wallet at {} on Base to start paying for x402 capabilities.",
        c(YELLOW, &summary.identity.wallet_address),
    );
    eprintln!();

    Ok(())
}

/// Library-friendly entry point: runs `life init` against the given root.
///
/// Returns an `InitSummary` so callers can introspect without reparsing
/// files. Used by the binary `run()` against `cwd`; tests call it directly
/// with `TempDir` roots so they don't pollute the developer's shell.
pub fn run_in(root: &Path) -> Result<InitSummary> {
    eprintln!();
    eprintln!(
        "  Initializing .life/ in {}",
        c(DIM, &root.display().to_string()),
    );
    eprintln!();

    let life_dir = create_life_dir(root)?;
    check(".life/ directory ready");

    let wrote_config = write_config(&life_dir)?;
    if wrote_config {
        check("Wrote .life/config.toml");
    } else {
        check(".life/config.toml already present (kept)");
    }

    let wrote_policy = write_policy(&life_dir)?;
    if wrote_policy {
        check("Wrote .life/control/policy.yaml");
    } else {
        check(".life/control/policy.yaml already present (kept)");
    }

    let (identity, identity_created) = bootstrap_anima_identity(&life_dir)?;
    print_identity_block(&identity, identity_created);

    update_gitignore(root)?;
    check("Updated .gitignore");

    Ok(InitSummary {
        life_dir,
        identity,
        identity_created,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Existing happy path — keep the v0.3 invariants on config + policy.
    #[test]
    fn init_creates_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        let summary = run_in(root).unwrap();
        assert_eq!(summary.life_dir, root.join(".life"));
        assert!(summary.identity_created);

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
        assert!(content.contains("!.life/identity/"));
        assert!(content.contains(".life/identity/seed.local.bin"));
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

    /// Bootstrap produces a valid Anima identity + EVM wallet derived from
    /// the same master seed. DID format is `did:key:zDn…` per Spec D L4-D6
    /// (P-256 multicodec `0x1200`).
    #[test]
    fn init_bootstraps_anima_identity() {
        let tmp = tempfile::TempDir::new().unwrap();
        let summary = run_in(tmp.path()).unwrap();

        assert!(summary.identity_created);

        // DID format from Spec D L4-D6 (P-256 multicodec 0x1200).
        assert!(
            summary.identity.did.starts_with("did:key:zDn"),
            "expected did:key:zDn… prefix, got {}",
            summary.identity.did
        );

        // Wallet is a 0x-prefixed EVM address.
        assert!(summary.identity.wallet_address.starts_with("0x"));
        assert_eq!(summary.identity.wallet_address.len(), 42);

        // Soul hash is non-empty.
        assert!(!summary.identity.soul_hash.is_empty());

        // Custody backend is InProcess for `life init`.
        assert_eq!(summary.identity.custody_kind, "in_process");

        // Files on disk.
        let soul_path = tmp.path().join(".life/identity/soul.json");
        let seed_path = tmp.path().join(".life/identity/seed.local.bin");
        assert!(soul_path.exists(), "soul.json should exist");
        assert!(seed_path.exists(), "seed.local.bin should exist");

        // Seed file is exactly 32 bytes.
        let seed_bytes = std::fs::read(&seed_path).unwrap();
        assert_eq!(seed_bytes.len(), 32);

        // soul.json parses back to the same DID/wallet/hash.
        let doc: SoulDocument =
            serde_json::from_slice(&std::fs::read(&soul_path).unwrap()).unwrap();
        assert_eq!(doc.did, summary.identity.did);
        assert_eq!(doc.wallet.address, summary.identity.wallet_address);
        assert_eq!(doc.soul_hash, summary.identity.soul_hash);
        assert_eq!(doc.custody.kind, "in_process");
    }

    /// Seed file is mode 0o600 on Unix so it's not group/world-readable.
    #[cfg(unix)]
    #[test]
    fn seed_file_is_chmod_600() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        run_in(tmp.path()).unwrap();

        let seed_path = tmp.path().join(".life/identity/seed.local.bin");
        let mode = std::fs::metadata(&seed_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "seed.local.bin must be chmod 0o600, got {:o}",
            mode
        );
    }

    /// Re-running `life init` does NOT regenerate the seed — same DID and
    /// wallet on the second call. This matches `zero init`'s "wallet
    /// already configured" behavior.
    #[test]
    fn init_is_idempotent_for_identity() {
        let tmp = tempfile::TempDir::new().unwrap();
        let first = run_in(tmp.path()).unwrap();
        assert!(first.identity_created);

        let second = run_in(tmp.path()).unwrap();
        assert!(
            !second.identity_created,
            "second init must not regenerate identity"
        );
        assert_eq!(first.identity.did, second.identity.did);
        assert_eq!(
            first.identity.wallet_address,
            second.identity.wallet_address
        );
        assert_eq!(first.identity.soul_hash, second.identity.soul_hash);
    }

    /// Two separate projects produce two distinct identities (the seed is
    /// random, not derived from the project path).
    #[test]
    fn different_projects_get_different_identities() {
        let a = tempfile::TempDir::new().unwrap();
        let b = tempfile::TempDir::new().unwrap();
        let ra = run_in(a.path()).unwrap();
        let rb = run_in(b.path()).unwrap();
        assert_ne!(ra.identity.did, rb.identity.did);
        assert_ne!(ra.identity.wallet_address, rb.identity.wallet_address);
    }
}
