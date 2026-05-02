//! Parser + validator for `/etc/lifegw/config.toml`.
//!
//! The daemon loads the config once at startup. Every field has a sensible
//! default so an empty file (`LifegwConfig::load(None)`) starts a usable
//! daemon against the locked Spec C₃ paths.
//!
//! Sub-phase A scope (Spec C₃ §12.A):
//! - `[tls]` cert + key paths (rustls bind).
//! - `[listen]` listener address (default `[::]:443`).
//! - `[upstream]` `lifed_uds_path` (default `/run/life/life.sock`).
//! - `[auth]` Tier-2 mint settings + dev-signer toggle.
//! - `[rate_limit]`, `[observability]` — fields present, unused in A.
//!
//! Real Vercel JWKS, KMS provider, and OTLP exporter wiring live in B/D/E.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{LifegwError, LifegwResult};

/// Top-level `/etc/lifegw/config.toml` schema.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LifegwConfig {
    /// TLS termination configuration.
    #[serde(default)]
    pub tls: TlsConfig,
    /// Listener configuration (TCP bind addresses).
    #[serde(default)]
    pub listen: ListenConfig,
    /// Upstream lifed dial path.
    #[serde(default)]
    pub upstream: UpstreamConfig,
    /// Authn / authz plumbing.
    #[serde(default)]
    pub auth: AuthConfig,
    /// Sub-phase D (D2): admin-plane UDS configuration.
    #[serde(default)]
    pub admin_plane: AdminPlaneConfig,
    /// Rate-limit defaults (Sub-phase D D1).
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
    /// Vigil OTLP exporter wiring (Sub-phase D fills this in).
    #[serde(default)]
    pub observability: ObservabilityConfig,
    /// Spec D D-Sub-C: anima custody route configuration. Optional —
    /// when absent, the `/anima/custody/*` proxy routes return
    /// `501 Not Implemented` while the rest of the gateway stays
    /// functional. Production deploys with soma admin custody-oracle
    /// enabled MUST populate this with the soma admin UDS path.
    #[serde(default)]
    pub anima_custody: Option<AnimaCustodyConfig>,
}

/// Spec D D-Sub-C anima custody configuration.
///
/// Wires the lifegw-side proxy at `/anima/custody/*` to soma's admin
/// custody-oracle UDS. When unset, the proxy routes degrade gracefully
/// (501 Not Implemented) so lifegw still starts on operator boxes that
/// haven't enabled soma custody.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AnimaCustodyConfig {
    /// Path to soma's admin custody-oracle UDS. Production default is
    /// `/run/life/soma-admin.sock` — see Spec D D-Sub-E. When `None`,
    /// the proxy routes return 501.
    #[serde(default)]
    pub soma_uds_path: Option<PathBuf>,
}

/// Admin-plane UDS configuration. Sub-phase D (D2).
///
/// The admin plane is a separate UDS (default `/run/life/lifegw-admin.sock`)
/// hosting `life.admin.gw.v1.GatewayAdmin`. Authn is SO_PEERCRED +
/// group membership — `unix_socket_group = "life-admin"` gives that
/// group's members admin authority. Setting `unix_socket_group` to
/// `None` (e.g. for tests) places the policy table in permissive
/// mode — every connecting peer is granted admin authority. That
/// matches the lifed convention and is safe in tests because the
/// socket path is in a tempdir only the test owner can reach.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AdminPlaneConfig {
    /// Path to the admin UDS. Default `/run/life/lifegw-admin.sock`.
    #[serde(default = "default_admin_socket")]
    pub unix_socket: PathBuf,
    /// Group that owns the admin socket. Production deploys set this
    /// to `life-admin`; tests may set it to `None` for permissive
    /// access.
    #[serde(default = "default_admin_group")]
    pub unix_socket_group: Option<String>,
    /// File mode (default `0o660`). Combined with the group, this
    /// gives `life-admin` members read+write but other users no
    /// access.
    #[serde(default = "default_admin_mode")]
    pub unix_socket_mode: Option<u32>,
}

fn default_admin_socket() -> PathBuf {
    PathBuf::from("/run/life/lifegw-admin.sock")
}

fn default_admin_group() -> Option<String> {
    Some("life-admin".to_string())
}

fn default_admin_mode() -> Option<u32> {
    Some(0o660)
}

