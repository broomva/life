//! TierUserMinter — Spec D D-Sub-C.
//!
//! Mints short-lived (15-min) ES256 JWS capabilities for browser users
//! (and Rust `RemoteAnima` callers). Distinct from Tier-2
//! (lifed → arcand) and Tier-1 (Vercel → lifegw):
//!
//! - `aud`: `"anima.user-cap"` (vs `"lifed"` for Tier-2).
//! - `kid`: identical to the Tier-2 active KMS kid — the same KMS signer
//!   produces both tokens. Per Spec D L4-D6 the user-scoped path uses the
//!   same ES256/P-256 substrate the Tier-2 KMS already wires.
//! - TTL: 15 min (cap). Configurable via `cfg.auth.tier_user_ttl`.
//! - `sub`: `user_id` (matches anima per-user namespacing).
//! - `scope`: vector of capability scopes the cap covers (e.g.
//!   `["anima.user.sign_auth", "anima.user.sign_wallet"]`).
//!
//! Same `KmsSigner` trait as Tier-2 — `VaultTransit` / `StaticKeystore`
//! / `AwsKms` / `GcpKms` all work without modification. The TS browser
//! and the Rust `RemoteAnima` both verify these caps against the same
//! published JWKS document.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::kms::KmsSigner;
use crate::error::{LifegwError, LifegwResult};

/// Default Tier-User TTL — 15 minutes (Spec D D-Sub-C cap).
///
/// Production deploys MAY shorten this via
/// `cfg.auth.tier_user_ttl_secs`; lengthening past 15 min is rejected
/// at config-validate time alongside the existing Tier-2 TTL cap.
pub const DEFAULT_TIER_USER_TTL: Duration = Duration::from_secs(15 * 60);

/// Tier-User capability claim shape (Spec D D-Sub-C).
///
/// Mirrors `Tier2Claims` in shape but with user-scoped audience and a
/// `scope` field (vs Tier-2's `scopes` mirror of Tier-1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TierUserClaims {
    /// Issuer — always `"lifegw"` (matches Tier-2 issuer).
    pub iss: String,
    /// Subject — the `user_id` the cap is bound to.
    pub sub: String,
    /// Audience — fixed `"anima.user-cap"` so verifiers can dispatch on
    /// the audience claim (Tier-2 = `"lifed"`, Tier-User = this).
    pub aud: String,
    /// Issued-at — wall-clock seconds since epoch.
    pub iat: u64,
    /// Not-before — set 5 s in the past to tolerate clock skew between
    /// the gateway and downstream verifiers.
    pub nbf: u64,
    /// Expiration — `iat + ttl_secs`. Spec D D-Sub-C caps ttl at 15 min.
    pub exp: u64,
    /// 128-bit random JWT id — observability + replay-attack detection.
    pub jti: String,
    /// Capability scopes — e.g. `["anima.user.sign_auth",
    /// "anima.user.sign_wallet"]`. Each `/anima/custody/*` route
    /// enforces its required scope against this vector.
    pub scope: Vec<String>,
}

/// Tier-User minter — wraps a [`KmsSigner`] + audience + issuer + ttl.
///
/// Uses the SAME `Arc<dyn KmsSigner>` Tier-2 uses — operators provision
/// a single KMS key per gateway and both token types ride the same
/// signer. The kid in the JWS header is `signer.active_kid()` so the
/// published JWKS document covers Tier-2 + Tier-User verification with
/// no extra plumbing.
#[derive(Clone)]
#[non_exhaustive]
pub struct TierUserMinter {
    signer: Arc<dyn KmsSigner>,
    audience: String,
    issuer: String,
    ttl: Duration,
}

impl TierUserMinter {
    /// Build a minter from a KMS signer + audience + issuer + ttl.
    ///
    /// Production callers thread the same `Arc<dyn KmsSigner>` they
    /// hand to `Tier2Minter::new` so both token types share key
    /// material + JWKS publish.
    pub fn new(
        signer: Arc<dyn KmsSigner>,
        issuer: impl Into<String>,
        audience: impl Into<String>,
        ttl: Duration,
    ) -> Self {
        Self {
            signer,
            audience: audience.into(),
            issuer: issuer.into(),
            ttl,
        }
    }

    /// Build a minter using the conventional defaults — issuer
    /// `"lifegw"`, audience `"anima.user-cap"`. Convenience for the
    /// bootstrap path where every parameter matches the spec.
    pub fn with_defaults(signer: Arc<dyn KmsSigner>, ttl: Duration) -> Self {
        Self::new(signer, "lifegw", "anima.user-cap", ttl)
    }

    /// Borrow the inner signer — used by tests + bootstrap to publish
    /// the JWKS once and verify minted tokens with the same key.
    pub fn signer(&self) -> &Arc<dyn KmsSigner> {
        &self.signer
    }

    /// Borrow the configured audience.
    pub fn audience(&self) -> &str {
        &self.audience
    }

