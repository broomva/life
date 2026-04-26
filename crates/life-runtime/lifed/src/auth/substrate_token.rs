//! Tier-3 substrate-token mint.
//!
//! Sub-phase A ships a deterministic dev signer that returns
//! `dev-token:{audience}:{user_id}:{sid}` strings. Mock substrates accept
//! these directly. Sub-phase B replaces this with real ES256 JWS plus a
//! published JWKS file (`/run/life/lifed-jwks.json`) that real substrates
//! verify against.

use std::time::{Duration, Instant};

use crate::auth::capability::CapabilityClaims;
use crate::error::{LifedError, LifedResult};

#[derive(Clone, Copy)]
pub enum Audience {
    Arcan,
    Lago,
    Haima,
    Anima,
    Soma,
}

impl Audience {
    pub fn as_str(&self) -> &'static str {
        match self {
            Audience::Arcan => "arcan",
            Audience::Lago => "lago",
            Audience::Haima => "haima",
            Audience::Anima => "anima",
            Audience::Soma => "soma",
        }
    }
}

pub fn mint_substrate_token(claims: &CapabilityClaims, audience: Audience) -> LifedResult<String> {
    if claims.exp < Instant::now() {
        return Err(LifedError::Auth("Tier-2 expired".to_string()));
    }
    // Sub-phase A: deterministic dev token.
    let token = format!(
        "dev-token:{aud}:{user}:{sid}",
        aud = audience.as_str(),
        user = claims.user_id,
        sid = claims.sid.value,
    );
    let _exp = std::cmp::min(claims.exp, Instant::now() + Duration::from_secs(30));
    Ok(token)
}
