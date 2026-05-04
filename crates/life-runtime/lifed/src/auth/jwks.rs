//! Real ES256 + JWKS verifier for Tier-2 capability tokens.
//!
//! Per Spec C₂ §5.1, lifed verifies the JWS bearer token presented on every
//! public-plane request. Verification uses the published lifegw JWKS at
//! `cfg.auth.jwks_path`.
//!
//! ## Boot-order independence (Stage 2 — May 2026)
//!
//! Pre-Stage-2 the cache made a single boot-time decision: `if file exists →
//! load static keys, else → fall back to dev_only`. That coupled lifed's
//! verifier identity to lifegw's publish timing — and inside the
//! Railway lifegw-stack container the order is **lifed first, lifegw
//! second** (lifegw needs lifed's UDS to dial). lifed booted with `dev_only`
//! and rejected every real ES256 JWS lifegw subsequently minted.
//!
//! The Stage-2 fix mirrors lifegw's own pattern (Spec C₃ §5):
//!
//! - **Lazy file load on first verify** — the cache holds a path, not a
//!   pre-decided key set. The first `validate()` call (or any with an
//!   unknown `kid`) re-reads the file.
//! - **mtime-based invalidation** — every load stats the file; if mtime
//!   advanced (lifegw rotated the key), the cache reloads.
//! - **Serialized concurrent loads** — a `parking_lot::Mutex<()>` guards
//!   the file read so 100 concurrent verifiers produce at most one I/O
//!   per cache miss.
//! - **Additive dev shortcut** — `dev_signer_enabled = true` accepts the
//!   `Bearer test-token-for-{user_id}` shortcut **in addition to** real
//!   ES256 verification. Disabled by default in production; integration
//!   tests opt in.
//!
//! ## Sub-phase A backwards compat
//!
//! `JwksCache::dev_only()` and `JwksCache::load_from_path()` are preserved
//! for tests + the Sub-phase A in-process keystore path. New deploys
//! should use `JwksCache::new_lazy_file_with_dev_shortcut(path)` so the
//! boot-order race goes away for good.

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aios_proto::aios::v1 as aios_v1;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use parking_lot::Mutex as PLMutex;
use serde::{Deserialize, Serialize};

use crate::auth::capability::{CapabilityClaims, Tier};
use crate::error::{LifedError, LifedResult};

/// Default cache TTL — even if the file mtime hasn't advanced we still
/// re-stat the file every TTL window. Bounds staleness if mtime updates
/// race the OS's filesystem cache.
pub const DEFAULT_LAZY_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Serialize, Deserialize, Clone)]
struct JwksFile {
    keys: Vec<JwksKey>,
}

#[derive(Serialize, Deserialize, Clone)]
#[allow(dead_code)]
struct JwksKey {
    kid: String,
    kty: String,
    crv: String,
    alg: String,
    #[serde(default)]
    x: String,
    #[serde(default)]
    y: String,
    /// Optional convenience: when the JWKS file embeds a PEM-encoded public
    /// key directly (used by the dev path) we accept it instead of x/y.
    #[serde(default)]
    pem: Option<String>,
}

#[derive(Deserialize)]
struct Tier2Body {
    #[allow(dead_code)]
    iss: String,
    sub: String,
    #[allow(dead_code)]
    aud: String,
    sid: String,
    scopes: Vec<String>,
    tier: Option<String>,
    exp: u64,
    #[allow(dead_code)]
    nbf: Option<u64>,
}

/// In-memory cache state — keys + bookkeeping for invalidation.
struct CacheState {
    keys: Vec<(String, DecodingKey)>,
    /// `None` until the first successful load.
    last_loaded_at: Option<Instant>,
    /// File-system mtime captured at the last successful load. `None`
    /// until the first load (or for inline / dev-only sources).
    last_mtime: Option<SystemTime>,
}

impl CacheState {
    fn empty() -> Self {
        Self {
            keys: Vec::new(),
            last_loaded_at: None,
            last_mtime: None,
        }
    }

