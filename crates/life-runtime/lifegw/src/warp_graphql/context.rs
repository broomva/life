//! Per-request context attached to every GraphQL invocation.
//!
//! Built once per HTTP request (or once per WS subscription session)
//! and carried via `async_graphql::Context::data::<WarpRequestCtx>()`
//! into every resolver.

use std::sync::Arc;

use crate::auth::tier_user::TierUserMinter;
use crate::warp_graphql::auth::WarpClaims;
use crate::warp_graphql::dataloader::LifedDataloaders;
use crate::warp_graphql::error::WarpGraphqlError;

/// Per-request state carried via async-graphql's `Context::data` into every
/// resolver. Built once per HTTP request (or once per WS subscription
/// session). Cheap to clone — `WarpClaims` is `Clone`, `Arc<LifedDataloaders>`
/// and `Arc<TierUserMinter>` are refcount bumps, `String` is a single heap
/// clone.
#[derive(Clone)]
pub struct WarpRequestCtx {
    pub claims: WarpClaims,
    pub dataloaders: Arc<LifedDataloaders>,
    /// Trace span id for cross-service correlation in vigil.
    pub trace_id: String,
    /// Tier-User minter shared with `/anima/custody/*` (Spec D D-Sub-C).
    ///
    /// Task 15 (M7-W) uses this to mint short-lived ES256 capability JWS
    /// for the four T1 token resolvers (`mintCustomToken`,
    /// `issueTaskIdentityToken`, `generateApiKey`, `expireApiKey`).
    /// Phase α skips the spec's broomva.tech OAuth-PKCE round-trip and
    /// mints caps directly from the bearer-validated user_id already
    /// present in [`WarpClaims`]. The full OAuth flow is β-only.
    ///
    /// Bootstrap populates this with the same `Arc<TierUserMinter>` the
    /// `/anima/custody/*` routes use — single shared signing key (Spec D
    /// L4-D6) so verifiers see a uniform JWKS across the gateway.
    pub minter: Arc<TierUserMinter>,
}

impl WarpRequestCtx {
    /// Construct a request context. Called from `route.rs` (Task 11) after
    /// the auth extractor produces `WarpClaims`.
    pub fn new(
        claims: WarpClaims,
        dataloaders: Arc<LifedDataloaders>,
        trace_id: String,
        minter: Arc<TierUserMinter>,
    ) -> Self {
        Self {
            claims,
            dataloaders,
            trace_id,
            minter,
        }
    }

    /// Convenience: returns the user_id of the authenticated caller.
    pub fn user_id(&self) -> &str {
        &self.claims.user_id
    }

    /// Returns `Err(Unauthenticated)` if the caller lacks the named scope.
    pub fn require_scope(&self, scope: &str) -> Result<(), WarpGraphqlError> {
        if self.claims.has_scope(scope) {
            Ok(())
        } else {
            Err(WarpGraphqlError::Unauthenticated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::kms::{KmsSigner, StaticKeystore};
    use crate::auth::tier_user::DEFAULT_TIER_USER_TTL;

    /// Build a `TierUserMinter` over an ephemeral `StaticKeystore` for
    /// tests. Mirrors `tier_user::tests::dev_minter` so resolver tests
    /// can construct a request ctx without booting the full lifegw
    /// bootstrap pipeline.
    pub(crate) fn test_minter() -> Arc<TierUserMinter> {
        let signer = Arc::new(StaticKeystore::generate_dev().expect("dev keystore"));
        Arc::new(TierUserMinter::with_defaults(
            signer as Arc<dyn KmsSigner>,
            DEFAULT_TIER_USER_TTL,
        ))
    }

    fn ctx_with_scopes(scopes: Vec<&str>) -> WarpRequestCtx {
        WarpRequestCtx {
            claims: WarpClaims {
                user_id: "u1".into(),
                audience: "lumen".into(),
                scopes: scopes.into_iter().map(str::to_string).collect(),
                anima_did: None,
                expires_at: 9999999999,
            },
            dataloaders: Arc::new(LifedDataloaders::test_stub()),
            trace_id: "trace-1".into(),
            minter: test_minter(),
        }
    }

    // `ctx_with_scopes` constructs a `LifedDataloaders::test_stub()` which
    // uses `Channel::from_static(...).connect_lazy()`. That call registers
    // a `TokioTimer` and therefore needs an active runtime — so every
    // test here is `#[tokio::test]` (post Task 8: real tonic clients
    // replaced the empty stub).
    #[tokio::test]
    async fn user_id_returns_sub() {
        let ctx = ctx_with_scopes(vec!["agent:read"]);
        assert_eq!(ctx.user_id(), "u1");
    }

    #[tokio::test]
    async fn require_scope_passes_when_present() {
        let ctx = ctx_with_scopes(vec!["agent:read", "agent:write"]);
        assert!(ctx.require_scope("agent:read").is_ok());
    }

    #[tokio::test]
    async fn require_scope_fails_when_absent() {
        let ctx = ctx_with_scopes(vec!["agent:read"]);
        assert!(ctx.require_scope("agent:write").is_err());
    }
}
