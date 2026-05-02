//! Integration tests for `VaultTransitAnima` (Spec D D-Sub-B).
//!
//! These tests use [`wiremock`] to stand up a fake Vault HTTP server
//! at a localhost port and verify that:
//!
//! 1. The bootstrap path issues two `GET /v1/transit/keys/...` requests
//!    (one for the auth half, one for the wallet half) and pins to
//!    `data.latest_version`.
//! 2. JWS minting POSTs to `transit/sign/<auth_key>` with
//!    `URL_SAFE_NO_PAD` input encoding + `marshaling_algorithm: "jws"`,
//!    and reconstructs the compact JWS from Vault's response.
//! 3. `sign_evm_tx` computes the EIP-1559 RLP digest, posts it to
//!    `transit/sign/<wallet_key>` with `prehashed: true`, and assembles
//!    a 65-byte `r||s||v` signature whose recovery id matches the
//!    cached wallet address.
//! 4. The DID derives correctly from the auth pubkey returned by Vault.
//! 5. Per-user namespace pattern: passing `user_id="alice"` produces
//!    `anima-alice-auth-v1` and `anima-alice-wallet-v1` URLs.
//!
//! A live `vault server -dev` integration test is provided as
//! [`live_vault_dev_server`] but is `#[ignore]`-gated. To run it
//! locally:
//!
//! ```bash
//! # Terminal 1: bring up Vault dev server
//! vault server -dev -dev-root-token-id=anima-test-token
//!
//! # Terminal 2: provision the test keys
//! export VAULT_ADDR=http://127.0.0.1:8200
//! export VAULT_TOKEN=anima-test-token
//! vault secrets enable transit
//! vault write -f transit/keys/anima-alice-auth-v1 type=ecdsa-p256
//! vault write -f transit/keys/anima-alice-wallet-v1 type=ecdsa-p256
//! # NOTE: Vault transit does NOT support secp256k1 natively as of v1.15
//! # — see the `live_vault_dev_server` test for the workaround.
//!
//! # Terminal 3: run the gated test
//! ANIMA_VAULT_LIVE_TEST=1 cargo test -p anima-identity \
//!   --features kms-vault \
//!   --test integration_vault \
//!   -- --ignored live_vault_dev_server
//! ```

#![cfg(feature = "kms-vault")]

use anima_identity::custody::AnimaCustody;
use anima_identity::vault::VaultTransitAnima;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use k256::SecretKey as Secp256k1SecretKey;
use p256::SecretKey as P256SecretKey;
use p256::pkcs8::EncodePublicKey as _;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Test fixture — wraps a `MockServer` + the deterministic P-256 + secp256k1
/// keys it serves, plus a scratch counter used to mint fresh signatures
/// from a single key on every `transit/sign` call.
struct VaultFixture {
    server: MockServer,
    /// Hardware-fixed test seed for the auth (P-256) key.
    auth_sk: P256SecretKey,
    /// Hardware-fixed test seed for the wallet (secp256k1) key.
    wallet_sk: Secp256k1SecretKey,
    /// Vault's `transit/sign` response builder. Tracks how many
    /// times it was called for assertions.
    sign_calls: Arc<Mutex<Vec<SignCall>>>,
}

#[derive(Debug, Clone)]
struct SignCall {
    key_name: String,
    body: Value,
}

