//! Tier-2 token verifier.
//!
//! Sub-phase A ships a dev-mode verifier that accepts tokens of the form
//! `Bearer test-token-for-{user_id}` and synthesises CapabilityClaims for
//! the user. Sub-phase B replaces this with a real ES256 JWKS verifier
//! (jsonwebtoken crate, gateway public key, blocklist check).

use std::time::{Duration, Instant};

use aios_proto::aios::v1 as aios_v1;

use crate::auth::capability::{CapabilityClaims, Tier};
use crate::error::{LifedError, LifedResult};

pub struct JwksCache {
    /// Sub-phase A: holds nothing; the dev verifier ignores it.
    /// Sub-phase B will load + cache real gateway public keys here.
    _placeholder: (),
}

impl JwksCache {
    pub fn load_from_path(_path: &std::path::Path) -> LifedResult<Self> {
        // Sub-phase A: no JWKS file required.
        Ok(Self { _placeholder: () })
    }

    /// Validate a Tier-2 bearer token.
    pub fn validate(&self, bearer: &str) -> LifedResult<CapabilityClaims> {
        // Sub-phase A dev verifier: accept "test-token-for-{user_id}" and
        // synthesise minimal claims. Sub-phase B replaces with real ES256.
        let prefix = "test-token-for-";
        if let Some(user_id) = bearer.strip_prefix(prefix) {
            return Ok(CapabilityClaims {
                user_id: user_id.to_string(),
                project_id: "project-demo".to_string(),
                sid: aios_v1::SessionId {
                    value: String::new(),
                },
                scopes: vec!["agent:dispatch".to_string(), "events:read".to_string()],
                tier: Tier::Free,
                exp: Instant::now() + Duration::from_secs(900),
            });
        }
        Err(LifedError::Auth("invalid bearer token".to_string()))
    }
}
