//! Tier-2 capability-token mint (Spec C₃ §5.4).
//!
//! Tier-2 tokens are ES256-signed JWS with `aud=lifed`, `iss=lifegw`,
//! lifetime ≤ 15 min. Sub-phase B routes signing through a
//! [`KmsSigner`] trait object so production builds swap in a KMS-backed
//! provider without touching the Tier-2 minter or the auth Layer.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::kms::KmsSigner;
use crate::auth::tier1::Tier1Claims;
use crate::config::AuthConfig;
use crate::error::{LifegwError, LifegwResult};

/// Tier-2 claim shape (Spec C₃ §5.4).
///
/// Sub-phase B extends Sub-phase A's body with `nbf` so downstream
/// verifiers (lifed) reject not-yet-valid tokens. `#[non_exhaustive]`
/// per Sub-phase A code-quality review prerequisite — adding fields in
/// future sub-phases (e.g. `tenant`, `dispatch_idempotency_seed`) won't
/// break consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Tier2Claims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    /// Not-before — tokens minted with a small `nbf` skew tolerate
    /// clock drift between gateway and downstream.
    pub nbf: u64,
    /// Issued-at — wall-clock seconds since epoch.
    pub iat: u64,
    /// Expiration — `iat + ttl_secs`. Spec C₃ §5.4 caps ttl at 15 min.
    pub exp: u64,
    /// 128-bit random unique JWT id — observability + replay-attack
    /// detection.
    pub jti: String,
    /// Session id when the route is session-scoped. For session-creating
    /// routes (`Agent.CreateSession`) the gateway emits the empty
    /// string — lifed re-mints once the saga produces a sid.
    pub sid: String,
    /// Project id propagated from Tier-1.
    pub project_id: String,
    /// Capability scopes.
    pub scopes: Vec<String>,
    /// Tier name (`free` / `paid` / `enterprise` / `anon`).
    pub tier: String,
}

/// Tier-2 minter — wraps a [`KmsSigner`] + AuthConfig.
///
/// Sub-phase A used a concrete [`Keystore`] handle. Sub-phase B
/// abstracts behind `Arc<dyn KmsSigner>` so production builds swap in
/// a KMS-backed signer without touching the auth Layer.
#[derive(Clone)]
#[non_exhaustive]
pub struct Tier2Minter {
    signer: Arc<dyn KmsSigner>,
    audience: String,
    issuer: String,
    ttl: Duration,
}

impl Tier2Minter {
    /// Build a minter from a KMS signer + auth config. The auth config
    /// supplies the issuer, audience, and lifetime cap.
    pub fn new(signer: Arc<dyn KmsSigner>, cfg: &AuthConfig) -> Self {
        Self {
            signer,
            audience: cfg.tier2_audience.clone(),
            issuer: cfg.tier2_issuer.clone(),
            ttl: cfg.tier2_ttl,
        }
    }

    /// Borrow the inner signer — used by bootstrap to publish the JWKS
    /// and by tests to verify minted tokens with the same key.
    pub fn signer(&self) -> &Arc<dyn KmsSigner> {
        &self.signer
    }

