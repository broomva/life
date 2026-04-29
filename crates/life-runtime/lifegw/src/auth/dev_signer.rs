//! Tier-1 bearer verification entry point.
//!
//! Sub-phase A historical note: this module's body originally accepted a
//! magic `Bearer dev-token-for-{user_id}` shortcut and synthesised
//! [`Tier1Claims`]. Sub-phase B replaces the body with a real
//! Vercel-style JWKS ES256/RS256 verifier delegated to
//! [`crate::auth::jwks::JwksCache`]. The function name + signature are
//! preserved so the auth `Layer` above doesn't change — see
//! `auth/middleware.rs::AuthService::call`.
//!
//! ## Wiring (Sub-phase D, D7)
//!
//! As of Sub-phase D the per-`AuthService<S>` middleware carries an
//! explicit `Arc<JwksCache>` handle, threaded through
//! `AuthLayer::with_jwks(cache)`. The legacy process-global
//! `TIER1_VERIFIER` `OnceLock` survives behind a deprecated shim so
//! existing tests that called `install_tier1_verifier` keep passing,
//! but the production hot path (the Tower middleware in
//! [`crate::auth::middleware`]) reads the JWKS via the explicit
//! per-service `Arc` first and only falls back to the global as a
//! transition convenience. The global will be removed in Sub-phase E.
//!
//! Threading the JWKS explicitly through `AuthService` unblocks
//! per-test verifier swaps: previously, a single test process could
//! install only one verifier (OnceLock semantics — first set wins),
//! so the Sub-phase B integration test `integration_jwks_round_trip`
//! had to bundle three sub-suites into a single `#[tokio::test]`
//! function. With D7 each `AuthLayer::with_jwks(...)` instance owns
//! its cache, so the sub-suites can run as separate `#[tokio::test]`
//! functions inside the same test binary.
//!
//! When the cache is constructed via [`JwksCache::dev_only`], the
//! `Bearer dev-token-for-{user_id}` shortcut still works — preserving
//! the existing integration-test contract.

use std::sync::Arc;
use std::sync::OnceLock;

use crate::auth::jwks::JwksCache;
use crate::auth::tier1::Tier1Claims;
use crate::error::{LifegwError, LifegwResult};

/// Process-global Tier-1 verifier. Set by `bootstrap` (or by a test
/// helper) before any RPC is served. Reads are lock-free after the
/// initial set.
///
/// **Deprecated as of Sub-phase D (D7):** the production hot path now
/// uses the explicit per-`AuthService<S>` `Arc<JwksCache>` handle
/// installed via `AuthLayer::with_jwks(cache)`. This global remains as
/// a transition convenience so the legacy `dev_signer::verify(bearer)`
/// entry-point keeps working for tests + the Sub-phase B initial
/// wiring path. It will be removed in Sub-phase E.
static TIER1_VERIFIER: OnceLock<Arc<JwksCache>> = OnceLock::new();

/// Install the Tier-1 verifier. Idempotent: subsequent calls after a
/// successful set are silent no-ops (we don't reset auth wiring during
/// daemon runtime — restart for that).
///
/// **Deprecated as of Sub-phase D (D7).** Production callers should
/// instead pass `Arc<JwksCache>` to `AuthLayer::with_jwks(cache)`,
/// which threads the cache through to the middleware without touching
/// process-global state. This shim remains for tests that exercise the
/// legacy `dev_signer::verify(bearer)` entry-point.
#[deprecated(
    since = "0.3.0",
    note = "Use `AuthLayer::with_jwks(cache)` instead — the global \
            `OnceLock<JwksCache>` handle is being removed in Sub-phase E. \
            Existing test wiring continues to work via this shim."
)]
pub fn install_tier1_verifier(cache: Arc<JwksCache>) {
    let _ = TIER1_VERIFIER.set(cache);
}

