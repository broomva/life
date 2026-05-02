//! Spec D D-Sub-C — `RemoteAnima` integration tests.
//!
//! Stands up a [`wiremock::MockServer`] that mocks lifegw's
//! `/anima/custody/*` HTTP/JSON proxy and verifies that
//! `RemoteAnima`:
//!
//! 1. Bootstraps via two `GET /anima/custody/get_*_pubkey/{user_id}`
//!    calls — DID + wallet address derive correctly from the cached
//!    pubkeys.
//! 2. `sign_jws` POSTs to `/anima/custody/sign_auth` with the SHA-256
//!    digest of `<header>.<body>` and reassembles the compact JWS.
//! 3. `sign_digest` round-trips a 32-byte prehash → 64-byte raw r||s.
//! 4. `sign_evm_tx` produces a 65-byte r||s||v whose v ∈ {27, 28}
//!    recovers the cached wallet pubkey.
//! 5. `rotate()` returns the documented "journal-driven" error
//!    (does NOT hit the network).
//!
//! lifegw routes themselves ship in Stream R-2 (separate PR). The
//! mock here verifies only the client-side request/response shapes.

#![cfg(feature = "kms-remote")]

use std::sync::{Arc, Mutex};

use anima_identity::custody::{AnimaCustody, BackendKind, TxRequest};
use anima_identity::remote::{RemoteAnima, TierUserCap};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64_STANDARD, URL_SAFE_NO_PAD};
use k256::SecretKey as Secp256k1SecretKey;
use k256::ecdsa::{
    RecoveryId, Signature as K256Signature, SigningKey as K256SigningKey,
    VerifyingKey as K256VerifyingKey,
};
use k256::elliptic_curve::sec1::ToEncodedPoint;
use p256::SecretKey as P256SecretKey;
use p256::ecdsa::{
    Signature as P256Signature, SigningKey as P256SigningKey,
    signature::hazmat::PrehashSigner as P256PrehashSigner,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const ALICE: &str = "alice";
const ALICE_AUTH_SCALAR: [u8; 32] = [7u8; 32];
const ALICE_WALLET_SCALAR: [u8; 32] = [11u8; 32];

/// Test fixture — owns the wiremock server, the deterministic test
/// keys, and the call recorder used for assertions.
struct RemoteFixture {
    server: MockServer,
    auth_sk: P256SecretKey,
    wallet_sk: Secp256k1SecretKey,
    sign_calls: Arc<Mutex<Vec<SignCall>>>,
}

#[derive(Debug, Clone)]
struct SignCall {
    route: String,
    body: Value,
    has_bearer: bool,
}

impl RemoteFixture {
    async fn build() -> Self {
        let server = MockServer::start().await;
        let auth_sk = P256SecretKey::from_bytes(&ALICE_AUTH_SCALAR.into()).unwrap();
        let wallet_sk = Secp256k1SecretKey::from_bytes(&ALICE_WALLET_SCALAR.into()).unwrap();
        let sign_calls = Arc::new(Mutex::new(Vec::new()));

        // GET /anima/custody/get_auth_pubkey/{user_id}
        let auth_pk = auth_sk.public_key();
        let auth_pt = auth_pk.to_encoded_point(true); // SEC1 compressed
        let auth_pubkey_bytes = auth_pt.as_bytes().to_vec();
        Mock::given(method("GET"))
            .and(path(format!(
                "/anima/custody/get_auth_pubkey/{ALICE}"
            )))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({
                    "pubkey_b64": B64_STANDARD.encode(&auth_pubkey_bytes)
                })),
            )
            .mount(&server)
            .await;

        // GET /anima/custody/get_wallet_pubkey/{user_id}
        let wallet_pk = wallet_sk.public_key();
        let wallet_pt = wallet_pk.to_encoded_point(false); // SEC1 uncompressed
        let wallet_pubkey_bytes = wallet_pt.as_bytes().to_vec();
        Mock::given(method("GET"))
            .and(path(format!(
                "/anima/custody/get_wallet_pubkey/{ALICE}"
            )))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({
                    "pubkey_b64": B64_STANDARD.encode(&wallet_pubkey_bytes)
                })),
            )
            .mount(&server)
            .await;

        Self {
            server,
            auth_sk,
            wallet_sk,
            sign_calls,
        }
    }

    /// Mount the `/anima/custody/sign_auth` mock.
    async fn mount_sign_auth(&self) {
        let calls = self.sign_calls.clone();
        let auth_sk = self.auth_sk.clone();
        Mock::given(method("POST"))
            .and(path("/anima/custody/sign_auth"))
            .respond_with(SignAuthResponder { calls, auth_sk })
            .mount(&self.server)
            .await;
    }

    /// Mount the `/anima/custody/sign_wallet` mock.
    async fn mount_sign_wallet(&self) {
        let calls = self.sign_calls.clone();
        let wallet_sk = self.wallet_sk.clone();
        Mock::given(method("POST"))
            .and(path("/anima/custody/sign_wallet"))
            .respond_with(SignWalletResponder { calls, wallet_sk })
            .mount(&self.server)
            .await;
    }

    fn base_url(&self) -> String {
        self.server.uri()
    }
}

