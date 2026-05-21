//! Parser + validator for `/etc/soma/config.toml`.
//!
//! The daemon loads the config once at startup. Every field has a sensible
//! default so the minimal config (`[server]\nunix_socket = "/run/life/soma.sock"`)
//! starts a usable daemon backed by `arcan-provider-local`, `NoOp` gates,
//! and an in-memory Lago journal.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{SomaError, SomaResult};

/// Top-level `/etc/soma/config.toml` schema.
///
/// Every nested section has its own `Default` impl so an empty file still
/// produces a valid `SomaConfig`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SomaConfig {
    /// Transport configuration (Unix socket path, optional vsock listener, shutdown drain).
    #[serde(default)]
    pub server: ServerConfig,
    /// Backend wiring — selects which hypervisor providers the daemon exposes.
    #[serde(default)]
    pub backends: BackendsConfig,
    /// Gate-chain wiring — policy, budget, and network isolation implementations.
    #[serde(default)]
    pub gates: GatesConfig,
    /// Lago event store wiring for the canonical `kernel.*` event stream.
    #[serde(default)]
    pub lago: LagoConfig,
    /// Vigil OpenTelemetry exporter wiring for daemon-level tracing.
    #[serde(default)]
    pub vigil: VigilConfig,
    /// Spec D D-Sub-E: admin custody-oracle UDS configuration.
    /// `None` means the admin plane is disabled — production deploys
    /// targeting Spec D should always populate this section.
    #[serde(default)]
    pub admin_plane: Option<AdminPlaneConfig>,
    /// BRO-1215 — Spec D D-Sub-E M9-E: which key-store backs the admin
    /// `CustodyOracle`. Default `in_process` keeps freelance + dev
    /// deploys on the existing path; production multi-tenant deploys
    /// flip to `vault` and populate `[custody.vault]`. The `vault`
    /// variant is only available when soma is built with the
    /// `kms-vault` feature.
    #[serde(default)]
    pub custody: CustodyConfig,
}

/// BRO-1215 — Spec D D-Sub-E M9-E: custody backend selection for the
/// admin `CustodyOracle`. Mirrors the lifegw `kms_provider` shape — the
/// backend variant is the type-tagged union, additional credentials live
/// in per-variant sub-tables.
///
/// ```toml
/// # Default freelance / dev: keys in process memory.
/// [custody]
/// backend = "in_process"
///
/// # Production multi-tenant — Vault Transit per-user keys.
/// [custody]
/// backend = "vault"
/// [custody.vault]
/// addr = "https://vault.internal:8200"
/// token_env = "SOMA_VAULT_TOKEN"
/// kid_prefix = "anima"
/// ```
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CustodyConfig {
    /// Which key store the admin custody-oracle dispatches to.
    #[serde(default)]
    pub backend: CustodyBackendKind,
    /// HashiCorp Vault Transit configuration. Required when `backend =
    /// "vault"`; ignored otherwise. Cross-field validation rejects the
    /// mismatched cases at load time.
    #[serde(default)]
    pub vault: Option<CustodyVaultConfig>,
}

/// BRO-1215 — Spec D D-Sub-E M9-E: which `CustodyKeyStore` implementation
/// backs the admin `CustodyOracle`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CustodyBackendKind {
    /// `InProcessCustodyKeys` — keys live in soma's process memory
    /// (Zeroizing-wrapped). Default for freelance + dev deploys.
    #[default]
    InProcess,
    /// `VaultCustodyKeys` — per-user keys held in HashiCorp Vault's
    /// transit engine (P-256 auth + secp256k1 wallet). Production
    /// multi-tenant deploys use this. Requires the `kms-vault` cargo
    /// feature.
    Vault,
}

/// BRO-1215 — Spec D D-Sub-E M9-E: HashiCorp Vault Transit configuration
/// for the soma custody backend. Loaded only when `custody.backend =
/// "vault"`.
///
/// The token field intentionally names an env var (`token_env`) — soma
/// refuses to embed Vault tokens in the config TOML (committed to source
/// control). Operators inject the token via the deployment platform's
/// secret manager (Railway secrets / k8s Secret / etc.).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CustodyVaultConfig {
    /// Vault HTTP base URL, e.g. `https://vault.internal:8200`.
    pub addr: String,
    /// Name of the env var that holds the Vault token with
    /// `transit/sign/anima-*` capability. The token MUST be renewable
    /// — soma spawns a background renewal task on bootstrap when
    /// `renew_interval` is set.
    #[serde(default = "defaults::vault_token_env")]
    pub token_env: String,
    /// Prefix used when deriving per-user transit key names. The
    /// convention is `{kid_prefix}-{user_id}-auth-v{n}` /
    /// `{kid_prefix}-{user_id}-wallet-v{n}`. Default `anima` matches
    /// the `VaultTransitAnima::new` convention.
    #[serde(default = "defaults::vault_kid_prefix")]
    pub kid_prefix: String,
    /// Optional token-renewal cadence. `None` disables the background
    /// task. Recommendation: half of the token's TTL. Format: humantime
    /// (e.g. `"1d"`, `"6h"`).
    #[serde(default, with = "duration_opt_secs")]
    pub renew_interval_secs: Option<u64>,
}

