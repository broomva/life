//! Bearer JWT extraction for warp_graphql resolvers.
//!
//! Reuses lifegw's existing `JwksCache` (Spec C₃ M7-D shipped). The
//! request flows: Warp client sends `Authorization: Bearer <jwt>` →
//! axum extractor pulls the header → [`JwksCache::verify_capability_token`]
//! validates signature + audience + scopes → typed [`WarpClaims`] is
//! attached to the GraphQL `Request` data.
//!
//! ## Adaptation note (Task 6)
//!
//! The shipped [`JwksCache::verify_capability_token`] is **synchronous**
//! (the only async hop is `force_refetch()` for cache misses; the
//! verifier function itself blocks via internal `block_in_place` /
//! side-thread strategies in [`crate::auth::jwks::fetch_via_reqwest`]).
//! We expose [`verify_to_claims`] as `async fn` anyway so future
//! migrations to a truly-async verifier don't ripple through callers.
//!
//! The shipped verifier also takes an explicit `expected_issuer`
//! parameter (not just an audience). We accept both here — the route
//! handler wires `audience_required = "lumen"` + `expected_issuer =
//! "lifegw"` from `AuthConfig` per Spec C₃ §5.4.
//!
//! The shipped [`crate::auth::jwks::VerifiedCapClaims`] does NOT carry
//! `anima_did` (Spec D D-Sub-E added the wire claim on lifegw's
//! `/anima/custody/*` routes but the cap-token verifier hasn't been
//! extended to project the claim out yet). We default `anima_did =
//! None` here; surfacing the bound DID through `WarpClaims` is a
//! follow-up tracked under the M7-W milestone. Similarly `expires_at`
//! is not exposed by the verifier (it enforces `exp` internally via
//! `jsonwebtoken::Validation` and returns only the audience/sub/scope
//! tuple); we default `expires_at = u64::MAX` for now and let the
//! verifier own expiry enforcement.

use crate::warp_graphql::error::WarpGraphqlError;

/// Verified caller claims, attached to every GraphQL request that
/// passed the auth gate. Tier-3 stubs that don't need claims still
/// receive them (extractor runs before resolver dispatch).
#[derive(Debug, Clone)]
pub struct WarpClaims {
    /// `sub` claim — the broomva.tech-issued user_id.
    pub user_id: String,
    /// `aud` claim — must equal "lumen" for warp_graphql route.
    pub audience: String,
    /// `scope` claim — capability scopes, e.g. `agent:read`, `agent:write`.
    pub scopes: Vec<String>,
    /// `anima_did` extension claim — DID bound by AnimaCustody.
    ///
    /// Currently `None` because the shipped
    /// [`crate::auth::jwks::VerifiedCapClaims`] does not project the
    /// `anima_did` JWT claim. Wiring this through is a follow-up.
    pub anima_did: Option<String>,
    /// `exp` claim — Unix timestamp.
    ///
    /// Currently set to `u64::MAX` because the shipped verifier enforces
    /// `exp` internally and does not surface the value. The token will
    /// have been rejected by `verify_capability_token` if expired.
    pub expires_at: u64,
}

impl WarpClaims {
    /// Anonymous fallback used by `createAnonymousUser` mutation.
    /// Carries an unsigned, ephemeral user_id; upgraded to a real
    /// claim on `mintCustomToken` success.
    pub fn anonymous(ephemeral_id: String) -> Self {
        Self {
            user_id: ephemeral_id,
            audience: "lumen".to_string(),
            scopes: vec!["agent:anonymous".to_string()],
            anima_did: None,
            expires_at: u64::MAX,
        }
    }

    /// Returns `true` if the caller has the named scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// Extract a bearer token from the `Authorization` header.
///
/// Returns `None` if the header is missing or malformed; the caller
/// decides whether to reject or fall through to anonymous.
pub fn extract_bearer(headers: &http::HeaderMap) -> Option<&str> {
    let header = headers.get(http::header::AUTHORIZATION)?;
    let value = header.to_str().ok()?;
    value.strip_prefix("Bearer ")
}

/// Verify a bearer token via the existing
/// [`JwksCache::verify_capability_token`] and produce typed
/// [`WarpClaims`].
///
/// `audience_required` is the `aud` value the token must carry; for the
/// Lumen route this is always `"lumen"`. Tokens minted for other
/// audiences (e.g. internal Tier-2 `lifed` capability tokens) are
/// rejected here.
///
/// `expected_issuer` is the `iss` value the verifier checks against;
/// for the Lumen route this is `"lifegw"` (matching `AuthConfig::tier2_issuer`).
///
/// The function is declared `async` for forward-compatibility with a
/// future truly-async verifier (today the underlying call is
/// synchronous; see the module-level note).
///
/// [`JwksCache::verify_capability_token`]: crate::auth::JwksCache::verify_capability_token
pub async fn verify_to_claims(
    jwks: &crate::auth::JwksCache,
    token: &str,
    audience_required: &str,
    expected_issuer: &str,
) -> Result<WarpClaims, WarpGraphqlError> {
    let allowed_audiences = [audience_required];
    let claims = jwks
        .verify_capability_token(token, &allowed_audiences, expected_issuer)
        .map_err(|_| WarpGraphqlError::Unauthenticated)?;

    Ok(WarpClaims {
        user_id: claims.sub,
        audience: claims.aud,
        scopes: claims.scope,
        anima_did: None,
        expires_at: u64::MAX,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{HeaderMap, HeaderValue};

    #[test]
    fn extract_bearer_pulls_token() {
        let mut h = HeaderMap::new();
        h.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc.def.ghi"),
        );
        assert_eq!(extract_bearer(&h), Some("abc.def.ghi"));
    }

    #[test]
    fn extract_bearer_rejects_missing_prefix() {
        let mut h = HeaderMap::new();
        h.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("abc.def.ghi"),
        );
        assert_eq!(extract_bearer(&h), None);
    }

    #[test]
    fn extract_bearer_returns_none_when_absent() {
        let h = HeaderMap::new();
        assert_eq!(extract_bearer(&h), None);
    }

    #[test]
    fn anonymous_claims_have_anonymous_scope() {
        let c = WarpClaims::anonymous("eph-1234".into());
        assert_eq!(c.user_id, "eph-1234");
        assert!(c.has_scope("agent:anonymous"));
        assert!(!c.has_scope("agent:write"));
    }

    #[test]
    fn has_scope_checks_membership() {
        let c = WarpClaims {
            user_id: "u1".into(),
            audience: "lumen".into(),
            scopes: vec!["agent:read".into(), "agent:write".into()],
            anima_did: None,
            expires_at: 9999999999,
        };
        assert!(c.has_scope("agent:read"));
        assert!(c.has_scope("agent:write"));
        assert!(!c.has_scope("admin"));
    }
}
