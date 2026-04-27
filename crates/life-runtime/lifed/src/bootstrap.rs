//! Bootstrap — wires config → substrate clients → router → listener.
//!
//! Sub-phase B entrypoints:
//! - `run_with_mocks` (used by integration tests + the dev daemon path):
//!   `MockSubstrates` injected via the per-substrate `*Call` traits.
//! - `run_with_real_substrates` (production): real `*-proxy` crates dial
//!   the substrate UDS sockets. See Task B16.
//!
//! Sub-phase C additions:
//! - `LifedHandles` exposes routing/saga/idempotency/blocklist registries
//!   so admin-plane services + tests can introspect the in-process state.
//! - The admin-plane listener runs alongside the public-plane listener;
//!   both drain on the same shutdown channel.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::oneshot;
use tonic::transport::Server;

use anima_proxy::{AnimaCall, AnimaProxy};
use arcan_proxy::{ArcanCall, ArcanProxy};
use haima_proxy::{HaimaCall, HaimaProxy};
use lago_proxy::{LagoCall, LagoProxy};
use life_runtime_proto::life::admin::v1 as adm;
use life_runtime_proto::life::v1 as pb;

use crate::idempotency::lago_store::LagoBackedStore;

use crate::auth::blocklist::RevokedSidSet;
use crate::auth::jwks::JwksCache;
use crate::auth::keystore::Keystore;
use crate::auth::middleware::AuthLayer;
use crate::auth::peercred;
use crate::config::LifedConfig;
use crate::dev_mocks::MockSubstrates;
use crate::error::{LifedError, LifedResult};
use crate::idempotency::{IdempotencyStore, boxed_in_memory};
use crate::listener::admin as admin_listener;
use crate::listener::public as public_listener;
use crate::routing::cache::RoutingCache;
use crate::saga::driver::{LagoSagaJournal, SagaDriver, SagaJournal};
use crate::saga::registry::SagaRegistry;
use crate::services::admin::{
    AdminPolicy, RoutingCacheAdminService, RuntimeAdminService, SagaAdminService,
};
use crate::services::agent::AgentService;
use crate::services::events::EventsService;
use crate::services::identity::IdentityService;
use crate::services::wallet::WalletService;

/// In-process state exposed by the bootstrap path so admin-plane services
/// and integration tests can introspect lifed without re-dialing the
/// public plane. Sub-phase C addition; sub-phase B's `run_with_*` paths
/// owned these registries privately and tests could not reach them.
#[derive(Clone)]
pub struct LifedHandles {
    pub routing: Arc<RoutingCache>,
    pub revoked: Arc<RevokedSidSet>,
    pub idem: Arc<dyn IdempotencyStore>,
    pub saga_registry: Arc<SagaRegistry>,
    pub approval_locks: Arc<crate::services::agent::ApprovalLocks>,
}