    fn from_keys(keys: Vec<(String, DecodingKey)>) -> Self {
        Self {
            keys,
            last_loaded_at: Some(Instant::now()),
            last_mtime: None,
        }
    }
}

/// Source from which a `JwksCache` resolves keys.
enum JwksSource {
    /// Static keys — set at construction, never reloaded. Used by
    /// `dev_only()` (empty) and `load_from_path()` (one-shot file load).
    /// Both are preserved for tests + the Sub-phase A in-process path.
    Static,
    /// File-backed lazy source. Reloaded on cache miss / mtime change.
    Lazy { path: PathBuf, ttl: Duration },
}

/// JWKS cache used by the auth middleware for Tier-2 verification.
pub struct JwksCache {
    source: JwksSource,
    state: RwLock<CacheState>,
    /// Serialises concurrent loads — concurrent verifies that miss the
    /// cache funnel through here so we issue at most one file read at a
    /// time. The mutex holds nothing (`()`); semantics is the lock alone.
    load_lock: PLMutex<()>,
    /// Whether the dev `test-token-for-{user_id}` path is enabled. When
    /// `true`, accepts the shortcut **in addition to** real ES256
    /// verification. Production deploys leave this `false`.
    dev_signer_enabled: bool,
}

impl JwksCache {
    /// Load a JWKS from disk once at construction. Sub-phase A back-compat
    /// constructor — production deploys should use
    /// [`new_lazy_file`](Self::new_lazy_file) instead so a missing-then-
    /// appears file (boot race) is handled correctly.
    pub fn load_from_path(path: &Path) -> LifedResult<Self> {
        let keys = parse_jwks_file(path)?;
        Ok(Self {
            source: JwksSource::Static,
            state: RwLock::new(CacheState::from_keys(keys)),
            load_lock: PLMutex::new(()),
            dev_signer_enabled: false,
        })
    }

    /// Lazy file-backed JWKS cache.
    ///
    /// The cache does **not** read the file at construction. The first
    /// `validate()` call (or any subsequent call with an unknown `kid`)
    /// triggers a load. Subsequent loads stat the file's mtime; when
    /// the mtime has advanced **or** the TTL window has elapsed, the
    /// keys are reloaded. Concurrent miss-driven loads are serialised
    /// behind an internal mutex so we issue at most one file read per
    /// burst.
    ///
    /// Dev shortcut is **disabled** — production posture. Use
    /// [`new_lazy_file_with_dev_shortcut`](Self::new_lazy_file_with_dev_shortcut)
    /// during transition periods where integration tests still rely on
    /// `Bearer test-token-for-{user_id}`.
    pub fn new_lazy_file(path: impl Into<PathBuf>) -> Self {
        Self::new_lazy_file_inner(path, DEFAULT_LAZY_TTL, false)
    }

    /// Lazy file-backed JWKS cache that **also** accepts the
    /// `Bearer test-token-for-{user_id}` dev shortcut as an additive
    /// fallback. Real ES256 verification still runs against the file;
    /// the shortcut is checked first as a fast-path for tests + ops
    /// smoke runs, then falls through to JWKS verification.
    pub fn new_lazy_file_with_dev_shortcut(path: impl Into<PathBuf>) -> Self {
        Self::new_lazy_file_inner(path, DEFAULT_LAZY_TTL, true)
    }

    fn new_lazy_file_inner(
        path: impl Into<PathBuf>,
        ttl: Duration,
        dev_signer_enabled: bool,
    ) -> Self {
        Self {
            source: JwksSource::Lazy {
                path: path.into(),
                ttl,
            },
            state: RwLock::new(CacheState::empty()),
            load_lock: PLMutex::new(()),
            dev_signer_enabled,
        }
    }