impl Default for AdminPlaneConfig {
    fn default() -> Self {
        Self {
            unix_socket: default_admin_socket(),
            unix_socket_group: default_admin_group(),
            unix_socket_mode: default_admin_mode(),
        }
    }
}

/// TLS termination configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TlsConfig {
    /// Path to the PEM-encoded full-chain certificate.
    #[serde(default = "default_cert_path")]
    pub cert_path: PathBuf,
    /// Path to the PEM-encoded private key.
    #[serde(default = "default_key_path")]
    pub key_path: PathBuf,
    /// Whether ACME-based cert issuance is enabled. Sub-phase E feature; field
    /// present here so config-file consumers don't fail validation when they
    /// pre-declare it. Default `false`.
    #[serde(default)]
    pub acme_enabled: bool,
}

fn default_cert_path() -> PathBuf {
    PathBuf::from("/etc/lifegw/tls/fullchain.pem")
}

fn default_key_path() -> PathBuf {
    PathBuf::from("/etc/lifegw/tls/privkey.pem")
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            cert_path: default_cert_path(),
            key_path: default_key_path(),
            acme_enabled: false,
        }
    }
}

/// Listener configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ListenConfig {
    /// HTTPS bind address (default `[::]:443`). Production uses systemd
    /// socket activation which inherits the bound fd; the dev fallback
    /// reads this field directly.
    #[serde(default = "default_https_addr")]
    pub https_addr: String,
    /// Optional HTTP→HTTPS redirect bind. `None` disables the redirect
    /// listener (the redirect is a hint, not a hard requirement).
    #[serde(default = "default_http_redirect_addr")]
    pub http_redirect_addr: Option<String>,
}

fn default_https_addr() -> String {
    "[::]:443".to_string()
}

fn default_http_redirect_addr() -> Option<String> {
    Some("[::]:80".to_string())
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            https_addr: default_https_addr(),
            http_redirect_addr: default_http_redirect_addr(),
        }
    }
}

/// Upstream lifed dial path.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct UpstreamConfig {
    /// Path to the lifed public-plane UDS (Spec C₂ §3 — default
    /// `/run/life/life.sock`).
    #[serde(default = "default_lifed_uds_path")]
    pub lifed_uds_path: PathBuf,
}

fn default_lifed_uds_path() -> PathBuf {
    PathBuf::from("/run/life/life.sock")
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            lifed_uds_path: default_lifed_uds_path(),
        }
    }
}

