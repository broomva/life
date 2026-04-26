//! Auth subsystem — Tier-2 capability validation + Tier-3 substrate-token mint.
//!
//! Spec C₂ §5. Sub-phase A ships a dev signer accepting
//! `Bearer test-token-for-{user_id}`; sub-phase B replaces it with real
//! ES256 + JWKS verification.

pub mod blocklist;
pub mod capability;
pub mod jwks;
pub mod middleware;
pub mod substrate_token;

pub use capability::{CapabilityClaims, Tier};
pub use middleware::AuthLayer;