/// Sub-phase B mocks entrypoint — used by integration tests + the dev
/// daemon path until B16 wires the real-substrate path.
pub async fn run_with_mocks(
    cfg: &LifedConfig,
    mocks: Arc<MockSubstrates>,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifedResult<()> {
    run_with_mocks_handles(cfg, mocks, shutdown_rx, None).await
}

/// Variant of `run_with_mocks` that publishes the in-process handles
/// over a oneshot channel so callers (tests) can introspect the
/// running daemon.
pub async fn run_with_mocks_handles(
    cfg: &LifedConfig,
    mocks: Arc<MockSubstrates>,
    shutdown_rx: oneshot::Receiver<()>,
    handles_tx: Option<oneshot::Sender<LifedHandles>>,
) -> LifedResult<()> {
    let _vigil_guard = crate::observability::init(&cfg.vigil)?;

    tracing::info!(
        public_socket = %cfg.public_plane.unix_socket.display(),
        admin_socket  = %cfg.admin_plane.unix_socket.display(),
        "lifed starting (sub-phase C — mock substrates)",
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

    let arcan: Arc<dyn ArcanCall> = Arc::new(mocks.arcan.clone());
    let lago: Arc<dyn LagoCall> = Arc::new(mocks.lago.clone());
    let haima: Arc<dyn HaimaCall> = Arc::new(mocks.haima.clone());
    let anima: Arc<dyn AnimaCall> = Arc::new(mocks.anima.clone());

    let handles = build_handles(cfg, Arc::clone(&lago));

    let routing = Arc::clone(&handles.routing);
    let revoked = Arc::clone(&handles.revoked);
    let idem = Arc::clone(&handles.idem);
    let saga_registry = Arc::clone(&handles.saga_registry);
    let approval_locks = Arc::clone(&handles.approval_locks);

    let ks = Arc::new(Keystore::generate_dev());
    let saga = build_saga_driver(Arc::clone(&saga_registry), Arc::clone(&lago));

    let admin_policy = build_admin_policy(cfg);

    let services = build_services(
        Arc::clone(&arcan),
        Arc::clone(&lago),
        Arc::clone(&haima),
        Arc::clone(&anima),
        Arc::clone(&routing),
        Arc::clone(&revoked),
        Arc::clone(&idem),
        Arc::clone(&approval_locks),
        Arc::clone(&saga_registry),
        Arc::clone(&admin_policy),
        Arc::clone(&ks),
        saga,
    );

    if let Some(tx) = handles_tx {
        let _ = tx.send(handles);
    }

    serve_planes(cfg, auth, services, shutdown_rx).await
}

/// Sub-phase B daemon entrypoint. Tries the real-substrate path first;
/// falls back to mocks if any substrate UDS socket is missing (dev/CI
/// mode). The real path is gated behind `cfg.substrates.*.unix_socket`
/// existing so a fresh dev box without arcand/lagod/haimad/animad still
/// boots a usable lifed.
pub async fn run_daemon(config_path: Option<&Path>) -> LifedResult<()> {
    let cfg = LifedConfig::load(config_path)?;
    let _vigil_guard = crate::observability::init(&cfg.vigil)?;
    let shutdown_rx = crate::shutdown::install_signal_handler();
    if all_substrate_sockets_present(&cfg) {
        run_with_real_substrates(&cfg, shutdown_rx).await
    } else {
        tracing::warn!(
            "one or more substrate UDS sockets missing — booting with MockSubstrates (dev mode)"
        );
        let mocks = Arc::new(MockSubstrates::new());
        run_with_mocks(&cfg, mocks, shutdown_rx).await
    }
}

fn all_substrate_sockets_present(cfg: &LifedConfig) -> bool {
    cfg.substrates.arcan.unix_socket.exists()
        && cfg.substrates.lago.unix_socket.exists()
        && cfg.substrates.haima.unix_socket.exists()
        && cfg.substrates.anima.unix_socket.exists()
}

/// Sub-phase B real-substrate entrypoint per Spec C₂ §12.B. Dials the
/// four substrate UDS sockets, mints + publishes the substrate-token JWKS,
/// builds the public-plane router, and serves until the shutdown channel
/// fires.
pub async fn run_with_real_substrates(
    cfg: &LifedConfig,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifedResult<()> {
    let _vigil_guard = crate::observability::init(&cfg.vigil)?;
    tracing::info!(
        public_socket = %cfg.public_plane.unix_socket.display(),
        admin_socket  = %cfg.admin_plane.unix_socket.display(),
        "lifed starting (sub-phase C — real substrates)",
    );

    let arcan: Arc<dyn ArcanCall> = Arc::new(
        ArcanProxy::connect(cfg.substrates.arcan.unix_socket.clone())
            .await
            .map_err(|e| LifedError::Substrate(format!("arcan dial: {e}")))?,
    );
    let lago_proxy = LagoProxy::connect(cfg.substrates.lago.unix_socket.clone())
        .await
        .map_err(|e| LifedError::Substrate(format!("lago dial: {e}")))?;
    let lago: Arc<dyn LagoCall> = Arc::new(lago_proxy);
    let haima: Arc<dyn HaimaCall> = Arc::new(
        HaimaProxy::connect(cfg.substrates.haima.unix_socket.clone())
            .await
            .map_err(|e| LifedError::Substrate(format!("haima dial: {e}")))?,
    );
    let anima: Arc<dyn AnimaCall> = Arc::new(
        AnimaProxy::connect(cfg.substrates.anima.unix_socket.clone())
            .await
            .map_err(|e| LifedError::Substrate(format!("anima dial: {e}")))?,
    );

    // Substrate-token signing keystore + JWKS publish.
    let ks = Arc::new(if cfg.auth.substrate_signing_key_path.exists() {
        let pub_path = cfg
            .auth
            .substrate_signing_key_path
            .with_extension("pub.pem");
        Keystore::load_from_files(&cfg.auth.substrate_signing_key_path, &pub_path)?
    } else {
        Keystore::generate_dev()
    });
    publish_jwks(&ks, &cfg.auth.substrate_jwks_publish_path)?;

    let handles = build_handles(cfg, Arc::clone(&lago));
    let routing = Arc::clone(&handles.routing);
    let revoked = Arc::clone(&handles.revoked);
    let idem = Arc::clone(&handles.idem);
    let saga_registry = Arc::clone(&handles.saga_registry);
    let approval_locks = Arc::clone(&handles.approval_locks);
    let saga = build_saga_driver(Arc::clone(&saga_registry), Arc::clone(&lago));
    let admin_policy = build_admin_policy(cfg);

    let services = build_services(
        Arc::clone(&arcan),
        Arc::clone(&lago),
        Arc::clone(&haima),
        Arc::clone(&anima),
        Arc::clone(&routing),
        Arc::clone(&revoked),
        Arc::clone(&idem),
        Arc::clone(&approval_locks),
        Arc::clone(&saga_registry),
        Arc::clone(&admin_policy),
        Arc::clone(&ks),
        saga,
    );

    let jwks = if cfg.auth.jwks_path.exists() {
        Arc::new(JwksCache::load_from_path(&cfg.auth.jwks_path)?)
    } else {
        tracing::warn!(
            path = %cfg.auth.jwks_path.display(),
            "lifegw JWKS missing — using built-in dev keystore"
        );
        Arc::new(JwksCache::dev_only())
    };
    let auth = AuthLayer::new(jwks);

    // Spec C₂ §5.4 + §6.3 sweepers: revocation snapshot + routing-cache eviction.
    spawn_revoked_snapshot_sweeper(Arc::clone(&revoked), cfg.auth.revoked_sids_path.clone());
    spawn_routing_eviction_sweeper(
        Arc::clone(&routing),
        std::time::Duration::from_secs(cfg.routing.idle_threshold_secs),
        cfg.routing.hard_cap,
        std::time::Duration::from_secs(cfg.routing.eviction_interval_secs),
    );

    serve_planes(cfg, auth, services, shutdown_rx).await
}

fn build_handles(cfg: &LifedConfig, lago: Arc<dyn LagoCall>) -> LifedHandles {
    let routing = Arc::new(RoutingCache::new());
    let revoked = Arc::new(RevokedSidSet::new());
    let idem: Arc<dyn IdempotencyStore> = match cfg.idempotency.backend {
        crate::config::IdempotencyBackend::Lago => Arc::new(LagoBackedStore::new(lago)),
        crate::config::IdempotencyBackend::InMemory => {
            boxed_in_memory(std::time::Duration::from_secs(cfg.idempotency.ttl_secs))
        }
    };
    let saga_registry = Arc::new(SagaRegistry::new());
    let approval_locks = Arc::new(crate::services::agent::ApprovalLocks::new());
    LifedHandles {
        routing,
        revoked,
        idem,
        saga_registry,
        approval_locks,
    }
}

fn build_saga_driver(registry: Arc<SagaRegistry>, lago: Arc<dyn LagoCall>) -> Arc<SagaDriver> {
    let journal: Arc<dyn SagaJournal> = Arc::new(LagoSagaJournal::new(lago));
    Arc::new(SagaDriver::with_registry(
        "lifed-runtime",
        registry,
        journal,
    ))
}

fn build_admin_policy(cfg: &LifedConfig) -> Arc<AdminPolicy> {
    let admin_gid = cfg
        .admin_plane
        .unix_socket_group
        .as_deref()
        .and_then(|g| peercred::group_gid(g).ok().flatten())
        .unwrap_or(0);
    Arc::new(AdminPolicy {
        admin_gid,
        autonomic_uid: None, // wired in C₆ alongside autonomic-as-Π
    })
}

#[allow(clippy::too_many_arguments)]
struct LifedServices {
    agent: AgentService,
    events: EventsService,
    wallet: WalletService,
    identity: IdentityService,
    runtime_admin: RuntimeAdminService,
    saga_admin: SagaAdminService,
    routing_admin: RoutingCacheAdminService,
}

#[allow(clippy::too_many_arguments)]
fn build_services(
    arcan: Arc<dyn ArcanCall>,
    lago: Arc<dyn LagoCall>,
    haima: Arc<dyn HaimaCall>,
    anima: Arc<dyn AnimaCall>,
    routing: Arc<RoutingCache>,
    revoked: Arc<RevokedSidSet>,
    idem: Arc<dyn IdempotencyStore>,
    approval_locks: Arc<crate::services::agent::ApprovalLocks>,
    saga_registry: Arc<SagaRegistry>,
    admin_policy: Arc<AdminPolicy>,
    ks: Arc<Keystore>,
    saga: Arc<SagaDriver>,
) -> LifedServices {
    let agent = AgentService::new(
        Arc::clone(&arcan),
        Arc::clone(&lago),
        Arc::clone(&haima),
        Arc::clone(&anima),
        Arc::clone(&routing),
        Arc::clone(&ks),
        Arc::clone(&saga),
        Arc::clone(&approval_locks),
    );
    let events = EventsService::new(Arc::clone(&lago));
    let wallet = WalletService::new(Arc::clone(&haima), Arc::clone(&idem));
    let identity = IdentityService::new(
        Arc::clone(&anima),
        Arc::clone(&routing),
        Arc::clone(&revoked),
    );
    let runtime_admin = RuntimeAdminService::new(
        Arc::clone(&admin_policy),
        Arc::clone(&routing),
        Arc::clone(&idem),
    );
    let saga_admin = SagaAdminService::new(Arc::clone(&admin_policy), saga_registry);
    let routing_admin = RoutingCacheAdminService::new(Arc::clone(&admin_policy), routing);
    LifedServices {
        agent,
        events,
        wallet,
        identity,
        runtime_admin,
        saga_admin,
        routing_admin,
    }
}

async fn serve_planes(
    cfg: &LifedConfig,
    auth: AuthLayer,
    services: LifedServices,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifedResult<()> {
    let LifedServices {
        agent,
        events,
        wallet,
        identity,
        runtime_admin,
        saga_admin,
        routing_admin,
    } = services;

    // Public-plane router: AuthLayer + four life.v1 services.
    let public_router = Server::builder()
        .layer(auth)
        .add_service(pb::agent_server::AgentServer::new(agent))
        .add_service(pb::events_server::EventsServer::new(events))
        .add_service(pb::wallet_server::WalletServer::new(wallet))
        .add_service(pb::identity_server::IdentityServer::new(identity));

    // Admin-plane router: NO AuthLayer (peer-cred + AdminPolicy gates),
    // three life.admin.v1 services.
    let admin_router = Server::builder()
        .add_service(adm::runtime_server::RuntimeServer::new(runtime_admin))
        .add_service(adm::saga_server::SagaServer::new(saga_admin))
        .add_service(adm::routing_cache_server::RoutingCacheServer::new(
            routing_admin,
        ));

    let public_incoming = public_listener::bind(&cfg.public_plane).await?;
    let admin_incoming = admin_listener::bind(&cfg.admin_plane).await?;

    // Fork shutdown into two — one for each plane. When the outer signal
    // fires, both planes drain.
    let (public_shutdown_tx, public_shutdown_rx) = oneshot::channel::<()>();
    let (admin_shutdown_tx, admin_shutdown_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = shutdown_rx.await;
        let _ = public_shutdown_tx.send(());
        let _ = admin_shutdown_tx.send(());
    });

    let admin_handle = tokio::spawn(async move {
        admin_router
            .serve_with_incoming_shutdown(
                admin_incoming,
                admin_listener::shutdown_signal(admin_shutdown_rx),
            )
            .await
    });

    let public_result = public_router
        .serve_with_incoming_shutdown(
            public_incoming,
            public_listener::shutdown_signal(public_shutdown_rx),
        )
        .await
        .map_err(|e| LifedError::Server(format!("public-plane serve: {e}")));

    // Drain admin plane after public plane finishes.
    if let Err(e) = admin_handle.await {
        tracing::warn!(error = ?e, "admin-plane task join failed");
    }

    public_result
}

fn publish_jwks(ks: &Keystore, path: &std::path::Path) -> LifedResult<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)
            .map_err(|e| LifedError::Auth(format!("create {}: {e}", p.display())))?;
    }
    let jwks_json = serde_json::to_string_pretty(&ks.publish_jwks())
        .map_err(|e| LifedError::Auth(format!("jwks json: {e}")))?;
    std::fs::write(path, jwks_json)
        .map_err(|e| LifedError::Auth(format!("write {}: {e}", path.display())))?;
    Ok(())
}

fn spawn_revoked_snapshot_sweeper(revoked: Arc<RevokedSidSet>, path: std::path::PathBuf) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tick.tick().await;
            if let Err(e) = revoked.write_snapshot_to(&path) {
                tracing::warn!(path = %path.display(), error = ?e, "revoked-sids snapshot write failed");
            }
        }
    });
}

fn spawn_routing_eviction_sweeper(
    routing: Arc<RoutingCache>,
    idle: std::time::Duration,
    hard_cap: usize,
    interval: std::time::Duration,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            routing.evict_idle(idle);
            routing.evict_to_cap(hard_cap);
        }
    });
}