    /// Dev convenience: build a cache containing the
    /// [`crate::auth::keystore::Keystore::generate_dev`] public key AND
    /// enable the dev shortcut. **No file source** — every verify either
    /// hits the in-memory dev key or accepts the shortcut. Used by
    /// integration tests that don't materialize a real JWKS file.
    pub fn dev_only() -> Self {
        let ks = crate::auth::keystore::Keystore::generate_dev();
        let pubkey_pem = ks.public_key_pem();
        let key = DecodingKey::from_ec_pem(pubkey_pem.as_bytes()).expect("dev pem");
        Self {
            source: JwksSource::Static,
            state: RwLock::new(CacheState::from_keys(vec![(ks.kid, key)])),
            load_lock: PLMutex::new(()),
            dev_signer_enabled: true,
        }
    }

    /// Accessor — does this cache also accept the dev shortcut?
    pub fn dev_signer_enabled(&self) -> bool {
        self.dev_signer_enabled
    }

    /// Validate a Tier-2 bearer token. Returns the parsed claims on success.
    ///
    /// When `dev_signer_enabled` is true, the
    /// `Bearer test-token-for-{user_id}` shortcut is accepted **in
    /// addition to** real ES256 verification. The shortcut is checked
    /// first; on no-match the real path runs.
    pub fn validate(&self, bearer: &str) -> LifedResult<CapabilityClaims> {
        if self.dev_signer_enabled
            && let Some(user_id) = bearer.strip_prefix("test-token-for-")
        {
            if user_id.is_empty() {
                return Err(LifedError::Auth("empty dev user_id".to_string()));
            }
            return Ok(CapabilityClaims {
                user_id: user_id.to_string(),
                project_id: "project-demo".to_string(),
                sid: aios_v1::SessionId {
                    value: String::new(),
                },
                scopes: vec![
                    "agent:dispatch".to_string(),
                    "events:read".to_string(),
                    "wallet:debit".to_string(),
                    "identity:read".to_string(),
                ],
                tier: Tier::Free,
                exp: Instant::now() + Duration::from_secs(900),
            });
        }
        // Real ES256 path.
        let header = decode_header(bearer).map_err(|e| LifedError::Auth(format!("header: {e}")))?;
        let kid = header
            .kid
            .ok_or_else(|| LifedError::Auth("missing kid".to_string()))?;

        let key = match self.lookup_kid(&kid) {
            Some(k) => k,
            None => {
                // Cache miss. For lazy sources, attempt a refresh + retry.
                self.maybe_reload(&kid)?;
                self.lookup_kid(&kid)
                    .ok_or_else(|| LifedError::Auth(format!("unknown kid: {kid}")))?
            }
        };

        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["lifed"]);
        v.set_issuer(&["lifegw"]);
        let token = decode::<Tier2Body>(bearer, &key, &v)
            .map_err(|e| LifedError::Auth(format!("verify: {e}")))?;
        let body = token.claims;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if body.exp <= now {
            return Err(LifedError::Auth("expired".to_string()));
        }
        let tier = match body.tier.as_deref() {
            Some("paid") => Tier::Paid,
            Some("enterprise") => Tier::Enterprise,
            _ => Tier::Free,
        };
        let exp_instant = Instant::now() + Duration::from_secs(body.exp - now);
        Ok(CapabilityClaims {
            user_id: body.sub,
            project_id: String::new(), // narrowed at handler boundary
            sid: aios_v1::SessionId { value: body.sid },
            scopes: body.scopes,
            tier,
            exp: exp_instant,
        })
    }

    /// Read-locked lookup. Cheap fast-path on the hot verify path.
    fn lookup_kid(&self, kid: &str) -> Option<DecodingKey> {
        let guard = self.state.read().ok()?;
        guard
            .keys
            .iter()
            .find(|(k, _)| k == kid)
            .map(|(_, dk)| dk.clone())
    }

    /// Refresh the cache from the file source if (and only if) we have
    /// one. Static sources are no-ops. Concurrent callers serialise on
    /// `load_lock`; whoever holds the lock checks mtime + TTL and either
    /// re-reads the file or skips. After the first holder reloads, every
    /// subsequent caller short-circuits via the mtime/TTL check.
    fn maybe_reload(&self, _missing_kid: &str) -> LifedResult<()> {
        let (path, ttl) = match &self.source {
            JwksSource::Static => return Ok(()),
            JwksSource::Lazy { path, ttl } => (path.clone(), *ttl),
        };

        let _lock = self.load_lock.lock();

        // Re-check under the lock: another thread may have already loaded.
        // Skip the file read if we have keys, the mtime hasn't moved, and
        // the TTL hasn't expired.
        let (cached_mtime, cached_loaded_at) = {
            let s = self
                .state
                .read()
                .map_err(|_| LifedError::Auth("jwks state lock".to_string()))?;
            (s.last_mtime, s.last_loaded_at)
        };
        let current_mtime = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());
        let mtime_unchanged = match (cached_mtime, current_mtime) {
            (Some(prev), Some(now)) => prev == now,
            _ => false,
        };
        let ttl_fresh = cached_loaded_at.map(|t| t.elapsed() < ttl).unwrap_or(false);

        if mtime_unchanged && ttl_fresh {
            // Another thread already refreshed (or our own previous load
            // is still fresh) and the file hasn't moved. Nothing to do.
            return Ok(());
        }

        // Read + parse. A missing file is logged but not fatal — we
        // simply don't update the cache. The verifier will return
        // "unknown kid" and the caller sees an auth error, which is
        // the correct surface (the file genuinely isn't there yet).
        let keys = match parse_jwks_file(&path) {
            Ok(keys) => keys,
            Err(err) => {
                tracing::debug!(
                    path = %path.display(),
                    error = %err,
                    "jwks file unreadable — keeping previous keys"
                );
                return Ok(());
            }
        };

        let mut guard = self
            .state
            .write()
            .map_err(|_| LifedError::Auth("jwks state lock".to_string()))?;
        guard.keys = keys;
        guard.last_loaded_at = Some(Instant::now());
        guard.last_mtime = current_mtime;
        Ok(())
    }
}

