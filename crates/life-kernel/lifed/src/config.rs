//! Parser + validator for `/etc/lifed/config.toml`.
//!
//! The daemon loads the config once at startup. Every field has a sensible
//! default so the minimal config (`[server]\nunix_socket = "/run/lifed/sock"`)
//! starts a usable daemon backed by `arcan-provider-local`, `NoOp` gates,
//! and an in-memory Lago journal.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{LifedError, LifedResult};

/// Top-level `/etc/lifed/config.toml` schema.
///
/// Every nested section has its own `Default` impl so an empty file still
/// produces a valid `LifedConfig`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LifedConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub backends: BackendsConfig,
    #[serde(default)]
    pub gates: GatesConfig,
    #[serde(default)]
    pub lago: LagoConfig,
    #[serde(default)]
    pub vigil: VigilConfig,
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
            unix_socket: PathBuf::from("/run/lifed/sock"),
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
        Self { local: defaults::enable_local(), cube: None, vercel: None }
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
        Self { namespace: defaults::lago_namespace(), store: LagoStoreKind::default() }
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
        Self { exporter: VigilExporter::default(), level: defaults::vigil_level() }
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
    pub(super) fn drain_secs() -> u64 { 30 }
    pub(super) fn enable_local() -> bool { true }
    pub(super) fn gate_policy() -> String { "static".into() }
    pub(super) fn gate_budget() -> String { "noop".into() }
    pub(super) fn gate_network() -> String { "noop".into() }
    pub(super) fn lago_namespace() -> String { "lifed".into() }
    pub(super) fn vigil_level() -> String { "info".into() }
}

impl LifedConfig {
    /// Load and validate a config file.
    ///
    /// `path = None` returns `LifedConfig::default()` — the all-defaults
    /// profile. Callers should prefer this over `Default::default()` so
    /// future validation steps (e.g. backend credential presence) run.
    pub fn load(path: Option<&Path>) -> LifedResult<Self> {
        let raw = match path {
            Some(p) => std::fs::read_to_string(p)
                .map_err(|e| LifedError::Config(format!("reading {}: {e}", p.display())))?,
            None => String::new(),
        };
        let cfg: Self = if raw.is_empty() {
            Self::default()
        } else {
            toml::from_str(&raw).map_err(|e| LifedError::Config(format!("parsing: {e}")))?
        };
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate the loaded configuration for semantic correctness.
    pub(crate) fn validate(&self) -> LifedResult<()> {
        if !self.backends.local
            && self.backends.cube.is_none()
            && self.backends.vercel.is_none()
        {
            return Err(LifedError::Config(
                "at least one backend must be enabled ([backends] section)".into(),
            ));
        }
        if let Some(vsock) = self.server.vsock.as_ref() {
            if !cfg!(all(target_os = "linux", feature = "vsock-listener")) {
                return Err(LifedError::Config(
                    "vsock listener requested but daemon not built with vsock-listener on Linux"
                        .into(),
                ));
            }
            if vsock.port == 0 {
                return Err(LifedError::Config("vsock.port must be non-zero".into()));
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
        let cfg = LifedConfig::load(None).unwrap();
        assert_eq!(cfg.server.unix_socket, PathBuf::from("/run/lifed/sock"));
        assert!(cfg.backends.local);
        assert_eq!(cfg.gates.budget, "noop");
        assert_eq!(cfg.lago.namespace, "lifed");
    }

    #[test]
    fn load_minimal_file_matches_defaults() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "[server]\nunix_socket = \"/tmp/lifed.sock\"").unwrap();
        let cfg = LifedConfig::load(Some(file.path())).unwrap();
        assert_eq!(cfg.server.unix_socket, PathBuf::from("/tmp/lifed.sock"));
    }

    #[test]
    fn load_rejects_unknown_fields() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "[server]\nunix_socket = \"/x\"\nfoo = 1").unwrap();
        let err = LifedConfig::load(Some(file.path())).unwrap_err();
        assert!(matches!(err, LifedError::Config(ref msg) if msg.contains("unknown field")));
    }

    #[test]
    fn validate_requires_at_least_one_backend() {
        let cfg: LifedConfig = toml::from_str("[backends]\nlocal = false\n").unwrap();
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, LifedError::Config(ref msg) if msg.contains("at least one backend")));
    }

    #[test]
    fn vsock_rejects_on_non_linux_or_without_feature() {
        let cfg: LifedConfig = toml::from_str(
            "[server]\nunix_socket = \"/x\"\n\n[server.vsock]\ncid = 2\nport = 10024\n",
        )
        .unwrap();
        if cfg!(all(target_os = "linux", feature = "vsock-listener")) {
            // vsock on: port = 10024 is valid. Try port 0 instead.
            let cfg2: LifedConfig = toml::from_str(
                "[server]\nunix_socket = \"/x\"\n\n[server.vsock]\ncid = 2\nport = 0\n",
            )
            .unwrap();
            assert!(matches!(
                cfg2.validate().unwrap_err(),
                LifedError::Config(ref msg) if msg.contains("port must be non-zero")
            ));
        } else {
            assert!(matches!(cfg.validate().unwrap_err(), LifedError::Config(_)));
        }
    }
}
