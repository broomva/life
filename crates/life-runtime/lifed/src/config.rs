//! Parser + validator for `/etc/life/lifed.toml`.
//!
//! The daemon loads the config once at startup. Every field has a sensible
//! default so an empty file (`LifedConfig::load(None)`) starts a usable
//! daemon against the locked master-spec UDS paths.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{LifedError, LifedResult};

/// Top-level `/etc/life/lifed.toml` schema.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LifedConfig {
    /// Public plane UDS configuration.
    #[serde(default)]
    pub public_plane: PublicPlaneConfig,
    /// Admin plane UDS configuration.
    #[serde(default)]
    pub admin_plane: AdminPlaneConfig,
    /// Per-substrate UDS endpoints.
    #[serde(default)]
    pub substrates: SubstratesConfig,
    /// Authn / authz plumbing.
    #[serde(default)]
    pub auth: AuthConfig,
    /// Routing-cache parameters.
    #[serde(default)]
    pub routing: RoutingConfig,
    /// Per-substrate connection-pool capacities (D1 fills these in).
    #[serde(default)]
    pub pools: PoolsConfig,
    /// Idempotency-store parameters.
    #[serde(default)]
    pub idempotency: IdempotencyConfig,
    /// Vigil OpenTelemetry exporter wiring.
    #[serde(default)]
    pub vigil: VigilConfig,
    /// Graceful shutdown drain.
    #[serde(default)]
    pub shutdown: ShutdownConfig,
}

/// Public plane UDS configuration — `/run/life/life.sock`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PublicPlaneConfig {
    pub unix_socket: PathBuf,
    #[serde(default)]
    pub unix_socket_mode: Option<u32>,
    #[serde(default)]
    pub unix_socket_group: Option<String>,
}

impl Default for PublicPlaneConfig {
    fn default() -> Self {
        Self {
            unix_socket: PathBuf::from("/run/life/life.sock"),
            unix_socket_mode: Some(0o660),
            unix_socket_group: Some("life-runtime".to_string()),
        }
    }
}

/// Admin plane UDS configuration — `/run/life/life-admin.sock`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AdminPlaneConfig {
    pub unix_socket: PathBuf,
    #[serde(default)]
    pub unix_socket_mode: Option<u32>,
    #[serde(default)]
    pub unix_socket_group: Option<String>,
}

impl Default for AdminPlaneConfig {
    fn default() -> Self {
        Self {
            unix_socket: PathBuf::from("/run/life/life-admin.sock"),
            unix_socket_mode: Some(0o660),
            unix_socket_group: Some("life-admin".to_string()),
        }
    }
}

/// Per-substrate UDS endpoints.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SubstratesConfig {
    #[serde(default = "default_arcan")]
    pub arcan: SubstrateEndpoint,
    #[serde(default = "default_lago")]
    pub lago: SubstrateEndpoint,
    #[serde(default = "default_haima")]
    pub haima: SubstrateEndpoint,
    #[serde(default = "default_anima")]
    pub anima: SubstrateEndpoint,
    #[serde(default = "default_soma")]
    pub soma: SubstrateEndpoint,
}

impl Default for SubstratesConfig {
    fn default() -> Self {
        Self {
            arcan: default_arcan(),
            lago: default_lago(),
            haima: default_haima(),
            anima: default_anima(),
            soma: default_soma(),
        }
    }
}