/// Custom wiremock responder for `/anima/custody/sign_auth`.
struct SignAuthResponder {
    calls: Arc<Mutex<Vec<SignCall>>>,
    auth_sk: P256SecretKey,
}

impl Respond for SignAuthResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).expect("body must be JSON");
        let has_bearer = req
            .headers
            .iter()
            .any(|(name, value)| {
                name.as_str().eq_ignore_ascii_case("authorization")
                    && value.to_str().is_ok_and(|v| v.starts_with("Bearer "))
            });
        self.calls.lock().unwrap().push(SignCall {
            route: "/anima/custody/sign_auth".into(),
            body: body.clone(),
            has_bearer,
        });
        let digest_b64 = body
            .get("digest_b64")
            .and_then(|v| v.as_str())
            .expect("digest_b64 required");
        let digest_bytes = B64_STANDARD.decode(digest_b64).expect("b64 decode");
        if digest_bytes.len() != 32 {
            return ResponseTemplate::new(400);
        }
        let mut digest_arr = [0u8; 32];
        digest_arr.copy_from_slice(&digest_bytes);

        let sk = P256SigningKey::from(self.auth_sk.clone());
        let signature: P256Signature = sk.sign_prehash(&digest_arr).unwrap();
        let raw = signature.to_bytes().to_vec();
        ResponseTemplate::new(200).set_body_json(json!({
            "signature_b64": B64_STANDARD.encode(&raw)
        }))
    }
}

/// Custom wiremock responder for `/anima/custody/sign_wallet`.
struct SignWalletResponder {
    calls: Arc<Mutex<Vec<SignCall>>>,
    wallet_sk: Secp256k1SecretKey,
}

impl Respond for SignWalletResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).expect("body must be JSON");
        let has_bearer = req
            .headers
            .iter()
            .any(|(name, value)| {
                name.as_str().eq_ignore_ascii_case("authorization")
                    && value.to_str().is_ok_and(|v| v.starts_with("Bearer "))
            });
        self.calls.lock().unwrap().push(SignCall {
            route: "/anima/custody/sign_wallet".into(),
            body: body.clone(),
            has_bearer,
        });
        let digest_b64 = body
            .get("digest_b64")
            .and_then(|v| v.as_str())
            .expect("digest_b64 required");
        let digest_bytes = B64_STANDARD.decode(digest_b64).expect("b64 decode");
        if digest_bytes.len() != 32 {
            return ResponseTemplate::new(400);
        }
        let mut digest_arr = [0u8; 32];
        digest_arr.copy_from_slice(&digest_bytes);

        let sk = K256SigningKey::from(self.wallet_sk.clone());
        let sig: K256Signature = sk.sign_prehash(&digest_arr).unwrap();
        let raw = sig.to_bytes().to_vec();
        ResponseTemplate::new(200).set_body_json(json!({
            "signature_b64": B64_STANDARD.encode(&raw)
        }))
    }
}