// OnceLock provides no public reset; we accept that the FIRST
// installer wins per process. Tests that need a different verifier
// instantiate `JwksCache` directly and exercise the verifier through
// the `JwksCache::verify` API rather than the `dev_signer::verify`
// global entry-point. Each integration test runs in its own process
// (cargo test default), so global state is naturally isolated.

/// Read the process-global Tier-1 verifier (if installed). Returns
/// `None` when no verifier was installed via the deprecated
/// `install_tier1_verifier` shim — the middleware fast-path uses the
/// explicit per-service handle in that case.
pub(crate) fn global_verifier() -> Option<Arc<JwksCache>> {
    TIER1_VERIFIER.get().cloned()
}

/// Verify a Tier-1 bearer token. Returns synthesised Tier-1 claims on
/// success; `LifegwError::Auth` otherwise.
///
/// **Deprecated as of Sub-phase D (D7).** Production callers should
/// invoke `cache.verify(bearer)` directly via the per-`AuthService<S>`
/// handle. This shim reads the legacy process-global so existing tests
/// and the documented Sub-phase B startup flow keep working.
///
/// # Panics
/// Never. Returns `LifegwError::Auth` if no verifier was installed.
#[deprecated(
    since = "0.3.0",
    note = "Use `cache.verify(bearer)` against the explicit \
            `Arc<JwksCache>` carried by `AuthLayer::with_jwks(cache)`. \
            The global verifier is being removed in Sub-phase E."
)]
pub fn verify(bearer: &str) -> LifegwResult<Tier1Claims> {
    match TIER1_VERIFIER.get() {
        Some(cache) => cache.verify(bearer),
        None => Err(LifegwError::Auth(
            "tier-1 verifier not installed (bootstrap must call install_tier1_verifier)"
                .to_string(),
        )),
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::auth::jwks::{JwksCache, JwksCacheConfig, JwksDoc, JwksSource};

    /// One-shot static fixture so successive `#[test]` cases share the
    /// same global verifier (OnceLock semantics — first set wins). All
    /// tests in this module use the same dev-only verifier.
    fn ensure_dev_verifier() {
        static FIXTURE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        FIXTURE.get_or_init(|| {
            let cache = Arc::new(JwksCache::dev_only());
            install_tier1_verifier(cache);
        });
    }

    #[test]
    fn dev_verifier_accepts_well_formed_bearer() {
        ensure_dev_verifier();
        let claims = verify("dev-token-for-user-1").expect("accept dev token");
        assert_eq!(claims.user_id, "user-1");
        assert_eq!(claims.project_id, "default-project");
        assert_eq!(claims.scopes, vec!["agent:dispatch".to_string()]);
        // Sub-phase C: dev verifier defaults `tier` to `free` so the
        // rate-limit tests can rely on a non-empty value.
        assert_eq!(claims.tier, crate::auth::tier1::DEFAULT_TIER);
    }

    #[test]
    fn dev_verifier_rejects_non_dev_bearer() {
        ensure_dev_verifier();
        assert!(matches!(
            verify("eyJhbGciOiJFUzI1NiJ9..."),
            Err(LifegwError::Auth(_))
        ));
        assert!(matches!(verify(""), Err(LifegwError::Auth(_))));
    }

    #[test]
    fn dev_verifier_rejects_empty_user_id() {
        ensure_dev_verifier();
        assert!(matches!(
            verify("dev-token-for-"),
            Err(LifegwError::Auth(_))
        ));
    }

    #[test]
    fn missing_verifier_returns_auth_error() {
        // Use a fresh cache config that's not the dev path — but we
        // can't reset OnceLock, so instead we test JwksCache::verify
        // directly without installing globally.
        let cache = JwksCache::new(JwksCacheConfig::new(
            JwksSource::Inline(JwksDoc::default()),
            "lifegw",
            "https://broomva.tech",
        ));
        // No kid, no header → fails on header decode.
        assert!(matches!(
            cache.verify("not-a-jwt"),
            Err(LifegwError::Auth(_))
        ));
    }
}
