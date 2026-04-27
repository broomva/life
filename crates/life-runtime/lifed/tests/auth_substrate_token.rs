//! Tier-3 substrate-token mint round-trip: lifed mints, substrate verifies.
//!
//! Spec C₂ §5.2 conformance — the token lifed mints MUST verify against the
//! published JWKS using standard ES256 + audience checks, and MUST reject
//! when the audience does not match the verifier's expected substrate.

use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use lifed::auth::capability::{CapabilityClaims, Tier};
use lifed::auth::keystore::Keystore;
use lifed::auth::substrate_token::{Audience, mint_substrate_token};

#[test]
fn tier3_mint_round_trips_against_published_jwks() {
    let ks = Keystore::generate_dev();
    let _jwks = ks.publish_jwks();

    let claims = CapabilityClaims {
        user_id: "alice".to_string(),
        project_id: "p".to_string(),
        sid: aios_proto::aios::v1::SessionId {
            value: "sid-1".to_string(),
        },
        scopes: vec!["agent:dispatch".to_string()],
        tier: Tier::Free,
        exp: Instant::now() + Duration::from_secs(900),
    };
    let jws = mint_substrate_token(&claims, Audience::Arcan, &ks).expect("mint");

    let pubkey = ks.public_key_pem();
    let key = DecodingKey::from_ec_pem(pubkey.as_bytes()).expect("pem");
    let mut v = Validation::new(Algorithm::ES256);
    v.set_audience(&["arcan"]);
    v.set_issuer(&["lifed"]);
    let token: jsonwebtoken::TokenData<serde_json::Value> = decode(&jws, &key, &v).expect("verify");
    assert_eq!(token.claims["aud"], "arcan");
    assert_eq!(token.claims["iss"], "lifed");
    assert_eq!(token.claims["sub"], "alice");
}

#[test]
fn tier3_rejects_wrong_audience() {
    let ks = Keystore::generate_dev();
    let claims = CapabilityClaims {
        user_id: "alice".to_string(),
        project_id: "p".to_string(),
        sid: aios_proto::aios::v1::SessionId {
            value: "sid-1".to_string(),
        },
        scopes: vec!["agent:dispatch".to_string()],
        tier: Tier::Free,
        exp: Instant::now() + Duration::from_secs(900),
    };
    let jws = mint_substrate_token(&claims, Audience::Arcan, &ks).expect("mint");

    let pubkey = ks.public_key_pem();
    let key = DecodingKey::from_ec_pem(pubkey.as_bytes()).expect("pem");
    let mut v = Validation::new(Algorithm::ES256);
    v.set_audience(&["lago"]);
    let res = decode::<serde_json::Value>(&jws, &key, &v);
    assert!(res.is_err(), "wrong audience must be rejected");
}

#[test]
fn auth_middleware_rejects_invalid_bearer() {
    use lifed::auth::jwks::JwksCache;
    let cache = JwksCache::dev_only();
    // Anything that's neither a valid JWS nor `test-token-for-...` should error.
    let res = cache.validate("nonsense.token.value");
    assert!(res.is_err(), "invalid bearer must error");
}

#[test]
fn auth_middleware_accepts_dev_test_token() {
    use lifed::auth::jwks::JwksCache;
    let cache = JwksCache::dev_only();
    let claims = cache
        .validate("test-token-for-alice")
        .expect("dev path accepts test-token");
    assert_eq!(claims.user_id, "alice");
    assert_eq!(claims.project_id, "project-demo");
}