fn default_arcan() -> SubstrateEndpoint {
    SubstrateEndpoint {
        unix_socket: PathBuf::from("/run/life/arcan.sock"),
    }
}
fn default_lago() -> SubstrateEndpoint {
    SubstrateEndpoint {
        unix_socket: PathBuf::from("/run/life/lago.sock"),
    }
}
fn default_haima() -> SubstrateEndpoint {
    SubstrateEndpoint {
        unix_socket: PathBuf::from("/run/life/haima.sock"),
    }
}
fn default_anima() -> SubstrateEndpoint {
    SubstrateEndpoint {
        unix_socket: PathBuf::from("/run/life/anima.sock"),
    }
}
fn default_soma() -> SubstrateEndpoint {
    SubstrateEndpoint {
        unix_socket: PathBuf::from("/run/life/soma-admin.sock"),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SubstrateEndpoint {
    pub unix_socket: PathBuf,
}

/// Authn / authz plumbing.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AuthConfig {
    /// Path to a JSON-encoded JWKS file used to verify Tier-2 capability tokens.
    /// In production, this file is published by lifegw at the path below.
    /// lifed reads it lazily on first verify (and on `kid` cache miss /
    /// mtime change) so there's no boot-order dependency between the two
    /// daemons — see `auth::jwks` Stage-2 docs.
    #[serde(default = "default_jwks_path")]
    pub jwks_path: PathBuf,
    /// Path to lifed's substrate-token signing key (PEM-encoded EC key).
    /// In dev/CI a deterministic test key ships with the daemon.
    #[serde(default = "default_signing_key_path")]
    pub substrate_signing_key_path: PathBuf,
    /// Path where lifed publishes its substrate-token JWKS.
    /// Substrates poll this file for Tier-3 verification keys.
    #[serde(default = "default_published_jwks_path")]
    pub substrate_jwks_publish_path: PathBuf,
    /// Path to the `revoked_sids.json` snapshot file (master spec §L4 invariant 5).
    #[serde(default = "default_revoked_sids_path")]
    pub revoked_sids_path: PathBuf,
    /// When `true`, the JWKS verifier ALSO accepts the
    /// `Bearer test-token-for-{user_id}` shortcut as an additive
    /// fallback (real ES256 verification still runs against `jwks_path`).
    /// MUST be `false` in production. Integration tests + ops smoke
    /// runs flip this on during the Stage 1 → Stage 2 transition; once
    /// every caller mints real JWS the flag flips back to `false`.
    #[serde(default)]
    pub dev_signer_enabled: bool,
}

fn default_jwks_path() -> PathBuf {
    PathBuf::from("/etc/life/lifegw-jwks.json")
}
fn default_signing_key_path() -> PathBuf {
    PathBuf::from("/etc/life/lifed-signing-key.pem")
}
fn default_published_jwks_path() -> PathBuf {
    PathBuf::from("/run/life/lifed-jwks.json")
}
fn default_revoked_sids_path() -> PathBuf {
    PathBuf::from("/run/life/revoked_sids.json")
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwks_path: default_jwks_path(),
            substrate_signing_key_path: default_signing_key_path(),
            substrate_jwks_publish_path: default_published_jwks_path(),
            revoked_sids_path: default_revoked_sids_path(),
            // Production posture — dev shortcut OFF by default.
            dev_signer_enabled: false,
        }
    }
}

/// Routing-cache parameters per Spec C₂ §6.3.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RoutingConfig {
    /// Idle threshold for `Detached` entries before eviction.
    #[serde(default = "default_idle_secs")]
    pub idle_threshold_secs: u64,
    /// Hard cap on cache size (LRU eviction over this).
    #[serde(default = "default_hard_cap")]
    pub hard_cap: usize,
    /// Eviction sweep interval.
    #[serde(default = "default_eviction_interval_secs")]
    pub eviction_interval_secs: u64,
}

fn default_idle_secs() -> u64 {
    3600
}
fn default_hard_cap() -> usize {
    100_000
}
fn default_eviction_interval_secs() -> u64 {
    300
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            idle_threshold_secs: default_idle_secs(),
            hard_cap: default_hard_cap(),
            eviction_interval_secs: default_eviction_interval_secs(),
        }
    }
}

/// Per-substrate connection-pool capacities per Spec C₂ §7.1.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PoolsConfig {
    #[serde(default = "default_pool_arcan")]
    pub arcan_capacity: u32,
    #[serde(default = "default_pool_lago")]
    pub lago_capacity: u32,
    #[serde(default = "default_pool_haima")]
    pub haima_capacity: u32,
    #[serde(default = "default_pool_anima")]
    pub anima_capacity: u32,
    #[serde(default = "default_pool_soma")]
    pub soma_capacity: u32,
}

