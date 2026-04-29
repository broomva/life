//! Integration test — Sub-phase A acceptance criterion #6.
//!
//! Brings up:
//! 1. A `lifed` instance bound to a tempdir UDS, fronted by mock substrates.
//! 2. A `lifegw` instance bound to `127.0.0.1:0` with a self-signed TLS cert,
//!    pointing at lifed's UDS.
//!
//! Then dials lifegw with a tonic client over rustls trusting the dev cert,
//! and asserts:
//! - `/healthz` returns HTTP 200 (browser-style probe).
//! - `Agent.CreateSession` round-trips → lifed's mock arcan observes one
//!   `create_agent` call.

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use http_body_util::BodyExt;
use hyper::Request;
use hyper_util::rt::TokioIo;
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use life_runtime_proto::life::v1::CreateSessionReq;
use life_runtime_proto::life::v1::agent_client::AgentClient;

#[tokio::test]
async fn proxy_forwards_create_session() {
    let env = TestEnv::start().await;

    let mut client = env.agent_client().await;
    let mut req = tonic::Request::new(CreateSessionReq {
        user_id: "user-pass-thru".to_string(),
        project_id: "demo".to_string(),
        label: "lifegw-roundtrip".to_string(),
        resume_sid: None,
        inherit_policy: None,
    });
    req.metadata_mut().insert(
        "authorization",
        "Bearer dev-token-for-user-pass-thru".parse().expect("hv"),
    );

    let session = client
        .create_session(req)
        .await
        .expect("create_session round-trips through lifegw → lifed → mock-arcan")
        .into_inner();

    assert_eq!(session.user_id, "user-pass-thru");
    {
        let arcan_calls = env.mocks.arcan.create_agent_calls.lock();
        assert_eq!(
            arcan_calls.len(),
            1,
            "mock arcan saw exactly one create_agent: {:?}",
            *arcan_calls
        );
    }
    env.shutdown().await;
}

#[tokio::test]
async fn health_endpoint_responds_200() {
    let env = TestEnv::start().await;

    let resp = env.healthz().await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.body, "OK");

    env.shutdown().await;
}

#[tokio::test]
async fn missing_bearer_returns_unauthenticated() {
    let env = TestEnv::start().await;

    let mut client = env.agent_client().await;
    let req = tonic::Request::new(CreateSessionReq {
        user_id: "u".to_string(),
        project_id: "p".to_string(),
        label: "no-auth".to_string(),
        resume_sid: None,
        inherit_policy: None,
    });
    let err = client
        .create_session(req)
        .await
        .expect_err("must fail without bearer");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    env.shutdown().await;
}

// ─── Test rig ───────────────────────────────────────────────────────────

struct TestEnv {
    _tempdir: TempDir,
    cert_pem: Vec<u8>,
    lifegw_addr: std::net::SocketAddr,
    mocks: Arc<lifed::dev_mocks::MockSubstrates>,
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
        let jwks_path = tempdir.path().join("lifegw-jwks.json");

        // Pre-generate the lifegw Tier-2 signing keystore + write its JWKS
        // to a path that lifed reads at startup. This makes the downstream
        // Tier-2 verifier trust the tokens lifegw mints during the test —
        // without this, lifed falls back to its own dev keystore (whose
        // public key is unrelated to lifegw's) and rejects every request
        // with `Unauthenticated`.
        let lifegw_keystore =
            lifegw::auth::keystore::Keystore::generate_dev().expect("dev keystore");
        let jwks_json =
            serde_json::to_string_pretty(&lifegw_keystore.publish_jwks()).expect("jwks json");
        std::fs::write(&jwks_path, jwks_json).expect("write jwks");