/// Authn / authz plumbing.
///
/// Sub-phase B (Spec C₃ §5):
/// - Real Vercel-style JWKS verification of inbound Tier-1 tokens.
/// - Tier-2 mint via a [`KmsProvider`]-resolved signer. Default
///   provider is `Vault` (production primary); dev / CI flips to `Dev`
///   alongside `dev_signer_enabled = true`.
/// - JWKS publish to `publish_jwks_path` so downstream verifiers
///   (lifed) can pick up rotation atomically.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AuthConfig {
    /// Vercel JWKS endpoint URL. Sub-phase B fetches this URL and uses
    /// the contents to verify inbound Tier-1 tokens. When
    /// `dev_signer_enabled` is true, the URL is ignored and the in-process
    /// dev shortcut handles verification.
    #[serde(default = "default_jwks_url")]
    pub jwks_url: String,
    /// JWKS cache TTL. Successful fetches are reused for this long
    /// before refetching. Cache misses (unknown kid) trigger a refetch
    /// regardless of TTL.
    #[serde(default = "default_jwks_cache_ttl", with = "humantime_serde")]
    pub jwks_cache_ttl: Duration,
    /// Key rotation grace per Spec C₃ §5: retired keys remain valid
    /// for this duration after the upstream JWKS publishes a
    /// replacement.
    #[serde(default = "default_jwks_rotation_grace", with = "humantime_serde")]
    pub jwks_rotation_grace: Duration,
    /// Expected `aud` claim on Tier-1 tokens. Default `lifegw`.
    #[serde(default = "default_tier1_audience")]
    pub tier1_audience: String,
    /// Expected `iss` claim on Tier-1 tokens. Default the apps/chat
    /// origin (`https://broomva.tech` for the demo deploy; production
    /// overrides via config).
    #[serde(default = "default_tier1_issuer")]
    pub tier1_issuer: String,
    /// KMS provider for Tier-2 signing.
    #[serde(default)]
    pub kms_provider: KmsProvider,
    /// HashiCorp Vault Transit configuration. Required when
    /// `kms_provider = "vault"`.
    #[serde(default)]
    pub vault: Option<VaultConfig>,
    /// AWS KMS configuration. Required when `kms_provider = "aws"`.
    /// Sub-phase E item #1.
    #[serde(default)]
    pub aws: Option<AwsConfig>,
    /// GCP Cloud KMS configuration. Required when `kms_provider = "gcp"`.
    /// Sub-phase E item #2.
    #[serde(default)]
    pub gcp: Option<GcpConfig>,
    /// Path to which the gateway publishes its Tier-2 JWKS document.
    /// `None` disables publish (used by tests that share key material
    /// in-memory). Default `/run/life/lifegw-jwks.json`.
    #[serde(default = "default_publish_jwks_path")]
    pub publish_jwks_path: Option<PathBuf>,
    /// Whether the `Bearer dev-token-for-{user_id}` shortcut is
    /// accepted. MUST be `false` in production; set `true` only in
    /// dev / CI.
    #[serde(default)]
    pub dev_signer_enabled: bool,
    /// Tier-2 audience (Spec C₃ §5.4). Default `lifed`.
    #[serde(default = "default_tier2_audience")]
    pub tier2_audience: String,
    /// Tier-2 issuer (Spec C₃ §5.4). Default `lifegw`.
    #[serde(default = "default_tier2_issuer")]
    pub tier2_issuer: String,
    /// Tier-2 capability lifetime cap (Spec C₃ §5.4 — ≤ 15 min).
    #[serde(default = "default_tier2_ttl", with = "humantime_serde")]
    pub tier2_ttl: Duration,
    /// Spec D D-Sub-C: Tier-User capability lifetime cap (≤ 15 min).
    /// `None` falls back to `DEFAULT_TIER_USER_TTL` (15 min). Operators
    /// MAY shorten this for high-security tenants but lengthening past
    /// 15 min is rejected at config-validate time.
    #[serde(default, with = "humantime_serde_opt")]
    pub tier_user_ttl: Option<Duration>,
}

/// HashiCorp Vault Transit configuration (Sub-phase B production
/// primary). Loaded only when `auth.kms_provider = "vault"`.
///
/// Sub-phase E adds:
/// - `[mtls]` — optional client-cert + client-key paths for
///   peer-authenticated TLS to Vault.
/// - `renew_interval` — optional cadence for the background
///   `auth/token/renew-self` task. Vault tokens with `renewable: true`
///   require periodic renewal to stay live.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct VaultConfig {
    /// Vault HTTP base URL, e.g. `https://vault.internal:8200`.
    pub addr: String,
    /// Vault token with `transit/sign/<key>` capability. Loaded via
    /// env-var indirection in production deployments — this field
    /// stores the resolved token at config-load time.
    pub token: String,
    /// Vault transit key name (the gateway never sees the private
    /// half).
    pub key_name: String,
    /// JWS `kid` value embedded in token headers + the published JWKS.
    /// Convention: same as `key_name`.
    pub kid: String,
    /// Sub-phase E (item #5): optional mTLS to Vault.
    #[serde(default)]
    pub mtls: Option<VaultMtlsConfig>,
    /// Sub-phase E (item #5): optional renewal cadence. `None`
    /// disables the background renewal task. Recommend half the
    /// token's TTL.
    #[serde(default, with = "humantime_serde_opt")]
    pub renew_interval: Option<Duration>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct VaultMtlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

/// AWS KMS configuration (Sub-phase E item #1). Loaded only when
/// `auth.kms_provider = "aws"`. AWS credentials + region are resolved
/// from the standard AWS credential chain (env vars, instance profile,
/// IAM role, …) — this struct only carries lifegw-specific identifiers.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AwsConfig {
    /// AWS KMS Key ID or full ARN
    /// (e.g. `alias/lifegw-tier2`, `arn:aws:kms:us-east-1:…`).
    pub key_id: String,
    /// JWS `kid` value embedded in token headers + the published JWKS.
    /// Should be a stable identifier across key rotations (e.g. the
    /// alias name rather than the underlying key ID, so rotations
    /// don't break verifiers).
    pub kid: String,
}

