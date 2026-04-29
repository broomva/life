//! Sub-phase B integration test — real JWKS verifier round-trip.
//!
//! Brings up:
//! 1. A `wiremock`-backed Vercel-style JWKS server on `127.0.0.1:0`.
//! 2. A `lifed` instance bound to a tempdir UDS, fronted by mock
//!    substrates and configured to verify Tier-2 tokens via lifegw's
//!    published JWKS.
//! 3. A `lifegw` instance bound to `127.0.0.1:0` with a self-signed TLS
//!    cert, configured to:
//!    - fetch its Tier-1 JWKS from the wiremock server,
//!    - verify the audience `lifegw` and issuer `https://broomva.test`
//!      (the wiremock server's URI),
//!    - mint Tier-2 capability tokens via a `StaticKeystore` so lifed
//!      can verify them.
//!
//! Then dials lifegw with a tonic client over rustls trusting the dev
//! cert, presenting a *real* ES256 Tier-1 JWS signed with the private
//! key whose public half lives in the wiremock JWKS document. Asserts:
//! - `Agent.CreateSession` round-trips → lifed's mock arcan observes
//!   one `create_agent` call.
//! - kid rotation: replace the JWKS with a fresh kid, sign a fresh
//!   token, lifegw refetches automatically and verifies.

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use hyper_util::rt::TokioIo;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::json;
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use lifegw::auth::jwks::{JwksDoc, JwksEntry};
use lifegw::auth::keystore::Keystore;
use lifegw::config::LifegwConfig;

use life_runtime_proto::life::v1::CreateSessionReq;
use life_runtime_proto::life::v1::agent_client::AgentClient;

