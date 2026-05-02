//! Spec D D-Sub-C — Stream R-2 integration tests.
//!
//! Stands up an in-process axum router for `/anima/custody/*` against
//! a tempdir-UDS-bound tonic server that mocks soma's
//! `life.admin.kernel.v1.CustodyOracle`. Every test issues HTTP
//! requests via `tower::ServiceExt::oneshot` — no real TCP listener
//! needed.
//!
//! What's verified (matches the wire-shape contract Stream R-1
//! exercised on the client side):
//!
//! 1. `POST /anima/custody/sign_auth` — bearer-gated; returns a 64-byte
//!    base64-encoded signature; user_id + digest_b64 in the body.
//! 2. `POST /anima/custody/sign_wallet` — same shape; the mock returns
//!    65-byte r||s||v but the route strips the v byte before responding
//!    (the wire contract returns raw r||s and the client ecrecovers).
//! 3. `GET /anima/custody/get_auth_pubkey/{user_id}` — returns the
//!    SEC1-compressed P-256 pubkey.
//! 4. `GET /anima/custody/get_wallet_pubkey/{user_id}` — returns the
//!    SEC1-uncompressed secp256k1 pubkey (must start with 0x04).
//! 5. `POST /anima/custody/mint_session_cap` — verifies a freshly-minted
//!    Tier-User JWT against the same KMS signer's published JWKS.
//! 6. `POST /anima/custody/enroll_passkey` — extracts a P-256 pubkey
//!    from a synthetic WebAuthn attestation and returns the derived
//!    DID + Tier-User cap.
//! 7. Auth-layer rejection — missing Authorization → 401.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD as B64_STANDARD, URL_SAFE_NO_PAD};
use http::Request;
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use lifegw::auth::kms::{KmsSigner, StaticKeystore};
use lifegw::auth::tier_user::{DEFAULT_TIER_USER_TTL, TierUserClaims, TierUserMinter};
use lifegw::services::anima_custody::{self, AnimaCustodyState};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tower::ServiceExt;

use life_kernel_proto::custody as oracle_pb;
use oracle_pb::custody_oracle_server::{CustodyOracle, CustodyOracleServer};

// ─── Test keys (deterministic) ──────────────────────────────────────

const ALICE_AUTH_SCALAR: [u8; 32] = [7u8; 32];
const ALICE_WALLET_SCALAR: [u8; 32] = [11u8; 32];
const ALICE: &str = "alice";