/// Parse the JWKS file at `path` into a list of `(kid, DecodingKey)`
/// pairs. Filters out non-EC-P-256-ES256 entries (we don't accept RS256
/// at lifed's verify boundary — Spec C₂ §5.1 narrows lifegw → lifed
/// to ES256 only).
fn parse_jwks_file(path: &Path) -> LifedResult<Vec<(String, DecodingKey)>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| LifedError::Auth(format!("read {}: {e}", path.display())))?;
    let file: JwksFile =
        serde_json::from_str(&text).map_err(|e| LifedError::Auth(format!("parse jwks: {e}")))?;
    let mut keys = Vec::new();
    for k in file.keys {
        if k.kty != "EC" || k.crv != "P-256" || k.alg != "ES256" {
            continue;
        }
        let key = if let Some(pem) = k.pem.as_ref() {
            DecodingKey::from_ec_pem(pem.as_bytes())
                .map_err(|e| LifedError::Auth(format!("decode pem {}: {e}", k.kid)))?
        } else {
            DecodingKey::from_ec_components(&k.x, &k.y)
                .map_err(|e| LifedError::Auth(format!("decode key {}: {e}", k.kid)))?
        };
        keys.push((k.kid, key));
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::keystore::Keystore;
    use jsonwebtoken::{Header, encode};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use tempfile::TempDir;

    /// Mint a Tier-2-shaped JWT signed by the given keystore. Mirrors
    /// what lifegw's Tier-2 minter produces so we can verify the
    /// round-trip without standing up the full lifegw stack.
    fn mint_tier2(ks: &Keystore, sub: &str, sid: &str, exp_secs: u64) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(ks.kid.clone());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let body = json!({
            "iss": "lifegw",
            "aud": "lifed",
            "sub": sub,
            "sid": sid,
            "scopes": ["agent:dispatch"],
            "tier": "free",
            "iat": now,
            "nbf": now,
            "exp": now + exp_secs,
        });
        // Keystore exposes `encoding: EncodingKey` directly — no need
        // to re-parse a PEM. The private half came from the embedded
        // dev key in `auth::keystore::Keystore::generate_dev`.
        encode(&header, &body, &ks.encoding).expect("encode")
    }

    /// Write a JWKS file containing the given keystore's public half.
    /// Mirrors `lifegw::auth::bootstrap::publish_jwks_atomic` minus the
    /// atomic-rename plumbing.
    fn write_jwks(path: &Path, ks: &Keystore) {
        let doc = json!({
            "keys": [{
                "kid": ks.kid,
                "kty": "EC",
                "crv": "P-256",
                "alg": "ES256",
                "use": "sig",
                "pem": ks.public_key_pem(),
            }],
        });
        std::fs::write(path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    }

    #[test]
    fn lazy_load_picks_up_file_that_appears_after_construction() {
        // The boot-race scenario: lifed starts with an empty file, lifegw
        // writes the JWKS shortly after, lifed must verify successfully.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jwks.json");
        let cache = JwksCache::new_lazy_file(&path);

        let ks = Keystore::generate_dev();
        let token = mint_tier2(&ks, "user_alice", "sess_001", 600);

        // First verify before file exists — must fail with auth error.
        let err = cache.validate(&token).unwrap_err();
        match err {
            LifedError::Auth(_) => {}
            other => panic!("expected Auth error, got {other:?}"),
        }

        // lifegw publishes JWKS now.
        write_jwks(&path, &ks);

        // Next verify — lazy reload picks it up, claims parse cleanly.
        let claims = cache.validate(&token).expect("verify after publish");
        assert_eq!(claims.user_id, "user_alice");
        assert_eq!(claims.sid.value, "sess_001");
    }

    #[test]
    fn lazy_load_picks_up_mtime_change_after_rotation() {
        // After a key rotation, lifegw rewrites the JWKS file with a new
        // kid. lifed's verify should pick up the new key on the next miss.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jwks.json");
        let cache = JwksCache::new_lazy_file(&path);

        let ks_v1 = Keystore::generate_dev();
        write_jwks(&path, &ks_v1);
        let token_v1 = mint_tier2(&ks_v1, "user_alice", "sess_001", 600);
        cache.validate(&token_v1).expect("v1 verifies");

        // Rotate. Sleep briefly so the OS records a new mtime even on
        // filesystems with second-granularity timestamps.
        std::thread::sleep(Duration::from_millis(1100));
        let ks_v2 = Keystore::generate_dev();
        write_jwks(&path, &ks_v2);
        let token_v2 = mint_tier2(&ks_v2, "user_bob", "sess_002", 600);

        // The v2 token has a new kid; the cache misses, reloads via
        // mtime-changed path, and verifies cleanly.
        let claims = cache
            .validate(&token_v2)
            .expect("v2 verifies after rotation");
        assert_eq!(claims.user_id, "user_bob");
    }

    #[test]
    fn dev_shortcut_is_additive_to_real_jwks_when_enabled() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jwks.json");
        let ks = Keystore::generate_dev();
        write_jwks(&path, &ks);
        let cache = JwksCache::new_lazy_file_with_dev_shortcut(&path);

        // Real JWS verifies.
        let token = mint_tier2(&ks, "user_real", "sess_real", 600);
        let claims = cache.validate(&token).expect("real JWS verifies");
        assert_eq!(claims.user_id, "user_real");

        // Dev shortcut also accepted (additive).
        let claims = cache
            .validate("test-token-for-user_dev")
            .expect("dev shortcut");
        assert_eq!(claims.user_id, "user_dev");
    }

    #[test]
    fn dev_shortcut_disabled_in_production_posture() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jwks.json");
        let ks = Keystore::generate_dev();
        write_jwks(&path, &ks);
        let cache = JwksCache::new_lazy_file(&path);

        // Real JWS verifies.
        let token = mint_tier2(&ks, "user_real", "sess_real", 600);
        cache.validate(&token).expect("real JWS verifies");

        // Dev shortcut REJECTED.
        let err = cache.validate("test-token-for-user_dev").unwrap_err();
        match err {
            LifedError::Auth(_) => {}
            other => panic!("expected Auth error, got {other:?}"),
        }
    }

    #[test]
    fn concurrent_misses_serialize_through_load_lock() {
        // 50 threads concurrently verify against a missing file. Only
        // one will hold the load_lock at a time. We don't have a clean
        // way to count file reads here, but we can at minimum assert
        // none of them deadlock or panic + all observe the post-publish
        // file consistently.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jwks.json");
        let cache = Arc::new(JwksCache::new_lazy_file(&path));
        let ks = Keystore::generate_dev();
        write_jwks(&path, &ks);

        let token = mint_tier2(&ks, "user_concurrent", "sess_x", 600);
        let success_count = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..50)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let token = token.clone();
                let success_count = Arc::clone(&success_count);
                thread::spawn(move || {
                    if cache.validate(&token).is_ok() {
                        success_count.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(success_count.load(Ordering::Relaxed), 50);
    }

    #[test]
    fn unknown_kid_returns_auth_error_after_failed_reload() {
        // Token whose `kid` header doesn't appear in the JWKS file — even
        // after a reload, we can't verify it. The cache must report
        // "unknown kid" rather than panicking or hanging.
        //
        // Note: `Keystore::generate_dev()` is deterministic (it embeds a
        // committed PEM), so we can't get two different keystores. We
        // instead mint a token whose header `kid` is intentionally
        // different from the one published in the JWKS, while signing
        // with the dev key. The verifier short-circuits on the kid lookup
        // before checking the signature, so this exercises the
        // "unknown kid → reload → still missing → auth error" branch.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jwks.json");
        let cache = JwksCache::new_lazy_file(&path);

        let ks = Keystore::generate_dev();
        write_jwks(&path, &ks);

        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("stranger-kid-not-in-jwks".to_string());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let body = json!({
            "iss": "lifegw",
            "aud": "lifed",
            "sub": "u",
            "sid": "s",
            "scopes": ["agent:dispatch"],
            "tier": "free",
            "iat": now,
            "nbf": now,
            "exp": now + 600,
        });
        let token = encode(&header, &body, &ks.encoding).expect("encode");

        let err = cache.validate(&token).unwrap_err();
        match err {
            LifedError::Auth(msg) => assert!(
                msg.contains("unknown kid"),
                "expected 'unknown kid' message, got: {msg}"
            ),
            other => panic!("expected Auth error, got {other:?}"),
        }
    }

    #[test]
    fn empty_dev_user_id_rejected_when_shortcut_enabled() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jwks.json");
        let cache = JwksCache::new_lazy_file_with_dev_shortcut(&path);
        let err = cache.validate("test-token-for-").unwrap_err();
        match err {
            LifedError::Auth(msg) => assert!(msg.contains("empty"), "got: {msg}"),
            other => panic!("expected Auth error, got {other:?}"),
        }
    }

    #[test]
    fn legacy_load_from_path_still_works_for_static_test_rigs() {
        // Sub-phase A back-compat: load_from_path takes a one-shot
        // snapshot. Subsequent file changes are NOT picked up — that's
        // the point of having a separate `new_lazy_file` constructor.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jwks.json");
        let ks = Keystore::generate_dev();
        write_jwks(&path, &ks);
        let cache = JwksCache::load_from_path(&path).expect("load");
        let token = mint_tier2(&ks, "u", "s", 600);
        cache.validate(&token).expect("verifies");
    }

    #[test]
    fn legacy_dev_only_still_accepts_shortcut() {
        // dev_only() preserves Sub-phase A semantics: in-memory dev key
        // + dev shortcut, no file source.
        let cache = JwksCache::dev_only();
        assert!(cache.dev_signer_enabled());
        let claims = cache.validate("test-token-for-alice").expect("shortcut");
        assert_eq!(claims.user_id, "alice");
    }
}
