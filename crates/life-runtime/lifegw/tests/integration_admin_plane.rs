//! Sub-phase D (D2) integration test — `life.admin.gw.v1.GatewayAdmin`
//! mounted on a UDS.
//!
//! Boots the gateway with the admin plane bound to a tempdir socket,
//! dials the admin UDS via tonic over Unix, and exercises:
//! - `HealthCheck` returns version + JWKS metadata.
//! - `BlocklistAdd` + `BlocklistList` round-trips.
//! - `RateLimitOverride` accepts a per-user budget bump.
//! - `CertReload` returns `reloaded=true` (no-op hook).
//! - `JwksDump` returns a non-empty key list when the cache is warm.
//!
//! The test rig binds the admin UDS without a `unix_socket_group`,
//! which puts the policy table in permissive mode (matches the lifed
//! convention) so we don't need to cross-check group membership.

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hyper_util::rt::TokioIo;
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use life_runtime_proto::life::admin::gw::v1 as adm;

#[tokio::test]
async fn admin_health_check_returns_version() {
    let env = TestEnv::start().await;

    let mut client = env.admin_client().await;
    let resp = client
        .health_check(adm::HealthReq {})
        .await
        .expect("health_check")
        .into_inner();
    assert!(resp.ok);
    assert!(!resp.version.is_empty());

    env.shutdown().await;
}

#[tokio::test]
async fn admin_blocklist_round_trips() {
    let env = TestEnv::start().await;
    let mut client = env.admin_client().await;

    // Add an IP block.
    client
        .blocklist_add(adm::BlocklistAddReq {
            subject: "ip:1.2.3.4".to_string(),
            reason: "test scraper".to_string(),
        })
        .await
        .expect("blocklist_add");

    // List confirms the entry.
    let listed = client
        .blocklist_list(adm::BlocklistListReq {})
        .await
        .expect("blocklist_list")
        .into_inner();
    assert_eq!(listed.entries.len(), 1);
    assert_eq!(listed.entries[0].subject, "ip:1.2.3.4");
    assert_eq!(listed.entries[0].reason, "test scraper");

    // Remove + re-list shows empty.
    client
        .blocklist_remove(adm::BlocklistRemoveReq {
            subject: "ip:1.2.3.4".to_string(),
        })
        .await
        .expect("blocklist_remove");
    let listed = client
        .blocklist_list(adm::BlocklistListReq {})
        .await
        .expect("blocklist_list")
        .into_inner();
    assert_eq!(listed.entries.len(), 0);

    env.shutdown().await;
}

#[tokio::test]
async fn admin_rate_limit_override_accepted() {
    let env = TestEnv::start().await;
    let mut client = env.admin_client().await;

    client
        .rate_limit_override(adm::RateLimitOverrideReq {
            user_id: "user:vip-1".to_string(),
            capacity: 1000,
            refill_per_sec: 1000,
        })
        .await
        .expect("rate_limit_override");

    env.shutdown().await;
}

#[tokio::test]
async fn admin_cert_reload_responds() {
    let env = TestEnv::start().await;
    let mut client = env.admin_client().await;

    // The bootstrap wires the real CertReloader-backed hook in
    // serve_with_listener_and_signer (Sub-phase D D3). The test rig
    // uses the same path via `serve_with_listener_and_keystore`, so
    // the hook reads the on-disk self-signed cert + key the rig
    // writes. The reload should succeed with cert_count >= 1.
    let resp = client
        .cert_reload(adm::CertReloadReq { force: true })
        .await
        .expect("cert_reload")
        .into_inner();
    assert!(resp.reloaded, "real reloader reports success");
    assert!(
        resp.cert_count >= 1,
        "cert_count must reflect on-disk material; got {}",
        resp.cert_count
    );
    assert!(resp.reason.is_empty());

    env.shutdown().await;
}

#[tokio::test]
async fn admin_jwks_dump_reflects_active_keys() {
    let env = TestEnv::start().await;
    let mut client = env.admin_client().await;

    let resp = client
        .jwks_dump(adm::JwksDumpReq {})
        .await
        .expect("jwks_dump")
        .into_inner();
    // The dev-only JwksCache starts empty; the dump returns 0 entries
    // until a verify primes it. The test asserts the dump call works
    // end-to-end + returns a well-formed response (the per-key
    // metadata is a Sub-phase E follow-up; see CLAUDE.md).
    assert_eq!(resp.keys.len(), 0);

    env.shutdown().await;
}

#[tokio::test]
async fn admin_blocklist_rejects_empty_subject() {
    let env = TestEnv::start().await;
    let mut client = env.admin_client().await;

    let err = client
        .blocklist_add(adm::BlocklistAddReq {
            subject: "".to_string(),
            reason: "no subject".to_string(),
        })
        .await
        .expect_err("empty subject must fail");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    env.shutdown().await;
}

// ─── Test rig ───────────────────────────────────────────────────────────

struct TestEnv {
    _tempdir: TempDir,
    admin_socket: PathBuf,
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
        let admin_socket = tempdir.path().join("lifegw-admin.sock");
        let jwks_path = tempdir.path().join("lifegw-jwks.json");

        let lifegw_keystore =
            lifegw::auth::keystore::Keystore::generate_dev().expect("dev keystore");
        let jwks_json =
            serde_json::to_string_pretty(&lifegw_keystore.publish_jwks()).expect("jwks json");
        std::fs::write(&jwks_path, jwks_json).expect("write jwks");

        // lifed boot.
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

        // lifegw boot with admin plane bound to tempdir.
        let cert_kp =
            rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("rcgen");
        let cert_pem = cert_kp.cert.pem().into_bytes();
        let key_pem = cert_kp.key_pair.serialize_pem().into_bytes();
        let cert_path = tempdir.path().join("cert.pem");
        let key_path = tempdir.path().join("key.pem");
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
        lifegw_cfg.admin_plane.unix_socket = admin_socket.clone();
        lifegw_cfg.admin_plane.unix_socket_group = None;
        lifegw_cfg.admin_plane.unix_socket_mode = None;

        let bind = lifegw::listener::bind(&lifegw_cfg.tls, &lifegw_cfg.listen)
            .await
            .expect("bind");
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

        // Wait for the admin socket to appear.
        for _ in 0..200 {
            if admin_socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        Self {
            _tempdir: tempdir,
            admin_socket,
            lifegw_shutdown_tx: Some(lifegw_shutdown_tx),
            lifed_shutdown_tx: Some(lifed_shutdown_tx),
            lifegw_handle: Some(lifegw_handle),
            lifed_handle: Some(lifed_handle),
        }
    }

    async fn admin_client(&self) -> adm::gateway_admin_client::GatewayAdminClient<Channel> {
        let path = self.admin_socket.clone();
        let endpoint = Endpoint::from_static("http://localhost")
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(5));
        let channel = endpoint
            .connect_with_connector(service_fn(move |_uri: Uri| {
                let path = path.clone();
                async move {
                    let stream = UnixStream::connect(&path).await?;
                    Ok::<_, std::io::Error>(TokioIo::new(stream))
                }
            }))
            .await
            .expect("connect admin uds");
        adm::gateway_admin_client::GatewayAdminClient::new(channel)
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