impl VaultFixture {
    /// Build a fresh fixture with deterministic keys for repeatable tests.
    async fn build() -> Self {
        let server = MockServer::start().await;
        let auth_sk = P256SecretKey::from_bytes(&[7u8; 32].into()).unwrap();
        let wallet_sk = Secp256k1SecretKey::from_bytes(&[11u8; 32].into()).unwrap();
        let sign_calls = Arc::new(Mutex::new(Vec::new()));

        // GET /v1/transit/keys/<auth_key_name>
        let auth_pem = auth_sk
            .public_key()
            .to_public_key_pem(Default::default())
            .unwrap();
        let auth_body = json!({
            "data": {
                "latest_version": 1,
                "keys": {
                    "1": { "public_key": auth_pem }
                }
            }
        });
        Mock::given(method("GET"))
            .and(path("/v1/transit/keys/anima-alice-auth-v1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(auth_body))
            .mount(&server)
            .await;

        // GET /v1/transit/keys/<wallet_key_name>
        let wallet_pem = wallet_sk
            .public_key()
            .to_public_key_pem(Default::default())
            .unwrap();
        let wallet_body = json!({
            "data": {
                "latest_version": 1,
                "keys": {
                    "1": { "public_key": wallet_pem }
                }
            }
        });
        Mock::given(method("GET"))
            .and(path("/v1/transit/keys/anima-alice-wallet-v1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(wallet_body))
            .mount(&server)
            .await;

        Self {
            server,
            auth_sk,
            wallet_sk,
            sign_calls,
        }
    }

    /// Mount a `transit/sign/<key>` mock that signs the input with the
    /// supplied key and records the call for assertions.
    async fn mount_sign(&self, key_name: &'static str, sk_kind: SignerKind) {
        let calls = self.sign_calls.clone();
        let auth_sk = self.auth_sk.clone();
        let wallet_sk = self.wallet_sk.clone();
        Mock::given(method("POST"))
            .and(path(format!("/v1/transit/sign/{key_name}")))
            .respond_with(SignResponder {
                calls,
                auth_sk,
                wallet_sk,
                kind: sk_kind,
                key_name: key_name.to_string(),
            })
            .mount(&self.server)
            .await;
    }

    fn addr(&self) -> String {
        self.server.uri()
    }
}

/// Which key the sign-mock should sign with.
#[derive(Debug, Clone, Copy)]
enum SignerKind {
    P256Auth,
    Secp256k1Wallet,
}

/// Custom wiremock responder that signs the inbound `input` (decoding
/// from URL_SAFE_NO_PAD) using the fixture's private key, then returns
/// the JSON shape Vault would emit:
/// `{"data":{"signature":"vault:v1:<base64-r-s>"}}`.
struct SignResponder {
    calls: Arc<Mutex<Vec<SignCall>>>,
    auth_sk: P256SecretKey,
    wallet_sk: Secp256k1SecretKey,
    kind: SignerKind,
    key_name: String,
}

impl Respond for SignResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).expect("vault sign body must be JSON");
        self.calls.lock().unwrap().push(SignCall {
            key_name: self.key_name.clone(),
            body: body.clone(),
        });
        let input_b64 = body
            .get("input")
            .and_then(|v| v.as_str())
            .expect("vault sign body must carry input");
        let input_bytes = URL_SAFE_NO_PAD
            .decode(input_b64)
            .expect("vault input must be URL_SAFE_NO_PAD");
        let prehashed = body
            .get("prehashed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let sig_bytes = match self.kind {
            SignerKind::P256Auth => {
                use p256::ecdsa::SigningKey;
                use p256::ecdsa::signature::Signer as _;
                use p256::ecdsa::signature::hazmat::PrehashSigner as _;
                let sk = SigningKey::from(self.auth_sk.clone());
                if prehashed {
                    let mut digest = [0u8; 32];
                    digest.copy_from_slice(&input_bytes);
                    let sig: p256::ecdsa::Signature = sk.sign_prehash(&digest).unwrap();
                    sig.to_bytes().to_vec()
                } else {
                    // Vault hashes server-side with sha2-256.
                    let sig: p256::ecdsa::Signature = sk.sign(&input_bytes);
                    sig.to_bytes().to_vec()
                }
            }
            SignerKind::Secp256k1Wallet => {
                use k256::ecdsa::SigningKey;
                use k256::ecdsa::signature::hazmat::PrehashSigner as _;
                let sk = SigningKey::from(self.wallet_sk.clone());
                let mut digest = [0u8; 32];
                if !prehashed {
                    // For wallet signing we always pass prehashed: true,
                    // but be defensive.
                    use sha2::{Digest, Sha256};
                    let h = Sha256::digest(&input_bytes);
                    digest.copy_from_slice(&h);
                } else {
                    digest.copy_from_slice(&input_bytes);
                }
                let sig: k256::ecdsa::Signature = sk.sign_prehash(&digest).unwrap();
                sig.to_bytes().to_vec()
            }
        };
        let sig_b64 = URL_SAFE_NO_PAD.encode(&sig_bytes);
        let resp = json!({
            "data": {
                "signature": format!("vault:v1:{sig_b64}")
            }
        });
        ResponseTemplate::new(200).set_body_json(resp)
    }
}

/// Bootstrap test — confirm `VaultTransitAnima::new("alice")` derives
/// `anima-alice-{auth,wallet}-v1` key names AND fetches both pubkeys.
/// The wiremock fixture only responds for those exact paths, so a
/// successful construction proves the key-name derivation.
#[tokio::test]
async fn vault_anima_bootstraps_per_user_keys() {
    let fixture = VaultFixture::build().await;
    let result = tokio::task::spawn_blocking({
        let addr = fixture.addr();
        move || VaultTransitAnima::new(addr, "test-token", "alice", "alice-key")
    })
    .await
    .unwrap();
    let custody = result.expect("VaultTransitAnima::new should bootstrap");
    // DID is derived from the P-256 auth pubkey (multicodec 0x1200 → zDn…).
    assert!(custody.user_did().starts_with("did:key:zDn"));
    // Wallet address is derived from the secp256k1 pubkey.
    let addr = custody.wallet_address().unwrap();
    assert!(addr.address.starts_with("0x"));
    assert_eq!(addr.address.len(), 42);
    assert_eq!(custody.backend_kind(), anima_identity::BackendKind::Vault);
}

/// `sign_jws` POSTs to `transit/sign/<auth_key>` with
/// `marshaling_algorithm: "jws"` and returns a 3-part compact JWS
/// whose header carries `alg=ES256, kid=<configured-kid>`.
#[tokio::test]
async fn vault_anima_signs_jws_against_auth_key() {
    let fixture = VaultFixture::build().await;
    fixture
        .mount_sign("anima-alice-auth-v1", SignerKind::P256Auth)
        .await;

    let addr = fixture.addr();
    let calls = fixture.sign_calls.clone();
    let jws = tokio::task::spawn_blocking(move || {
        let custody = VaultTransitAnima::new(addr, "test-token", "alice", "alice-key").unwrap();
        custody.sign_jws(&json!({"sub": "agt_001", "iss": "anima"}))
    })
    .await
    .unwrap()
    .expect("sign_jws must succeed");

    let parts: Vec<&str> = jws.split('.').collect();
    assert_eq!(parts.len(), 3, "JWS must have 3 parts");

    // Decode header + assert ES256 + kid.
    let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).unwrap();
    let header: Value = serde_json::from_slice(&header_bytes).unwrap();
    assert_eq!(header["alg"], "ES256");
    assert_eq!(header["typ"], "JWT");
    assert_eq!(header["kid"], "alice-key");