/// Single end-to-end test. The three Sub-phase B integration scenarios
/// run sequentially against ONE shared TestEnv because lifegw installs
/// its Tier-1 verifier into a process-global `OnceLock` at bootstrap —
/// independent `#[tokio::test]` functions in the same binary would
/// race on first install + one would silently lose. Sharing the env
/// also exercises lifegw's "in-place rotation" code-path more
/// realistically: real production deployments swap the upstream JWKS
/// without restarting the gateway.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jwks_round_trip_suite() {
    let mut env = TestEnv::start().await;

    // Sub-suite 1 — happy path: real ES256 Tier-1 token verifies + the
    // gateway proxies CreateSession to lifed's mock arcan.
    {
        let bearer = env.mint_tier1_token("user-real");
        let mut client = env.agent_client().await;
        let mut req = tonic::Request::new(CreateSessionReq {
            user_id: "user-real".to_string(),
            project_id: "demo".to_string(),
            label: "real-jwks-roundtrip".to_string(),
            resume_sid: None,
            inherit_policy: None,
        });
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {bearer}").parse().expect("hv"),
        );
        let session = client
            .create_session(req)
            .await
            .expect("create_session round-trips through real JWKS verify")
            .into_inner();
        assert_eq!(session.user_id, "user-real");
        let arcan_calls = env.mocks.arcan.create_agent_calls.lock();
        assert_eq!(arcan_calls.len(), 1);
    }

    // Sub-suite 2 — alg:none rejection. The verifier MUST reject before
    // any JWKS work.
    {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\",\"typ\":\"JWT\"}");
        let body = URL_SAFE_NO_PAD
            .encode(br#"{"sub":"u","aud":"lifegw","iss":"https://broomva.test","exp":9999999999}"#);
        let bearer = format!("{header}.{body}.");
        let mut client = env.agent_client().await;
        let mut req = tonic::Request::new(CreateSessionReq {
            user_id: "u".to_string(),
            project_id: "p".to_string(),
            label: "alg-none".to_string(),
            resume_sid: None,
            inherit_policy: None,
        });
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {bearer}").parse().expect("hv"),
        );
        let err = client
            .create_session(req)
            .await
            .expect_err("alg:none must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    // Sub-suite 3 — kid rotation. Replace the JWKS document on the
    // wiremock server with a fresh keypair under a new kid. The first
    // request announcing the new kid triggers lifegw's
    // refetch-on-miss path in `JwksCache::lookup_kid`.
    {
        let new_kid = "rotated-k2";
        let new_signer = env.rotate_jwks(new_kid).await;
        let claims = json!({
            "sub": "user-rotated",
            "aud": "lifegw",
            "iss": env.jwks_issuer.clone(),
            "exp": now_secs() + 600,
            "nbf": now_secs() - 5,
            "project_id": "demo",
            "scopes": ["agent:dispatch"],
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(new_kid.to_string());
        let bearer =
            encode(&header, &claims, &new_signer.encoding_key).expect("encode rotated token");
        let mut client = env.agent_client().await;
        let mut req = tonic::Request::new(CreateSessionReq {
            user_id: "user-rotated".to_string(),
            project_id: "demo".to_string(),
            label: "rotation".to_string(),
            resume_sid: None,
            inherit_policy: None,
        });
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {bearer}").parse().expect("hv"),
        );
        let session = client
            .create_session(req)
            .await
            .expect("kid rotation triggers refetch and verifies")
            .into_inner();
        assert_eq!(session.user_id, "user-rotated");
    }

    env.shutdown().await;
}

// ─── Test rig ──────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

struct SignerKeyPair {
    encoding_key: EncodingKey,
    pem: String,
    kid: String,
}

impl SignerKeyPair {
    fn generate(kid: &str) -> Self {
        let ks = Keystore::generate_dev().expect("ks");
        Self {
            encoding_key: ks.encoding,
            pem: ks.public_pem,
            kid: kid.to_string(),
        }
    }
}

struct TestEnv {
    _tempdir: TempDir,
    cert_pem: Vec<u8>,
    lifegw_addr: std::net::SocketAddr,
    mocks: Arc<lifed::dev_mocks::MockSubstrates>,
    jwks_server: MockServer,
    jwks_issuer: String,
    /// Currently-active Tier-1 signer (kid + public key in wiremock).
    active_signer: SignerKeyPair,
    lifegw_shutdown_tx: Option<oneshot::Sender<()>>,
    lifed_shutdown_tx: Option<oneshot::Sender<()>>,
    lifegw_handle: Option<tokio::task::JoinHandle<()>>,
    lifed_handle: Option<tokio::task::JoinHandle<()>>,
}

impl TestEnv {
    async fn start() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let tempdir = TempDir::new().expect("tempdir");
        let lifed_socket = tempdir.path().join("life.sock");
        let jwks_publish_path = tempdir.path().join("lifegw-jwks.json");

        // Tier-1 signer + wiremock JWKS server.
        let active_signer = SignerKeyPair::generate("k1");
        let jwks_server = MockServer::start().await;
        let jwks_issuer = jwks_server.uri();
        Self::install_jwks(&jwks_server, &active_signer).await;

        // Pre-generate the lifegw Tier-2 keystore so lifed verifies the
        // tokens lifegw mints.
        let lifegw_keystore = Keystore::generate_dev().expect("dev keystore");
        let jwks_json =
            serde_json::to_string_pretty(&lifegw_keystore.publish_jwks()).expect("jwks json");
        std::fs::write(&jwks_publish_path, jwks_json).expect("write jwks");

        // Boot lifed.
        let mocks = Arc::new(lifed::dev_mocks::MockSubstrates::new());
        let mut lifed_cfg = lifed::config::LifedConfig::default();
        lifed_cfg.public_plane.unix_socket = lifed_socket.clone();
        lifed_cfg.public_plane.unix_socket_group = None;
        lifed_cfg.admin_plane.unix_socket = tempdir.path().join("life-admin.sock");
        lifed_cfg.admin_plane.unix_socket_group = None;
        lifed_cfg.auth.jwks_path = jwks_publish_path.clone();
        let (lifed_shutdown_tx, lifed_shutdown_rx) = oneshot::channel();
        let mocks_for_lifed = Arc::clone(&mocks);
        let lifed_handle = tokio::spawn(async move {
            lifed::bootstrap::run_with_mocks(&lifed_cfg, mocks_for_lifed, lifed_shutdown_rx)
                .await
                .expect("lifed boots");
        });
        for _ in 0..200 {
            if lifed_socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(lifed_socket.exists());

        // Self-signed cert for lifegw.
        let cert_kp = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
        ])
        .expect("rcgen");
        let cert_pem = cert_kp.cert.pem().into_bytes();
        let key_pem = cert_kp.key_pair.serialize_pem().into_bytes();
        let cert_path = tempdir.path().join("lifegw-cert.pem");
        let key_path = tempdir.path().join("lifegw-key.pem");
        std::fs::write(&cert_path, &cert_pem).expect("write cert");
        std::fs::write(&key_path, &key_pem).expect("write key");

        // Boot lifegw — REAL Tier-1 verifier path (dev_signer_enabled = false).
        let mut lifegw_cfg = LifegwConfig::default();
        lifegw_cfg.tls.cert_path = cert_path.clone();
        lifegw_cfg.tls.key_path = key_path.clone();
        lifegw_cfg.listen.https_addr = "127.0.0.1:0".to_string();
        lifegw_cfg.listen.http_redirect_addr = None;
        lifegw_cfg.upstream.lifed_uds_path = lifed_socket.clone();
        lifegw_cfg.auth.dev_signer_enabled = false;
        lifegw_cfg.auth.jwks_url = format!("{}/jwks.json", jwks_issuer);
        lifegw_cfg.auth.tier1_audience = "lifegw".to_string();
        lifegw_cfg.auth.tier1_issuer = jwks_issuer.clone();
        lifegw_cfg.auth.publish_jwks_path = None;
        // Sub-phase D (D2): admin plane bound to a tempdir UDS so the
        // test rig doesn't try to write `/run/life/lifegw-admin.sock`.
        lifegw_cfg.admin_plane.unix_socket = tempdir.path().join("lifegw-admin.sock");
        lifegw_cfg.admin_plane.unix_socket_group = None;
        lifegw_cfg.admin_plane.unix_socket_mode = None;
        // Short refetch grace so kid-rotation tests don't have to wait
        // out the default 5 min TTL — but the unknown-kid refetch path
        // doesn't depend on TTL, so the test still demonstrates the
        // refetch-on-miss invariant without needing tokio::time::pause.
        lifegw_cfg.auth.jwks_cache_ttl = Duration::from_secs(60);

        let bind = lifegw::listener::bind(&lifegw_cfg.tls, &lifegw_cfg.listen)
            .await
            .expect("bind");
        let lifegw_addr = bind.local_addr;
        let (lifegw_shutdown_tx, lifegw_shutdown_rx) = oneshot::channel();
        let lifegw_handle = tokio::spawn(async move {
            lifegw::bootstrap::serve_with_listener_and_keystore(
                lifegw_cfg,
                bind,
                lifegw_keystore,
                lifegw_shutdown_rx,
            )
            .await
            .expect("lifegw boots");
        });

        for _ in 0..50 {
            if TcpStream::connect(&lifegw_addr).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        Self {
            _tempdir: tempdir,
            cert_pem,
            lifegw_addr,
            mocks,
            jwks_server,
            jwks_issuer,
            active_signer,
            lifegw_shutdown_tx: Some(lifegw_shutdown_tx),
            lifed_shutdown_tx: Some(lifed_shutdown_tx),
            lifegw_handle: Some(lifegw_handle),
            lifed_handle: Some(lifed_handle),
        }
    }

    fn mint_tier1_token(&self, sub: &str) -> String {
        let claims = json!({
            "sub": sub,
            "aud": "lifegw",
            "iss": self.jwks_issuer.clone(),
            "exp": now_secs() + 600,
            "nbf": now_secs() - 5,
            "project_id": "demo",
            "scopes": ["agent:dispatch"],
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.active_signer.kid.clone());
        encode(&header, &claims, &self.active_signer.encoding_key).expect("encode")
    }

    /// Replace the wiremock JWKS document with a new keypair under
    /// `new_kid`. Returns the new signer so the caller can mint tokens.
    async fn rotate_jwks(&mut self, new_kid: &str) -> SignerKeyPair {
        let new_signer = SignerKeyPair::generate(new_kid);
        // Reset wiremock + remount with the new key.
        self.jwks_server.reset().await;
        Self::install_jwks(&self.jwks_server, &new_signer).await;
        let kp = SignerKeyPair {
            encoding_key: new_signer.encoding_key.clone(),
            pem: new_signer.pem.clone(),
            kid: new_signer.kid.clone(),
        };
        self.active_signer = new_signer;
        kp
    }

    /// Mount a `/jwks.json` route on the wiremock server with the
    /// given signer's JWKS document.
    async fn install_jwks(server: &MockServer, signer: &SignerKeyPair) {
        let entry = JwksEntry::ec_p256_pem(signer.kid.clone(), signer.pem.clone());
        let doc = JwksDoc::new(vec![entry]);
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&doc))
            .mount(server)
            .await;
    }

    async fn agent_client(&self) -> AgentClient<Channel> {
        AgentClient::new(self.dial_lifegw().await)
    }

    async fn dial_lifegw(&self) -> Channel {
        let cert_pem = self.cert_pem.clone();
        let addr = self.lifegw_addr;
        let endpoint = Endpoint::try_from("https://localhost").expect("endpoint");
        endpoint
            .connect_with_connector(service_fn(move |_: Uri| {
                let cert_pem = cert_pem.clone();
                async move {
                    let tls = tls_dial(addr, &cert_pem).await?;
                    Ok::<_, std::io::Error>(TokioIo::new(BoxedAsyncIo(Box::new(tls))))
                }
            }))
            .await
            .expect("connect lifegw")
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.lifegw_shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(tx) = self.lifed_shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.lifegw_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), h).await;
        }
        if let Some(h) = self.lifed_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), h).await;
        }
    }
}

async fn tls_dial(
    addr: std::net::SocketAddr,
    cert_pem: &[u8],
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let mut roots = rustls::RootCertStore::empty();
    let mut reader = std::io::BufReader::new(cert_pem);
    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert.map_err(|e| std::io::Error::other(format!("parse cert: {e}")))?;
        roots
            .add(cert)
            .map_err(|e| std::io::Error::other(format!("root: {e}")))?;
    }
    let mut client_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
    let stream = TcpStream::connect(addr).await?;
    let domain = rustls::pki_types::ServerName::try_from("localhost")
        .map_err(|e| std::io::Error::other(format!("name: {e}")))?;
    let tls = connector.connect(domain, stream).await?;
    Ok(tls)
}

struct BoxedAsyncIo(Box<dyn DynAsyncIo + Send + Unpin>);

impl AsyncRead for BoxedAsyncIo {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut *self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for BoxedAsyncIo {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut *self.0).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut *self.0).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut *self.0).poll_shutdown(cx)
    }
}

trait DynAsyncIo: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> DynAsyncIo for T {}
