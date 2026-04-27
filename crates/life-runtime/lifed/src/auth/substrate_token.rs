//! Tier-3 substrate-token mint per Spec C₂ §5.2.
//!
//! lifed signs an ES256 JWS for each substrate-bound call, narrowing scopes
//! by audience and capping `exp` at 30 s. Substrates verify via the
//! published JWKS (see [`crate::auth::keystore::Keystore::publish_jwks`]).

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, Header, encode};
use serde::{Deserialize, Serialize};

use crate::auth::capability::CapabilityClaims;
use crate::auth::keystore::Keystore;
use crate::error::{LifedError, LifedResult};

/// Per-substrate audience tag used for `aud` and scope narrowing.
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

#[derive(Serialize, Deserialize, Debug)]
struct SubstrateClaims {
    iss: String,
    sub: String,
    aud: String,
    sid: String,
    scopes: Vec<String>,
    exp: u64,
    nbf: u64,
}

/// Mint a Tier-3 substrate-token for `audience`. Returns the JWS as a
/// String. Caller attaches it as `authorization: Bearer <jws>` on the
/// substrate UDS RPC.
pub fn mint_substrate_token(
    claims: &CapabilityClaims,
    audience: Audience,
    ks: &Keystore,
) -> LifedResult<String> {
    if claims.exp < Instant::now() {
        return Err(LifedError::Auth("Tier-2 expired".to_string()));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Cap Tier-3 lifetime at min(remaining Tier-2 budget, 30 s) per
    // Spec C₂ §5.2 step 2.
    let exp_in = std::cmp::min(
        claims
            .exp
            .saturating_duration_since(Instant::now())
            .as_secs(),
        30,
    );
    let body = SubstrateClaims {
        iss: "lifed".to_string(),
        sub: claims.user_id.clone(),
        aud: audience.as_str().to_string(),
        sid: claims.sid.value.clone(),
        scopes: scope_narrow_for(audience, &claims.scopes),
        exp: now + exp_in,
        nbf: now,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(ks.kid.clone());
    encode(&header, &body, &ks.encoding).map_err(|e| LifedError::Auth(format!("sign: {e}")))
}

fn scope_narrow_for(audience: Audience, scopes: &[String]) -> Vec<String> {
    let prefix = match audience {
        Audience::Arcan => "agent:",
        Audience::Lago => "events:",
        Audience::Haima => "wallet:",
        Audience::Anima => "identity:",
        Audience::Soma => "soma:",
    };
    scopes
        .iter()
        .filter(|s| s.starts_with(prefix))
        .cloned()
        .collect()
}
