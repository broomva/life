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
use crate::dev_mocks::{MockArcan, MockSubstrates};
use crate::error::{LifedError, LifedResult};
use crate::idempotency::{IdempotencyStore, boxed_in_memory};
use crate::listener::admin as admin_listener;
use crate::listener::public as public_listener;
use crate::routing::cache::RoutingCache;
use crate::routing::pools::{Pool, SubstrateKind, SubstratePools, SubstratePoolsInitial};
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
///
/// Sub-phase D: also exposes `pools` so chaos / backpressure tests can
/// read the breaker state without rebuilding the daemon.
#[derive(Clone)]
pub struct LifedHandles {
    pub routing: Arc<RoutingCache>,
    pub revoked: Arc<RevokedSidSet>,
    pub idem: Arc<dyn IdempotencyStore>,
    pub saga_registry: Arc<SagaRegistry>,
    pub approval_locks: Arc<crate::services::agent::ApprovalLocks>,
    pub pools: Arc<SubstratePools>,
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

    // Stage 2 (May 2026): lazy file-backed JWKS — no boot-order race.
    // The previous `if exists() { real } else { dev_only }` branch
    // pinned the cache identity at boot time, which inside the Railway
    // lifegw-stack container caused lifed to ALWAYS land on dev_only
    // (lifegw publishes the JWKS ~100 ms after lifed binds its UDS).
    // The new cache lazy-reads on first verify + on mtime change, so
    // it picks up lifegw's publish without coordination. The dev
    // shortcut is now an explicit additive flag — production deploys
    // leave `cfg.auth.dev_signer_enabled = false` and reject the
    // shortcut outright.
    let jwks = if cfg.auth.dev_signer_enabled {
        Arc::new(JwksCache::new_lazy_file_with_dev_shortcut(
            &cfg.auth.jwks_path,
        ))
    } else {
        Arc::new(JwksCache::new_lazy_file(&cfg.auth.jwks_path))
    };
    tracing::info!(
        path = %cfg.auth.jwks_path.display(),
        dev_shortcut = cfg.auth.dev_signer_enabled,
        "jwks cache initialised (lazy file-backed)"
    );
    let auth = AuthLayer::new(Arc::clone(&jwks));

    // Sub-phase E: build pools first so we can wrap each substrate impl
    // in its proxy crate's `Pooled<C>` adapter — the same pool lives on
    // the trait object handlers consume AND on the `LifedHandles::pools`
    // exposed to admin/integration tests.
    let skel = build_handles_skeleton(cfg);
    let pools_for_handlers = Arc::clone(&skel.pools);

    // Stage 3b (May 2026): per-substrate backend selection. Today the
    // only knob is the `arcan` substrate, gated by
    // `LIFED_ARCAN_BACKEND`:
    //
    //   - unset / "mock"             → MockArcan (canned events; default)
    //   - "vercel_ai_gateway"        → VercelAiGatewayArcan (real LLM
    //                                  streaming via OpenAI-compatible
    //                                  endpoint; reads OPENAI_API_KEY +
    //                                  OPENAI_BASE_URL + OPENAI_MODEL)
    //
    // This is transitional: when `arcan-proto` ships and arcand exposes
    // a real tonic UDS server, the canonical path becomes the existing
    // mock-fallback gate (`run_with_real_substrates`). The per-backend
    // selection here lets us flip on real LLM tokens NOW without
    // blocking on the wider arcan-proto rollout.
    //
    // Other substrates (lago / haima / anima) stay on mocks — their
    // tonic-substrate equivalents are the canonical Spec C₂ §11.4 path.
    let arcan: Arc<dyn ArcanCall> =
        build_arcan_fallback_slot(&mocks.arcan, pools_for_handlers.arcan.load_full())?;
    let lago: Arc<dyn LagoCall> = Arc::new(lago_proxy::Pooled::new(
        mocks.lago.clone(),
        pools_for_handlers.lago.load_full(),
    ));
    let haima: Arc<dyn HaimaCall> = Arc::new(haima_proxy::Pooled::new(
        mocks.haima.clone(),
        pools_for_handlers.haima.load_full(),
    ));
    let anima: Arc<dyn AnimaCall> = Arc::new(anima_proxy::Pooled::new(
        mocks.anima.clone(),
        pools_for_handlers.anima.load_full(),
    ));