/// Spec D D-Sub-E: configuration for the soma admin custody-oracle UDS.
///
/// Mirrors lifegw's `AdminPlaneConfig` shape — the soma admin UDS is a
/// sibling of the kernel UDS, hosted on a separate socket so per-RPC
/// RBAC and FS-level access controls evolve independently.
///
/// ```toml
/// [admin_plane]
/// unix_socket = "/run/life/soma-admin.sock"
/// unix_socket_mode = 0o660
/// unix_socket_group = "life-runtime"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AdminPlaneConfig {
    /// Path to the admin Unix domain socket. Production: `/run/life/soma-admin.sock`.
    pub unix_socket: PathBuf,
    /// File mode bits (e.g. `0o660`).
    #[serde(default = "defaults::admin_socket_mode")]
    pub unix_socket_mode: Option<u32>,
    /// System group that should own the admin socket. The same group is
    /// the RBAC root for the SO_PEERCRED policy check — peers whose
    /// primary or supplementary group list contains this group are
    /// admitted to all CustodyOracle RPCs.
    pub unix_socket_group: Option<String>,
}

impl Default for AdminPlaneConfig {
    fn default() -> Self {
        Self {
            unix_socket: PathBuf::from("/run/life/soma-admin.sock"),
            unix_socket_mode: defaults::admin_socket_mode(),
            unix_socket_group: Some("life-runtime".to_string()),
        }
    }
}

/// Transport configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ServerConfig {
    /// Path to the Unix domain socket the daemon listens on.
    pub unix_socket: PathBuf,
    /// File mode bits for the socket (e.g. `0o660`).
    #[serde(default)]
    pub unix_socket_mode: Option<u32>,
    /// System group that should own the socket.
    #[serde(default)]
    pub unix_socket_group: Option<String>,
    /// Optional vsock listener (Linux only; requires the `vsock-listener` feature).
    #[serde(default)]
    pub vsock: Option<VsockConfig>,
    /// Graceful shutdown drain deadline (seconds).
    #[serde(default = "defaults::drain_secs")]
    pub drain_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            unix_socket: PathBuf::from("/run/life/soma.sock"),
            unix_socket_mode: Some(0o660),
            unix_socket_group: None,
            vsock: None,
            drain_secs: defaults::drain_secs(),
        }
    }
}

/// vsock listener configuration.
///
/// Only evaluated on Linux builds with the `vsock-listener` feature active.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct VsockConfig {
    /// VMADDR_CID_ANY = u32::MAX; most hosts want `VMADDR_CID_HOST` (2) for
    /// daemon side. Ignored on non-Linux builds.
    pub cid: u32,
    /// vsock port number (must be non-zero).
    pub port: u32,
}

/// Which backends the daemon exposes and how they're parameterised.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BackendsConfig {
    /// Enable the local `arcan-provider-local` backend (default: true).
    #[serde(default = "defaults::enable_local")]
    pub local: bool,
    /// Optional Cube backend configuration.
    #[serde(default)]
    pub cube: Option<CubeBackendConfig>,
    /// Optional Vercel backend configuration.
    #[serde(default)]
    pub vercel: Option<VercelBackendConfig>,
}

impl Default for BackendsConfig {
    fn default() -> Self {
        Self {
            local: defaults::enable_local(),
            cube: None,
            vercel: None,
        }
    }
}

/// Cube remote backend wiring.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CubeBackendConfig {
    /// Base URL of the Cube gRPC endpoint.
    pub endpoint: String,
    /// Name of the environment variable carrying the API token.
    pub api_token_env: String,
}

/// Vercel backend wiring.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct VercelBackendConfig {
    /// Vercel project identifier.
    pub project_id: String,
    /// Optional team slug or ID.
    pub team_id: Option<String>,
    /// Name of the environment variable carrying the Vercel token.
    pub token_env: String,
}

