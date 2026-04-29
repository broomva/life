//! Sub-phase D (D1) integration test — token-bucket limiter wired
//! into the auth Layer.
//!
//! Boots the gateway with a deliberately tiny per-user budget
//! (capacity 2, no refill) and asserts the 3rd request returns
//! `Code::ResourceExhausted` (NOT `Unavailable` — that distinction
//! is locked in by the prompt's hard rule that maps `resource_exhausted`
//! to WS close 4001).

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use hyper_util::rt::TokioIo;
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use life_runtime_proto::life::v1::CreateSessionReq;
use life_runtime_proto::life::v1::agent_client::AgentClient;

#[tokio::test]
async fn rate_limit_returns_resource_exhausted_after_burst() {
    let env = TestEnv::start().await;

    let mut client = env.agent_client().await;

    // Capacity 2 → first 2 requests allowed; the 3rd within the same
    // second (no refill) returns ResourceExhausted.
    for i in 0..2 {
        let mut req = tonic::Request::new(CreateSessionReq {
            user_id: "rl-burst".to_string(),
            project_id: "demo".to_string(),
            label: format!("attempt-{i}"),
            resume_sid: None,
            inherit_policy: None,
        });
        req.metadata_mut().insert(
            "authorization",
            "Bearer dev-token-for-rl-burst".parse().expect("hv"),
        );
        client
            .create_session(req)
            .await
            .expect("first 2 requests succeed");
    }

    let mut req3 = tonic::Request::new(CreateSessionReq {
        user_id: "rl-burst".to_string(),
        project_id: "demo".to_string(),
        label: "attempt-3-rejected".to_string(),
        resume_sid: None,
        inherit_policy: None,
    });
    req3.metadata_mut().insert(
        "authorization",
        "Bearer dev-token-for-rl-burst".parse().expect("hv"),
    );
    let err = client
        .create_session(req3)
        .await
        .expect_err("3rd request must hit the rate limit");
    assert_eq!(
        err.code(),
        tonic::Code::ResourceExhausted,
        "rate limit MUST return ResourceExhausted, not {:?}",
        err.code()
    );

    env.shutdown().await;
}

// ─── Test rig (mirrors integration_proxy_passthrough.rs) ────────────────

struct TestEnv {
    _tempdir: TempDir,
    cert_pem: Vec<u8>,
    lifegw_addr: std::net::SocketAddr,
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

        // Pre-mint lifegw's keystore so lifed can verify its Tier-2 tokens.
        let lifegw_keystore =
            lifegw::auth::keystore::Keystore::generate_dev().expect("dev keystore");
        let jwks_json =
            serde_json::to_string_pretty(&lifegw_keystore.publish_jwks()).expect("jwks json");
        std::fs::write(&jwks_path, jwks_json).expect("write jwks");

        // Boot lifed.
        let mocks = Arc::new(lifed::dev_mocks::MockSubstrates::new());
        let mut lifed_cfg = lifed::config::LifedConfig::default();
        lifed_cfg.public_plane.unix_socket = lifed_socket.clone();
        lifed_cfg.public_plane.unix_socket_group = None;
        lifed_cfg.admin_plane.unix_socket = tempdir.path().join("life-admin.sock");
        lifed_cfg.admin_plane.unix_socket_group = None;
        lifed_cfg.auth.jwks_path = jwks_path.clone();
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

        // Generate self-signed cert for lifegw.
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

        let mut lifegw_cfg = lifegw::config::LifegwConfig::default();
        lifegw_cfg.tls.cert_path = cert_path.clone();
        lifegw_cfg.tls.key_path = key_path.clone();
        lifegw_cfg.listen.https_addr = "127.0.0.1:0".to_string();
        lifegw_cfg.listen.http_redirect_addr = None;
        lifegw_cfg.upstream.lifed_uds_path = lifed_socket.clone();
        lifegw_cfg.auth.dev_signer_enabled = true;
        lifegw_cfg.auth.publish_jwks_path = None;
        lifegw_cfg.admin_plane.unix_socket = tempdir.path().join("lifegw-admin.sock");
        lifegw_cfg.admin_plane.unix_socket_group = None;
        lifegw_cfg.admin_plane.unix_socket_mode = None;
        // Tight per-user budget; per-IP budget left high.
        lifegw_cfg.rate_limit.per_user_capacity = 2;
        lifegw_cfg.rate_limit.per_user_refill_per_sec = 0;
        lifegw_cfg.rate_limit.per_ip_capacity = 1000;
        lifegw_cfg.rate_limit.per_ip_refill_per_min = 1000;

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
            lifegw_shutdown_tx: Some(lifegw_shutdown_tx),
            lifed_shutdown_tx: Some(lifed_shutdown_tx),
            lifegw_handle: Some(lifegw_handle),
            lifed_handle: Some(lifed_handle),
        }
    }

    async fn agent_client(&self) -> AgentClient<Channel> {
        AgentClient::new(self.dial_lifegw().await)
    }

    async fn dial_lifegw(&self) -> Channel {
        let cert_pem = self.cert_pem.clone();
        let addr = self.lifegw_addr;
        let endpoint = Endpoint::from_static("https://localhost")
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(5));
        endpoint
            .connect_with_connector(service_fn(move |_uri: Uri| {
                let cert_pem = cert_pem.clone();
                async move {
                    let stream = TcpStream::connect(&addr).await.map_err(into_io)?;
                    let mut roots = rustls::RootCertStore::empty();
                    let mut reader = std::io::BufReader::new(&cert_pem[..]);
                    for cert in rustls_pemfile::certs(&mut reader) {
                        let cert = cert.map_err(into_io)?;
                        roots
                            .add(cert)
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                    }
                    let cfg = rustls::ClientConfig::builder()
                        .with_root_certificates(roots)
                        .with_no_client_auth();
                    let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));
                    let domain =
                        rustls::pki_types::ServerName::try_from("localhost").map_err(into_io)?;
                    let tls = connector.connect(domain, stream).await.map_err(into_io)?;
                    Ok::<_, std::io::Error>(TokioIo::new(tls))
                }
            }))
            .await
            .expect("connect")
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.lifegw_shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(tx) = self.lifed_shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.lifegw_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
        }
        if let Some(h) = self.lifed_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
        }
    }
}

fn into_io<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> std::io::Error {
    std::io::Error::other(e)
}