        // Boot lifed against the tempdir UDS with mocks.
        let mocks = Arc::new(lifed::dev_mocks::MockSubstrates::new());
        let mut lifed_cfg = lifed::config::LifedConfig::default();
        lifed_cfg.public_plane.unix_socket = lifed_socket.clone();
        lifed_cfg.public_plane.unix_socket_group = None;
        lifed_cfg.admin_plane.unix_socket = tempdir.path().join("life-admin.sock");
        lifed_cfg.admin_plane.unix_socket_group = None;
        // Tell lifed to load lifegw's published JWKS so Tier-2 verification
        // recognises the keys lifegw mints with.
        lifed_cfg.auth.jwks_path = jwks_path.clone();
        let (lifed_shutdown_tx, lifed_shutdown_rx) = oneshot::channel();
        let mocks_for_lifed = Arc::clone(&mocks);
        let lifed_handle = tokio::spawn(async move {
            lifed::bootstrap::run_with_mocks(&lifed_cfg, mocks_for_lifed, lifed_shutdown_rx)
                .await
                .expect("lifed boots");
        });
        // Wait for lifed's UDS to appear.
        for _ in 0..200 {
            if lifed_socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(lifed_socket.exists(), "lifed bound its UDS");

        // Generate a self-signed cert for lifegw.
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

        // Boot lifegw on a free port. The config structs are
        // `#[non_exhaustive]`, so we mutate a `default()` instead of
        // struct-literal construction (which is forbidden across crates).
        let mut lifegw_cfg = lifegw::config::LifegwConfig::default();
        lifegw_cfg.tls.cert_path = cert_path.clone();
        lifegw_cfg.tls.key_path = key_path.clone();
        lifegw_cfg.listen.https_addr = "127.0.0.1:0".to_string();
        lifegw_cfg.listen.http_redirect_addr = None;
        lifegw_cfg.upstream.lifed_uds_path = lifed_socket.clone();
        lifegw_cfg.auth.dev_signer_enabled = true;
        // Sub-phase B's default publish_jwks_path = /run/life/lifegw-jwks.json
        // is not writable in the unit-test sandbox; the rig shares
        // keystore material in-memory through serve_with_listener_and_keystore
        // so disabling the publish step is safe here.
        lifegw_cfg.auth.publish_jwks_path = None;
        // Sub-phase D (D2): admin plane bound to a tempdir UDS so the
        // test rig doesn't try to write `/run/life/lifegw-admin.sock`
        // (read-only on the macOS sandbox + collision-prone with a
        // running production daemon).
        lifegw_cfg.admin_plane.unix_socket = tempdir.path().join("lifegw-admin.sock");
        lifegw_cfg.admin_plane.unix_socket_group = None;
        lifegw_cfg.admin_plane.unix_socket_mode = None;
        // Pre-bind so we can extract the resolved port.
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

        // Briefly poll the gateway for readiness — TLS handshake needs the
        // crypto provider to be installed and the bound listener to be in
        // accept(). 50 attempts × 20 ms = 1 s.
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
            lifegw_shutdown_tx: Some(lifegw_shutdown_tx),
            lifed_shutdown_tx: Some(lifed_shutdown_tx),
            lifegw_handle: Some(lifegw_handle),
            lifed_handle: Some(lifed_handle),
        }
    }

    async fn agent_client(&self) -> AgentClient<Channel> {
        AgentClient::new(self.dial_lifegw().await)
    }

    /// Dial the gateway via a rustls-wrapped TcpStream that trusts the
    /// self-signed cert. Returns a tonic Channel ready for clients.
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

    async fn healthz(&self) -> HealthzResponse {
        let tls = tls_dial(self.lifegw_addr, &self.cert_pem)
            .await
            .expect("tls dial");
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .expect("h1 handshake");
        let conn_handle = tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!(error = %e, "h1 conn done");
            }
        });
        let req = Request::builder()
            .method("GET")
            .uri("/healthz")
            .header("host", "localhost")
            .body(http_body_util::Empty::<bytes::Bytes>::new())
            .expect("build req");
        let resp = sender.send_request(req).await.expect("send /healthz");
        let status = resp.status();
        let body_bytes = resp
            .into_body()
            .collect()
            .await
            .expect("collect")
            .to_bytes();
        let body = String::from_utf8_lossy(&body_bytes).into_owned();
        drop(sender);
        let _ = tokio::time::timeout(Duration::from_secs(1), conn_handle).await;
        HealthzResponse { status, body }
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

struct HealthzResponse {
    status: StatusCode,
    body: String,
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
    // Sub-phase A's tonic-web layer expects HTTP/1.1 for browser fetch and
    // HTTP/2 for native gRPC — for the gRPC client below we negotiate
    // `h2` via ALPN.
    client_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
    let stream = TcpStream::connect(addr).await?;
    let domain = rustls::pki_types::ServerName::try_from("localhost")
        .map_err(|e| std::io::Error::other(format!("name: {e}")))?;
    let tls = connector.connect(domain, stream).await?;
    Ok(tls)
}

/// Box the `AsyncRead+AsyncWrite` so a trait-object-using `service_fn` can
/// return a uniform connector across both gRPC and HTTP/1.1 dials.
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

// Suppress warnings about the `_support` mod placeholder.
#[allow(dead_code)]
fn _force_path_unused(_: PathBuf) {}