/// Gate-chain wiring. Phase 2 keeps the NoOp defaults from Phase 1; Phase 4
/// flips the budget + network impls to real ones.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GatesConfig {
    /// Policy gate implementation name (`"static"` in Phase 2).
    #[serde(default = "defaults::gate_policy")]
    pub policy: String,
    /// Budget gate implementation name (`"noop"` in Phase 2).
    #[serde(default = "defaults::gate_budget")]
    pub budget: String,
    /// Network isolation implementation name (`"noop"` in Phase 2).
    #[serde(default = "defaults::gate_network")]
    pub network: String,
}

impl Default for GatesConfig {
    fn default() -> Self {
        Self {
            policy: defaults::gate_policy(),
            budget: defaults::gate_budget(),
            network: defaults::gate_network(),
        }
    }
}

/// Lago event store wiring — the canonical `EventStorePort` instance for
/// `kernel.*` events. `in_memory` is the MVS default; production uses the
/// redb-backed adapter.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LagoConfig {
    /// Lago journal namespace for all kernel events.
    #[serde(default = "defaults::lago_namespace")]
    pub namespace: String,
    /// Backing store implementation.
    #[serde(default)]
    pub store: LagoStoreKind,
}

impl Default for LagoConfig {
    fn default() -> Self {
        Self {
            namespace: defaults::lago_namespace(),
            store: LagoStoreKind::default(),
        }
    }
}

/// Lago backing store variant.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum LagoStoreKind {
    /// Ephemeral in-memory store — default for development and tests.
    #[default]
    InMemory,
    /// Durable redb-backed store.
    Redb {
        /// Path to the redb database file.
        path: PathBuf,
    },
}

/// Vigil OTEL exporter wiring.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct VigilConfig {
    /// Which exporter to use (`console` or `otlp`).
    #[serde(default)]
    pub exporter: VigilExporter,
    /// Level filter for the console fallback exporter.
    #[serde(default = "defaults::vigil_level")]
    pub level: String,
}

impl Default for VigilConfig {
    fn default() -> Self {
        Self {
            exporter: VigilExporter::default(),
            level: defaults::vigil_level(),
        }
    }
}

/// Vigil exporter variant.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum VigilExporter {
    /// stdout/journald fallback — always available.
    #[default]
    Console,
    /// OTLP/gRPC to a collector.
    Otlp {
        /// gRPC endpoint URL (e.g. `http://localhost:4317`).
        endpoint: String,
    },
}

mod defaults {
    pub(super) fn drain_secs() -> u64 {
        30
    }
    pub(super) fn enable_local() -> bool {
        true
    }
    pub(super) fn gate_policy() -> String {
        "static".into()
    }
    pub(super) fn gate_budget() -> String {
        "noop".into()
    }
    pub(super) fn gate_network() -> String {
        "noop".into()
    }
    pub(super) fn lago_namespace() -> String {
        "soma".into()
    }
    pub(super) fn vigil_level() -> String {
        "info".into()
    }
    pub(super) fn admin_socket_mode() -> Option<u32> {
        Some(0o660)
    }
    pub(super) fn vault_token_env() -> String {
        "SOMA_VAULT_TOKEN".into()
    }
    pub(super) fn vault_kid_prefix() -> String {
        "anima".into()
    }
}

/// BRO-1215 — humantime-style serde for `Option<u64>` seconds. We use
/// integer seconds rather than humantime strings to avoid a new dep on
/// `humantime_serde` (lifegw uses the crate; soma deliberately keeps
/// its dep tree slim). Operators write `renew_interval_secs = 43200`
/// (12h) directly. The naming makes the unit explicit at config-read
/// time.
mod duration_opt_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(v: &Option<u64>, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        v.serialize(ser)
    }

    pub fn deserialize<'de, D>(de: D) -> Result<Option<u64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<u64>::deserialize(de)
    }
}

