//! M5 Spec C₂ §15.5 conformance: substrates verify lifed-minted Tier-3 tokens.
//!
//! This test exercises the full mint → publish → verify round-trip using
//! lifed's keystore + the reference verifier from `lifed-conformance`.
//! Each of the five audiences (arcan, lago, haima, anima, soma) must
//! verify cleanly when the substrate's expected audience matches; and
//! reject when it doesn't.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use lifed::auth::capability::{CapabilityClaims, Tier};
use lifed::auth::keystore::Keystore;
use lifed::auth::substrate_token::{Audience, mint_substrate_token};
use lifed_conformance::{SubstrateUnderTest, VerificationCase, reference_verify, run_battery};

struct TestSubstrate(&'static str);

#[async_trait]
impl SubstrateUnderTest for TestSubstrate {
    fn audience(&self) -> &'static str {
        self.0
    }
    async fn verify(
        &self,
        jws: &str,
        audience: &str,
        pubkey_pem: &str,
    ) -> Result<(), tonic::Status> {
        reference_verify(jws, audience, pubkey_pem)
    }
}

fn make_claims(sid: &str) -> CapabilityClaims {
    CapabilityClaims {
        user_id: "alice".to_string(),
        project_id: "p".to_string(),
        sid: aios_proto::aios::v1::SessionId {
            value: sid.to_string(),
        },
        scopes: vec![
            "agent:dispatch".to_string(),
            "events:read".to_string(),
            "wallet:debit".to_string(),
            "identity:read".to_string(),
            "soma:admin".to_string(),
        ],
        tier: Tier::Free,
        exp: Instant::now() + Duration::from_secs(900),
    }
}

#[tokio::test]
async fn all_substrates_verify_minted_tokens() {
    let ks = Keystore::generate_dev();
    let pubkey_pem = ks.public_key_pem();
    let claims = make_claims("sid-1");

    for (name, audience) in [
        ("arcan", Audience::Arcan),
        ("lago", Audience::Lago),
        ("haima", Audience::Haima),
        ("anima", Audience::Anima),
        ("soma", Audience::Soma),
    ] {
        let jws = mint_substrate_token(&claims, audience, &ks).expect("mint");
        let sut = TestSubstrate(name);
        let cases = vec![VerificationCase {
            name: "mint_round_trip",
            jws,
            expected_aud: name,
            should_pass: true,
        }];
        run_battery(&sut, &pubkey_pem, &cases).await.expect(name);
    }
}

#[tokio::test]
async fn token_for_arcan_rejected_by_lago() {
    let ks = Keystore::generate_dev();
    let pubkey_pem = ks.public_key_pem();
    let claims = make_claims("sid-2");
    let jws = mint_substrate_token(&claims, Audience::Arcan, &ks).expect("mint");
    let sut = TestSubstrate("lago");
    let cases = vec![VerificationCase {
        name: "wrong_audience_rejected",
        jws,
        expected_aud: "lago",
        should_pass: false,
    }];
    run_battery(&sut, &pubkey_pem, &cases).await.expect("ok");
}

#[tokio::test]
async fn jwks_publish_includes_kid() {
    let ks = Keystore::generate_dev();
    let jwks = ks.publish_jwks();
    assert_eq!(jwks.keys.len(), 1);
    assert_eq!(jwks.keys[0].kid, "dev-1");
    assert_eq!(jwks.keys[0].alg, "ES256");
    assert_eq!(jwks.keys[0].kty, "EC");
    assert_eq!(jwks.keys[0].crv, "P-256");
}