/// GCP Cloud KMS configuration (Sub-phase E item #2). Loaded only when
/// `auth.kms_provider = "gcp"`. GCP credentials are resolved from the
/// standard GCP credential chain (workload identity, ADC, service-account
/// JSON file path in `GOOGLE_APPLICATION_CREDENTIALS`, …).
///
/// **Operator note**: the `resource` string MUST carry the full
/// `cryptoKeyVersions/<n>` suffix — `asymmetric_sign` operates on a key
/// version, not a crypto key. Operators who supply only the cryptoKey
/// path will get a 404 from the API.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GcpConfig {
    /// Full GCP KMS resource path including version suffix, e.g.
    /// `projects/broomva-prod/locations/global/keyRings/lifegw/cryptoKeys/tier2/cryptoKeyVersions/1`.
    pub resource: String,
    /// JWS `kid` value embedded in token headers + the published JWKS.
    pub kid: String,
}

fn default_jwks_url() -> String {
    "https://broomva.tech/api/auth/jwks.json".to_string()
}

fn default_jwks_cache_ttl() -> Duration {
    Duration::from_secs(5 * 60)
}

fn default_jwks_rotation_grace() -> Duration {
    Duration::from_secs(30 * 60)
}

fn default_tier1_audience() -> String {
    "lifegw".to_string()
}

fn default_tier1_issuer() -> String {
    "https://broomva.tech".to_string()
}

fn default_publish_jwks_path() -> Option<PathBuf> {
    Some(PathBuf::from("/run/life/lifegw-jwks.json"))
}

fn default_tier2_audience() -> String {
    "lifed".to_string()
}

fn default_tier2_issuer() -> String {
    "lifegw".to_string()
}

fn default_tier2_ttl() -> Duration {
    Duration::from_secs(15 * 60)
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum KmsProvider {
    /// In-process dev signer. Sub-phase A default.
    #[default]
    Dev,
    /// AWS KMS (Sub-phase E, behind feature flag `kms-aws`).
    Aws,
    /// GCP Cloud KMS (Sub-phase E, behind feature flag `kms-gcp`).
    Gcp,
    /// HashiCorp Vault Transit (Sub-phase E recommendation).
    Vault,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwks_url: default_jwks_url(),
            jwks_cache_ttl: default_jwks_cache_ttl(),
            jwks_rotation_grace: default_jwks_rotation_grace(),
            tier1_audience: default_tier1_audience(),
            tier1_issuer: default_tier1_issuer(),
            kms_provider: KmsProvider::default(),
            vault: None,
            aws: None,
            gcp: None,
            publish_jwks_path: default_publish_jwks_path(),
            dev_signer_enabled: false,
            tier2_audience: default_tier2_audience(),
            tier2_issuer: default_tier2_issuer(),
            tier2_ttl: default_tier2_ttl(),
            tier_user_ttl: None,
        }
    }
}

/// Rate-limit configuration. Spec C₃ §7 — fields present, unused in
/// Sub-phase A. Defaults match master spec §L12 #10.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RateLimitConfig {
    /// Per-user token-bucket capacity (default 60 req).
    #[serde(default = "default_per_user_capacity")]
    pub per_user_capacity: u32,
    /// Per-user refill rate (default 60 req/sec).
    #[serde(default = "default_per_user_refill_per_sec")]
    pub per_user_refill_per_sec: u32,
    /// Per-IP token-bucket capacity for pre-auth requests (default 60 req).
    #[serde(default = "default_per_ip_capacity")]
    pub per_ip_capacity: u32,
    /// Per-IP refill rate (default 60 req/min).
    #[serde(default = "default_per_ip_refill_per_min")]
    pub per_ip_refill_per_min: u32,
    /// Concurrent WS connection cap per user (default 10 — `free` tier).
    #[serde(default = "default_concurrent_ws_per_user")]
    pub concurrent_ws_per_user: u32,
}

fn default_per_user_capacity() -> u32 {
    60
}
fn default_per_user_refill_per_sec() -> u32 {
    60
}
fn default_per_ip_capacity() -> u32 {
    60
}
fn default_per_ip_refill_per_min() -> u32 {
    60
}
fn default_concurrent_ws_per_user() -> u32 {
    10
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            per_user_capacity: default_per_user_capacity(),
            per_user_refill_per_sec: default_per_user_refill_per_sec(),
            per_ip_capacity: default_per_ip_capacity(),
            per_ip_refill_per_min: default_per_ip_refill_per_min(),
            concurrent_ws_per_user: default_concurrent_ws_per_user(),
        }
    }
}

