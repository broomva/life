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

/// Sub-phase D follow-up #2: the published JWKS must include the PEM
/// material so substrates can verify Tier-3 tokens using ONLY the
/// JWKS file — no shared in-memory keystore state. The test rebuilds
/// the substrate's verifier from the parsed JWKS and runs the standard
/// conformance battery against it.
#[tokio::test]
async fn substrate_verifies_using_only_published_jwks_pem() {
    let ks = Keystore::generate_dev();
    let claims = make_claims("sid-jwks-pem");
    let jws = mint_substrate_token(&claims, Audience::Arcan, &ks).expect("mint");

    // Serialise the JWKS the way lifed publishes it on disk.
    let jwks = ks.publish_jwks();
    let jwks_json = serde_json::to_string_pretty(&jwks).expect("serialise");

    // Parse it back as a substrate would, extract the PEM, and verify.
    #[derive(serde::Deserialize)]
    struct JwksFile {
        keys: Vec<JwksFileKey>,
    }
    #[derive(serde::Deserialize)]
    struct JwksFileKey {
        #[allow(dead_code)]
        kid: String,
        pem: Option<String>,
    }
    let parsed: JwksFile = serde_json::from_str(&jwks_json).expect("parse");
    let pem = parsed.keys[0]
        .pem
        .as_ref()
        .expect("Sub-phase D requires `pem` in published JWKS so substrates can verify standalone")
        .clone();

    let sut = TestSubstrate("arcan");
    let cases = vec![VerificationCase {
        name: "jwks_pem_round_trip",
        jws,
        expected_aud: "arcan",
        should_pass: true,
    }];
    run_battery(&sut, &pem, &cases).await.expect("verify");
}