    // Idempotency store sees the already-Pooled lago so its persists
    // bracket through the breaker uniformly.
    let idem: Arc<dyn IdempotencyStore> = match cfg.idempotency.backend {
        crate::config::IdempotencyBackend::Lago => {
            Arc::new(LagoBackedStore::new(Arc::clone(&lago)))
        }
        crate::config::IdempotencyBackend::InMemory => {
            boxed_in_memory(std::time::Duration::from_secs(cfg.idempotency.ttl_secs))
        }
    };

    let handles = LifedHandles {
        routing: skel.routing,
        revoked: skel.revoked,
        idem: Arc::clone(&idem),
        saga_registry: skel.saga_registry,
        approval_locks: skel.approval_locks,
        pools: skel.pools,
    };

    let routing = Arc::clone(&handles.routing);
    let revoked = Arc::clone(&handles.revoked);
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

/// Sub-phase B daemon entrypoint.
///
/// Sub-phase D follow-up #8: the silent mock-fallback that sub-phase B
/// shipped is now gated behind `allow_mock_fallback`. Production
/// deployments leave this `false` and lifed fails fast with
/// [`LifedError::Substrate`] when a substrate socket is missing —
/// which matches Spec C₂ §11.4's expectation that systemd must
/// re-launch a daemon whose dependencies aren't ready. Dev and CI
/// boxes pass `--allow-mock-fallback` (or `LIFED_ALLOW_MOCK_FALLBACK=1`)
/// when they want the documented mock-substrate path.
///
/// Stage 5 (June 2026): substrate selection is now PER-SUBSTRATE
/// instead of all-or-nothing. Each of the four substrates whose UDS
/// socket is present at boot dials the real `*-proxy` client; the
/// absent ones fall back to the in-process mock (still gated by
/// `allow_mock_fallback`). The boot log names exactly which substrates
/// run real vs mock, so a container that ships only arcand serves real
/// agent traffic while lago/haima/anima honestly report as mocked.
pub async fn run_daemon(config_path: Option<&Path>, allow_mock_fallback: bool) -> LifedResult<()> {
    let cfg = LifedConfig::load(config_path)?;
    let _vigil_guard = crate::observability::init(&cfg.vigil)?;
    let shutdown_rx = crate::shutdown::install_signal_handler();
    let presence = SocketPresence::from_config(&cfg);
    let selection = select_substrates(presence, allow_mock_fallback).map_err(|_missing| {
        let missing = list_missing_substrate_sockets(&cfg);
        LifedError::Substrate(format!(
            "substrate UDS socket(s) missing — refusing to boot with MockSubstrates by default. \
             Pass --allow-mock-fallback (or LIFED_ALLOW_MOCK_FALLBACK=1) to opt into the dev path. \
             Missing sockets: {missing:?}"
        ))
    })?;
    run_with_substrate_selection(&cfg, selection, shutdown_rx).await
}

/// Per-substrate backend decision (Stage 5, June 2026). `Real` means
/// the substrate's UDS socket was present at boot and lifed dials the
/// real `*-proxy` client; `Mock` means the in-process mock stands in
/// (only ever chosen when `--allow-mock-fallback` is set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubstrateBackend {
    Real,
    Mock,
}

impl SubstrateBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Real => "real",
            Self::Mock => "mock",
        }
    }
}

/// Which backend each of the four substrates boots with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubstrateSelection {
    pub arcan: SubstrateBackend,
    pub lago: SubstrateBackend,
    pub haima: SubstrateBackend,
    pub anima: SubstrateBackend,
}

impl SubstrateSelection {
    pub fn all_real() -> Self {
        Self {
            arcan: SubstrateBackend::Real,
            lago: SubstrateBackend::Real,
            haima: SubstrateBackend::Real,
            anima: SubstrateBackend::Real,
        }
    }