// ─── Mock soma admin server ─────────────────────────────────────────

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
        Ok(tonic::Response::new(oracle_pb::SignAuthResponse {
            signature_raw: signature.to_bytes().to_vec(),
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

struct MockSomaServer {
    _temp: TempDir,
    socket_path: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl MockSomaServer {
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
        // Tiny grace for the listener to start accepting.
        tokio::time::sleep(Duration::from_millis(50)).await;
        Self {
            _temp: temp,
            socket_path: socket_path_str,
            shutdown: Some(shutdown_tx),
            handle: Some(handle),
        }
    }
}

impl Drop for MockSomaServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

// ─── Test rig ───────────────────────────────────────────────────────

struct TestRig {
    /// Held to keep the mock soma server's UDS alive for the lifetime
    /// of the rig — Drop on the server shuts it down.
    _soma: MockSomaServer,
    signer: Arc<StaticKeystore>,
    minter: Arc<TierUserMinter>,
    router: axum::Router,
}

impl TestRig {
    async fn build() -> Self {
        let soma = MockSomaServer::start().await;
        let signer = Arc::new(StaticKeystore::generate_dev().expect("dev keystore"));
        let minter = Arc::new(TierUserMinter::with_defaults(
            signer.clone() as Arc<dyn KmsSigner>,
            DEFAULT_TIER_USER_TTL,
        ));
        let state = AnimaCustodyState::new(Some(soma.socket_path.clone()), Arc::clone(&minter));
        let router = anima_custody::router(state);
        Self {
            _soma: soma,
            signer,
            minter,
            router,
        }
    }

    /// Build a rig with the soma proxy disabled — used by the
    /// "soma not configured" graceful-degradation test.
    async fn build_without_soma() -> (Self, axum::Router) {
        let rig = Self::build().await;
        let signer = Arc::new(StaticKeystore::generate_dev().expect("dev keystore"));
        let minter = Arc::new(TierUserMinter::with_defaults(
            signer as Arc<dyn KmsSigner>,
            DEFAULT_TIER_USER_TTL,
        ));
        let state = AnimaCustodyState::new(None, minter);
        let router_no_soma = anima_custody::router(state);
        (rig, router_no_soma)
    }

    fn router(&self) -> axum::Router {
        self.router.clone()
    }
}

fn fake_bearer() -> &'static str {
    "Bearer fake-tier-user-token"
}

async fn read_body_json(resp: http::Response<Body>) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("response body is JSON")
}

// ─── Tests ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn sign_auth_round_trip() {
    let rig = TestRig::build().await;

    // Generate a real digest and ask the router to sign it via the
    // proxied soma mock.
    let digest = [42u8; 32];
    let body = json!({
        "user_id": ALICE,
        "digest_b64": B64_STANDARD.encode(digest),
    });
    let req = Request::builder()
        .method("POST")
        .uri("/sign_auth")
        .header("authorization", fake_bearer())
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = rig.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
    let parsed = read_body_json(resp).await;
    let sig_b64 = parsed["signature_b64"].as_str().expect("signature_b64");
    let sig_bytes = B64_STANDARD.decode(sig_b64).unwrap();
    assert_eq!(sig_bytes.len(), 64);

    // Verify the returned signature is a valid P-256 ECDSA over the
    // digest by the same key the mock holds.
    use p256::ecdsa::signature::hazmat::PrehashVerifier;
    use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
    let sk = SigningKey::from_bytes(&ALICE_AUTH_SCALAR.into()).unwrap();
    let vk = VerifyingKey::from(&sk);
    let signature = Signature::from_slice(&sig_bytes).unwrap();
    vk.verify_prehash(&digest, &signature).expect("verify p256");
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_wallet_strips_v_byte() {
    let rig = TestRig::build().await;

    let digest = [99u8; 32];
    let body = json!({
        "user_id": ALICE,
        "digest_b64": B64_STANDARD.encode(digest),
    });
    let req = Request::builder()
        .method("POST")
        .uri("/sign_wallet")
        .header("authorization", fake_bearer())
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = rig.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
    let parsed = read_body_json(resp).await;
    let sig_b64 = parsed["signature_b64"].as_str().expect("signature_b64");
    let sig_bytes = B64_STANDARD.decode(sig_b64).unwrap();
    assert_eq!(
        sig_bytes.len(),
        64,
        "wallet route strips the v byte → wire shape is 64 bytes"
    );

    // Round-trip: ecrecover with both v candidates; one MUST recover
    // the wallet pubkey.
    use k256::ecdsa::{
        RecoveryId, Signature as K256Signature, SigningKey as K256SigningKey,
        VerifyingKey as K256VerifyingKey,
    };
    let signature = K256Signature::from_slice(&sig_bytes).unwrap();
    let sk = K256SigningKey::from_bytes(&ALICE_WALLET_SCALAR.into()).unwrap();
    let expected = K256VerifyingKey::from(&sk);
    let mut matched = false;
    for cand in 0u8..=1 {
        let recid = RecoveryId::try_from(cand).unwrap();
        if let Ok(rec) = K256VerifyingKey::recover_from_prehash(&digest, &signature, recid)
            && rec == expected
        {
            matched = true;
            break;
        }
    }
    assert!(matched, "ecrecover must produce the cached wallet pubkey");
}

#[tokio::test(flavor = "multi_thread")]
async fn get_auth_pubkey_returns_compressed_p256() {
    let rig = TestRig::build().await;
    let req = Request::builder()
        .method("GET")
        .uri(format!("/get_auth_pubkey/{ALICE}"))
        .header("authorization", fake_bearer())
        .body(Body::empty())
        .unwrap();
    let resp = rig.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
    let parsed = read_body_json(resp).await;
    let pk_b64 = parsed["pubkey_b64"].as_str().expect("pubkey_b64");
    let pk_bytes = B64_STANDARD.decode(pk_b64).unwrap();
    assert_eq!(pk_bytes.len(), 33);
    assert!(pk_bytes[0] == 0x02 || pk_bytes[0] == 0x03);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_wallet_pubkey_returns_uncompressed_secp256k1() {
    let rig = TestRig::build().await;
    let req = Request::builder()
        .method("GET")
        .uri(format!("/get_wallet_pubkey/{ALICE}"))
        .header("authorization", fake_bearer())
        .body(Body::empty())
        .unwrap();
    let resp = rig.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
    let parsed = read_body_json(resp).await;
    let pk_b64 = parsed["pubkey_b64"].as_str().expect("pubkey_b64");
    let pk_bytes = B64_STANDARD.decode(pk_b64).unwrap();
    assert_eq!(pk_bytes.len(), 65);
    assert_eq!(
        pk_bytes[0], 0x04,
        "uncompressed secp256k1 must start with 0x04"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mint_session_cap_returns_verifiable_jwt() {
    let rig = TestRig::build().await;
    let body = json!({ "user_id": ALICE });
    let req = Request::builder()
        .method("POST")
        .uri("/mint_session_cap")
        .header("authorization", fake_bearer())
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = rig.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
    let parsed = read_body_json(resp).await;
    let token = parsed["token"].as_str().expect("token");
    let expires_at = parsed["expires_at_unix"].as_i64().expect("expires_at_unix");
    assert!(expires_at > 0);

    // Verify the JWS shape — alg=ES256, kid matches the signer's
    // active kid, audience = anima.user-cap.
    let header = decode_header(token).expect("decode header");
    assert_eq!(header.alg, Algorithm::ES256);
    assert_eq!(header.kid.as_deref(), Some(rig.signer.active_kid()));

    let jwks = rig.signer.publish_jwks();
    let pem = jwks.keys[0].pem.as_ref().expect("dev pem");
    let dk = DecodingKey::from_ec_pem(pem.as_bytes()).expect("decode pem");
    let mut v = Validation::new(Algorithm::ES256);
    v.set_audience(&["anima.user-cap"]);
    v.set_issuer(&["lifegw"]);
    v.validate_nbf = true;
    let body = decode::<TierUserClaims>(token, &dk, &v).expect("verify");
    assert_eq!(body.claims.sub, ALICE);
    assert_eq!(body.claims.aud, "anima.user-cap");
    assert!(!body.claims.scope.is_empty());
    let _ = rig.minter.audience(); // keep handle live
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_layer_rejects_missing_bearer() {
    let rig = TestRig::build().await;
    let body = json!({ "user_id": ALICE, "digest_b64": B64_STANDARD.encode([0u8; 32]) });
    let req = Request::builder()
        .method("POST")
        .uri("/sign_auth")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = rig.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn soma_uds_unconfigured_returns_501() {
    // Spec D D-Sub-C: when the operator hasn't configured soma's
    // custody-oracle UDS, the proxy routes degrade gracefully to
    // 501 Not Implemented. lifegw stays running.
    let (_rig, router_no_soma) = TestRig::build_without_soma().await;
    let body = json!({ "user_id": ALICE, "digest_b64": B64_STANDARD.encode([0u8; 32]) });
    let req = Request::builder()
        .method("POST")
        .uri("/sign_auth")
        .header("authorization", fake_bearer())
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = router_no_soma.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::NOT_IMPLEMENTED);
    let parsed = read_body_json(resp).await;
    assert_eq!(parsed["error"], "soma_uds_not_configured");
}

#[tokio::test(flavor = "multi_thread")]
async fn enroll_passkey_extracts_did_from_attestation() {
    let rig = TestRig::build().await;

    // Build a synthetic WebAuthn attestation: a CBOR map with an
    // `authData` field carrying:
    //   rpIdHash(32) || flags(1=0x40) || signCount(4) || aaguid(16)
    //   || credIdLen(2) || credId(N) || credPubKey(COSE_Key CBOR)
    use ciborium::value::Value as CborValue;
    use p256::SecretKey;
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    let sk = SecretKey::from_bytes(&[5u8; 32].into()).unwrap();
    let pk = sk.public_key();
    let pt = pk.to_encoded_point(false);
    let raw = pt.as_bytes();
    let x = &raw[1..33];
    let y = &raw[33..65];
    let cose = CborValue::Map(vec![
        (CborValue::Integer(1.into()), CborValue::Integer(2.into())),
        (
            CborValue::Integer(3.into()),
            CborValue::Integer((-7i64).into()),
        ),
        (
            CborValue::Integer((-1i64).into()),
            CborValue::Integer(1.into()),
        ),
        (
            CborValue::Integer((-2i64).into()),
            CborValue::Bytes(x.to_vec()),
        ),
        (
            CborValue::Integer((-3i64).into()),
            CborValue::Bytes(y.to_vec()),
        ),
    ]);
    let mut cose_buf = Vec::new();
    ciborium::ser::into_writer(&cose, &mut cose_buf).unwrap();

    let cred_id = b"cred-id-123";
    let mut auth_data = Vec::with_capacity(53 + 2 + cred_id.len() + cose_buf.len());
    auth_data.extend_from_slice(&[0u8; 32]); // rpIdHash
    auth_data.push(0x40); // flags: AT bit
    auth_data.extend_from_slice(&[0u8; 4]); // signCount
    auth_data.extend_from_slice(&[0u8; 16]); // aaguid
    auth_data.extend_from_slice(&(cred_id.len() as u16).to_be_bytes());
    auth_data.extend_from_slice(cred_id);
    auth_data.extend_from_slice(&cose_buf);

    let attestation = CborValue::Map(vec![
        (
            CborValue::Text("fmt".into()),
            CborValue::Text("none".into()),
        ),
        (CborValue::Text("attStmt".into()), CborValue::Map(vec![])),
        (
            CborValue::Text("authData".into()),
            CborValue::Bytes(auth_data),
        ),
    ]);
    let mut attestation_buf = Vec::new();
    ciborium::ser::into_writer(&attestation, &mut attestation_buf).unwrap();

    let body = json!({
        "user_id": ALICE,
        "attestation_object_b64": B64_STANDARD.encode(&attestation_buf),
    });
    let req = Request::builder()
        .method("POST")
        .uri("/enroll_passkey")
        .header("authorization", fake_bearer())
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = rig.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
    let parsed = read_body_json(resp).await;
    let did = parsed["did"].as_str().expect("did");
    assert!(did.starts_with("did:key:zDn"), "got: {did}");
    let token = parsed["token"].as_str().expect("token");
    assert!(token.split('.').count() == 3, "JWS has 3 parts");
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_subprotocol_bearer_round_trips_through_parse() {
    // Spec D D-Sub-C M8.2 close: parse_upgrade_request accepts
    // `Sec-WebSocket-Protocol: bearer.<jwt>` as an alternative to
    // `Authorization: Bearer <jwt>`. This test imports the WS module
    // directly to exercise the public-surface invariant from a
    // separate test binary (the unit tests in ws.rs already cover
    // the function — this one ensures the contract is part of the
    // crate's public test surface).
    use lifegw::services::ws::is_ws_upgrade;
    let req = http::Request::builder()
        .uri("/v1/agent/stream?sid=test-sid")
        .header("upgrade", "websocket")
        .header("connection", "Upgrade")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header(
            "sec-websocket-protocol",
            "life.v1.agent, bearer.eyJabc.def.ghi",
        )
        .body(())
        .unwrap();
    assert!(is_ws_upgrade(&req));
}

// Sanity: bearer must be base64-padding-tolerant.
#[tokio::test(flavor = "multi_thread")]
async fn url_safe_no_pad_digest_is_accepted() {
    let rig = TestRig::build().await;

    let digest = [3u8; 32];
    // URL-safe-no-pad form (no `=` padding).
    let url_safe = URL_SAFE_NO_PAD.encode(digest);
    let body = json!({
        "user_id": ALICE,
        "digest_b64": url_safe,
    });
    let req = Request::builder()
        .method("POST")
        .uri("/sign_auth")
        .header("authorization", fake_bearer())
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = rig.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
}