fn fake_cap() -> TierUserCap {
    TierUserCap {
        token: "tier-user-jwt-fake".into(),
        expires_at_unix: i64::MAX,
    }
}

// ─── Tests ─────────────────────────────────────────────────────────

/// Bootstrap: `RemoteAnima::new` fetches both pubkeys, derives DID
/// (`did:key:zDn…`) and a 0x-prefixed 42-char wallet address.
#[tokio::test(flavor = "multi_thread")]
async fn remote_anima_bootstrap_fetches_pubkeys() {
    let fixture = RemoteFixture::build().await;
    let anima = RemoteAnima::new(fixture.base_url(), ALICE, fake_cap())
        .await
        .expect("bootstrap must succeed");
    assert_eq!(anima.backend_kind(), BackendKind::Remote);
    assert!(
        anima.user_did().starts_with("did:key:zDn"),
        "DID must derive from P-256 auth pubkey: {}",
        anima.user_did()
    );
    let addr = anima.wallet_address().expect("wallet address resolved");
    assert!(addr.address.starts_with("0x"));
    assert_eq!(addr.address.len(), 42);
    assert_eq!(anima.user_id(), ALICE);
}

/// `sign_jws` hits `/anima/custody/sign_auth` with a 32-byte digest
/// in the body, and the returned 3-part JWS has `alg=ES256` + the
/// user DID as the kid.
#[tokio::test(flavor = "multi_thread")]
async fn sign_jws_hits_sign_auth_route() {
    let fixture = RemoteFixture::build().await;
    fixture.mount_sign_auth().await;
    let base_url = fixture.base_url();
    let calls = fixture.sign_calls.clone();
    let anima = RemoteAnima::new(base_url, ALICE, fake_cap()).await.unwrap();

    let jws = tokio::task::spawn_blocking(move || {
        anima.sign_jws(&json!({"sub": "agt_001", "iss": "anima"}))
    })
    .await
    .unwrap()
    .expect("sign_jws must succeed");

    let parts: Vec<&str> = jws.split('.').collect();
    assert_eq!(parts.len(), 3, "JWS must have 3 parts");

    let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).unwrap();
    let header: Value = serde_json::from_slice(&header_bytes).unwrap();
    assert_eq!(header["alg"], "ES256");
    assert_eq!(header["typ"], "JWT");
    let kid = header["kid"].as_str().unwrap();
    assert!(
        kid.starts_with("did:key:zDn"),
        "kid must be P-256 DID, got {kid}"
    );

    let recorded = calls.lock().unwrap();
    let auth_call = recorded
        .iter()
        .find(|c| c.route == "/anima/custody/sign_auth")
        .expect("sign_auth call recorded");
    assert!(auth_call.has_bearer, "sign_auth must carry bearer token");
    let user_id = auth_call.body["user_id"].as_str().unwrap();
    assert_eq!(user_id, ALICE);
    let digest_b64 = auth_call.body["digest_b64"].as_str().unwrap();
    let digest_bytes = B64_STANDARD.decode(digest_b64).unwrap();
    assert_eq!(digest_bytes.len(), 32, "digest must be 32 bytes (SHA-256)");
}

/// `sign_digest` returns a 64-byte raw r||s — verify it matches a
/// fresh ECDSA-P256 signature over the same prehash by the same key.
#[tokio::test(flavor = "multi_thread")]
async fn sign_digest_returns_remote_signature() {
    let fixture = RemoteFixture::build().await;
    fixture.mount_sign_auth().await;
    let base_url = fixture.base_url();
    let anima = RemoteAnima::new(base_url, ALICE, fake_cap()).await.unwrap();

    let digest = {
        let mut d = [0u8; 32];
        let h = Sha256::digest(b"test message");
        d.copy_from_slice(&h);
        d
    };
    let digest_for_block = digest;
    let sig_bytes = tokio::task::spawn_blocking(move || anima.sign_digest(&digest_for_block))
        .await
        .unwrap()
        .expect("sign_digest must succeed");
    assert_eq!(sig_bytes.len(), 64);

    // Verify the returned signature against the test key over the same
    // digest — the mock signs deterministically, so this is a strong
    // round-trip assertion.
    let sk = P256SigningKey::from(fixture.auth_sk.clone());
    let expected: P256Signature = sk.sign_prehash(&digest).unwrap();
    assert_eq!(sig_bytes.to_vec(), expected.to_bytes().to_vec());
}

