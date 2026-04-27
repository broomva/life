//! Tier-1 identity-token claim shape (Spec C₃ §5.2).
//!
//! Sub-phase A's `dev_signer` synthesises these claims from a magic Bearer
//! string. Sub-phase B's real ES256+JWKS verifier produces the same shape so
//! the rest of the gateway is unchanged.

use serde::{Deserialize, Serialize};

/// Subset of Tier-1 claims the gateway propagates downstream into Tier-2.
///
/// Required claims per Spec C₃ §5.2: `iss`, `aud`, `sub`, `exp`.
/// Sub-phase B will additionally validate `kid` (header), `nbf`, and `alg`.
/// Sub-phase A only carries the fields the Tier-2 minter needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Tier1Claims {
    /// Subject — canonical user_id.
    pub user_id: String,
    /// Active project id. Falls back to `default-project` when the dev
    /// signer is in use; Sub-phase B reads from the `project_id` claim or
    /// the `X-Life-Project-Id` header.
    pub project_id: String,
    /// Identity-scoped permissions intersected with the route's required
    /// scope (Sub-phase B). Sub-phase A returns a single broad `agent:dispatch`
    /// scope so the proxy can ship something usable.
    pub scopes: Vec<String>,
}