impl SomaConfig {
    /// Load and validate a config file.
    ///
    /// `path = None` returns `SomaConfig::default()` — the all-defaults
    /// profile. Callers should prefer this over `Default::default()` so
    /// future validation steps (e.g. backend credential presence) run.
    pub fn load(path: Option<&Path>) -> SomaResult<Self> {
        let raw = match path {
            Some(p) => std::fs::read_to_string(p)
                .map_err(|e| SomaError::Config(format!("reading {}: {e}", p.display())))?,
            None => String::new(),
        };
        let cfg: Self = if raw.is_empty() {
            Self::default()
        } else {
            toml::from_str(&raw).map_err(|e| SomaError::Config(format!("parsing: {e}")))?
        };
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate the loaded configuration for semantic correctness.
    pub(crate) fn validate(&self) -> SomaResult<()> {
        if !self.backends.local && self.backends.cube.is_none() && self.backends.vercel.is_none() {
            return Err(SomaError::Config(
                "at least one backend must be enabled ([backends] section)".into(),
            ));
        }
        if let Some(vsock) = self.server.vsock.as_ref() {
            if !cfg!(all(target_os = "linux", feature = "vsock-listener")) {
                return Err(SomaError::Config(
                    "vsock listener requested but daemon not built with vsock-listener on Linux"
                        .into(),
                ));
            }
            if vsock.port == 0 {
                return Err(SomaError::Config("vsock.port must be non-zero".into()));
            }
        }
        // BRO-1215 — Spec D D-Sub-E M9-E: custody backend cross-field
        // validation. `backend = "vault"` requires `[custody.vault]` to
        // be populated; `backend = "in_process"` rejects a populated
        // `[custody.vault]` so operators don't accidentally ship Vault
        // credentials behind an in-process backend.
        match (&self.custody.backend, &self.custody.vault) {
            (CustodyBackendKind::Vault, None) => {
                return Err(SomaError::Config(
                    "custody.backend = \"vault\" requires [custody.vault] to be populated".into(),
                ));
            }
            (CustodyBackendKind::InProcess, Some(_)) => {
                return Err(SomaError::Config(
                    "[custody.vault] is set but custody.backend != \"vault\"; \
                     remove [custody.vault] or change the backend"
                        .into(),
                ));
            }
            _ => {}
        }
        if let Some(vault) = self.custody.vault.as_ref() {
            if vault.addr.is_empty() {
                return Err(SomaError::Config(
                    "custody.vault.addr must not be empty".into(),
                ));
            }
            if vault.token_env.is_empty() {
                return Err(SomaError::Config(
                    "custody.vault.token_env must not be empty".into(),
                ));
            }
            if vault.kid_prefix.is_empty() {
                return Err(SomaError::Config(
                    "custody.vault.kid_prefix must not be empty".into(),
                ));
            }
            if !cfg!(feature = "kms-vault") {
                return Err(SomaError::Config(
                    "custody.backend = \"vault\" requested but soma was not built with \
                     the `kms-vault` cargo feature"
                        .into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_empty_path_uses_defaults() {
        let cfg = SomaConfig::load(None).unwrap();
        assert_eq!(cfg.server.unix_socket, PathBuf::from("/run/life/soma.sock"));
        assert!(cfg.backends.local);
        assert_eq!(cfg.gates.budget, "noop");
        assert_eq!(cfg.lago.namespace, "soma");
    }

    #[test]
    fn load_minimal_file_matches_defaults() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "[server]\nunix_socket = \"/tmp/soma.sock\"").unwrap();
        let cfg = SomaConfig::load(Some(file.path())).unwrap();
        assert_eq!(cfg.server.unix_socket, PathBuf::from("/tmp/soma.sock"));
    }

    #[test]
    fn load_rejects_unknown_fields() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "[server]\nunix_socket = \"/x\"\nfoo = 1").unwrap();
        let err = SomaConfig::load(Some(file.path())).unwrap_err();
        assert!(matches!(err, SomaError::Config(ref msg) if msg.contains("unknown field")));
    }

    #[test]
    fn validate_requires_at_least_one_backend() {
        let cfg: SomaConfig = toml::from_str("[backends]\nlocal = false\n").unwrap();
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, SomaError::Config(ref msg) if msg.contains("at least one backend")));
    }

    #[test]
    fn vsock_rejects_on_non_linux_or_without_feature() {
        let cfg: SomaConfig = toml::from_str(
            "[server]\nunix_socket = \"/x\"\n\n[server.vsock]\ncid = 2\nport = 10024\n",
        )
        .unwrap();
        if cfg!(all(target_os = "linux", feature = "vsock-listener")) {
            // vsock on: port = 10024 is valid. Try port 0 instead.
            let cfg2: SomaConfig = toml::from_str(
                "[server]\nunix_socket = \"/x\"\n\n[server.vsock]\ncid = 2\nport = 0\n",
            )
            .unwrap();
            assert!(matches!(
                cfg2.validate().unwrap_err(),
                SomaError::Config(ref msg) if msg.contains("port must be non-zero")
            ));
        } else {
            assert!(matches!(cfg.validate().unwrap_err(), SomaError::Config(_)));
        }
    }

