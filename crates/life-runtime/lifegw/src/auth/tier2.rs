//! Tier-2 capability-token mint (Spec C₃ §5.4).
//!
//! Tier-2 tokens are ES256-signed JWS with `aud=lifed`, `iss=lifegw`,
//! lifetime ≤ 15 min. Sub-phase A signs with the in-process [`Keystore`];
//! Sub-phase E swaps in the KMS-backed signer.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, Header, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::keystore::Keystore;
use crate::auth::tier1::Tier1Claims;
use crate::config::AuthConfig;
use crate::error::{LifegwError, LifegwResult};

/// Tier-2 claim shape — subset of the spec body relevant to Sub-phase A.
/// Spec C₃ §5.4 lists the full claim set; the fields below are sufficient
/// for lifed (Spec C₂ §5.1) to verify and route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tier2Claims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub exp: u64,
    pub iat: u64,
    /// 128-bit random unique JWT id — observability + replay-attack tagging.
    pub jti: String,
    /// Session id when the route is session-scoped. For session-creating
    /// routes (`Agent.CreateSession`) the gateway emits the empty string.
    pub sid: String,
    /// Project id propagated from Tier-1.
    pub project_id: String,
    /// Capability scopes (intersection of Tier-1 claim scopes with the
    /// route's required scope per Spec C₃ §5.4).
    pub scopes: Vec<String>,
    /// Optional tier name (`free` / `paid` / `enterprise` / `anon`). Sub-phase
    /// A always emits `free`; Sub-phase B copies it from the Tier-1 `tier`
    /// claim.
    pub tier: String,
}

/// Tier-2 minter — a thin wrapper around the keystore + AuthConfig.
#[derive(Clone)]
pub struct Tier2Minter {
    keystore: Keystore,
    audience: String,
    issuer: String,
    ttl: Duration,
}

impl Tier2Minter {
    pub fn new(keystore: Keystore, cfg: &AuthConfig) -> Self {
        Self {
            keystore,
            audience: cfg.tier2_audience.clone(),
            issuer: cfg.tier2_issuer.clone(),
            ttl: cfg.tier2_ttl,
        }
    }

    /// Mint a Tier-2 capability JWS from a Tier-1 claim set. Returns the
    /// compact-form JWS string the gateway places in the upstream
    /// `authorization` metadata header.
    pub fn mint(&self, t1: &Tier1Claims) -> LifegwResult<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| LifegwError::Auth(format!("clock: {e}")))?
            .as_secs();
        let claims = Tier2Claims {
            iss: self.issuer.clone(),
            sub: t1.user_id.clone(),
            aud: self.audience.clone(),
            iat: now,
            exp: now + self.ttl.as_secs(),
            jti: Uuid::new_v4().to_string(),
            sid: String::new(),
            project_id: t1.project_id.clone(),
            scopes: t1.scopes.clone(),
            tier: "free".to_string(),
        };
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.keystore.kid.clone());
        encode(&header, &claims, &self.keystore.encoding)
            .map_err(|e| LifegwError::Auth(format!("encode tier-2: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Validation, decode, decode_header};

    fn dev_minter() -> Tier2Minter {
        let ks = Keystore::generate_dev().expect("dev keystore");
        let cfg = AuthConfig {
            tier2_audience: "lifed".to_string(),
            tier2_issuer: "lifegw".to_string(),
            tier2_ttl: Duration::from_secs(900),
            ..AuthConfig::default()
        };
        Tier2Minter::new(ks, &cfg)
    }

    #[test]
    fn mint_round_trip() {
        let minter = dev_minter();
        let t1 = Tier1Claims {
            user_id: "user-1".to_string(),
            project_id: "demo".to_string(),
            scopes: vec!["agent:dispatch".to_string()],
        };
        let jws = minter.mint(&t1).expect("mint");
        let header = decode_header(&jws).expect("decode_header");
        assert_eq!(header.alg, Algorithm::ES256);
        assert_eq!(header.kid.as_deref(), Some(minter.keystore.kid.as_str()));

        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["lifed"]);
        v.set_issuer(&["lifegw"]);
        let body = decode::<Tier2Claims>(&jws, &minter.keystore.decoding, &v).expect("verify");
        assert_eq!(body.claims.sub, "user-1");
        assert_eq!(body.claims.project_id, "demo");
        assert_eq!(body.claims.aud, "lifed");
        assert_eq!(body.claims.iss, "lifegw");
        assert!(body.claims.exp > body.claims.iat);
        assert!(!body.claims.jti.is_empty());
    }

    #[test]
    fn mint_uses_configured_lifetime() {
        let ks = Keystore::generate_dev().expect("ks");
        let cfg = AuthConfig {
            tier2_audience: "lifed".to_string(),
            tier2_issuer: "lifegw".to_string(),
            tier2_ttl: Duration::from_secs(60),
            ..AuthConfig::default()
        };
        let m = Tier2Minter::new(ks, &cfg);
        let t1 = Tier1Claims {
            user_id: "u".to_string(),
            project_id: "p".to_string(),
            scopes: vec![],
        };
        let jws = m.mint(&t1).expect("mint");
        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["lifed"]);
        v.set_issuer(&["lifegw"]);
        let body = decode::<Tier2Claims>(&jws, &m.keystore.decoding, &v).expect("verify");
        assert_eq!(body.claims.exp - body.claims.iat, 60);
    }
}
