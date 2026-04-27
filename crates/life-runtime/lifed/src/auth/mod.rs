//! Auth subsystem — Tier-2 capability validation + Tier-3 substrate-token mint.
//!
//! Spec C₂ §5. Sub-phase B ships real ES256 + JWKS verification. The dev
//! signer (`test-token-for-{user_id}` shortcut) is preserved behind
//! [`jwks::JwksCache::dev_only`] so existing integration tests continue
//! passing without forcing every test to mint real JWS tokens.

pub mod blocklist;
pub mod capability;
pub mod jwks;
pub mod keystore;
pub mod middleware;
pub mod peercred;
pub mod substrate_token;

pub use capability::{CapabilityClaims, Tier};
pub use middleware::AuthLayer;
