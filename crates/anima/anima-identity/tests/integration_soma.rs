//! Spec D D-Sub-E — `SomaCustody` integration tests.
//!
//! Spins up an in-process tonic server on a tempdir UDS that mocks
//! soma's `life.admin.kernel.v1.CustodyOracle`. The fake server uses
//! deterministic test keys so we can verify:
//!
//! 1. Construction: `SomaCustody::new` fetches both pubkeys + derives
//!    DID + wallet address.
//! 2. JWS minting: `sign_jws` produces a 3-part token with
//!    `alg=ES256` + the kid from construction.
//! 3. Digest signing: `sign_digest` returns a 64-byte raw r||s.
//! 4. EVM tx signing: `sign_evm_tx` produces a 65-byte r||s||v with
//!    a legacy v in {27, 28}.
//! 5. `rotate()` returns the documented "must go through anima-lago"
//!    error rather than silently misrouting the call.

#![cfg(feature = "kms-soma")]

use std::time::Duration;

use anima_identity::custody::{AnimaCustody, BackendKind};
use anima_identity::soma::SomaCustody;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use tempfile::TempDir;
use tokio::net::UnixListener;

use life_kernel_proto::custody as oracle_pb;
use oracle_pb::custody_oracle_server::{CustodyOracle, CustodyOracleServer};
use tokio_stream::wrappers::UnixListenerStream;

// ── Test keys (deterministic) ──────────────────────────────────────

const ALICE_AUTH_SCALAR: [u8; 32] = [7u8; 32];
const ALICE_WALLET_SCALAR: [u8; 32] = [11u8; 32];
const ALICE: &str = "alice";

// ── Mock soma admin server ─────────────────────────────────────────

#[derive(Clone)]
struct MockSomaOracle {
    user_id: String,
    auth_scalar: [u8; 32],
    wallet_scalar: [u8; 32],
}

#[tonic::async_trait]
impl CustodyOracle for MockSomaOracle {
    async fn sign_auth(
        &self,
        request: tonic::Request<oracle_pb::SignAuthRequest>,
    ) -> Result<tonic::Response<oracle_pb::SignAuthResponse>, tonic::Status> {
        let inner = request.into_inner();
        if inner.user_id != self.user_id {
            return Err(tonic::Status::not_found("unknown user"));
        }
        if inner.digest.len() != 32 {
            return Err(tonic::Status::invalid_argument("digest must be 32 bytes"));
        }
        let mut digest_arr = [0u8; 32];
        digest_arr.copy_from_slice(&inner.digest);

        use p256::ecdsa::signature::hazmat::PrehashSigner;
        use p256::ecdsa::{Signature, SigningKey};
        let signing_key = SigningKey::from_bytes(&self.auth_scalar.into()).unwrap();
        let signature: Signature = signing_key.sign_prehash(&digest_arr).unwrap();
        let bytes = signature.to_bytes();
        Ok(tonic::Response::new(oracle_pb::SignAuthResponse {
            signature_raw: bytes.to_vec(),
        }))
    }

    async fn sign_wallet(
        &self,
        request: tonic::Request<oracle_pb::SignWalletRequest>,
    ) -> Result<tonic::Response<oracle_pb::SignWalletResponse>, tonic::Status> {
        let inner = request.into_inner();
        if inner.user_id != self.user_id {
            return Err(tonic::Status::not_found("unknown user"));
        }
        if inner.digest.len() != 32 {
            return Err(tonic::Status::invalid_argument("digest must be 32 bytes"));
        }
        let mut digest_arr = [0u8; 32];
        digest_arr.copy_from_slice(&inner.digest);

        use k256::ecdsa::{RecoveryId, Signature, SigningKey};
        let signing_key = SigningKey::from_bytes(&self.wallet_scalar.into()).unwrap();
        let (sig, recid): (Signature, RecoveryId) =
            signing_key.sign_prehash_recoverable(&digest_arr).unwrap();
        let mut out = vec![0u8; 65];
        out[..64].copy_from_slice(sig.to_bytes().as_slice());
        out[64] = recid.to_byte() + 27;
        Ok(tonic::Response::new(oracle_pb::SignWalletResponse {
            signature_rsv: out,
        }))
    }

    async fn get_auth_pubkey(
        &self,
        request: tonic::Request<oracle_pb::GetAuthPubkeyRequest>,
    ) -> Result<tonic::Response<oracle_pb::GetAuthPubkeyResponse>, tonic::Status> {
        use p256::ecdsa::SigningKey;
        let inner = request.into_inner();
        if inner.user_id != self.user_id {
            return Err(tonic::Status::not_found("unknown user"));
        }
        let signing_key = SigningKey::from_bytes(&self.auth_scalar.into()).unwrap();
        let verifying = signing_key.verifying_key();
        let point = verifying.to_encoded_point(true);
        Ok(tonic::Response::new(oracle_pb::GetAuthPubkeyResponse {
            pubkey_sec1_compressed: point.as_bytes().to_vec(),
        }))
    }

    async fn get_wallet_pubkey(
        &self,
        request: tonic::Request<oracle_pb::GetWalletPubkeyRequest>,
    ) -> Result<tonic::Response<oracle_pb::GetWalletPubkeyResponse>, tonic::Status> {
        use k256::ecdsa::SigningKey;
        let inner = request.into_inner();
        if inner.user_id != self.user_id {
            return Err(tonic::Status::not_found("unknown user"));
        }
        let signing_key = SigningKey::from_bytes(&self.wallet_scalar.into()).unwrap();
        let verifying = signing_key.verifying_key();
        let point = verifying.to_encoded_point(false);
        Ok(tonic::Response::new(oracle_pb::GetWalletPubkeyResponse {
            pubkey_sec1_uncompressed: point.as_bytes().to_vec(),
        }))
    }
}