    /// Mint a Tier-2 capability JWS from a Tier-1 claim set. Returns
    /// the compact-form JWS string the gateway places in the upstream
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
            // `nbf` set 5 s in the past to tolerate downstream clock
            // skew without expanding the verifier's leeway window.
            nbf: now.saturating_sub(5),
            iat: now,
            exp: now + self.ttl.as_secs(),
            jti: Uuid::new_v4().to_string(),
            sid: String::new(),
            project_id: t1.project_id.clone(),
            scopes: t1.scopes.clone(),
            tier: "free".to_string(),
        };
        let body = serde_json::to_value(&claims)
            .map_err(|e| LifegwError::Auth(format!("encode tier-2 claims: {e}")))?;
        self.signer.sign_jws(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::kms::StaticKeystore;
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

    fn dev_minter() -> (Arc<StaticKeystore>, Tier2Minter) {
        let signer = Arc::new(StaticKeystore::generate_dev().expect("dev keystore"));
        let cfg = AuthConfig {
            tier2_audience: "lifed".to_string(),
            tier2_issuer: "lifegw".to_string(),
            tier2_ttl: Duration::from_secs(900),
            ..AuthConfig::default()
        };
        let minter = Tier2Minter::new(signer.clone() as Arc<dyn KmsSigner>, &cfg);
        (signer, minter)
    }

    #[test]
    fn mint_round_trip_uses_kms_signer() {
        let (signer, minter) = dev_minter();
        let t1 = Tier1Claims {
            user_id: "user-1".to_string(),
            project_id: "demo".to_string(),
            scopes: vec!["agent:dispatch".to_string()],
        };
        let jws = minter.mint(&t1).expect("mint");
        let header = decode_header(&jws).expect("decode_header");
        assert_eq!(header.alg, Algorithm::ES256);
        assert_eq!(header.kid.as_deref(), Some(signer.active_kid()));

        // Verify with the publish_jwks PEM — closes the cross-process
        // trust loop the JWKS publish step relies on.
        let jwks = signer.publish_jwks();
        let pem = jwks.keys[0].pem.as_ref().expect("dev pem");
        let dk = DecodingKey::from_ec_pem(pem.as_bytes()).expect("decode pem");
        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["lifed"]);
        v.set_issuer(&["lifegw"]);
        v.validate_nbf = true;
        let body = decode::<Tier2Claims>(&jws, &dk, &v).expect("verify");
        let claims = body.claims;
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.project_id, "demo");
        assert_eq!(claims.aud, "lifed");
        assert_eq!(claims.iss, "lifegw");
        assert!(claims.exp > claims.iat);
        assert!(claims.nbf <= claims.iat);
        assert!(!claims.jti.is_empty());
    }

    #[test]
    fn mint_uses_configured_lifetime() {
        let signer = Arc::new(StaticKeystore::generate_dev().expect("ks"));
        let cfg = AuthConfig {
            tier2_audience: "lifed".to_string(),
            tier2_issuer: "lifegw".to_string(),
            tier2_ttl: Duration::from_secs(60),
            ..AuthConfig::default()
        };
        let m = Tier2Minter::new(signer.clone() as Arc<dyn KmsSigner>, &cfg);
        let t1 = Tier1Claims {
            user_id: "u".to_string(),
            project_id: "p".to_string(),
            scopes: vec![],
        };
        let jws = m.mint(&t1).expect("mint");
        let jwks = signer.publish_jwks();
        let pem = jwks.keys[0].pem.as_ref().expect("dev pem");
        let dk = DecodingKey::from_ec_pem(pem.as_bytes()).expect("decode pem");
        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["lifed"]);
        v.set_issuer(&["lifegw"]);
        v.validate_nbf = true;
        let body = decode::<Tier2Claims>(&jws, &dk, &v).expect("verify");
        assert_eq!(body.claims.exp - body.claims.iat, 60);
    }

    #[test]
    fn nbf_is_in_the_past() {
        let (_, minter) = dev_minter();
        let t1 = Tier1Claims {
            user_id: "u".to_string(),
            project_id: "p".to_string(),
            scopes: vec![],
        };
        let jws = minter.mint(&t1).expect("mint");
        // Decode body without verifying signature to inspect raw nbf.
        let parts: Vec<&str> = jws.split('.').collect();
        assert_eq!(parts.len(), 3);
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let body_bytes = URL_SAFE_NO_PAD.decode(parts[1]).expect("decode body");
        let body: Tier2Claims = serde_json::from_slice(&body_bytes).expect("parse body");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // nbf must be ≤ iat ≤ now (within 30 s leeway).
        assert!(body.nbf <= body.iat);
        assert!(body.iat <= now + 30);
    }
}