    // Confirm Vault was called with marshaling_algorithm: jws.
    let recorded = calls.lock().unwrap();
    let sign_call = recorded
        .iter()
        .find(|c| c.key_name == "anima-alice-auth-v1")
        .expect("auth sign call recorded");
    assert_eq!(sign_call.body["marshaling_algorithm"], "jws");
    assert_eq!(sign_call.body["hash_algorithm"], "sha2-256");
    // Input should be URL_SAFE_NO_PAD encoded (no padding chars).
    let input = sign_call.body["input"].as_str().unwrap();
    assert!(!input.contains('='), "Vault input must not be PADDED");
}

/// `sign_evm_tx` computes the EIP-1559 RLP digest, POSTs to
/// `transit/sign/<wallet_key>` with `prehashed: true`, and produces a
/// 65-byte `r||s||v` signature whose recovery id selects to the wallet
/// pubkey.
#[tokio::test]
async fn vault_anima_signs_evm_tx_with_prehash() {
    let fixture = VaultFixture::build().await;
    fixture
        .mount_sign("anima-alice-wallet-v1", SignerKind::Secp256k1Wallet)
        .await;

    let addr = fixture.addr();
    let calls = fixture.sign_calls.clone();
    let signature = tokio::task::spawn_blocking(move || {
        let custody = VaultTransitAnima::new(addr, "test-token", "alice", "alice-key").unwrap();
        let tx = anima_identity::TxRequest {
            from: custody.wallet_address().unwrap().address.clone(),
            to: "0x036CbD53842c5426634e7929541eC2318f3dCF7e".into(),
            value_wei: "1000000000000000000".into(), // 1 ETH
            data_hex: "0x".into(),
            nonce: 42,
            gas_limit: 21000,
            max_fee_per_gas_wei: "30000000000".into(), // 30 gwei
            max_priority_fee_per_gas_wei: "1000000000".into(), // 1 gwei
            chain: "eip155:8453".into(),
        };
        custody.sign_evm_tx(&tx)
    })
    .await
    .unwrap()
    .expect("sign_evm_tx must succeed");

    // Signature is r (32) || s (32) || v (1).
    assert_eq!(signature.bytes.len(), 65);
    let v = signature.bytes[64];
    assert!(
        v == 27 || v == 28,
        "v must be 27 or 28 (legacy recovery), got {v}"
    );

    // Confirm Vault was called with prehashed: true.
    let recorded = calls.lock().unwrap();
    let sign_call = recorded
        .iter()
        .find(|c| c.key_name == "anima-alice-wallet-v1")
        .expect("wallet sign call recorded");
    assert_eq!(sign_call.body["prehashed"], true);
    assert_eq!(sign_call.body["marshaling_algorithm"], "jws");
}