    pub fn all_mock() -> Self {
        Self {
            arcan: SubstrateBackend::Mock,
            lago: SubstrateBackend::Mock,
            haima: SubstrateBackend::Mock,
            anima: SubstrateBackend::Mock,
        }
    }

    fn slots(&self) -> [(&'static str, SubstrateBackend); 4] {
        [
            ("arcan", self.arcan),
            ("lago", self.lago),
            ("haima", self.haima),
            ("anima", self.anima),
        ]
    }

    pub fn any_mock(&self) -> bool {
        self.slots()
            .iter()
            .any(|(_, b)| *b == SubstrateBackend::Mock)
    }

    pub fn any_real(&self) -> bool {
        self.slots()
            .iter()
            .any(|(_, b)| *b == SubstrateBackend::Real)
    }

    /// Names of the substrates running on in-process mocks.
    pub fn mock_names(&self) -> Vec<&'static str> {
        self.slots()
            .iter()
            .filter(|(_, b)| *b == SubstrateBackend::Mock)
            .map(|(name, _)| *name)
            .collect()
    }

    /// Boot-summary line, e.g. `arcan=real lago=mock haima=mock anima=mock`.
    pub fn summary(&self) -> String {
        self.slots()
            .iter()
            .map(|(name, b)| format!("{name}={}", b.as_str()))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Socket-presence snapshot for the four substrate UDS paths. Sampled
/// once at boot — substrates that come up after lifed boots are not
/// picked up until a restart (matching the pre-Stage-5 behavior).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketPresence {
    pub arcan: bool,
    pub lago: bool,
    pub haima: bool,
    pub anima: bool,
}

impl SocketPresence {
    pub fn from_config(cfg: &LifedConfig) -> Self {
        Self {
            arcan: cfg.substrates.arcan.unix_socket.exists(),
            lago: cfg.substrates.lago.unix_socket.exists(),
            haima: cfg.substrates.haima.unix_socket.exists(),
            anima: cfg.substrates.anima.unix_socket.exists(),
        }
    }
}

/// Pure per-substrate selection logic (Stage 5). A present socket
/// always selects the real proxy; an absent socket selects the mock
/// when `allow_mock_fallback` is set, otherwise the substrate's name
/// lands in the `Err` list so the caller can fail fast naming exactly
/// what is missing.
pub fn select_substrates(
    presence: SocketPresence,
    allow_mock_fallback: bool,
) -> Result<SubstrateSelection, Vec<&'static str>> {
    let pick = |present: bool| {
        if present {
            SubstrateBackend::Real
        } else {
            SubstrateBackend::Mock
        }
    };
    let selection = SubstrateSelection {
        arcan: pick(presence.arcan),
        lago: pick(presence.lago),
        haima: pick(presence.haima),
        anima: pick(presence.anima),
    };
    if !allow_mock_fallback && selection.any_mock() {
        return Err(selection.mock_names());
    }
    Ok(selection)
}

fn list_missing_substrate_sockets(cfg: &LifedConfig) -> Vec<String> {
    let mut out = Vec::new();
    if !cfg.substrates.arcan.unix_socket.exists() {
        out.push(format!(
            "arcan@{}",
            cfg.substrates.arcan.unix_socket.display()
        ));
    }
    if !cfg.substrates.lago.unix_socket.exists() {
        out.push(format!(
            "lago@{}",
            cfg.substrates.lago.unix_socket.display()
        ));
    }
    if !cfg.substrates.haima.unix_socket.exists() {
        out.push(format!(
            "haima@{}",
            cfg.substrates.haima.unix_socket.display()
        ));
    }
    if !cfg.substrates.anima.unix_socket.exists() {
        out.push(format!(
            "anima@{}",
            cfg.substrates.anima.unix_socket.display()
        ));
    }
    out
}

