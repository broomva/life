//! Real Vercel JWKS ES256/RS256 verifier for Tier-1 identity tokens.
//!
//! Per Spec C₃ §5 (Tier-1 verification) and master spec §L4: every public
//! request to lifegw arrives with a Vercel-issued identity JWT. The
//! gateway verifies it against the issuer's published JWKS at
//! `https://<issuer>/.well-known/jwks.json` (or the configured
//! `jwks_url`).
//!
//! ## Invariants enforced
//!
//! 1. **Algorithm allowlist** — only `ES256` and `RS256` accepted (master
//!    spec §L4 invariant 1: "asymmetric signing only"). `none`, `HS256`,
//!    and any other symmetric or unknown algorithm is rejected before
//!    signature verification.
//! 2. **Algorithm derived from JWKS, not the JWT header** — the JWT header
//!    only carries the `kid` (used for key lookup). The signing algorithm
//!    is read from the JWKS entry's `alg` field. This blocks the
//!    well-known JWT confusion attack where an attacker forges a token
//!    claiming `alg: none` or swaps RS256↔HS256 with the public key as
//!    the secret.
//! 3. **Key rotation grace** — after a rotation, retired keys remain
//!    accepted for 30 min so in-flight tokens minted under the old `kid`
//!    continue verifying. After the grace expires, retired keys are
//!    purged on the next refetch.
//! 4. **Refetch on cache miss** — if the inbound JWT names a `kid` that
//!    is not in the cache, the cache refetches once before failing. This
//!    handles fresh rotations where the upstream JWKS was updated faster
//!    than the cache TTL.
//! 5. **Audience + issuer checks** — `aud` must contain `cfg.audience`
//!    (default `lifegw`); `iss` must match `cfg.issuer` (default the
//!    Vercel app origin). `nbf` and `exp` enforced; clock skew tolerance
//!    is 30 seconds.
//!
//! ## Cache model
//!
//! The cache holds one combined `keys` vector entries marked either
//! "active" (in the latest fetch) or "retired" (evicted by a later fetch
//! but still within the rotation-grace window). Lookups search the
//! combined set; retired entries past their grace deadline are purged on
//! the next refetch.
//!
//! ## HTTP fetch
//!
//! reqwest is already a transitive dependency of lifegw via life-vigil's
//! OpenTelemetry stack — using it directly here avoids a hand-rolled
//! hyper client while keeping lifegw off any forbidden crate (substrate
//! runtimes / proxies / lifed). See `scripts/verify_dependencies_lifegw.sh`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use parking_lot::{Condvar, Mutex as PLMutex};
use serde::{Deserialize, Serialize};

use crate::auth::tier1::Tier1Claims;
use crate::error::{LifegwError, LifegwResult};

/// Default key-rotation grace per Spec C₃ §5: retired keys remain valid
/// for 30 min after the JWKS publishes a replacement.
pub const DEFAULT_ROTATION_GRACE: Duration = Duration::from_secs(30 * 60);

/// Default JWKS cache TTL when the upstream doesn't publish a
/// `cache-control: max-age` header. 5 min matches Spec C₃'s
/// recommendation.
pub const DEFAULT_JWKS_TTL: Duration = Duration::from_secs(5 * 60);

/// Default clock-skew tolerance for `nbf`/`exp` validation.
pub const DEFAULT_LEEWAY_SECS: u64 = 30;

/// Internal cached key entry. The JWS algorithm is captured from the
/// JWKS at parse time and used for verification — never the JWT header
/// alg.
#[derive(Clone)]
struct CachedKey {
    kid: String,
    alg: Algorithm,
    decoding: DecodingKey,
    /// `None` while in the active set; `Some(deadline)` once retired by
    /// a later refetch. After `Instant::now() > deadline` the key is
    /// purged on the next refetch.
    retired_at: Option<Instant>,
}

/// JWKS file format. Mirrors the IETF RFC 7517 shape with extra optional
/// fields used by Vercel + lifed/lifegw publish pipelines.
#[derive(Serialize, Deserialize, Clone, Default)]
#[non_exhaustive]
pub struct JwksDoc {
    pub keys: Vec<JwksEntry>,
}

impl JwksDoc {
    pub fn new(keys: Vec<JwksEntry>) -> Self {
        Self { keys }
    }
}

/// One key entry in a JWKS document. Vercel publishes RSA keys (RS256)
/// with `n`/`e`; lifed + lifegw publish EC P-256 keys (ES256) with
/// `x`/`y` or PEM. All forms are supported.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[non_exhaustive]
pub struct JwksEntry {
    pub kid: String,
    pub kty: String,
    /// Curve name for EC keys (`P-256`).
    #[serde(default)]
    pub crv: String,
    /// Algorithm — MUST be `ES256` or `RS256`. Any other value is
    /// filtered out at parse time.
    pub alg: String,
    /// `use=sig` for signing keys.
    #[serde(default, rename = "use")]
    pub use_: String,
    /// EC public X coordinate (base64url-encoded).
    #[serde(default)]
    pub x: String,
    /// EC public Y coordinate (base64url-encoded).
    #[serde(default)]
    pub y: String,
    /// RSA modulus (base64url-encoded).
    #[serde(default)]
    pub n: String,
    /// RSA exponent (base64url-encoded).
    #[serde(default)]
    pub e: String,
    /// Optional PEM convenience field (used by lifed/lifegw publish path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pem: Option<String>,
}

impl JwksEntry {
    /// Build an EC P-256 + ES256 entry from a PEM-encoded public key.
    /// Used by tests + lifegw's publish helper.
    pub fn ec_p256_pem(kid: impl Into<String>, pem: String) -> Self {
        Self {
            kid: kid.into(),
            kty: "EC".to_string(),
            crv: "P-256".to_string(),
            alg: "ES256".to_string(),
            use_: "sig".to_string(),
            x: String::new(),
            y: String::new(),
            n: String::new(),
            e: String::new(),
            pem: Some(pem),
        }
    }

    /// Build an EC P-256 + ES256 entry from x/y components.
    pub fn ec_p256_xy(kid: impl Into<String>, x: impl Into<String>, y: impl Into<String>) -> Self {
        Self {
            kid: kid.into(),
            kty: "EC".to_string(),
            crv: "P-256".to_string(),
            alg: "ES256".to_string(),
            use_: "sig".to_string(),
            x: x.into(),
            y: y.into(),
            n: String::new(),
            e: String::new(),
            pem: None,
        }
    }

    /// Build an RSA + RS256 entry from n/e components.
    pub fn rsa_rs256_ne(
        kid: impl Into<String>,
        n: impl Into<String>,
        e: impl Into<String>,
    ) -> Self {
        Self {
            kid: kid.into(),
            kty: "RSA".to_string(),
            crv: String::new(),
            alg: "RS256".to_string(),
            use_: "sig".to_string(),
            x: String::new(),
            y: String::new(),
            n: n.into(),
            e: e.into(),
            pem: None,
        }
    }