    /// Borrow the configured issuer.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Borrow the configured TTL.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Mint a Tier-User capability JWS for `user_id` with the given
    /// `scope` vector. Returns the compact-form JWS string + the unix
    /// expiry timestamp so the caller can return both to the client.
    pub fn mint(&self, user_id: &str, scope: Vec<String>) -> LifegwResult<(String, i64)> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| LifegwError::Auth(format!("clock: {e}")))?
            .as_secs();
        let exp = now.saturating_add(self.ttl.as_secs());
        let claims = TierUserClaims {
            iss: self.issuer.clone(),
            sub: user_id.to_string(),
            aud: self.audience.clone(),
            iat: now,
            nbf: now.saturating_sub(5),
            exp,
            jti: Uuid::new_v4().to_string(),
            scope,
        };
        let body = serde_json::to_value(&claims)
            .map_err(|e| LifegwError::Auth(format!("encode tier-user claims: {e}")))?;
        let token = self.signer.sign_jws(&body)?;
        Ok((token, exp as i64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::kms::StaticKeystore;
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

    fn dev_minter() -> (Arc<StaticKeystore>, TierUserMinter) {
        let signer = Arc::new(StaticKeystore::generate_dev().expect("dev keystore"));
        let minter = TierUserMinter::with_defaults(
            signer.clone() as Arc<dyn KmsSigner>,
            DEFAULT_TIER_USER_TTL,
        );
        (signer, minter)
    }

    #[test]
    fn mint_round_trip_uses_kms_signer() {
        let (signer, minter) = dev_minter();
        let (jws, expires_at) = minter
            .mint("alice", vec!["anima.user.sign_auth".to_string()])
            .expect("mint");
        let header = decode_header(&jws).expect("decode_header");
        assert_eq!(header.alg, Algorithm::ES256);
        assert_eq!(header.kid.as_deref(), Some(signer.active_kid()));

        // Verify with the published JWKS PEM.
        let jwks = signer.publish_jwks();
        let pem = jwks.keys[0].pem.as_ref().expect("dev pem");
        let dk = DecodingKey::from_ec_pem(pem.as_bytes()).expect("decode pem");
        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["anima.user-cap"]);
        v.set_issuer(&["lifegw"]);
        v.validate_nbf = true;
        let body = decode::<TierUserClaims>(&jws, &dk, &v).expect("verify");
        let claims = body.claims;
        assert_eq!(claims.sub, "alice");
        assert_eq!(claims.aud, "anima.user-cap");
        assert_eq!(claims.iss, "lifegw");
        assert!(claims.exp > claims.iat);
        assert!(claims.nbf <= claims.iat);
        assert!(!claims.jti.is_empty());
        assert_eq!(claims.scope, vec!["anima.user.sign_auth".to_string()]);
        assert_eq!(expires_at, claims.exp as i64);
    }

    #[test]
    fn mint_uses_configured_lifetime() {
        let signer = Arc::new(StaticKeystore::generate_dev().expect("ks"));
        let minter = TierUserMinter::new(
            signer.clone() as Arc<dyn KmsSigner>,
            "lifegw",
            "anima.user-cap",
            Duration::from_secs(60),
        );
        let (jws, _) = minter.mint("u", vec![]).expect("mint");
        let jwks = signer.publish_jwks();
        let pem = jwks.keys[0].pem.as_ref().expect("dev pem");
        let dk = DecodingKey::from_ec_pem(pem.as_bytes()).expect("decode pem");
        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["anima.user-cap"]);
        v.set_issuer(&["lifegw"]);
        v.validate_nbf = true;
        let body = decode::<TierUserClaims>(&jws, &dk, &v).expect("verify");
        assert_eq!(body.claims.exp - body.claims.iat, 60);
    }

    #[test]
    fn mint_distinct_audience_from_tier2() {
        // Spec D D-Sub-C invariant: Tier-User caps carry
        // `aud=anima.user-cap`, distinct from Tier-2's `aud=lifed`.
        // Verifiers dispatch on this audience to route Tier-User vs
        // Tier-2 enforcement.
        let signer = Arc::new(StaticKeystore::generate_dev().expect("ks"));
        let minter =
            TierUserMinter::with_defaults(signer as Arc<dyn KmsSigner>, DEFAULT_TIER_USER_TTL);
        assert_eq!(minter.audience(), "anima.user-cap");
        assert_ne!(minter.audience(), "lifed");
    }

    #[test]
    fn mint_propagates_scope() {
        let (signer, minter) = dev_minter();
        let scopes = vec![
            "anima.user.sign_auth".to_string(),
            "anima.user.sign_wallet".to_string(),
        ];
        let (jws, _) = minter.mint("alice", scopes.clone()).expect("mint");
        let jwks = signer.publish_jwks();
        let pem = jwks.keys[0].pem.as_ref().expect("pem");
        let dk = DecodingKey::from_ec_pem(pem.as_bytes()).expect("decode");
        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["anima.user-cap"]);
        v.set_issuer(&["lifegw"]);
        v.validate_nbf = true;
        let body = decode::<TierUserClaims>(&jws, &dk, &v).expect("verify");
        assert_eq!(body.claims.scope, scopes);
    }

    #[test]
    fn nbf_is_in_the_past() {
        let (_, minter) = dev_minter();
        let (jws, _) = minter.mint("u", vec![]).expect("mint");
        let parts: Vec<&str> = jws.split('.').collect();
        assert_eq!(parts.len(), 3);
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let body_bytes = URL_SAFE_NO_PAD.decode(parts[1]).expect("decode body");
        let body: TierUserClaims = serde_json::from_slice(&body_bytes).expect("parse body");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(body.nbf <= body.iat);
        assert!(body.iat <= now + 30);
    }
}
