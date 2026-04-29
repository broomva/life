//! TestEnv — boots a tempdir-rooted lifed daemon plus the substrate set
//! (mocks under sub-phase A; real substrates under sub-phase B).
//!
//! Sub-phase C addition: `TestEnv` exposes the in-process `LifedHandles`
//! (routing cache, blocklist, idempotency store, saga registry, approval
//! locks) so tests can assert side-effects without re-dialing the public
//! plane. Tests can also `dial_admin()` the admin UDS to exercise the
//! `life.admin.v1.*` services.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;

use life_runtime_proto::life::v1::agent_client::AgentClient;
use life_runtime_proto::life::v1::identity_client::IdentityClient;
use life_runtime_proto::life::v1::wallet_client::WalletClient;
use life_runtime_proto::life::v1::{CreateSessionReq, Session};
use lifed::bootstrap::LifedHandles;
use lifed::config::LifedConfig;

use super::mock_substrates::MockSubstrates;

/// Sub-phase A test environment: lifed bound to a tempdir UDS, mock substrates
/// behind the substrate-proxy stubs, deterministic dev signing key.
///
/// Sub-phase C: also exposes admin UDS + `LifedHandles`.
pub struct TestEnv {
    _tempdir: TempDir,
    public_socket: PathBuf,
    admin_socket: PathBuf,
    pub mocks: Arc<MockSubstrates>,
    pub handles: LifedHandles,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl TestEnv {
    /// Start a lifed instance backed by mock substrates.
    pub async fn start_with_mocks() -> Self {
        let tempdir = TempDir::new().expect("tempdir");
        let public_socket = tempdir.path().join("life.sock");
        let admin_socket = tempdir.path().join("life-admin.sock");
        let mocks = Arc::new(MockSubstrates::new());

        let mut cfg = LifedConfig::default();
        cfg.public_plane.unix_socket = public_socket.clone();
        cfg.admin_plane.unix_socket = admin_socket.clone();
        cfg.public_plane.unix_socket_group = None;
        // Tests bind to the test user's primary GID for the admin socket
        // by leaving `unix_socket_group` empty: the admin policy resolves
        // the configured group name to GID, and an empty/missing group
        // falls back to GID 0. To keep tests permissive, the admin
        // policy treats `admin_gid == cred.gid` as authorised, and on
        // macOS the dev fallback returns the real getuid()/getgid() —
        // so the user already matches.
        cfg.admin_plane.unix_socket_group = None;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (handles_tx, handles_rx) = oneshot::channel::<LifedHandles>();
        let mocks_clone = Arc::clone(&mocks);
        let server_handle = tokio::spawn(async move {
            lifed::bootstrap::run_with_mocks_handles(
                &cfg,
                mocks_clone,
                shutdown_rx,
                Some(handles_tx),
            )
            .await
            .expect("lifed boots in test mode");
        });

        // Wait for the socket to appear (poll up to 2 s).
        for _ in 0..200 {
            if public_socket.exists() && admin_socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(public_socket.exists(), "public socket bound");
        assert!(admin_socket.exists(), "admin socket bound");

        let handles = handles_rx.await.expect("handles published by bootstrap");

        Self {
            _tempdir: tempdir,
            public_socket,
            admin_socket,
            mocks,
            handles,
            shutdown_tx: Some(shutdown_tx),
            server_handle: Some(server_handle),
        }
    }

    /// Dial the test's admin UDS and return the underlying tonic Channel.
    pub async fn dial_admin(&self) -> tonic::transport::Channel {
        let socket = self.admin_socket.clone();
        let endpoint = Endpoint::try_from("http://[::]:0").unwrap();
        endpoint
            .connect_with_connector(service_fn(move |_: Uri| {
                let socket = socket.clone();
                async move {
                    let stream = UnixStream::connect(socket).await?;
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
                }
            }))
            .await
            .expect("connect admin")
    }

    /// Dial the test's public UDS and return the underlying tonic Channel.
    pub async fn dial_public(&self) -> tonic::transport::Channel {
        let socket = self.public_socket.clone();
        let endpoint = Endpoint::try_from("http://[::]:0").unwrap();
        endpoint
            .connect_with_connector(service_fn(move |_: Uri| {
                let socket = socket.clone();
                async move {
                    let stream = UnixStream::connect(socket).await?;
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
                }
            }))
            .await
            .expect("connect")
    }

    /// Returns an `AgentClient` connected to the test's public socket.
    pub async fn agent_client(&self) -> AgentClient<tonic::transport::Channel> {
        AgentClient::new(self.dial_public().await)
    }

    /// Returns a `WalletClient` connected to the test's public socket.
    pub async fn wallet_client(&self) -> WalletClient<tonic::transport::Channel> {
        WalletClient::new(self.dial_public().await)
    }

    /// Returns an `IdentityClient` connected to the test's public socket.
    pub async fn identity_client(&self) -> IdentityClient<tonic::transport::Channel> {
        IdentityClient::new(self.dial_public().await)
    }

    /// Convenience: build a `CreateSessionReq` with a dev Tier-2 token in metadata
    /// and dispatch it.
    pub async fn create_session_dev(
        &self,
        user_id: &str,
        project_id: &str,
        label: &str,
    ) -> Result<Session, tonic::Status> {
        let mut client = self.agent_client().await;
        let mut req = tonic::Request::new(CreateSessionReq {
            user_id: user_id.to_string(),
            project_id: project_id.to_string(),
            label: label.to_string(),
            resume_sid: None,
            inherit_policy: None,
        });
        // Sub-phase A: the dev signer accepts a deterministic test token whose
        // body is `bearer test-token-for-{user_id}`. Real ES256 lands in B5.
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer test-token-for-{user_id}").parse().unwrap(),
        );
        client.create_session(req).await.map(|r| r.into_inner())
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.server_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), h).await;
        }
    }

    /// Inject a one-shot failure into the next mock arcan `create_agent` call.
    pub fn inject_arcan_fault(&self) {
        self.mocks.arcan.inject_fault();
    }

    /// Inject a one-shot failure into the next mock lago `open_namespace` call.
    pub fn inject_lago_fault(&self) {
        self.mocks.lago.inject_fault();
    }

    /// Inject a one-shot failure into the next mock haima `bind_wallet` call.
    pub fn inject_haima_fault(&self) {
        self.mocks.haima.inject_fault();
    }

    /// Inject a one-shot failure into the next mock anima `register_session` call.
    pub fn inject_anima_fault(&self) {
        self.mocks.anima.inject_fault();
    }

    // Sub-phase D7 chaos-test helpers.

    /// Read the current circuit-breaker state for the named substrate.
    pub fn arcan_breaker_state(&self) -> lifed::routing::breaker::BreakerState {
        self.handles.pools.arcan.load().breaker_state()
    }

    pub fn lago_breaker_state(&self) -> lifed::routing::breaker::BreakerState {
        self.handles.pools.lago.load().breaker_state()
    }

    pub fn haima_breaker_state(&self) -> lifed::routing::breaker::BreakerState {
        self.handles.pools.haima.load().breaker_state()
    }

    pub fn anima_breaker_state(&self) -> lifed::routing::breaker::BreakerState {
        self.handles.pools.anima.load().breaker_state()
    }

    /// Drive a sustained outage for the named substrate. Used by chaos tests.
    pub fn fail_lago(&self) {
        self.mocks.lago.set_force_fail(true);
    }
    pub fn recover_lago(&self) {
        self.mocks.lago.set_force_fail(false);
    }

    /// Sub-phase E: force the lago breaker's `open_until` anchor into the
    /// past so the next state read flips Open→HalfOpen lazily without
    /// the chaos test sleeping for the full 10 s open window. Backed by
    /// `life-runtime-pool`'s `test-support` feature.
    pub fn force_lago_open_window_elapsed(&self) {
        self.handles
            .pools
            .lago
            .load()
            .breaker
            .force_open_window_elapsed();
    }
}
