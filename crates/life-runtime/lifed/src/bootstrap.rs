//! Bootstrap — wires config → substrate clients → router → listener.
//!
//! Sub-phase B entrypoints:
//! - `run_with_mocks` (used by integration tests + the dev daemon path):
//!   `MockSubstrates` injected via the per-substrate `*Call` traits.
//! - `run_with_real_substrates` (production): real `*-proxy` crates dial
//!   the substrate UDS sockets. See Task B16.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::oneshot;
use tonic::transport::Server;

use anima_proxy::AnimaCall;
use arcan_proxy::ArcanCall;
use haima_proxy::HaimaCall;
use lago_proxy::LagoCall;
use life_runtime_proto::life::v1 as pb;

use crate::auth::blocklist::RevokedSidSet;
use crate::auth::jwks::JwksCache;
use crate::auth::keystore::Keystore;
use crate::auth::middleware::AuthLayer;
use crate::config::LifedConfig;
use crate::dev_mocks::MockSubstrates;
use crate::error::{LifedError, LifedResult};
use crate::idempotency::{IdempotencyStore, boxed_in_memory};
use crate::listener::public as public_listener;
use crate::routing::cache::RoutingCache;
use crate::saga::driver::SagaDriver;
use crate::services::agent::AgentService;
use crate::services::events::EventsService;
use crate::services::identity::IdentityService;
use crate::services::wallet::WalletService;

/// Sub-phase B mocks entrypoint — used by integration tests + the dev
/// daemon path until B16 wires the real-substrate path.
pub async fn run_with_mocks(
    cfg: &LifedConfig,
    mocks: Arc<MockSubstrates>,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifedResult<()> {
    let _vigil_guard = crate::observability::init(&cfg.vigil)?;

    tracing::info!(
        public_socket = %cfg.public_plane.unix_socket.display(),
        admin_socket  = %cfg.admin_plane.unix_socket.display(),
        "lifed starting (sub-phase B — mock substrates)",
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
    let ks = Arc::new(Keystore::generate_dev());
    let saga = Arc::new(SagaDriver::new("lifed-runtime"));

    let arcan: Arc<dyn ArcanCall> = Arc::new(mocks.arcan.clone());
    let lago: Arc<dyn LagoCall> = Arc::new(mocks.lago.clone());
    let haima: Arc<dyn HaimaCall> = Arc::new(mocks.haima.clone());
    let anima: Arc<dyn AnimaCall> = Arc::new(mocks.anima.clone());

    let idem: Arc<dyn IdempotencyStore> =
        boxed_in_memory(std::time::Duration::from_secs(cfg.idempotency.ttl_secs));
    let revoked = Arc::new(RevokedSidSet::new());

    let agent = AgentService::new(
        Arc::clone(&arcan),
        Arc::clone(&lago),
        Arc::clone(&haima),
        Arc::clone(&anima),
        Arc::clone(&routing),
        Arc::clone(&ks),
        Arc::clone(&saga),
    );
    let events = EventsService::new(Arc::clone(&lago));
    let wallet = WalletService::new(Arc::clone(&haima), Arc::clone(&idem));
    let identity = IdentityService::new(
        Arc::clone(&anima),
        Arc::clone(&routing),
        Arc::clone(&revoked),
    );

    let router = Server::builder()
        .layer(auth)
        .add_service(pb::agent_server::AgentServer::new(agent))
        .add_service(pb::events_server::EventsServer::new(events))
        .add_service(pb::wallet_server::WalletServer::new(wallet))
        .add_service(pb::identity_server::IdentityServer::new(identity));

    let incoming = public_listener::bind(&cfg.public_plane).await?;
    router
        .serve_with_incoming_shutdown(incoming, public_listener::shutdown_signal(shutdown_rx))
        .await
        .map_err(|e| LifedError::Server(format!("public-plane serve: {e}")))
}

/// Sub-phase B daemon entrypoint. Sub-phase A's `MockSubstrates` daemon path
/// is preserved here. B16 introduces the parallel `run_with_real_substrates`
/// entrypoint.
pub async fn run_daemon(config_path: Option<&Path>) -> LifedResult<()> {
    let cfg = LifedConfig::load(config_path)?;
    let _vigil_guard = crate::observability::init(&cfg.vigil)?;
    let shutdown_rx = crate::shutdown::install_signal_handler();
    let mocks = Arc::new(MockSubstrates::new());
    run_with_mocks(&cfg, mocks, shutdown_rx).await
}