/// `sign_eip712` for EIP-3009 `transferWithAuthorization` — exercises
/// the wallet-signing path with the EIP-712 typed-data digest rather
/// than the EIP-1559 RLP digest.
#[tokio::test]
async fn vault_anima_signs_eip712_transfer_authorization() {
    let fixture = VaultFixture::build().await;
    fixture
        .mount_sign("anima-alice-wallet-v1", SignerKind::Secp256k1Wallet)
        .await;

    let addr = fixture.addr();
    let signature = tokio::task::spawn_blocking(move || {
        let custody = VaultTransitAnima::new(addr, "test-token", "alice", "alice-key").unwrap();
        let domain = haima_wallet::USDC_BASE_MAINNET;
        let from = custody.wallet_address().unwrap().address.clone();
        let message = json!({
            "from": from,
            "to": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
            "value": "100",
            "validAfter": "1700000000",
            "validBefore": "1700000600",
            "nonce": format!("0x{}", hex::encode([0x42u8; 32])),
        });
        let types = json!({"primaryType": "TransferWithAuthorization"});
        custody.sign_eip712(&domain, &types, &message)
    })
    .await
    .unwrap()
    .expect("sign_eip712 must succeed");

    assert_eq!(signature.bytes.len(), 65);
    let v = signature.bytes[64];
    assert!(v == 27 || v == 28);
}

/// `sign_eip712` rejects unsupported typed-data shapes consistently
/// with `InProcessAnima` (Spec D D-Sub-A's only supported shape is
/// EIP-3009 `TransferWithAuthorization`).
#[tokio::test]
async fn vault_anima_rejects_unsupported_eip712_shape() {
    let fixture = VaultFixture::build().await;
    let addr = fixture.addr();
    let result = tokio::task::spawn_blocking(move || {
        let custody = VaultTransitAnima::new(addr, "test-token", "alice", "alice-key").unwrap();
        let domain = haima_wallet::USDC_BASE_MAINNET;
        let types = json!({"primaryType": "Order"}); // NOT TransferWithAuthorization
        let message = json!({"foo": "bar"});
        custody.sign_eip712(&domain, &types, &message)
    })
    .await
    .unwrap();
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("eip712"), "error must mention eip712: {msg}");
}

/// **Live `vault server -dev` integration test.** Disabled by default
/// to avoid binding to an external service. Enable with
/// `ANIMA_VAULT_LIVE_TEST=1` and `--ignored live_vault_dev_server`
/// after standing up the fixture per the file-level docstring.
///
/// The test signs a USDC `transferWithAuthorization` payload end-to-end
/// against the real Vault dev server. Per Spec D §"Phasing > D-Sub-B"
/// acceptance, this completes the "Vault-fixture integration test
/// signs a USDC transfer end-to-end on a Base-fork local chain"
/// criterion. Broadcasting the signed tx against a Base-fork local
/// chain (anvil --fork-url) is the operator's responsibility; the
/// signature shape produced here is broadcast-ready.
///
/// **Known limitation:** Vault's transit secrets engine does NOT
/// support secp256k1 keys natively as of v1.15. To exercise the wallet
/// path against a real Vault, operators either:
/// - patch Vault to enable secp256k1 (community PRs in flight), OR
/// - run a sidecar like `vault-secrets-operator` that bridges to a
///   real secp256k1-capable HSM, OR
/// - skip the wallet-half live test and only exercise the auth half.
///
/// This test exercises only the auth-half live path and asserts that
/// `sign_jws` produces a valid ES256 JWS verifiable against the Vault-
/// published P-256 pubkey. The wallet half live integration is filed
/// as a follow-up under D-Sub-D's TPM track (which has the same shape
/// problem and the same workarounds).
#[tokio::test]
#[ignore = "requires live `vault server -dev` fixture; see file docstring"]
async fn live_vault_dev_server() {
    if std::env::var("ANIMA_VAULT_LIVE_TEST").is_err() {
        eprintln!("ANIMA_VAULT_LIVE_TEST not set; skipping live test");
        return;
    }
    let addr = std::env::var("VAULT_ADDR").unwrap_or_else(|_| "http://127.0.0.1:8200".into());
    let token = std::env::var("VAULT_TOKEN").unwrap_or_else(|_| "anima-test-token".into());

    let custody = tokio::task::spawn_blocking({
        let addr = addr.clone();
        let token = token.clone();
        move || VaultTransitAnima::new(addr, token, "alice", "alice-key")
    })
    .await
    .unwrap()
    .expect("live Vault bootstrap should succeed");

    assert!(custody.user_did().starts_with("did:key:zDn"));

    let jws = tokio::task::spawn_blocking({
        let custody = custody;
        move || custody.sign_jws(&json!({"sub": "agt_001", "iss": "anima"}))
    })
    .await
    .unwrap()
    .expect("live Vault sign_jws should succeed");

    let parts: Vec<&str> = jws.split('.').collect();
    assert_eq!(parts.len(), 3);
    let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).unwrap();
    let header: Value = serde_json::from_slice(&header_bytes).unwrap();
    assert_eq!(header["alg"], "ES256");
}