/// Vigil OTLP exporter configuration. Sub-phase D wires the real exporter.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ObservabilityConfig {
    /// OTLP endpoint URL. Empty / `None` = no remote exporter (stdout fallback).
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
    /// Trace sampler ratio (0.0–1.0). Default 0.05 — Spec C₃ §9.4.
    #[serde(default = "default_trace_sample_ratio")]
    pub trace_sample_ratio: f64,
    /// Metric reader interval (default 60s).
    #[serde(default = "default_metric_interval", with = "humantime_serde")]
    pub metric_interval: Duration,
}

fn default_trace_sample_ratio() -> f64 {
    0.05
}

fn default_metric_interval() -> Duration {
    Duration::from_secs(60)
}

impl LifegwConfig {
    /// Load a config from `path` if provided, otherwise produce all-defaults.
    pub fn load(path: Option<&Path>) -> LifegwResult<Self> {
        match path {
            Some(p) => {
                let text = std::fs::read_to_string(p)
                    .map_err(|e| LifegwError::Config(format!("read {}: {e}", p.display())))?;
                let cfg: LifegwConfig = toml::from_str(&text)
                    .map_err(|e| LifegwError::Config(format!("parse {}: {e}", p.display())))?;
                cfg.validate()?;
                Ok(cfg)
            }
            None => {
                let cfg = LifegwConfig::default();
                cfg.validate()?;
                Ok(cfg)
            }
        }
    }

    /// Cross-field validation. Pure — no I/O.
    pub fn validate(&self) -> LifegwResult<()> {
        if self.listen.https_addr.is_empty() {
            return Err(LifegwError::Config(
                "listen.https_addr must not be empty".to_string(),
            ));
        }
        if self.auth.tier2_audience.is_empty() {
            return Err(LifegwError::Config(
                "auth.tier2_audience must not be empty".to_string(),
            ));
        }
        if self.auth.tier2_issuer.is_empty() {
            return Err(LifegwError::Config(
                "auth.tier2_issuer must not be empty".to_string(),
            ));
        }
        // Spec C₃ §5.4 LOCKED L4-D2: Tier-2 lifetime ≤ 15 minutes.
        if self.auth.tier2_ttl > Duration::from_secs(15 * 60) {
            return Err(LifegwError::Config(format!(
                "auth.tier2_ttl ({}s) exceeds Spec C₃ §5.4 cap of 15 minutes",
                self.auth.tier2_ttl.as_secs()
            )));
        }
        if self.auth.tier2_ttl.is_zero() {
            return Err(LifegwError::Config(
                "auth.tier2_ttl must be > 0".to_string(),
            ));
        }
        // Spec D D-Sub-C: Tier-User caps follow the same 15-min cap.
        if let Some(ttl) = self.auth.tier_user_ttl {
            if ttl > Duration::from_secs(15 * 60) {
                return Err(LifegwError::Config(format!(
                    "auth.tier_user_ttl ({}s) exceeds Spec D D-Sub-C cap of 15 minutes",
                    ttl.as_secs()
                )));
            }
            if ttl.is_zero() {
                return Err(LifegwError::Config(
                    "auth.tier_user_ttl must be > 0".to_string(),
                ));
            }
        }
        if !(0.0..=1.0).contains(&self.observability.trace_sample_ratio) {
            return Err(LifegwError::Config(
                "observability.trace_sample_ratio must be in [0.0, 1.0]".to_string(),
            ));
        }
        Ok(())
    }
}

// Inline serde helper for `Duration` round-tripping via `humantime`.
mod humantime_serde {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

    pub fn serialize<S>(d: &Duration, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = format!("{}s", d.as_secs());
        s.serialize(ser)
    }

    pub fn deserialize<'de, D>(de: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(de)?;
        // Accept either "<n>s" (lifed-style) or a bare number-of-seconds string.
        if let Some(stripped) = s.strip_suffix('s') {
            let n: u64 = stripped
                .parse()
                .map_err(|e| D::Error::custom(format!("parse seconds: {e}")))?;
            return Ok(Duration::from_secs(n));
        }
        let n: u64 = s
            .parse()
            .map_err(|e| D::Error::custom(format!("parse duration: {e}")))?;
        Ok(Duration::from_secs(n))
    }
}

/// Sub-phase E: `Option<Duration>` round-trip via humantime. Used for
/// `[auth.vault].renew_interval` which is optional.
mod humantime_serde_opt {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

