//! Per-request context attached to every GraphQL invocation.
//!
//! Built once per HTTP request (or once per WS subscription session)
//! and carried via `async_graphql::Context::data::<WarpRequestCtx>()`
//! into every resolver.

use std::sync::Arc;

use crate::warp_graphql::auth::WarpClaims;
use crate::warp_graphql::dataloader::LifedDataloaders;

#[derive(Clone)]
pub struct WarpRequestCtx {
    pub claims: WarpClaims,
    pub dataloaders: Arc<LifedDataloaders>,
    /// Trace span id for cross-service correlation in vigil.
    pub trace_id: String,
}

impl WarpRequestCtx {
    pub fn new(
        claims: WarpClaims,
        dataloaders: Arc<LifedDataloaders>,
        trace_id: String,
    ) -> Self {
        Self {
            claims,
            dataloaders,
            trace_id,
        }
    }

    /// Convenience: returns the user_id of the authenticated caller.
    pub fn user_id(&self) -> &str {
        &self.claims.user_id
    }

    /// Returns `Err(Unauthenticated)` if the caller lacks the named scope.
    pub fn require_scope(
        &self,
        scope: &str,
    ) -> Result<(), crate::warp_graphql::error::WarpGraphqlError> {
        if self.claims.has_scope(scope) {
            Ok(())
        } else {
            Err(crate::warp_graphql::error::WarpGraphqlError::Unauthenticated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    #[test]
    fn user_id_returns_sub() {
        let ctx = ctx_with_scopes(vec!["agent:read"]);
        assert_eq!(ctx.user_id(), "u1");
    }

    #[test]
    fn require_scope_passes_when_present() {
        let ctx = ctx_with_scopes(vec!["agent:read", "agent:write"]);
        assert!(ctx.require_scope("agent:read").is_ok());
    }

    #[test]
    fn require_scope_fails_when_absent() {
        let ctx = ctx_with_scopes(vec!["agent:read"]);
        assert!(ctx.require_scope("agent:write").is_err());
    }
}