/// Stage 3b (May 2026): per-substrate backend choice for the `arcan`
/// slot. Selected via the `LIFED_ARCAN_BACKEND` env var.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArcanBackendChoice {
    /// `MockArcan` — canned events, no LLM. The default.
    Mock,
    /// `VercelAiGatewayArcan` — real chat-completions streaming via
    /// any OpenAI-compatible endpoint (Vercel AI Gateway by default).
    /// Requires `OPENAI_API_KEY` (and optionally `OPENAI_BASE_URL`,
    /// `OPENAI_MODEL`).
    VercelAiGateway,
}

fn arcan_backend_from_env() -> ArcanBackendChoice {
    match std::env::var("LIFED_ARCAN_BACKEND") {
        Ok(s) => match s.trim().to_ascii_lowercase().as_str() {
            "vercel_ai_gateway" | "vercel-ai-gateway" => ArcanBackendChoice::VercelAiGateway,
            "mock" | "" => ArcanBackendChoice::Mock,
            other => {
                tracing::warn!(
                    value = other,
                    "LIFED_ARCAN_BACKEND has unknown value; falling back to mock",
                );
                ArcanBackendChoice::Mock
            }
        },
        Err(_) => ArcanBackendChoice::Mock,
    }
}

/// Build the arcan slot when its UDS socket is ABSENT: either the
/// transitional `VercelAiGatewayArcan` (Stage 3b,
/// `LIFED_ARCAN_BACKEND=vercel_ai_gateway`) or the in-process
/// `MockArcan`. Shared by [`run_with_mocks_handles`] and the
/// per-substrate daemon path so the env-knob semantics stay identical.
fn build_arcan_fallback_slot(mock: &MockArcan, pool: Arc<Pool>) -> LifedResult<Arc<dyn ArcanCall>> {
    match arcan_backend_from_env() {
        ArcanBackendChoice::Mock => Ok(Arc::new(arcan_proxy::Pooled::new(mock.clone(), pool))),
        ArcanBackendChoice::VercelAiGateway => {
            match arcan_proxy::VercelAiGatewayArcan::from_env() {
                Ok(real) => {
                    tracing::info!(
                        "lifed: arcan substrate using VercelAiGatewayArcan (real LLM streaming)"
                    );
                    Ok(Arc::new(arcan_proxy::Pooled::new(real, pool)))
                }
                Err(e) => {
                    // Operator selected the real backend but the env is
                    // missing the API key. Fail fast at boot — silent
                    // fallback to the mock would mislead operators expecting
                    // real LLM output.
                    Err(LifedError::Substrate(format!(
                        "LIFED_ARCAN_BACKEND=vercel_ai_gateway requires OPENAI_API_KEY \
                     (and optionally OPENAI_BASE_URL / OPENAI_MODEL): {e}"
                    )))
                }
            }
        }
    }
}