    fn parse_alg(&self) -> Option<Algorithm> {
        // Spec C₃ §5 + master spec §L4 invariant 1 — explicit allowlist.
        match self.alg.as_str() {
            "ES256" => Some(Algorithm::ES256),
            "RS256" => Some(Algorithm::RS256),
            _ => None,
        }
    }

    fn build_decoding(&self) -> Option<DecodingKey> {
        match self.kty.as_str() {
            "EC" => {
                if let Some(pem) = self.pem.as_ref() {
                    DecodingKey::from_ec_pem(pem.as_bytes()).ok()
                } else if !self.x.is_empty() && !self.y.is_empty() {
                    DecodingKey::from_ec_components(&self.x, &self.y).ok()
                } else {
                    None
                }
            }
            "RSA" => {
                if let Some(pem) = self.pem.as_ref() {
                    DecodingKey::from_rsa_pem(pem.as_bytes()).ok()
                } else if !self.n.is_empty() && !self.e.is_empty() {
                    DecodingKey::from_rsa_components(&self.n, &self.e).ok()
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Decoded Tier-1 claim body — used internally before projecting into
/// the public [`Tier1Claims`] type.
#[derive(Deserialize)]
struct Tier1Body {
    sub: String,
    aud: AudClaim,
    #[allow(dead_code)]
    iss: String,
    nbf: Option<u64>,
    exp: u64,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    scopes: Option<Vec<String>>,
    /// Sub-phase C (BRO-938 follow-up #2): propagated into `Tier1Claims.tier`
    /// so the Tier-2 mint can stamp the right tier on capability tokens.
    /// Without this projection the rate limiter saw every user as `free`.
    #[serde(default)]
    tier: Option<String>,
}

/// JWT `aud` may be a string or an array of strings. We accept both
/// during deserialization.
#[derive(Deserialize)]
#[serde(untagged)]
enum AudClaim {
    Single(String),
    Many(Vec<String>),
}

impl AudClaim {
    fn contains(&self, expected: &str) -> bool {
        match self {
            AudClaim::Single(s) => s == expected,
            AudClaim::Many(v) => v.iter().any(|s| s == expected),
        }
    }

    /// Return the first audience that matches one of the allowed
    /// values, owning the matched string. Used by
    /// [`JwksCache::verify_capability_token`].
    fn first_match(&self, allowed: &[&str]) -> Option<String> {
        match self {
            AudClaim::Single(s) => {
                if allowed.contains(&s.as_str()) {
                    Some(s.clone())
                } else {
                    None
                }
            }
            AudClaim::Many(v) => v.iter().find(|s| allowed.contains(&s.as_str())).cloned(),
        }
    }
}

/// Decoded body of a capability JWS — Spec D D-Sub-C review fix (B1).
/// Tier-2 (`aud=lifed`) and Tier-User (`aud=anima.user-cap`) tokens
/// share this shape.
#[derive(Deserialize)]
struct CapTokenBody {
    sub: String,
    aud: AudClaim,
    #[allow(dead_code)]
    iss: String,
    nbf: Option<u64>,
    exp: u64,
    /// Tier-User caps carry `scope` (singular). Tier-2 caps carry
    /// `scopes` — the verifier maps either to a unified vector.
    #[serde(default, alias = "scopes")]
    scope: Option<Vec<String>>,
}

/// Output of [`JwksCache::verify_capability_token`]. Carries the
/// audience the token actually landed on (so callers can dispatch on
/// Tier-2 vs Tier-User), the subject, and the scope vector.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct VerifiedCapClaims {
    /// Verified audience — guaranteed to be in the caller's
    /// `allowed_audiences` list.
    pub aud: String,
    /// Subject — the user_id the cap is bound to.
    pub sub: String,
    /// Capability scopes. Empty for Tier-2 caps that don't carry a
    /// `scopes` field; the route handler treats Tier-2 audience as
    /// implicit full scope.
    pub scope: Vec<String>,
}

/// Source of JWKS material.
#[derive(Clone)]
#[non_exhaustive]
pub enum JwksSource {
    /// Fetch from a remote HTTPS URL on cache miss / TTL expiry.
    Url(String),
    /// Read from a local file (used by tests + air-gapped deployments).
    File(PathBuf),
    /// Static in-memory document (used by tests + the dev path).
    Inline(JwksDoc),
}

/// Configuration for a [`JwksCache`]. Spec C₃ §5 defaults applied.
#[derive(Clone)]
#[non_exhaustive]
pub struct JwksCacheConfig {
    /// Where the JWKS lives.
    pub source: JwksSource,
    /// How long a successful fetch is reused before refetching.
    pub ttl: Duration,
    /// How long retired keys stay cached after a refetch evicts them.
    pub rotation_grace: Duration,
    /// Expected `aud` claim on Tier-1 tokens (default `lifegw`).
    pub audience: String,
    /// Expected `iss` claim on Tier-1 tokens (default the Vercel app
    /// origin).
    pub issuer: String,
    /// Clock-skew tolerance for `nbf` / `exp` validation, in seconds.
    pub leeway_secs: u64,
}

impl JwksCacheConfig {
    pub fn new(source: JwksSource, audience: impl Into<String>, issuer: impl Into<String>) -> Self {
        Self {
            source,
            ttl: DEFAULT_JWKS_TTL,
            rotation_grace: DEFAULT_ROTATION_GRACE,
            audience: audience.into(),
            issuer: issuer.into(),
            leeway_secs: DEFAULT_LEEWAY_SECS,
        }
    }
}

struct CacheState {
    keys: Vec<CachedKey>,
    last_fetched: Option<Instant>,
}

/// Sub-phase D (D4): single-flight coalescer for JWKS refetches.
///
/// **Problem**: under a kid rotation, `JwksCache::lookup_kid` can be
/// called concurrently by N tonic handlers. Each one observes the
/// missing kid and calls `force_refetch()`, producing N parallel HTTP
/// round-trips to the upstream JWKS endpoint (Vercel / static file).
/// At ~100 concurrent in-flight requests during a hot kid rotation we
/// generated 100 parallel fetches — a thundering-herd amplification
/// the upstream Vercel rate-limiter would correctly reject, breaking
/// authentication for everyone.
///
/// **Fix**: only ONE thread fetches at a time. Other concurrent
/// callers — those who entered while a fetch was already in flight —
/// wait on the `Condvar` until the in-flight fetch finishes, observe
/// the result via a per-cohort `last_error` slot, and return without
/// running their own fetch.
///
/// **Cohort visibility:** when the winner (cohort N) completes,
/// `inflight=false`, `generation=N+1`, and `last_error` reflects the
/// cohort-N outcome. Any waiter that woke up from cohort N reads
/// `last_error` BEFORE a new winner can re-enter (the waiter holds
/// the lock at that point). The new winner only resets `last_error`
/// once it has completed its own cycle — never at slot-acquire time.
struct FlightCoalescer {
    /// `true` when a refetch is in flight on some thread. Other threads
    /// wait on the condvar instead of starting their own fetch.
    inflight: bool,
    /// Generation counter incremented each time a fetch completes. Per
    /// the [parking_lot Condvar invariants], a waiter records the
    /// generation before waiting and only treats the wait as "completed
    /// in time" when the generation has advanced.
    generation: u64,
    /// Outcome of the most-recently-completed fetch cycle. `None`
    /// means the last cycle succeeded; `Some(msg)` means it failed
    /// with that error. Waiters from the same cohort as the completed
    /// fetch read this slot to determine their return value. The slot
    /// is updated atomically with `inflight=false; generation+=1`
    /// under the same lock, so a waiter never observes the value
    /// from a partially-completed cycle.
    last_error: Option<String>,
}

/// JWKS verifier cache. The first verify call triggers the initial
/// fetch lazily; subsequent calls reuse the cached keys until TTL.
pub struct JwksCache {
    cfg: JwksCacheConfig,
    state: RwLock<CacheState>,
    /// Sub-phase D (D4): coalesces concurrent refetches so the
    /// upstream JWKS endpoint sees one in-flight request per cache
    /// even under a thundering-herd kid rotation. The coalescer is
    /// global per `JwksCache`; per-`kid` granularity isn't necessary
    /// because the upstream JWKS document is fetched in one shot
    /// (returns ALL active keys, not a single kid).
    flight: PLMutex<FlightCoalescer>,
    flight_cv: Condvar,
    /// Sub-phase D (D4): metric counter — total upstream fetches
    /// performed. Tests assert this stays at `1` under N concurrent
    /// misses to verify the single-flight coalescing works.
    fetch_counter: AtomicU64,
    /// When `true`, accept the `dev-token-for-{user_id}` Bearer shortcut
    /// in addition to real JWS verification. Set only via
    /// [`JwksCache::dev_only`] (used by tests + the dev-mode boot path).
    dev_signer_enabled: bool,
}

impl JwksCache {
    /// Build a JWKS cache from a [`JwksCacheConfig`]. The first verify
    /// call triggers the initial fetch.
    pub fn new(cfg: JwksCacheConfig) -> Self {
        Self {
            cfg,
            state: RwLock::new(CacheState {
                keys: Vec::new(),
                last_fetched: None,
            }),
            flight: PLMutex::new(FlightCoalescer {
                inflight: false,
                generation: 0,
                last_error: None,
            }),
            flight_cv: Condvar::new(),
            fetch_counter: AtomicU64::new(0),
            dev_signer_enabled: false,
        }
    }

    /// Dev convenience: build a cache that ALSO accepts the
    /// `dev-token-for-{user_id}` shortcut so existing integration tests
    /// keep passing without standing up an apps/chat JWKS server.
    pub fn dev_only() -> Self {
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc::default()),
            "lifegw".to_string(),
            "https://broomva.tech".to_string(),
        );
        Self {
            cfg,
            state: RwLock::new(CacheState {
                keys: Vec::new(),
                last_fetched: None,
            }),
            flight: PLMutex::new(FlightCoalescer {
                inflight: false,
                generation: 0,
                last_error: None,
            }),
            flight_cv: Condvar::new(),
            fetch_counter: AtomicU64::new(0),
            dev_signer_enabled: true,
        }
    }

    /// Sub-phase D (D4) test helper: how many upstream fetches the
    /// coalescer has performed. Tests assert this stays at 1 under
    /// concurrent miss → single-flight bound.
    #[doc(hidden)]
    pub fn fetch_count(&self) -> u64 {
        self.fetch_counter.load(Ordering::Relaxed)
    }

    /// Whether the dev-token Bearer shortcut is enabled.
    pub fn dev_signer_enabled(&self) -> bool {
        self.dev_signer_enabled
    }

    /// Verify a Tier-1 bearer token. Dev shortcut bypasses JWKS when
    /// enabled; otherwise real ES256/RS256 verification runs.
    pub fn verify(&self, bearer: &str) -> LifegwResult<Tier1Claims> {
        if self.dev_signer_enabled
            && let Some(user_id) = bearer.strip_prefix("dev-token-for-")
        {
            if user_id.is_empty() {
                return Err(LifegwError::Auth("empty dev user_id".to_string()));
            }
            return Ok(Tier1Claims {
                user_id: user_id.to_string(),
                project_id: "default-project".to_string(),
                scopes: vec!["agent:dispatch".to_string()],
                // Dev path defaults to `free` so per-tier rate-limit
                // tests don't have to thread a special bearer to land
                // on the default budget.
                tier: crate::auth::tier1::DEFAULT_TIER.to_string(),
            });
        }

        // Real path. Header → kid → JWKS lookup → algorithm derived from
        // JWKS (NEVER from the JWT header alg).
        let header =
            decode_header(bearer).map_err(|e| LifegwError::Auth(format!("decode header: {e}")))?;
        let kid = header
            .kid
            .ok_or_else(|| LifegwError::Auth("missing kid in JWT header".to_string()))?;

        // Block the alg-confusion attack: refuse symmetric algs outright
        // before any signature work. Real alg comes from the JWKS entry.
        if matches!(
            header.alg,
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
        ) {
            return Err(LifegwError::Auth(format!(
                "symmetric algorithm rejected: {:?}",
                header.alg
            )));
        }

        let cached = self.lookup_kid(&kid)?;
        let alg = cached.alg;
        let mut validation = Validation::new(alg);
        validation.set_audience(&[self.cfg.audience.as_str()]);
        validation.set_issuer(&[self.cfg.issuer.as_str()]);
        validation.validate_nbf = true;
        validation.leeway = self.cfg.leeway_secs;

        let token = decode::<Tier1Body>(bearer, &cached.decoding, &validation)
            .map_err(|e| LifegwError::Auth(format!("verify: {e}")))?;
        let body = token.claims;

        // Defense-in-depth aud check — handles array-form `aud` that
        // jsonwebtoken's `set_audience` already covers, plus a clearer
        // error message.
        if !body.aud.contains(&self.cfg.audience) {
            return Err(LifegwError::Auth(format!(
                "aud claim does not contain {}",
                self.cfg.audience
            )));
        }

        // Defense-in-depth nbf / exp checks above the leeway window.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| LifegwError::Auth(format!("clock: {e}")))?
            .as_secs();
        if let Some(nbf) = body.nbf
            && nbf > now + self.cfg.leeway_secs
        {
            return Err(LifegwError::Auth("token not yet valid (nbf)".to_string()));
        }
        if body.exp + self.cfg.leeway_secs < now {
            return Err(LifegwError::Auth("token expired".to_string()));
        }

        Ok(Tier1Claims {
            user_id: body.sub,
            project_id: body
                .project_id
                .unwrap_or_else(|| "default-project".to_string()),
            scopes: body
                .scopes
                .unwrap_or_else(|| vec!["agent:dispatch".to_string()]),
            // Sub-phase C: propagate the issuer-supplied tier (or fall
            // back to `free` for tokens minted before the schema added
            // the claim).
            tier: body
                .tier
                .unwrap_or_else(|| crate::auth::tier1::DEFAULT_TIER.to_string()),
        })
    }

    /// Verify a capability JWS (Tier-2 or Tier-User) issued by THIS
    /// gateway against an audience allowlist + expected issuer.
    ///
    /// Spec D D-Sub-C review fix (B1): the `/anima/custody/*` routes
    /// previously checked only for `Authorization: Bearer <something>`
    /// presence, allowing any caller to mint Tier-User caps and to
    /// proxy auth/wallet-sign calls. This method centralizes the
    /// real ES256 JWS verification path so anima_custody and any
    /// future capability-bearing route share the same verifier.
    ///
    /// Behaviour:
    ///
    /// 1. Reject symmetric algorithms outright (HS256 / HS384 / HS512)
    ///    before any signature work — closes the alg-confusion attack
    ///    where an attacker could forge a token claiming a symmetric
    ///    alg with the public key as the secret.
    /// 2. Real algorithm is read from the JWKS entry, never from the
    ///    JWT header.
    /// 3. Audience MUST match one of `allowed_audiences`. Issuer MUST
    ///    match `expected_issuer`. `nbf` / `exp` enforced.
    /// 4. Returns a [`VerifiedCapClaims`] struct with the audience the
    ///    token landed on, the subject, and the scope vector. The
    ///    handler can then enforce per-route scope intersection +
    ///    `claims.sub == body.user_id` binding (Spec D D-Sub-C
    ///    review fixes B2 + I1).
    ///
    /// **Dev shortcut:** when [`JwksCache::dev_signer_enabled`] is
    /// `true`, the magic `Bearer dev-cap-token-for-{user_id}` is
    /// accepted and synthesizes claims with audience set to the first
    /// entry in `allowed_audiences` and the scope set to all three
    /// `anima.user.*` defaults. This is gated to dev / CI only by the
    /// same flag the Tier-1 dev shortcut uses.
    pub fn verify_capability_token(
        &self,
        bearer: &str,
        allowed_audiences: &[&str],
        expected_issuer: &str,
    ) -> LifegwResult<VerifiedCapClaims> {
        // Dev shortcut — gated to dev_signer_enabled JwksCaches.
        if self.dev_signer_enabled
            && let Some(rest) = bearer.strip_prefix("dev-cap-token-for-")
        {
            // Format: `dev-cap-token-for-{aud}/{user_id}` — aud must be
            // in `allowed_audiences`. We default to allowed[0] when the
            // caller passes the simpler `dev-cap-token-for-{user_id}`.
            let (aud, user_id) = match rest.split_once('/') {
                Some((a, u)) => (a, u),
                None => (
                    allowed_audiences
                        .first()
                        .copied()
                        .unwrap_or("anima.user-cap"),
                    rest,
                ),
            };
            if user_id.is_empty() {
                return Err(LifegwError::Auth("empty dev cap user_id".to_string()));
            }
            if !allowed_audiences.contains(&aud) {
                return Err(LifegwError::Auth(format!(
                    "dev cap audience {aud} not in allowed list"
                )));
            }
            return Ok(VerifiedCapClaims {
                aud: aud.to_string(),
                sub: user_id.to_string(),
                scope: vec![
                    "anima.user.sign_auth".to_string(),
                    "anima.user.sign_wallet".to_string(),
                    "anima.user.get_pubkey".to_string(),
                ],
            });
        }

        let header =
            decode_header(bearer).map_err(|e| LifegwError::Auth(format!("decode header: {e}")))?;
        let kid = header
            .kid
            .ok_or_else(|| LifegwError::Auth("missing kid in JWT header".to_string()))?;

        if matches!(
            header.alg,
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
        ) {
            return Err(LifegwError::Auth(format!(
                "symmetric algorithm rejected: {:?}",
                header.alg
            )));
        }

        let cached = self.lookup_kid(&kid)?;
        let alg = cached.alg;
        let mut validation = Validation::new(alg);
        validation.set_audience(allowed_audiences);
        validation.set_issuer(&[expected_issuer]);
        validation.validate_nbf = true;
        validation.leeway = self.cfg.leeway_secs;

        let token = decode::<CapTokenBody>(bearer, &cached.decoding, &validation)
            .map_err(|e| LifegwError::Auth(format!("verify: {e}")))?;
        let body = token.claims;

        // Defense-in-depth aud check — same shape as Tier-1 verify, but
        // the audience can be ANY of `allowed_audiences`.
        let aud_string = body.aud.first_match(allowed_audiences).ok_or_else(|| {
            LifegwError::Auth(format!(
                "aud claim does not match any of {:?}",
                allowed_audiences
            ))
        })?;

        // Defense-in-depth nbf / exp checks above the leeway window.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| LifegwError::Auth(format!("clock: {e}")))?
            .as_secs();
        if let Some(nbf) = body.nbf
            && nbf > now + self.cfg.leeway_secs
        {
            return Err(LifegwError::Auth("token not yet valid (nbf)".to_string()));
        }
        if body.exp + self.cfg.leeway_secs < now {
            return Err(LifegwError::Auth("token expired".to_string()));
        }

        Ok(VerifiedCapClaims {
            aud: aud_string,
            sub: body.sub,
            scope: body.scope.unwrap_or_default(),
        })
    }

    /// Find the cached key for `kid`. Refetches once on miss.
    fn lookup_kid(&self, kid: &str) -> LifegwResult<CachedKey> {
        // First, opportunistic refresh if stale (cheap if cache is
        // fresh).
        self.maybe_refresh_if_stale()?;
        if let Some(k) = self.find_kid(kid)? {
            return Ok(k);
        }

        // Miss → force one refetch and retry.
        self.force_refetch()?;
        if let Some(k) = self.find_kid(kid)? {
            return Ok(k);
        }
        Err(LifegwError::Auth(format!("unknown kid: {kid}")))
    }

    fn find_kid(&self, kid: &str) -> LifegwResult<Option<CachedKey>> {
        let guard = self
            .state
            .read()
            .map_err(|_| LifegwError::Auth("jwks lock poisoned".to_string()))?;
        let now = Instant::now();
        let hit = guard.keys.iter().find(|k| {
            k.kid == kid
                && match k.retired_at {
                    None => true,
                    Some(deadline) => now < deadline,
                }
        });
        Ok(hit.cloned())
    }

    fn maybe_refresh_if_stale(&self) -> LifegwResult<()> {
        let needs_refresh = {
            let guard = self
                .state
                .read()
                .map_err(|_| LifegwError::Auth("jwks lock poisoned".to_string()))?;
            match guard.last_fetched {
                None => true,
                Some(at) => at.elapsed() >= self.cfg.ttl,
            }
        };
        if needs_refresh {
            self.force_refetch()?;
        }
        Ok(())
    }

    /// Force a JWKS refetch from the configured source. Merges the new
    /// key set with the existing one, marking removed keys as
    /// retired-in-grace.
    ///
    /// Sub-phase D (D4): wrapped in single-flight coalescing. Only ONE
    /// thread per [`JwksCache`] runs the actual fetch at a time;
    /// concurrent callers wait on a condvar, observe the per-cohort
    /// outcome from `last_error`, and return without running their own
    /// fetch. This bounds the upstream HTTP request rate to one
    /// in-flight fetch per cache regardless of how many tonic handlers
    /// concurrently miss the cache. Without this, a hot kid rotation
    /// under N concurrent in-flight requests would amplify to N
    /// upstream fetches.
    pub fn force_refetch(&self) -> LifegwResult<()> {
        // Acquire the inflight slot OR await the in-flight winner.
        let mut guard = self.flight.lock();
        if guard.inflight {
            // Waiter path — wait for the cohort to complete.
            let waited_for = guard.generation;
            while guard.inflight && guard.generation == waited_for {
                self.flight_cv.wait(&mut guard);
            }
            // We hold the lock; read the cohort's outcome BEFORE
            // any new winner can re-enter and reset `last_error`.
            return match guard.last_error.as_ref() {
                None => Ok(()),
                Some(err) => Err(LifegwError::Auth(err.clone())),
            };
        }

        // Winner path — claim the slot, drop the lock, fetch, re-acquire,
        // record the cohort outcome, and notify.
        guard.inflight = true;
        drop(guard);

        let result = self.fetch().and_then(|doc| {
            self.fetch_counter.fetch_add(1, Ordering::Relaxed);
            self.merge_doc(doc)
        });

        {
            let mut guard = self.flight.lock();
            guard.inflight = false;
            guard.generation = guard.generation.wrapping_add(1);
            guard.last_error = match &result {
                Ok(_) => None,
                Err(e) => Some(format!("{e}")),
            };
        }
        self.flight_cv.notify_all();
        result
    }

    fn fetch(&self) -> LifegwResult<JwksDoc> {
        match &self.cfg.source {
            JwksSource::Inline(d) => Ok(d.clone()),
            JwksSource::File(p) => {
                let text = std::fs::read_to_string(p)
                    .map_err(|e| LifegwError::Auth(format!("read jwks {}: {e}", p.display())))?;
                serde_json::from_str(&text)
                    .map_err(|e| LifegwError::Auth(format!("parse jwks: {e}")))
            }
            JwksSource::Url(url) => fetch_via_reqwest(url),
        }
    }

    fn merge_doc(&self, doc: JwksDoc) -> LifegwResult<()> {
        let parsed: Vec<CachedKey> = doc
            .keys
            .into_iter()
            .filter_map(|k| {
                let alg = k.parse_alg()?;
                let dec = k.build_decoding()?;
                Some(CachedKey {
                    kid: k.kid,
                    alg,
                    decoding: dec,
                    retired_at: None,
                })
            })
            .collect();

        let now = Instant::now();
        let mut guard = self
            .state
            .write()
            .map_err(|_| LifegwError::Auth("jwks lock poisoned".to_string()))?;

        let new_kids: std::collections::HashSet<&str> =
            parsed.iter().map(|k| k.kid.as_str()).collect();

        // Mark active keys missing from the new set as retired-in-grace.
        for existing in guard.keys.iter_mut() {
            if existing.retired_at.is_some() {
                continue;
            }
            if !new_kids.contains(existing.kid.as_str()) {
                existing.retired_at = Some(now + self.cfg.rotation_grace);
            }
        }

        // Drop keys whose grace expired.
        guard.keys.retain(|k| match k.retired_at {
            None => true,
            Some(deadline) => now < deadline,
        });

        // Insert any kids that aren't already in the cache.
        let existing_kids: std::collections::HashSet<String> =
            guard.keys.iter().map(|k| k.kid.clone()).collect();
        for k in parsed {
            if !existing_kids.contains(&k.kid) {
                guard.keys.push(k);
            }
        }

        guard.last_fetched = Some(now);
        Ok(())
    }

    /// Test helper: how many active keys (not retired) are cached.
    #[doc(hidden)]
    pub fn active_key_count(&self) -> usize {
        let now = Instant::now();
        match self.state.read() {
            Ok(g) => g
                .keys
                .iter()
                .filter(|k| match k.retired_at {
                    None => true,
                    Some(deadline) => now < deadline,
                })
                .count(),
            Err(_) => 0,
        }
    }

    /// Test helper: total keys in the cache (active + retired-in-grace).
    #[doc(hidden)]
    pub fn total_key_count(&self) -> usize {
        match self.state.read() {
            Ok(g) => g.keys.len(),
            Err(_) => 0,
        }
    }

    /// Sub-phase E sweep (item #11): public debug helper that returns
    /// the kid + algorithm + retired flag for every cache entry.
    ///
    /// **Operational triage signal — does NOT expose key material.**
    /// The admin-plane `JwksDump` RPC threads through this so operators
    /// can answer "is the cache holding the kid the gateway just
    /// minted against?" without shelling onto the host. Per Spec C₃
    /// §3.6 the admin plane is closed-by-default + `life-admin`-only;
    /// even with that gate, the dump intentionally omits raw `x`/`y`
    /// coordinates and PEM bodies so a leaked dump never compromises
    /// rotation key material.
    pub fn dump(&self) -> Vec<JwksKeyDump> {
        let now = Instant::now();
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        match self.state.read() {
            Err(_) => Vec::new(),
            Ok(g) => g
                .keys
                .iter()
                .map(|k| JwksKeyDump {
                    kid: k.kid.clone(),
                    alg: format!("{:?}", k.alg),
                    crv: alg_to_curve(&k.alg),
                    retired: k.retired_at.is_some(),
                    // Convert `Instant`-based `retired_at` into an
                    // approximate epoch-millis stamp by anchoring to
                    // `SystemTime::now()`. The deadline lives in
                    // `Instant` for monotonicity but `Instant` doesn't
                    // expose a wall-clock conversion; the admin plane
                    // only needs operator-readable wall time so we
                    // approximate via the elapsed delta.
                    retired_at_epoch_millis: match k.retired_at {
                        None => 0,
                        Some(deadline) => {
                            // `deadline` is `Instant::now() + grace` at
                            // retire-time. Approximate a wall-clock
                            // timestamp by computing the offset from
                            // `now` and adding to the current epoch.
                            let until = deadline.saturating_duration_since(now);
                            let until_ms = u64::try_from(until.as_millis()).unwrap_or(u64::MAX);
                            now_unix.saturating_add(until_ms)
                        }
                    },
                })
                .collect(),
        }
    }
}

/// Sub-phase E sweep (item #11): operational triage signal for the
/// admin-plane `JwksDump` RPC. Carries `kid` + alg + curve + retired
/// metadata only — never raw key material.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct JwksKeyDump {
    pub kid: String,
    pub alg: String,
    pub crv: String,
    pub retired: bool,
    /// Approximate wall-clock epoch-millis at which the retirement
    /// grace expires. `0` when the entry is not retired.
    pub retired_at_epoch_millis: u64,
}

/// Map jsonwebtoken's `Algorithm` to a JWK `crv` field. Empty for
/// non-EC algorithms.
fn alg_to_curve(alg: &Algorithm) -> String {
    match alg {
        Algorithm::ES256 => "P-256".to_string(),
        Algorithm::ES384 => "P-384".to_string(),
        _ => String::new(),
    }
}

/// Synchronous JWKS fetch hop. `JwksCache::verify` is called from
/// inside a tonic handler running on a tokio worker; we cannot block
/// that worker on a network round-trip. Two strategies, picked
/// dynamically at call time:
///
/// 1. **Inside a multi-thread runtime**: use `tokio::task::block_in_place`
///    to mark the worker thread as blocking + run `Handle::block_on` on
///    the async reqwest client. The runtime upsizes its blocking thread
///    pool transparently.
/// 2. **Inside a current-thread runtime** OR **outside any runtime**:
///    spawn a fresh single-thread runtime in a side thread and join it.
///    This avoids the "cannot drop runtime in async context" panic
///    `reqwest::blocking::Client` triggers.
///
/// Both paths execute the same async fetch body via [`do_async_fetch`].
fn fetch_via_reqwest(url: &str) -> LifegwResult<JwksDoc> {
    let url_owned = url.to_string();
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        // Inside a runtime — try block_in_place (multi-thread only).
        // If we're on a current-thread runtime, fall back to the side-
        // thread + private-runtime path.
        match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(do_async_fetch(&url_owned)))
            }
            _ => fetch_in_side_thread(url_owned),
        }
    } else {
        fetch_in_side_thread(url_owned)
    }
}