    pub fn serialize<S>(d: &Option<Duration>, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match d {
            None => ser.serialize_none(),
            Some(dur) => format!("{}s", dur.as_secs()).serialize(ser),
        }
    }

    pub fn deserialize<'de, D>(de: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = Option::<String>::deserialize(de)?;
        match s {
            None => Ok(None),
            Some(s) => {
                if let Some(stripped) = s.strip_suffix('s') {
                    let n: u64 = stripped
                        .parse()
                        .map_err(|e| D::Error::custom(format!("parse seconds: {e}")))?;
                    return Ok(Some(Duration::from_secs(n)));
                }
                let n: u64 = s
                    .parse()
                    .map_err(|e| D::Error::custom(format!("parse duration: {e}")))?;
                Ok(Some(Duration::from_secs(n)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates() {
        let cfg = LifegwConfig::default();
        cfg.validate().expect("default config validates");
        assert_eq!(cfg.listen.https_addr, "[::]:443");
        assert_eq!(cfg.auth.tier2_audience, "lifed");
        assert_eq!(cfg.auth.tier2_issuer, "lifegw");
        assert_eq!(cfg.auth.tier2_ttl, Duration::from_secs(15 * 60));
        assert!(!cfg.auth.dev_signer_enabled);
        assert_eq!(
            cfg.upstream.lifed_uds_path.to_string_lossy(),
            "/run/life/life.sock"
        );
    }

    #[test]
    fn serde_roundtrip() {
        // Minimal config: every field defaults.
        let cfg = LifegwConfig::default();
        let toml_text = toml::to_string(&cfg).expect("serialize default");
        let parsed: LifegwConfig = toml::from_str(&toml_text).expect("re-parse default");
        parsed.validate().expect("re-parsed default validates");

        // Overridden config exercises every section + the `humantime_serde` helper.
        let overridden_toml = r#"
[tls]
cert_path = "/tmp/cert.pem"
key_path = "/tmp/key.pem"
acme_enabled = true

[listen]
https_addr = "127.0.0.1:8443"
http_redirect_addr = "127.0.0.1:8080"

[upstream]
lifed_uds_path = "/tmp/life.sock"

[auth]
jwks_url = "https://example.test/jwks"
kms_provider = "vault"
dev_signer_enabled = true
tier2_audience = "lifed"
tier2_issuer = "lifegw"
tier2_ttl = "600s"

[rate_limit]
per_user_capacity = 100
per_user_refill_per_sec = 100
per_ip_capacity = 30
per_ip_refill_per_min = 30
concurrent_ws_per_user = 5

[observability]
otlp_endpoint = "http://otlp.test:4317"
trace_sample_ratio = 0.1
metric_interval = "30s"
"#;
        let cfg: LifegwConfig = toml::from_str(overridden_toml).expect("parse overrides");
        cfg.validate().expect("overridden config validates");
        assert!(cfg.tls.acme_enabled);
        assert_eq!(cfg.listen.https_addr, "127.0.0.1:8443");
        assert!(matches!(cfg.auth.kms_provider, KmsProvider::Vault));
        assert!(cfg.auth.dev_signer_enabled);
        assert_eq!(cfg.auth.tier2_ttl, Duration::from_secs(600));
        assert_eq!(cfg.rate_limit.per_user_capacity, 100);
        assert_eq!(cfg.observability.metric_interval, Duration::from_secs(30));
    }

    #[test]
    fn validate_rejects_oversized_tier2_ttl() {
        let mut cfg = LifegwConfig::default();
        cfg.auth.tier2_ttl = Duration::from_secs(20 * 60);
        let err = cfg.validate().expect_err("must reject > 15min");
        match err {
            LifegwError::Config(m) => assert!(m.contains("15 minutes"), "got: {m}"),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_zero_tier2_ttl() {
        let mut cfg = LifegwConfig::default();
        cfg.auth.tier2_ttl = Duration::from_secs(0);
        assert!(matches!(cfg.validate(), Err(LifegwError::Config(_))));
    }

    #[test]
    fn validate_rejects_oob_sample_ratio() {
        let mut cfg = LifegwConfig::default();
        cfg.observability.trace_sample_ratio = 1.5;
        assert!(matches!(cfg.validate(), Err(LifegwError::Config(_))));
    }
}