/// Sub-phase B real-substrate entrypoint per Spec C₂ §12.B. Stage 5
/// turned this into a thin wrapper over the per-substrate path with an
/// all-real selection: dials the four substrate UDS sockets, mints +
/// publishes the substrate-token JWKS, builds the public-plane router,
/// and serves until the shutdown channel fires.
pub async fn run_with_real_substrates(
    cfg: &LifedConfig,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifedResult<()> {
    run_with_substrate_selection(cfg, SubstrateSelection::all_real(), shutdown_rx).await
}

/// Stage 5 (June 2026): per-substrate daemon path. Each of the four
/// substrates independently boots `Real` (UDS socket present at boot →
/// real `*-proxy` dial) or `Mock` (in-process mock). This replaces the
/// all-or-nothing real-vs-mock split: a container that ships only
/// arcand serves real agent traffic while lago/haima/anima stay
/// mocked, with the boot summary naming exactly which substrates are
/// which. The saga/pool/breaker wiring is identical across backends —
/// every slot brackets through its `SubstratePools` pool, either via
/// the proxy's `with_pool` (real) or the `Pooled<C>` adapter (mock).
pub async fn run_with_substrate_selection(
    cfg: &LifedConfig,
    selection: SubstrateSelection,
    shutdown_rx: oneshot::Receiver<()>,
) -> LifedResult<()> {
    let _vigil_guard = crate::observability::init(&cfg.vigil)?;
    tracing::info!(
        public_socket = %cfg.public_plane.unix_socket.display(),
        admin_socket  = %cfg.admin_plane.unix_socket.display(),
        substrates    = %selection.summary(),
        "lifed starting (stage 5 — per-substrate selection)",
    );
    if selection.any_mock() {
        tracing::warn!(
            substrates = %selection.summary(),
            mock = ?selection.mock_names(),
            "substrate UDS socket(s) missing — per-substrate mock fallback active; \
             traffic to the mocked substrates is canned, NOT real",
        );
    }

    // Sub-phase E: pool ownership pushed inside each `*Proxy` via
    // `with_pool`. Handlers no longer bracket — every call through the
    // trait object brackets internally per Spec C₂ §7. Mock slots wrap
    // in the proxy crate's `Pooled<C>` adapter so the breaker exercises
    // identical paths.
    let mocks = MockSubstrates::new();
    let skel = build_handles_skeleton(cfg);

    let arcan: Arc<dyn ArcanCall> = match selection.arcan {
        SubstrateBackend::Real => {
            if arcan_backend_from_env() != ArcanBackendChoice::Mock {
                tracing::warn!(
                    "arcan UDS socket present — ignoring LIFED_ARCAN_BACKEND override; \
                     the real arcand substrate wins"
                );
            }
            let proxy = ArcanProxy::connect(cfg.substrates.arcan.unix_socket.clone())
                .await
                .map_err(|e| LifedError::Substrate(format!("arcan dial: {e}")))?;
            Arc::new(proxy.with_pool(skel.pools.arcan.load_full()))
        }
        SubstrateBackend::Mock => {
            build_arcan_fallback_slot(&mocks.arcan, skel.pools.arcan.load_full())?
        }
    };
    let lago: Arc<dyn LagoCall> = match selection.lago {
        SubstrateBackend::Real => {
            let proxy = LagoProxy::connect(cfg.substrates.lago.unix_socket.clone())
                .await
                .map_err(|e| LifedError::Substrate(format!("lago dial: {e}")))?;
            Arc::new(proxy.with_pool(skel.pools.lago.load_full()))
        }
        SubstrateBackend::Mock => Arc::new(lago_proxy::Pooled::new(
            mocks.lago.clone(),
            skel.pools.lago.load_full(),
        )),
    };
    let haima: Arc<dyn HaimaCall> = match selection.haima {
        SubstrateBackend::Real => {
            let proxy = HaimaProxy::connect(cfg.substrates.haima.unix_socket.clone())
                .await
                .map_err(|e| LifedError::Substrate(format!("haima dial: {e}")))?;
            Arc::new(proxy.with_pool(skel.pools.haima.load_full()))
        }
        SubstrateBackend::Mock => Arc::new(haima_proxy::Pooled::new(
            mocks.haima.clone(),
            skel.pools.haima.load_full(),
        )),
    };
    let anima: Arc<dyn AnimaCall> = match selection.anima {
        SubstrateBackend::Real => {
            let proxy = AnimaProxy::connect(cfg.substrates.anima.unix_socket.clone())
                .await
                .map_err(|e| LifedError::Substrate(format!("anima dial: {e}")))?;
            Arc::new(proxy.with_pool(skel.pools.anima.load_full()))
        }
        SubstrateBackend::Mock => Arc::new(anima_proxy::Pooled::new(
            mocks.anima.clone(),
            skel.pools.anima.load_full(),
        )),
    };

    // Substrate-token signing keystore + JWKS publish. Real substrates
    // verify lifed's Tier-3 tokens against the published JWKS, so any
    // real substrate ⇒ publish. The all-mock boot keeps the historical
    // behavior (no publish — mocks never verify tokens, and dev boxes
    // may not have write access to the publish path).
    let ks = Arc::new(if cfg.auth.substrate_signing_key_path.exists() {
        let pub_path = cfg
            .auth
            .substrate_signing_key_path
            .with_extension("pub.pem");
        Keystore::load_from_files(&cfg.auth.substrate_signing_key_path, &pub_path)?
    } else {
        Keystore::generate_dev()
    });
    if selection.any_real() {
        publish_jwks(&ks, &cfg.auth.substrate_jwks_publish_path)?;
    }

    let idem: Arc<dyn IdempotencyStore> = match cfg.idempotency.backend {
        crate::config::IdempotencyBackend::Lago => {
            Arc::new(LagoBackedStore::new(Arc::clone(&lago)))
        }
        crate::config::IdempotencyBackend::InMemory => {
            boxed_in_memory(std::time::Duration::from_secs(cfg.idempotency.ttl_secs))
        }
    };

    let handles = LifedHandles {
        routing: skel.routing,
        revoked: skel.revoked,
        idem: Arc::clone(&idem),
        saga_registry: skel.saga_registry,
        approval_locks: skel.approval_locks,
        pools: skel.pools,
    };

    let routing = Arc::clone(&handles.routing);
    let revoked = Arc::clone(&handles.revoked);
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

    // Stage 2 (May 2026): lazy file-backed JWKS — no boot-order race
    // with lifegw (see the matching comment in `run_with_mocks_handles`).
    // Stage 5 unifies the daemon paths on the lazy cache; the previous
    // eager `load_from_path`-or-`dev_only` branch silently accepted dev
    // tokens in production whenever lifegw published a few ms late.
    let jwks = if cfg.auth.dev_signer_enabled {
        Arc::new(JwksCache::new_lazy_file_with_dev_shortcut(
            &cfg.auth.jwks_path,
        ))
    } else {
        Arc::new(JwksCache::new_lazy_file(&cfg.auth.jwks_path))
    };
    tracing::info!(
        path = %cfg.auth.jwks_path.display(),
        dev_shortcut = cfg.auth.dev_signer_enabled,
        "jwks cache initialised (lazy file-backed)"
    );
    let auth = AuthLayer::new(jwks);

    // Spec C₂ §5.4 + §6.3 sweepers: revocation snapshot + routing-cache
    // eviction. The snapshot sweeper writes to disk every 30s, so it
    // only runs when at least one substrate is real (all-mock dev boxes
    // historically ran sweeper-free and may not have the path writable);
    // the eviction sweeper is in-memory and always safe to run.
    if selection.any_real() {
        spawn_revoked_snapshot_sweeper(Arc::clone(&revoked), cfg.auth.revoked_sids_path.clone());
    }
    spawn_routing_eviction_sweeper(
        Arc::clone(&routing),
        std::time::Duration::from_secs(cfg.routing.idle_threshold_secs),
        cfg.routing.hard_cap,
        std::time::Duration::from_secs(cfg.routing.eviction_interval_secs),
    );

    serve_planes(cfg, auth, services, shutdown_rx).await
}

/// Sub-phase E: pre-Pooled handle constructor. Builds everything except
/// the lago-backed idempotency store; the caller wraps lago in
/// `lago_proxy::Pooled<...>` (using the `pools.lago` from these handles)
/// then calls [`LifedHandles::with_idem_from_lago`] to attach the
/// final idem store.
fn build_handles_skeleton(cfg: &LifedConfig) -> LifedHandlesSkeleton {
    let routing = Arc::new(RoutingCache::new());
    let revoked = Arc::new(RevokedSidSet::new());
    let saga_registry = Arc::new(SagaRegistry::new());
    let approval_locks = Arc::new(crate::services::agent::ApprovalLocks::new());
    let pools = build_substrate_pools(cfg);
    LifedHandlesSkeleton {
        routing,
        revoked,
        saga_registry,
        approval_locks,
        pools,
    }
}

struct LifedHandlesSkeleton {
    routing: Arc<RoutingCache>,
    revoked: Arc<RevokedSidSet>,
    saga_registry: Arc<SagaRegistry>,
    approval_locks: Arc<crate::services::agent::ApprovalLocks>,
    pools: Arc<SubstratePools>,
}

/// Sub-phase D: construct [`SubstratePools`] from the config-supplied
/// per-substrate capacities. Each pool wraps a tonic Channel built
/// against the substrate's UDS path — but the channel is built lazily
/// (`connect_lazy`) so the pool exists even when the substrate is
/// down. The breaker layer then trips fast on every dispatch attempt
/// against an offline substrate.
fn build_substrate_pools(cfg: &LifedConfig) -> Arc<SubstratePools> {
    let chan = || {
        // A dummy lazy channel; production deployments use the real
        // proxy's tonic channel once Sub-phase E moves the pool inside
        // the proxy. For pool-bracketing semantics the channel itself
        // is unused — the breaker + semaphore are the load-bearing
        // pieces.
        tonic::transport::Endpoint::try_from("http://[::]:0")
            .expect("static endpoint")
            .connect_lazy()
    };
    Arc::new(SubstratePools::new(SubstratePoolsInitial {
        arcan: Pool::new(chan(), cfg.pools.arcan_capacity, SubstrateKind::Arcan),
        lago: Pool::new(chan(), cfg.pools.lago_capacity, SubstrateKind::Lago),
        haima: Pool::new(chan(), cfg.pools.haima_capacity, SubstrateKind::Haima),
        anima: Pool::new(chan(), cfg.pools.anima_capacity, SubstrateKind::Anima),
        soma: Pool::new(chan(), cfg.pools.soma_capacity, SubstrateKind::Soma),
    }))
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
    // When `unix_socket_group` is None the operator hasn't asked us to
    // enforce a group filter — fall back to permissive mode (Spec C₂
    // §5.3 expects systemd's SocketGroup directive to enforce access at
    // the filesystem layer in that case, and tests rely on this for the
    // tempdir socket that has no group).
    match cfg.admin_plane.unix_socket_group.as_deref() {
        None => Arc::new(AdminPolicy {
            admin_gid: 0,
            autonomic_uid: None,
            permissive: true,
        }),
        Some(group) => {
            let admin_gid = peercred::group_gid(group).ok().flatten().unwrap_or(0);
            Arc::new(AdminPolicy {
                admin_gid,
                autonomic_uid: None, // wired in C₆ alongside autonomic-as-Π
                permissive: false,
            })
        }
    }
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
    // Sub-phase E: handlers no longer accept a `pools` field — pool
    // bracketing happens inside each proxy crate's `Pooled<C>` adapter.
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
    let routing_admin =
        RoutingCacheAdminService::new(Arc::clone(&admin_policy), routing, Arc::clone(&lago));
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

    // Public-plane router: TracePropagationLayer + HandlerMetricsLayer +
    // AuthLayer + four life.v1 services. Spec C₂ §9.1: trace propagation
    // is an outer layer so the per-request span begins before
    // authentication runs. Spec C₂ §9.3: HandlerMetricsLayer records
    // `life.daemon.handler.duration_ms{namespace,method}`.
    let public_router = Server::builder()
        .layer(crate::observability::TracePropagationLayer)
        .layer(crate::observability::HandlerMetricsLayer)
        .layer(auth)
        .add_service(pb::agent_server::AgentServer::new(agent))
        .add_service(pb::events_server::EventsServer::new(events))
        .add_service(pb::wallet_server::WalletServer::new(wallet))
        .add_service(pb::identity_server::IdentityServer::new(identity));

    // Admin-plane router: TracePropagationLayer + HandlerMetricsLayer
    // (no AuthLayer — peer-cred + AdminPolicy gates), three life.admin.v1
    // services.
    let admin_router = Server::builder()
        .layer(crate::observability::TracePropagationLayer)
        .layer(crate::observability::HandlerMetricsLayer)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(arcan: bool, lago: bool, haima: bool, anima: bool) -> SocketPresence {
        SocketPresence {
            arcan,
            lago,
            haima,
            anima,
        }
    }

    #[test]
    fn all_sockets_present_selects_all_real_regardless_of_fallback_flag() {
        for allow in [false, true] {
            let sel = select_substrates(presence(true, true, true, true), allow)
                .expect("all-present must select");
            assert_eq!(sel, SubstrateSelection::all_real());
            assert!(!sel.any_mock());
            assert!(sel.any_real());
        }
    }

    #[test]
    fn no_sockets_with_fallback_selects_all_mock() {
        let sel = select_substrates(presence(false, false, false, false), true)
            .expect("fallback allows all-mock");
        assert_eq!(sel, SubstrateSelection::all_mock());
        assert!(sel.any_mock());
        assert!(!sel.any_real());
        assert_eq!(sel.mock_names(), vec!["arcan", "lago", "haima", "anima"]);
    }

    #[test]
    fn arcan_only_with_fallback_selects_mixed() {
        // The production rollout shape: lifegw-stack ships arcand but
        // not lagod/haimad/animad.
        let sel = select_substrates(presence(true, false, false, false), true)
            .expect("fallback allows mixed");
        assert_eq!(sel.arcan, SubstrateBackend::Real);
        assert_eq!(sel.lago, SubstrateBackend::Mock);
        assert_eq!(sel.haima, SubstrateBackend::Mock);
        assert_eq!(sel.anima, SubstrateBackend::Mock);
        assert!(sel.any_mock());
        assert!(sel.any_real());
        assert_eq!(sel.mock_names(), vec!["lago", "haima", "anima"]);
    }

    #[test]
    fn each_single_present_socket_maps_to_its_own_slot() {
        let cases = [
            (presence(true, false, false, false), "arcan"),
            (presence(false, true, false, false), "lago"),
            (presence(false, false, true, false), "haima"),
            (presence(false, false, false, true), "anima"),
        ];
        for (p, real_name) in cases {
            let sel = select_substrates(p, true).expect("fallback allows mixed");
            for (name, backend) in sel.slots() {
                if name == real_name {
                    assert_eq!(backend, SubstrateBackend::Real, "{name} should be real");
                } else {
                    assert_eq!(backend, SubstrateBackend::Mock, "{name} should be mock");
                }
            }
        }
    }

    #[test]
    fn missing_sockets_without_fallback_fail_naming_exactly_the_missing() {
        let err = select_substrates(presence(true, false, false, false), false)
            .expect_err("no fallback ⇒ refuse");
        assert_eq!(err, vec!["lago", "haima", "anima"]);

        let err = select_substrates(presence(false, false, false, false), false)
            .expect_err("no fallback ⇒ refuse");
        assert_eq!(err, vec!["arcan", "lago", "haima", "anima"]);

        let err = select_substrates(presence(true, true, true, false), false)
            .expect_err("no fallback ⇒ refuse");
        assert_eq!(err, vec!["anima"]);
    }

    #[test]
    fn summary_names_every_substrate_backend() {
        let sel = select_substrates(presence(true, false, false, false), true).expect("mixed");
        assert_eq!(sel.summary(), "arcan=real lago=mock haima=mock anima=mock");
        assert_eq!(
            SubstrateSelection::all_real().summary(),
            "arcan=real lago=real haima=real anima=real"
        );
        assert_eq!(
            SubstrateSelection::all_mock().summary(),
            "arcan=mock lago=mock haima=mock anima=mock"
        );
    }

    #[test]
    fn socket_presence_from_config_reflects_filesystem() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let mut cfg = LifedConfig::default();
        cfg.substrates.arcan.unix_socket = tmp.path().join("arcan.sock");
        cfg.substrates.lago.unix_socket = tmp.path().join("lago.sock");
        cfg.substrates.haima.unix_socket = tmp.path().join("haima.sock");
        cfg.substrates.anima.unix_socket = tmp.path().join("anima.sock");

        assert_eq!(
            SocketPresence::from_config(&cfg),
            presence(false, false, false, false)
        );

        // Touch only the arcan socket path — `exists()` is the boot
        // probe, so a plain file stands in for the UDS here.
        std::fs::write(&cfg.substrates.arcan.unix_socket, b"").expect("touch arcan.sock");
        assert_eq!(
            SocketPresence::from_config(&cfg),
            presence(true, false, false, false)
        );

        std::fs::write(&cfg.substrates.haima.unix_socket, b"").expect("touch haima.sock");
        assert_eq!(
            SocketPresence::from_config(&cfg),
            presence(true, false, true, false)
        );
    }
}