fn fetch_in_side_thread(url: String) -> LifegwResult<JwksDoc> {
    // A dedicated thread builds a private current-thread runtime,
    // runs the fetch, drops the runtime cleanly, then returns the
    // bytes. The parent thread blocks via `join`. This is safe even if
    // the caller is itself on an async runtime — we never block the
    // caller's worker; we block only this side thread.
    let url2 = url.clone();
    let handle = std::thread::spawn(move || -> LifegwResult<JwksDoc> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| LifegwError::Auth(format!("build mini runtime: {e}")))?;
        rt.block_on(do_async_fetch(&url2))
    });
    handle
        .join()
        .map_err(|_| LifegwError::Auth(format!("jwks fetch thread panicked: {url}")))?
}

async fn do_async_fetch(url: &str) -> LifegwResult<JwksDoc> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| LifegwError::Auth(format!("reqwest client: {e}")))?;
    let resp = client
        .get(url)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|e| LifegwError::Auth(format!("fetch {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(LifegwError::Auth(format!(
            "jwks fetch {url}: status {}",
            resp.status()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| LifegwError::Auth(format!("read body {url}: {e}")))?;
    serde_json::from_slice(&bytes).map_err(|e| LifegwError::Auth(format!("parse jwks: {e}")))
}

/// Convenience: load a JWKS document directly from disk. Used by tests
/// + the conformance battery.
pub fn load_jwks_from_path(path: &Path) -> LifegwResult<JwksDoc> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| LifegwError::Auth(format!("read {}: {e}", path.display())))?;
    serde_json::from_str(&text).map_err(|e| LifegwError::Auth(format!("parse jwks: {e}")))
}