    #[test]
    fn lago_store_kind_roundtrips() {
        // Verify that LagoStoreKind serialises and deserialises symmetrically.
        let toml_str = "[lago]\nnamespace = \"test\"\n\n[lago.store]\nkind = \"in_memory\"\n";
        let cfg: SomaConfig = toml::from_str(toml_str).unwrap();
        assert!(matches!(cfg.lago.store, LagoStoreKind::InMemory));

        let redb_str = "[lago]\nnamespace = \"prod\"\n\n[lago.store]\nkind = \"redb\"\npath = \"/tmp/e.redb\"\n";
        let cfg2: SomaConfig = toml::from_str(redb_str).unwrap();
        assert!(matches!(cfg2.lago.store, LagoStoreKind::Redb { .. }));
    }

    #[test]
    fn vigil_exporter_roundtrips() {
        let console = "[vigil]\nlevel = \"debug\"\n\n[vigil.exporter]\nkind = \"console\"\n";
        let cfg: SomaConfig = toml::from_str(console).unwrap();
        assert!(matches!(cfg.vigil.exporter, VigilExporter::Console));

        let otlp = "[vigil]\nlevel = \"trace\"\n\n[vigil.exporter]\nkind = \"otlp\"\nendpoint = \"http://localhost:4317\"\n";
        let cfg2: SomaConfig = toml::from_str(otlp).unwrap();
        assert!(matches!(cfg2.vigil.exporter, VigilExporter::Otlp { .. }));
    }

    // BRO-1215 — Spec D D-Sub-E M9-E: custody backend roundtrip + validation.

    #[test]
    fn custody_default_is_in_process() {
        let cfg = SomaConfig::default();
        assert!(matches!(cfg.custody.backend, CustodyBackendKind::InProcess));
        assert!(cfg.custody.vault.is_none());
    }

    #[test]
    fn custody_in_process_roundtrips() {
        let cfg: SomaConfig = toml::from_str("[custody]\nbackend = \"in_process\"\n").unwrap();
        assert!(matches!(cfg.custody.backend, CustodyBackendKind::InProcess));
        cfg.validate().expect("in_process default validates");
    }

    #[test]
    fn custody_vault_roundtrips() {
        let body = r#"
[custody]
backend = "vault"

[custody.vault]
addr = "https://vault.internal:8200"
token_env = "SOMA_VAULT_TOKEN"
kid_prefix = "anima"
renew_interval_secs = 43200
"#;
        let cfg: SomaConfig = toml::from_str(body).unwrap();
        assert!(matches!(cfg.custody.backend, CustodyBackendKind::Vault));
        let vault = cfg.custody.vault.as_ref().expect("vault block populated");
        assert_eq!(vault.addr, "https://vault.internal:8200");
        assert_eq!(vault.token_env, "SOMA_VAULT_TOKEN");
        assert_eq!(vault.kid_prefix, "anima");
        assert_eq!(vault.renew_interval_secs, Some(43200));
    }

    #[test]
    fn custody_vault_requires_block() {
        let cfg: SomaConfig = toml::from_str("[custody]\nbackend = \"vault\"\n").unwrap();
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(err, SomaError::Config(ref msg) if msg.contains("[custody.vault] to be populated")),
            "wrong err: {err:?}"
        );
    }

    #[test]
    fn custody_in_process_rejects_vault_block() {
        let body = r#"
[custody]
backend = "in_process"

[custody.vault]
addr = "https://vault.internal:8200"
token_env = "SOMA_VAULT_TOKEN"
kid_prefix = "anima"
"#;
        let cfg: SomaConfig = toml::from_str(body).unwrap();
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(err, SomaError::Config(ref msg) if msg.contains("[custody.vault] is set")),
            "wrong err: {err:?}"
        );
    }

    #[test]
    fn custody_vault_rejects_empty_addr() {
        let body = r#"
[custody]
backend = "vault"

[custody.vault]
addr = ""
token_env = "SOMA_VAULT_TOKEN"
kid_prefix = "anima"
"#;
        let cfg: SomaConfig = toml::from_str(body).unwrap();
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(err, SomaError::Config(ref msg) if msg.contains("custody.vault.addr")),
            "wrong err: {err:?}"
        );
    }

    /// When soma is NOT built with `kms-vault`, requesting the Vault
    /// backend at config-load time MUST fail at validate() rather than
    /// silently fall back to in-process keys (which would be a footgun).
    #[test]
    #[cfg(not(feature = "kms-vault"))]
    fn custody_vault_rejected_without_feature() {
        let body = r#"
[custody]
backend = "vault"

[custody.vault]
addr = "https://vault.internal:8200"
token_env = "SOMA_VAULT_TOKEN"
kid_prefix = "anima"
"#;
        let cfg: SomaConfig = toml::from_str(body).unwrap();
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(err, SomaError::Config(ref msg) if msg.contains("kms-vault")),
            "wrong err: {err:?}"
        );
    }
}
