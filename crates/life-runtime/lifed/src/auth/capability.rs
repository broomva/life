//! CapabilityClaims — extracted from a Tier-2 JWS by AuthLayer middleware.
//!
//! The middleware attaches `CapabilityClaims` to every incoming public-plane
//! request via `Request::extensions_mut().insert(...)`. Handlers read it via
//! `Self::claims(&req)?`.

use std::time::Instant;

use aios_proto::aios::v1 as aios_v1;

#[derive(Debug, Clone)]
pub struct CapabilityClaims {
    pub user_id: String,
    pub project_id: String,
    pub sid: aios_v1::SessionId,
    pub scopes: Vec<String>,
    pub tier: Tier,
    pub exp: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Free,
    Paid,
    Enterprise,
}

impl Default for CapabilityClaims {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            project_id: String::new(),
            sid: aios_v1::SessionId {
                value: String::new(),
            },
            scopes: Vec::new(),
            tier: Tier::Free,
            exp: Instant::now(),
        }
    }
}