struct MockServer {
    _temp: TempDir,
    socket_path: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl MockServer {
    async fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("soma-admin.sock");
        let socket_path_str = socket_path.to_string_lossy().to_string();
        let listener = UnixListener::bind(&socket_path).unwrap();
        let stream = UnixListenerStream::new(listener);
        let oracle = MockSomaOracle {
            user_id: ALICE.into(),
            auth_scalar: ALICE_AUTH_SCALAR,
            wallet_scalar: ALICE_WALLET_SCALAR,
        };
        let server = CustodyOracleServer::new(oracle);

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(server)
                .serve_with_incoming_shutdown(stream, async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        // Tiny grace period to ensure the server is accepting before
        // SomaCustody connects.
        tokio::time::sleep(Duration::from_millis(50)).await;

        Self {
            _temp: temp,
            socket_path: socket_path_str,
            shutdown: Some(shutdown_tx),
            handle: Some(handle),
        }
    }

    fn socket_path(&self) -> &str {
        &self.socket_path
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn soma_custody_construction_resolves_pubkeys() {
    let server = MockServer::start().await;
    let custody = SomaCustody::new(server.socket_path(), ALICE, "alice-kid")
        .await
        .expect("construct against mock soma");
    // DID should be a P-256 did:key (`did:key:zDn…`).
    assert!(custody.user_did().starts_with("did:key:zDn"));
    assert_eq!(custody.auth_pubkey().len(), 33);
    let wallet = custody.wallet_address().unwrap();
    assert!(wallet.address.starts_with("0x"));
    assert_eq!(wallet.address.len(), 42);
    assert_eq!(custody.backend_kind(), BackendKind::Soma);
}

#[tokio::test(flavor = "multi_thread")]
async fn soma_custody_sign_jws_emits_three_part_token() {
    let server = MockServer::start().await;
    let custody = SomaCustody::new(server.socket_path(), ALICE, "alice-kid")
        .await
        .expect("construct");
    let claims = serde_json::json!({"sub": "user1", "iss": "lago", "exp": 9999999999u64});
    let jws = custody.sign_jws(&claims).expect("sign jws");
    let parts: Vec<&str> = jws.split('.').collect();
    assert_eq!(parts.len(), 3, "JWS has 3 parts");

    // Header carries alg=ES256 + kid=alice-kid.
    let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).unwrap();
    let header: serde_json::Value = serde_json::from_slice(&header_bytes).unwrap();
    assert_eq!(header["alg"], "ES256");
    assert_eq!(header["kid"], "alice-kid");

    // Signature decodes to 64 bytes.
    let sig_bytes = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
    assert_eq!(sig_bytes.len(), 64);
}

#[tokio::test(flavor = "multi_thread")]
async fn soma_custody_sign_digest_returns_64_bytes() {
    let server = MockServer::start().await;
    let custody = SomaCustody::new(server.socket_path(), ALICE, "alice-kid")
        .await
        .unwrap();
    let digest = [42u8; 32];
    let sig = custody.sign_digest(&digest).expect("sign digest");
    assert_eq!(sig.len(), 64);

    // The signature should verify against the cached auth pubkey
    // (since the mock signs with the deterministic ALICE_AUTH_SCALAR).
    use p256::PublicKey;
    use p256::ecdsa::signature::hazmat::PrehashVerifier;
    use p256::ecdsa::{Signature, VerifyingKey};
    let signature = Signature::from_slice(&sig).unwrap();
    let pk = PublicKey::from_sec1_bytes(&custody.auth_pubkey()).unwrap();
    let verifying = VerifyingKey::from(&pk);
    verifying.verify_prehash(&digest, &signature).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn soma_custody_sign_evm_tx_returns_65_bytes_with_v() {
    use anima_identity::custody::TxRequest;
    let server = MockServer::start().await;
    let custody = SomaCustody::new(server.socket_path(), ALICE, "alice-kid")
        .await
        .unwrap();
    let wallet_addr = custody.wallet_address().unwrap().address.clone();
    let tx = TxRequest {
        from: wallet_addr,
        to: "0x036CbD53842c5426634e7929541eC2318f3dCF7e".into(),
        value_wei: "1000000000000000".into(), // 1e15 wei
        data_hex: String::new(),
        nonce: 7,
        gas_limit: 21_000,
        max_fee_per_gas_wei: "1500000000".into(),
        max_priority_fee_per_gas_wei: "100000000".into(),
        chain: "eip155:8453".into(),
    };
    let sig = custody.sign_evm_tx(&tx).expect("sign evm tx");
    assert_eq!(sig.bytes.len(), 65);
    let v = sig.bytes[64];
    assert!(v == 27 || v == 28, "v must be 27 or 28 (legacy form)");
}

#[tokio::test(flavor = "multi_thread")]
async fn soma_custody_rotate_returns_helpful_error() {
    let server = MockServer::start().await;
    let custody = SomaCustody::new(server.socket_path(), ALICE, "alice-kid")
        .await
        .unwrap();
    let outcome = custody.rotate();
    let err = match outcome {
        Ok(_) => panic!("rotate must fail with a journal-helper redirect"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("anima-lago write_rotation_event"),
        "rotate() error must point at the journal helper, got: {msg}"
    );
}
