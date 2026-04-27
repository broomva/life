//! Bootstrap — wires config → substrate clients → router → listener.
//!
//! Sub-phase A entrypoints:
//! - `run_with_mocks` (used by integration tests): mock substrates injected.
//! - `run_daemon` (used by `lifed daemon`): mock substrates because the real
//!   proxies don't exist yet. Sub-phase B replaces this with real-substrate
//!   wiring (B16).

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use tokio::sync::oneshot;
use tonic::transport::Server;

use life_runtime_proto::life::v1 as pb;

use crate::auth::jwks::JwksCache;
use crate::auth::middleware::AuthLayer;
use crate::config::LifedConfig;
use crate::dev_mocks::{MockAnima, MockArcan, MockHaima, MockLago, MockSubstrates};
use crate::error::LifedResult;
use crate::listener::public as public_listener;
use crate::routing::cache::RoutingCache;
use crate::services::agent::{
    AgentService, AnimaDispatch, ArcanDispatch, HaimaDispatch, LagoDispatch,
};
use crate::services::events::{EventsService, LagoTail};

/// Sub-phase A entrypoint for tests + the early daemon.
pub async fn run_with_mocks(
    cfg: &LifedConfig,
    mocks: Arc<MockSubstrates>,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifedResult<()> {
    let _vigil_guard = crate::observability::init(&cfg.vigil)?;

    tracing::info!(
        public_socket = %cfg.public_plane.unix_socket.display(),
        admin_socket  = %cfg.admin_plane.unix_socket.display(),
        "lifed starting (sub-phase A — mock substrates)",
    );

    let jwks = if cfg.auth.jwks_path.exists() {
        Arc::new(JwksCache::load_from_path(&cfg.auth.jwks_path)?)
    } else {
        tracing::warn!(
            path = %cfg.auth.jwks_path.display(),
            "jwks file missing — using dev keystore (test-token-for-{{user}} accepted)"
        );
        Arc::new(JwksCache::dev_only())
    };
    let auth = AuthLayer::new(Arc::clone(&jwks));
    let routing = Arc::new(RoutingCache::new());

    let arcan: Arc<dyn ArcanDispatch> = Arc::new(MockArcanAdapter(mocks.arcan.clone()));
    let lago_dispatch: Arc<dyn LagoDispatch> =
        Arc::new(MockLagoDispatchAdapter(mocks.lago.clone()));
    let lago_tail: Arc<dyn LagoTail> = Arc::new(MockLagoTailAdapter);
    let haima: Arc<dyn HaimaDispatch> = Arc::new(MockHaimaAdapter(mocks.haima.clone()));
    let anima: Arc<dyn AnimaDispatch> = Arc::new(MockAnimaAdapter(mocks.anima.clone()));

    let agent = AgentService::new(arcan, lago_dispatch, haima, anima, Arc::clone(&routing));
    let events = EventsService::new(lago_tail);

    // Mount the AuthLayer at the transport boundary so it runs once per http
    // request, BEFORE tonic dispatches to the service. Handlers then read
    // CapabilityClaims via Self::claims(&req); the dev signer in jwks.rs
    // accepts the `test-token-for-{user}` bearer.
    let router = Server::builder()
        .layer(auth)
        .add_service(pb::agent_server::AgentServer::new(agent))
        .add_service(pb::events_server::EventsServer::new(events));

    let incoming = public_listener::bind(&cfg.public_plane).await?;
    router
        .serve_with_incoming_shutdown(incoming, public_listener::shutdown_signal(shutdown_rx))
        .await
        .map_err(|e| crate::error::LifedError::Server(format!("public-plane serve: {e}")))
}

/// Sub-phase A daemon entrypoint. Replaced in B16 with real-substrate wiring.
pub async fn run_daemon(config_path: Option<&Path>) -> LifedResult<()> {
    let cfg = LifedConfig::load(config_path)?;
    let _vigil_guard = crate::observability::init(&cfg.vigil)?;
    let shutdown_rx = crate::shutdown::install_signal_handler();
    // Sub-phase A: synthesise mocks for the daemon entrypoint too. B16 swaps
    // in real proxies.
    let mocks = Arc::new(MockSubstrates::new());
    run_with_mocks(&cfg, mocks, shutdown_rx).await
}

// ── Mock adapter glue ────────────────────────────────────────────────────────

struct MockArcanAdapter(MockArcan);
struct MockLagoDispatchAdapter(MockLago);
struct MockLagoTailAdapter;
struct MockHaimaAdapter(MockHaima);
struct MockAnimaAdapter(MockAnima);

#[async_trait]
impl ArcanDispatch for MockArcanAdapter {
    async fn create_agent(&self, sid: &str) -> Result<String, tonic::Status> {
        self.0
            .create_agent(sid)
            .await
            .map_err(tonic::Status::internal)
    }
    async fn destroy_agent(&self, sid: &str) -> Result<(), tonic::Status> {
        self.0
            .destroy_agent(sid)
            .await
            .map_err(tonic::Status::internal)
    }
    async fn dispatch_message(
        &self,
        _sid: &str,
        _content: &str,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<pb::AgentEvent, tonic::Status>> + Send>>,
        tonic::Status,
    > {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<pb::AgentEvent, tonic::Status>>(8);
        // Emit one canned token then finish.
        tokio::spawn(async move {
            let _ = tx
                .send(Ok(pb::AgentEvent {
                    record: None,
                    kind: pb::AgentEventKind::Token as i32,
                }))
                .await;
            let _ = tx
                .send(Ok(pb::AgentEvent {
                    record: None,
                    kind: pb::AgentEventKind::Finish as i32,
                }))
                .await;
        });
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

#[async_trait]
impl LagoDispatch for MockLagoDispatchAdapter {
    async fn open_namespace(&self, sid: &str) -> Result<String, tonic::Status> {
        self.0
            .open_namespace(sid)
            .await
            .map_err(tonic::Status::internal)
    }
    async fn close_namespace(&self, ns: &str) -> Result<(), tonic::Status> {
        self.0
            .close_namespace(ns)
            .await
            .map_err(tonic::Status::internal)
    }
}

#[async_trait]
impl LagoTail for MockLagoTailAdapter {
    async fn read(
        &self,
        _sid: &str,
        _from: u64,
        _limit: u32,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<pb::EventRecord, tonic::Status>> + Send>>,
        tonic::Status,
    > {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<pb::EventRecord, tonic::Status>>(1);
        drop(tx); // empty stream
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
    async fn subscribe(
        &self,
        sid: &str,
        from: u64,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<pb::EventRecord, tonic::Status>> + Send>>,
        tonic::Status,
    > {
        self.read(sid, from, 0).await
    }
    async fn get_blob(&self, _ns: &str, _sha256: &str) -> Result<(Vec<u8>, String), tonic::Status> {
        Ok((b"empty".to_vec(), "application/octet-stream".to_string()))
    }
}

#[async_trait]
impl HaimaDispatch for MockHaimaAdapter {
    async fn bind_wallet(&self, sid: &str, project_id: &str) -> Result<String, tonic::Status> {
        self.0
            .bind_wallet(sid, project_id)
            .await
            .map_err(tonic::Status::internal)
    }
    async fn unbind_wallet(&self, wallet_id: &str) -> Result<(), tonic::Status> {
        self.0
            .unbind_wallet(wallet_id)
            .await
            .map_err(tonic::Status::internal)
    }
}

#[async_trait]
impl AnimaDispatch for MockAnimaAdapter {
    async fn register_session(&self, sid: &str, user_id: &str) -> Result<(), tonic::Status> {
        self.0
            .register_session(sid, user_id)
            .await
            .map_err(tonic::Status::internal)
    }
    async fn mark_session_closed(&self, sid: &str) -> Result<(), tonic::Status> {
        self.0
            .mark_session_closed(sid)
            .await
            .map_err(tonic::Status::internal)
    }
}