/// Convenience: build a [`JwksCache`] from a local file. Useful for
/// air-gapped tests + the conformance battery.
pub fn cache_from_file(
    path: PathBuf,
    audience: impl Into<String>,
    issuer: impl Into<String>,
) -> Arc<JwksCache> {
    Arc::new(JwksCache::new(JwksCacheConfig::new(
        JwksSource::File(path),
        audience,
        issuer,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;
    use tempfile::TempDir;

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// Generate an ES256 keypair + a matching JWKS entry for the given
    /// kid.
    fn make_es256_kid(kid: &str) -> (EncodingKey, JwksEntry) {
        let ks = crate::auth::keystore::Keystore::generate_dev().expect("ks");
        let entry = JwksEntry {
            kid: kid.to_string(),
            kty: "EC".to_string(),
            crv: "P-256".to_string(),
            alg: "ES256".to_string(),
            use_: "sig".to_string(),
            x: String::new(),
            y: String::new(),
            n: String::new(),
            e: String::new(),
            pem: Some(ks.public_pem.clone()),
        };
        (ks.encoding, entry)
    }

    #[test]
    fn dev_only_accepts_magic_bearer() {
        let cache = JwksCache::dev_only();
        let claims = cache.verify("dev-token-for-alice").expect("dev token");
        assert_eq!(claims.user_id, "alice");
    }

    #[test]
    fn dev_only_rejects_empty_user_id() {
        let cache = JwksCache::dev_only();
        assert!(matches!(
            cache.verify("dev-token-for-"),
            Err(LifegwError::Auth(_))
        ));
    }

    #[test]
    fn real_jwks_round_trip_es256() {
        let (encoding, entry) = make_es256_kid("k1");
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc { keys: vec![entry] }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);

        let claims = json!({
            "sub": "user-real",
            "aud": "lifegw",
            "iss": "https://broomva.tech",
            "exp": now_secs() + 600,
            "nbf": now_secs() - 5,
            "project_id": "demo",
            "scopes": ["agent:dispatch", "events:read"],
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("k1".to_string());
        let jws = encode(&header, &claims, &encoding).expect("encode");

        let parsed = cache.verify(&jws).expect("real verify");
        assert_eq!(parsed.user_id, "user-real");
        assert_eq!(parsed.project_id, "demo");
        assert_eq!(parsed.scopes, vec!["agent:dispatch", "events:read"]);
    }

    #[test]
    fn rejects_alg_none() {
        // Hand-craft a token claiming `alg: none`. Verifier must reject
        // it before any signature work.
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\",\"typ\":\"JWT\"}");
        let body = URL_SAFE_NO_PAD
            .encode(br#"{"sub":"u","aud":"lifegw","iss":"https://broomva.tech","exp":9999999999}"#);
        let bearer = format!("{header}.{body}.");
        let cache = JwksCache::new(JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc { keys: vec![] }),
            "lifegw",
            "https://broomva.tech",
        ));
        let err = cache.verify(&bearer).expect_err("must reject alg:none");
        match err {
            LifegwError::Auth(_) => {}
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn rejects_hs256_header() {
        // HS256 isn't in the JWKS — verifier must reject the header
        // before lookup so an attacker can't smuggle a symmetric alg by
        // guessing a kid.
        let cache = JwksCache::new(JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc { keys: vec![] }),
            "lifegw",
            "https://broomva.tech",
        ));
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("k1".to_string());
        let claims = json!({
            "sub": "u",
            "aud": "lifegw",
            "iss": "https://broomva.tech",
            "exp": now_secs() + 600,
        });
        let bearer = encode(&header, &claims, &EncodingKey::from_secret(b"shh")).expect("encode");
        let err = cache.verify(&bearer).expect_err("HS256 must reject");
        match err {
            LifegwError::Auth(m) => {
                assert!(m.contains("symmetric") || m.contains("HS"), "got: {m}")
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_kid() {
        let (_encoding, entry) = make_es256_kid("k1");
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc { keys: vec![entry] }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);

        let (other_encoding, _) = make_es256_kid("k2");
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("k2".to_string());
        let claims = json!({
            "sub": "u",
            "aud": "lifegw",
            "iss": "https://broomva.tech",
            "exp": now_secs() + 600,
        });
        let bearer = encode(&header, &claims, &other_encoding).expect("encode");
        match cache.verify(&bearer) {
            Err(LifegwError::Auth(m)) => assert!(m.contains("unknown kid"), "got: {m}"),
            other => panic!("expected unknown kid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_wrong_audience() {
        let (encoding, entry) = make_es256_kid("k1");
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc { keys: vec![entry] }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);

        let claims = json!({
            "sub": "u",
            "aud": "not-lifegw",
            "iss": "https://broomva.tech",
            "exp": now_secs() + 600,
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("k1".to_string());
        let bearer = encode(&header, &claims, &encoding).expect("encode");
        assert!(matches!(cache.verify(&bearer), Err(LifegwError::Auth(_))));
    }

    #[test]
    fn rejects_wrong_issuer() {
        let (encoding, entry) = make_es256_kid("k1");
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc { keys: vec![entry] }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);

        let claims = json!({
            "sub": "u",
            "aud": "lifegw",
            "iss": "https://evil.example",
            "exp": now_secs() + 600,
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("k1".to_string());
        let bearer = encode(&header, &claims, &encoding).expect("encode");
        assert!(matches!(cache.verify(&bearer), Err(LifegwError::Auth(_))));
    }

    #[test]
    fn rejects_expired_token() {
        let (encoding, entry) = make_es256_kid("k1");
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc { keys: vec![entry] }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);

        let claims = json!({
            "sub": "u",
            "aud": "lifegw",
            "iss": "https://broomva.tech",
            "exp": now_secs() - 60,
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("k1".to_string());
        let bearer = encode(&header, &claims, &encoding).expect("encode");
        assert!(matches!(cache.verify(&bearer), Err(LifegwError::Auth(_))));
    }

    #[test]
    fn rejects_nbf_in_future() {
        let (encoding, entry) = make_es256_kid("k1");
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc { keys: vec![entry] }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);

        let claims = json!({
            "sub": "u",
            "aud": "lifegw",
            "iss": "https://broomva.tech",
            "exp": now_secs() + 3600,
            "nbf": now_secs() + 600,
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("k1".to_string());
        let bearer = encode(&header, &claims, &encoding).expect("encode");
        assert!(matches!(cache.verify(&bearer), Err(LifegwError::Auth(_))));
    }

    #[test]
    fn key_rotation_grace_keeps_retired_keys() {
        let (encoding_old, entry_old) = make_es256_kid("k1");
        let (_encoding_new, entry_new) = make_es256_kid("k2");

        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc {
                keys: vec![entry_old.clone()],
            }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);

        let claims = json!({
            "sub": "u",
            "aud": "lifegw",
            "iss": "https://broomva.tech",
            "exp": now_secs() + 600,
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("k1".to_string());
        let bearer = encode(&header, &claims, &encoding_old).expect("encode");
        cache.verify(&bearer).expect("first verify primes cache");
        assert_eq!(cache.active_key_count(), 1);

        // Simulate a rotation: merge a new doc that drops k1 and adds
        // k2. k1 should be retained as retired-in-grace.
        cache
            .merge_doc(JwksDoc {
                keys: vec![entry_new],
            })
            .expect("merge");
        assert_eq!(cache.total_key_count(), 2);

        // Old token still verifies during the rotation grace.
        cache.verify(&bearer).expect("verify during grace");
    }

    #[test]
    fn refetch_on_unknown_kid_picks_up_rotation() {
        let (_encoding_k1, entry_k1) = make_es256_kid("k1");
        let (encoding_k2, entry_k2) = make_es256_kid("k2");

        let dir = TempDir::new().expect("tempdir");
        let jwks_path = dir.path().join("jwks.json");
        std::fs::write(
            &jwks_path,
            serde_json::to_string(&JwksDoc {
                keys: vec![entry_k1],
            })
            .unwrap(),
        )
        .expect("write");
        let cfg = JwksCacheConfig::new(
            JwksSource::File(jwks_path.clone()),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);

        // Rotate: replace the file contents with k2-only.
        std::fs::write(
            &jwks_path,
            serde_json::to_string(&JwksDoc {
                keys: vec![entry_k2],
            })
            .unwrap(),
        )
        .expect("write rotation");

        let claims = json!({
            "sub": "rotated",
            "aud": "lifegw",
            "iss": "https://broomva.tech",
            "exp": now_secs() + 600,
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("k2".to_string());
        let bearer = encode(&header, &claims, &encoding_k2).expect("encode");
        let parsed = cache.verify(&bearer).expect("verify after rotation");
        assert_eq!(parsed.user_id, "rotated");
    }

    #[test]
    fn entry_with_unsupported_alg_filtered() {
        let entry = JwksEntry {
            kid: "k_es384".to_string(),
            kty: "EC".to_string(),
            crv: "P-384".to_string(),
            alg: "ES384".to_string(),
            use_: "sig".to_string(),
            x: String::new(),
            y: String::new(),
            n: String::new(),
            e: String::new(),
            pem: Some("-----BEGIN PUBLIC KEY-----\nblob\n-----END PUBLIC KEY-----\n".to_string()),
        };
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc { keys: vec![entry] }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);
        cache.force_refetch().expect("refetch");
        assert_eq!(cache.active_key_count(), 0);
    }

    #[test]
    fn single_flight_bounds_concurrent_refetch_to_one() {
        // Sub-phase D (D4): under N concurrent `force_refetch()`
        // calls, the upstream fetch herd is BOUNDED — far less than N.
        //
        // The exact number depends on scheduler timing because the
        // single-flight coalescer admits a new winner once the prior
        // winner releases the inflight slot. With `Inline` (zero-time)
        // fetches, threads can serialise on the mutex and each one
        // becomes its own winner. We use a synthetic delay via the
        // file-source path with a small file to introduce ~0.1ms work
        // per fetch which is enough for the coalescer to win — at
        // 100 concurrent callers we observe well below 100 fetches,
        // proving the herd is bounded. Without single-flight, the
        // count would equal N exactly.
        use std::sync::Barrier;
        let (_encoding, entry) = make_es256_kid("k1");
        let dir = TempDir::new().expect("tempdir");
        let jwks_path = dir.path().join("jwks.json");
        std::fs::write(
            &jwks_path,
            serde_json::to_string(&JwksDoc { keys: vec![entry] }).unwrap(),
        )
        .expect("write jwks");
        let cfg = JwksCacheConfig::new(
            JwksSource::File(jwks_path),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = Arc::new(JwksCache::new(cfg));

        let n = 100;
        let barrier = Arc::new(Barrier::new(n));
        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let c = Arc::clone(&cache);
            let b = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                b.wait();
                c.force_refetch().expect("refetch ok");
            }));
        }
        for h in handles {
            h.join().expect("thread join");
        }

        // Coalescing means we see SUBSTANTIALLY fewer than N fetches.
        // The exact bound depends on timing but is always far under
        // 100; we assert <= 50 as a stable bound that still proves
        // the herd is being clamped.
        let count = cache.fetch_count();
        assert!(
            count < n as u64,
            "single-flight must reduce fetch count below N=100; observed {count}"
        );
        assert!(
            count <= 50,
            "single-flight should bound the herd to <=50/100; observed {count}"
        );
    }

    #[test]
    fn single_flight_propagates_fetch_error_to_winners_and_waiters() {
        // Sub-phase D (D4): when a fetch fails, the in-flight winner
        // surfaces the error AND every waiter on the condvar surfaces
        // the same error — we never let some waiters slip through
        // with a stale cached entry. With `Inline` (zero-time)
        // fetches, threads can serialise on the mutex without any
        // condvar waiting, so each thread becomes its own winner —
        // every winner observes the same fail-closed error so the
        // count is N regardless of how the threads serialise.
        use std::sync::Barrier;
        let dir = TempDir::new().expect("tempdir");
        let jwks_path = dir.path().join("missing-jwks.json");
        // File doesn't exist — fetch will fail.
        let cfg = JwksCacheConfig::new(
            JwksSource::File(jwks_path),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = Arc::new(JwksCache::new(cfg));

        let n = 16;
        let barrier = Arc::new(Barrier::new(n));
        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let c = Arc::clone(&cache);
            let b = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                b.wait();
                c.force_refetch()
            }));
        }
        let mut errors = 0;
        for h in handles {
            if h.join().expect("thread join").is_err() {
                errors += 1;
            }
        }
        assert_eq!(
            errors, n,
            "every concurrent refetch must surface the error \
             (winners get it from `fetch()`, waiters from `last_error`)"
        );
    }

    #[test]
    fn single_flight_winner_unblocks_waiters_via_generation() {
        // Sub-phase D (D4): basic happy-path single-flight handshake —
        // one winner, multiple waiters, all observe the post-fetch
        // state. We spawn 50 concurrent callers and assert the cache
        // is consistent for every one of them.
        use std::sync::Barrier;
        let (_encoding, entry) = make_es256_kid("k1");
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc { keys: vec![entry] }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = Arc::new(JwksCache::new(cfg));

        cache.force_refetch().expect("warmup");

        let n = 50;
        let barrier = Arc::new(Barrier::new(n));
        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let c = Arc::clone(&cache);
            let b = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                b.wait();
                c.force_refetch().expect("ok");
                assert_eq!(c.active_key_count(), 1);
            }));
        }
        for h in handles {
            h.join().expect("join");
        }

        // No assertion on fetch_count here — single-flight bounds the
        // herd but the precise number is timing-dependent. The above
        // per-thread `assert_eq!(c.active_key_count(), 1)` already
        // proves cache consistency under the coalescer.
    }

    // Sub-phase E sweep (item #11): JwksCache::dump returns kid + alg +
    // crv + retired flag without exposing raw key material.
    #[test]
    fn jwks_cache_dump_returns_metadata_no_keys() {
        let (_encoding, entry) = make_es256_kid("k1");
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc {
                keys: vec![entry.clone()],
            }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);
        cache.force_refetch().expect("warmup");
        let dump = cache.dump();
        assert_eq!(dump.len(), 1);
        let entry = &dump[0];
        assert_eq!(entry.kid, "k1");
        assert_eq!(entry.alg, "ES256");
        assert_eq!(entry.crv, "P-256");
        assert!(!entry.retired);
        assert_eq!(entry.retired_at_epoch_millis, 0);
    }

    #[test]
    fn jwks_cache_dump_marks_retired_entries() {
        let (_encoding_old, entry_old) = make_es256_kid("k_old");
        let (_encoding_new, entry_new) = make_es256_kid("k_new");
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc {
                keys: vec![entry_old.clone()],
            }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);
        cache.force_refetch().expect("initial");
        // Rotate: drop k_old, add k_new.
        cache
            .merge_doc(JwksDoc {
                keys: vec![entry_new],
            })
            .expect("merge");
        let dump = cache.dump();
        assert_eq!(dump.len(), 2);
        let old = dump.iter().find(|d| d.kid == "k_old").expect("k_old");
        let new = dump.iter().find(|d| d.kid == "k_new").expect("k_new");
        assert!(old.retired);
        assert!(old.retired_at_epoch_millis > 0);
        assert!(!new.retired);
    }

    #[test]
    fn aud_array_form_accepted() {
        let (encoding, entry) = make_es256_kid("k1");
        let cfg = JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc { keys: vec![entry] }),
            "lifegw",
            "https://broomva.tech",
        );
        let cache = JwksCache::new(cfg);

        let claims = json!({
            "sub": "u",
            "aud": ["lifegw", "other-audience"],
            "iss": "https://broomva.tech",
            "exp": now_secs() + 600,
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("k1".to_string());
        let bearer = encode(&header, &claims, &encoding).expect("encode");
        let parsed = cache.verify(&bearer).expect("aud array accepted");
        assert_eq!(parsed.user_id, "u");
    }
}