fn default_pool_arcan() -> u32 {
    32
}
fn default_pool_lago() -> u32 {
    64
}
fn default_pool_haima() -> u32 {
    16
}
fn default_pool_anima() -> u32 {
    16
}
fn default_pool_soma() -> u32 {
    8
}

impl Default for PoolsConfig {
    fn default() -> Self {
        Self {
            arcan_capacity: default_pool_arcan(),
            lago_capacity: default_pool_lago(),
            haima_capacity: default_pool_haima(),
            anima_capacity: default_pool_anima(),
            soma_capacity: default_pool_soma(),
        }
    }
}

/// Idempotency-store parameters per Spec C₂ §3.6.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IdempotencyConfig {
    /// Dedup TTL.
    #[serde(default = "default_idem_ttl")]
    pub ttl_secs: u64,
    /// Whether the in-memory backend is used (sub-phase A) or lago-backed (sub-phase B).
    #[serde(default)]
    pub backend: IdempotencyBackend,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdempotencyBackend {
    /// In-memory DashMap with TTL eviction sweeper. M5 sub-phase A default.
    #[default]
    InMemory,
    /// Lago-backed durable dedup. M5 sub-phase B default.
    Lago,
}

fn default_idem_ttl() -> u64 {
    24 * 3600
}

impl Default for IdempotencyConfig {
    fn default() -> Self {
        Self {
            ttl_secs: default_idem_ttl(),
            backend: IdempotencyBackend::default(),
        }
    }
}

/// Vigil OpenTelemetry exporter wiring.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct VigilConfig {
    /// OTLP endpoint URL. Empty = no remote exporter (stdout fallback).
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
    /// Trace sampler ratio (0.0–1.0). Default 0.01 = 1 % sampled, 100 % on errors.
    #[serde(default = "default_trace_sample")]
    pub trace_sample_ratio: f64,
    /// Metric reader interval.
    #[serde(default = "default_metric_interval")]
    pub metric_interval_secs: u64,
}

fn default_trace_sample() -> f64 {
    0.01
}
fn default_metric_interval() -> u64 {
    60
}

/// Graceful shutdown drain.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ShutdownConfig {
    #[serde(default = "default_drain_secs")]
    pub drain_secs: u64,
}

fn default_drain_secs() -> u64 {
    30
}

impl Default for ShutdownConfig {
    fn default() -> Self {
        Self {
            drain_secs: default_drain_secs(),
        }
    }
}

impl LifedConfig {
    /// Load a config from `path` if provided, otherwise produce all-defaults.
    pub fn load(path: Option<&Path>) -> LifedResult<Self> {
        match path {
            Some(p) => {
                let text = std::fs::read_to_string(p)
                    .map_err(|e| LifedError::Config(format!("read {}: {e}", p.display())))?;
                let cfg: LifedConfig = toml::from_str(&text)
                    .map_err(|e| LifedError::Config(format!("parse {}: {e}", p.display())))?;
                cfg.validate()?;
                Ok(cfg)
            }
            None => Ok(LifedConfig::default()),
        }
    }

    /// Cross-field validation. Pure — no I/O.
    fn validate(&self) -> LifedResult<()> {
        if self.public_plane.unix_socket == self.admin_plane.unix_socket {
            return Err(LifedError::Config(
                "public_plane.unix_socket and admin_plane.unix_socket must differ".to_string(),
            ));
        }
        if self.shutdown.drain_secs == 0 {
            return Err(LifedError::Config(
                "shutdown.drain_secs must be > 0".to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&self.vigil.trace_sample_ratio) {
            return Err(LifedError::Config(
                "vigil.trace_sample_ratio must be in [0.0, 1.0]".to_string(),
            ));
        }
        Ok(())
    }

    /// Convenience: shutdown duration as a `Duration`.
    pub fn drain_duration(&self) -> Duration {
        Duration::from_secs(self.shutdown.drain_secs)
    }
}
