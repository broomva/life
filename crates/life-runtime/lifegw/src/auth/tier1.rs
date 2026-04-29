//! Tier-1 identity-token claim shape (Spec C₃ §5.2).
//!
//! Sub-phase A's `dev_signer` synthesises these claims from a magic Bearer
//! string. Sub-phase B's real ES256+JWKS verifier produces the same shape so
//! the rest of the gateway is unchanged.

use serde::{Deserialize, Serialize};

/// Subset of Tier-1 claims the gateway propagates downstream into Tier-2.
///
/// Required claims per Spec C₃ §5.2: `iss`, `aud`, `sub`, `exp`.
/// Sub-phase B additionally validates `kid` (header), `nbf`, and `alg`.
///
/// Sub-phase C adds `tier` (BRO-938 follow-up #2): the rate limiter
/// (Sub-phase D) needs the user's plan tier to apply per-tier budgets
/// (Spec C₃ §7.2). Without this, every authenticated user looks like
/// `free` to the limiter. The field defaults to `"free"` when the
/// upstream JWKS-issued token omits it, preserving back-compat with
/// the apps/chat tokens deployed before the schema added a `tier`
/// claim.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Tier1Claims {
    /// Subject — canonical user_id.
    pub user_id: String,
    /// Active project id. Falls back to `default-project` when the dev
    /// signer is in use; Sub-phase B reads from the `project_id` claim
    /// or the `X-Life-Project-Id` header.
    pub project_id: String,
    /// Identity-scoped permissions intersected with the route's required
    /// scope (Sub-phase B). Sub-phase A returns a single broad
    /// `agent:dispatch` scope so the proxy can ship something usable.
    pub scopes: Vec<String>,
    /// User's plan tier (`free` / `paid` / `enterprise` / `anon`). The
    /// rate limiter applies per-tier budgets — `free` users get the
    /// strict budget, `paid` users get raised limits, etc. Defaults to
    /// [`DEFAULT_TIER`] (`"free"`) when the issuer omits the claim.
    pub tier: String,
}

/// Default tier returned when the upstream token omits the `tier`
/// claim. Sub-phase C threads this through to the Tier-2 minter so
/// downstream rate-limit + accounting code can rely on a non-empty
/// value.
pub const DEFAULT_TIER: &str = "free";

impl Tier1Claims {
    /// Build a [`Tier1Claims`] — used by tests + (future) admin
    /// helpers that need to synthesize a known identity. Defaults
    /// `tier` to [`DEFAULT_TIER`].
    pub fn new(
        user_id: impl Into<String>,
        project_id: impl Into<String>,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            project_id: project_id.into(),
            scopes,
            tier: DEFAULT_TIER.to_string(),
        }
    }

    /// Build a [`Tier1Claims`] with an explicit tier. Used by tests
    /// that exercise per-tier rate-limiter behaviour.
    pub fn with_tier(
        user_id: impl Into<String>,
        project_id: impl Into<String>,
        scopes: Vec<String>,
        tier: impl Into<String>,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            project_id: project_id.into(),
            scopes,
            tier: tier.into(),
        }
    }
}
