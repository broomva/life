//! Auth subsystem — Tier-1 verify (in) → Tier-2 mint (out).
//!
//! Sub-phase A scope (Spec C₃ §12.A):
//! - `keystore` — in-process P-256 ES256 dev signing key.
//! - `dev_signer` — accepts `Bearer dev-token-for-{user_id}` and synthesises
//!   minimal Tier-1 claims.
//! - `middleware` — tower Layer that validates inbound bearer, swaps it for
//!   a freshly-minted Tier-2 capability JWS, and forwards to the proxy.
//! - `tier1` — Tier-1 claim shape (used by both dev signer and Sub-phase B
//!   real verifier).
//! - `tier2` — Tier-2 capability claim shape + mint helper.
//!
//! Real ES256 + Vercel JWKS replaces `dev_signer` in Sub-phase B; the
//! middleware shape stays.

pub mod dev_signer;
pub mod keystore;
pub mod middleware;
pub mod tier1;
pub mod tier2;

pub use keystore::Keystore;
pub use middleware::AuthLayer;
pub use tier1::Tier1Claims;
pub use tier2::Tier2Claims;