/// `sign_evm_tx` produces a 65-byte r||s||v signature whose v
/// recovers the cached wallet pubkey. Mirrors the
/// `vault_anima_signs_evm_tx_with_recoverable_v` invariant.
#[tokio::test(flavor = "multi_thread")]
async fn sign_evm_tx_recovers_correct_v_byte() {
    let fixture = RemoteFixture::build().await;
    fixture.mount_sign_wallet().await;
    let wallet_sk = fixture.wallet_sk.clone();
    let base_url = fixture.base_url();
    let anima = RemoteAnima::new(base_url, ALICE, fake_cap()).await.unwrap();

    let tx = TxRequest {
        from: anima.wallet_address().unwrap().address.clone(),
        to: "0x036CbD53842c5426634e7929541eC2318f3dCF7e".into(),
        value_wei: "1000000000000000000".into(), // 1 ETH
        data_hex: "".into(),
        nonce: 7,
        gas_limit: 21_000,
        max_fee_per_gas_wei: "30000000000".into(),
        max_priority_fee_per_gas_wei: "1000000000".into(),
        chain: "eip155:8453".into(),
    };
    let tx_clone = tx.clone();

    let sig = tokio::task::spawn_blocking(move || anima.sign_evm_tx(&tx_clone))
        .await
        .unwrap()
        .expect("sign_evm_tx must succeed");
    let bytes = sig.bytes;
    assert_eq!(bytes.len(), 65, "EVM signature must be r||s||v (65 bytes)");
    let v = bytes[64];
    assert!(
        v == 27 || v == 28,
        "legacy v must be 27 or 28, got {v}"
    );

    // Recover the pubkey from the signed digest and confirm it matches
    // the fixture wallet pubkey.
    let envelope = anima_identity::rlp::encode_eip1559_unsigned(
        8453,
        tx.nonce,
        &anima_identity::rlp::parse_u256_str(&tx.max_priority_fee_per_gas_wei).unwrap(),
        &anima_identity::rlp::parse_u256_str(&tx.max_fee_per_gas_wei).unwrap(),
        tx.gas_limit,
        &anima_identity::rlp::parse_address_20(&tx.to).unwrap(),
        &anima_identity::rlp::parse_u256_str(&tx.value_wei).unwrap(),
        &anima_identity::rlp::parse_data_hex(&tx.data_hex).unwrap(),
    );
    let digest = anima_identity::rlp::keccak256(&envelope);
    let r_s: [u8; 64] = bytes[..64].try_into().unwrap();
    let signature = K256Signature::from_slice(&r_s).unwrap();
    let recid = RecoveryId::try_from(v - 27).unwrap();
    let recovered = K256VerifyingKey::recover_from_prehash(&digest, &signature, recid).unwrap();

    let expected = K256VerifyingKey::from(&K256SigningKey::from(wallet_sk));
    assert_eq!(recovered, expected, "recovered pubkey must match wallet");
}

/// `rotate()` returns the documented journal-driven error and does
/// NOT touch the network (no mock for `/anima/custody/rotate`, so a
/// network call would fail differently).
#[tokio::test(flavor = "multi_thread")]
async fn rotate_returns_helpful_error() {
    let fixture = RemoteFixture::build().await;
    let anima = RemoteAnima::new(fixture.base_url(), ALICE, fake_cap())
        .await
        .unwrap();
    let err = match anima.rotate() {
        Ok(_) => panic!("rotate must error"),
        Err(e) => e,
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("journal-driven"),
        "error must say journal-driven: {msg}"
    );
    assert!(
        msg.contains("write_rotation_event"),
        "error must point at helper: {msg}"
    );
    assert!(
        msg.contains("anima.identity_rotated"),
        "error must reference event kind: {msg}"
    );
}
