//! life.v1.Agent — public-plane agent namespace.
//!
//! Sub-phase B wires real `*-proxy` clients via the per-substrate `*Call`
//! traits. `CreateSession` runs the four-step saga (Spec C₂ §4.2) through
//! the new `SagaDriver`. Per Spec C₂ §1, every public RPC handler reads
//! the capability token, performs a single substrate route OR initiates
//! one saga, and returns. ≤20 LOC per handler is a hard constraint.
//!
//! Sub-phase C wires `ApproveDispatch` first-responder-wins per Spec C₂
//! §6.4: a per-sid `parking_lot::Mutex` whose held value is the
//! `dispatch_id` of the first approver. Subsequent approvals for the same
//! sid see the lock already taken and return `AlreadyExists`. Mutex is
//! parking_lot (NOT tokio::sync) — never held across an await.
//!
//! Sub-phase E pushes pool bracketing inside each proxy crate's
//! `Pooled<C>` adapter; agent handlers no longer carry a `pools` field.
//! The pump path retains its own [`PumpGuard`] for the
//! one-pump-per-session invariant — that guard is panic-safe via RAII
//! `Drop` (Spec C₂ §6.4 invariant).

use std::pin::Pin;
use std::sync::Arc;

use chrono::Utc;
use dashmap::DashMap;
use futures::Stream;
use parking_lot::Mutex as PlMutex;
use sha2::{Digest, Sha256};
use tonic::{Request, Response, Status};

use aios_proto::aios::v1 as aios_v1;
use anima_proxy::AnimaCall;
use arcan_proxy::ArcanCall;
use haima_proxy::HaimaCall;
use lago_proxy::LagoCall;
use life_runtime_proto::life::v1 as pb;

use crate::auth::capability::CapabilityClaims;
use crate::auth::keystore::Keystore;
use crate::routing::cache::RoutingCache;
use crate::routing::fanout::FanoutRegistry;
use crate::saga::driver::{SagaCtx, SagaDriver, SagaStep};
use crate::saga::steps::{BindWallet, CreateAgent, OpenLagoNamespace, RegisterAnimaSession};

/// Per-sid first-responder-wins approval lock per Spec C₂ §6.4. Each
/// inflight approval-pending dispatch holds a slot keyed by `sid`. The
/// first `ApproveDispatch` call wins; concurrent retries see
/// `AlreadyExists`. Uses `parking_lot::Mutex` so the lock is never held
/// across an `await`.
#[derive(Default)]
pub struct ApprovalLocks {
    inner: DashMap<String, Arc<PlMutex<Option<String>>>>,
}

impl ApprovalLocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Try to acquire the slot for `sid` with `dispatch_id`. Returns
    /// `Ok(())` if this caller wins the race; `Err(prior)` if a prior
    /// approval already won (with the prior dispatch_id).
    pub fn try_acquire(&self, sid: &str, dispatch_id: &str) -> Result<(), String> {
        // Insert a fresh slot if absent, then take its parking_lot lock.
        // Cloning the inner Arc lets us drop the DashMap shard guard
        // before locking (DashMap entry guards are sync; we want to keep
        // the section narrow).
        let slot = self
            .inner
            .entry(sid.to_string())
            .or_insert_with(|| Arc::new(PlMutex::new(None)))
            .clone();
        let mut guard = slot.lock();
        match guard.as_ref() {
            Some(prior) => Err(prior.clone()),
            None => {
                *guard = Some(dispatch_id.to_string());
                Ok(())
            }
        }
    }

    /// Release the slot for `sid` (e.g. after `CancelDispatch`). No-op
    /// if absent.
    pub fn release(&self, sid: &str) {
        if let Some(slot) = self.inner.get(sid) {
            *slot.lock() = None;
        }
    }

    /// Number of sids currently tracked. Used by tests + admin
    /// introspection.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Sub-phase E: RAII guard for the per-session upstream pump slot.
///
/// Spec C₂ §6.4 mandates exactly one upstream arcan dispatch pump per
/// session — even if `N` tabs attach to the same sid, only one upstream
/// stream runs and its events fan out to every attached sender. The
/// pump-active flag is a `compare_exchange(false, true)` claim; the
/// release was previously hand-wired in the spawned task's terminal
/// block, which would leak the slot on panic.
///
/// `PumpGuard` wraps the claim in an RAII shape: the guard's `Drop`
/// impl calls `release_pump()` regardless of how the spawned future
/// terminates (clean exit, error, or panic). The pump-future's
/// state-machine retains the guard until completion, so any unwind
/// surfaces the slot release to the next `try_claim_pump` caller
/// without manual bookkeeping.
pub struct PumpGuard {
    registry: Arc<FanoutRegistry>,
    /// Sub-phase E debug surface: the sid that owns the pump claim.
    /// Surfaces in tracing logs so operators can correlate slow pumps
    /// to their session ids.
    pub sid: String,
}

impl PumpGuard {
    /// Try to claim the upstream-pump slot for `sid`. Returns `Some` iff
    /// this caller wins the CAS; `None` if a pump is already in flight
    /// for the session.
    pub fn try_claim(registry: Arc<FanoutRegistry>, sid: String) -> Option<Self> {
        if registry.try_claim_pump() {
            Some(Self { registry, sid })
        } else {
            None
        }
    }
}

impl Drop for PumpGuard {
    fn drop(&mut self) {
        self.registry.release_pump();
    }
}

/// Background pump: fetch the upstream stream from arcan and broadcast
/// every event to every attached tab via the fan-out registry. Spec C₂ §6.4.
///
/// Sub-phase E: pool bracketing has moved INSIDE the arcan proxy crate's
/// `Pooled<C>` adapter — the pump no longer carries a [`PoolGuard`]
/// directly. The `dispatch_message` call brackets internally and returns
/// a `PoolGuardedStream` that records the breaker outcome on terminal
/// poll. The `PumpGuard` retained here only governs the
/// one-pump-per-session invariant. Drop is panic-safe (RAII).
fn spawn_or_attach_fanout_pump(
    fanout: &Arc<FanoutRegistry>,
    arcan: Arc<dyn ArcanCall>,
    sid: String,
    content: String,
) {
    let Some(pump_guard) = PumpGuard::try_claim(Arc::clone(fanout), sid.clone()) else {
        // A pump is already running for this session — the new
        // SendMessage's content is dropped (the upstream pump will
        // continue streaming the in-flight dispatch's events). Real
        // arcan provides queue semantics for chained messages; until
        // arcan-proto ships this is the documented Spec C₂ §6.4 path.
        return;
    };
    let fanout = Arc::clone(fanout);
    tokio::spawn(async move {
        // Sub-phase E: hold the pump_guard across the entire pump
        // lifetime. If the spawned future panics, Drop releases the
        // slot — Spec C₂ §6.4 invariant preserved.
        let _pump_guard = pump_guard;
        match arcan.dispatch_message(&sid, &content).await {
            Ok(mut up) => {
                use futures::StreamExt;
                while let Some(evt) = up.next().await {
                    match evt {
                        Ok(e) => fanout.broadcast(e),
                        Err(_) => break,
                    }
                }
                // PoolGuardedStream records the breaker outcome on
                // terminal poll inside the proxy crate. Nothing else
                // for the pump to do.
            }
            Err(e) => {
                tracing::warn!(sid = %sid, error = ?e, "fanout pump failed to dial arcan");
            }
        }
        // _pump_guard drops here, releasing the per-session slot.
    });
}

/// `Agent` service implementation. Holds typed substrate proxies, the
/// signing keystore, the saga driver, the routing cache, and the per-sid
/// `ApproveDispatch` first-responder lock table.
///
/// Sub-phase E removed the `pools` field — pool bracketing now lives
/// inside each proxy crate's `Pooled<C>` adapter (Spec C₂ §7).
pub struct AgentService {
    pub arcan_call: Arc<dyn ArcanCall>,
    pub lago_call: Arc<dyn LagoCall>,
    pub haima_call: Arc<dyn HaimaCall>,
    pub anima_call: Arc<dyn AnimaCall>,
    pub routing: Arc<RoutingCache>,
    pub ks: Arc<Keystore>,
    pub saga: Arc<SagaDriver>,
    pub approval_locks: Arc<ApprovalLocks>,
}

impl AgentService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        arcan_call: Arc<dyn ArcanCall>,
        lago_call: Arc<dyn LagoCall>,
        haima_call: Arc<dyn HaimaCall>,
        anima_call: Arc<dyn AnimaCall>,
        routing: Arc<RoutingCache>,
        ks: Arc<Keystore>,
        saga: Arc<SagaDriver>,
        approval_locks: Arc<ApprovalLocks>,
    ) -> Self {
        Self {
            arcan_call,
            lago_call,
            haima_call,
            anima_call,
            routing,
            ks,
            saga,
            approval_locks,
        }
    }

    fn claims<T>(req: &Request<T>) -> Result<&CapabilityClaims, Status> {
        req.extensions()
            .get::<CapabilityClaims>()
            .ok_or_else(|| Status::unauthenticated("missing capability claims"))
    }
}

/// Mint a SessionId from `(user_id, project_id, time, random)`. Sub-phase
/// B keeps the same shape as sub-phase A so existing tests stay stable.
fn mint_sid(user_id: &str, project_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(user_id.as_bytes());
    hasher.update(project_id.as_bytes());
    hasher.update(uuid_like::random_bytes_16());
    hasher.update(Utc::now().timestamp_millis().to_be_bytes());
    let digest = hasher.finalize();
    base32::encode(base32::Alphabet::Rfc4648 { padding: false }, &digest[..16]).to_lowercase()
}

mod uuid_like {
    use std::time::{SystemTime, UNIX_EPOCH};
    pub fn random_bytes_16() -> [u8; 16] {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        nanos.to_le_bytes()
    }
}

#[tonic::async_trait]
impl pb::agent_server::Agent for AgentService {
    type SendMessageStream = Pin<Box<dyn Stream<Item = Result<pb::AgentEvent, Status>> + Send>>;
    type StreamSessionStream = Self::SendMessageStream;

    async fn create_session(
        &self,
        req: Request<pb::CreateSessionReq>,
    ) -> Result<Response<pb::Session>, Status> {
        let claims = Self::claims(&req)?.clone();
        let body = req.into_inner();
        let sid_value = mint_sid(&body.user_id, &body.project_id);
        let sid = aios_v1::SessionId {
            value: sid_value.clone(),
        };
        let ctx = SagaCtx {
            saga_id: format!("create-session-{sid_value}"),
            user_id: body.user_id.clone(),
            project_id: body.project_id.clone(),
            sid: sid.clone(),
            idempotency_key: None,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
            trace: tracing::Span::current(),
            claims,
        };
        let steps: Vec<Box<dyn SagaStep>> = vec![
            Box::new(CreateAgent {
                arcan: Arc::clone(&self.arcan_call),
                ks: Arc::clone(&self.ks),
            }),
            Box::new(OpenLagoNamespace {
                lago: Arc::clone(&self.lago_call),
                ks: Arc::clone(&self.ks),
            }),
            Box::new(BindWallet {
                haima: Arc::clone(&self.haima_call),
                ks: Arc::clone(&self.ks),
            }),
            Box::new(RegisterAnimaSession {
                anima: Arc::clone(&self.anima_call),
                ks: Arc::clone(&self.ks),
            }),
        ];
        self.saga
            .run(ctx, steps)
            .await
            .map_err(|e| e.into_status())?;
        self.routing
            .insert_minimal(&sid, &body.user_id, &body.project_id);
        Ok(Response::new(pb::Session {
            sid: Some(sid),
            agent_id: Some(aios_v1::AgentId {
                value: format!("agent-{sid_value}"),
            }),
            user_id: body.user_id,
            project_id: body.project_id,
            created_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        }))
    }

    async fn describe_session(
        &self,
        req: Request<pb::SessionRef>,
    ) -> Result<Response<pb::Session>, Status> {
        let _claims = Self::claims(&req)?;
        let sid = req
            .get_ref()
            .sid
            .clone()
            .ok_or_else(|| Status::invalid_argument("missing sid"))?;
        let entry = self
            .routing
            .lookup(&sid)
            .ok_or_else(|| Status::not_found("session not found"))?;
        Ok(Response::new(pb::Session {
            sid: Some(sid),
            agent_id: Some(aios_v1::AgentId {
                value: entry.agent_id.clone(),
            }),
            user_id: entry.user_id.clone(),
            project_id: entry.project_id.clone(),
            created_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        }))
    }

    async fn close_session(
        &self,
        req: Request<pb::SessionRef>,
    ) -> Result<Response<pb::Empty>, Status> {
        let _claims = Self::claims(&req)?;
        let sid = req
            .get_ref()
            .sid
            .clone()
            .ok_or_else(|| Status::invalid_argument("missing sid"))?;
        self.anima_call
            .mark_session_closed(&sid.value)
            .await
            .map_err(Status::from)?;
        self.routing.evict(&sid);
        Ok(Response::new(pb::Empty {}))
    }

    async fn send_message(
        &self,
        req: Request<pb::SendMessageReq>,
    ) -> Result<Response<Self::SendMessageStream>, Status> {
        let _claims = Self::claims(&req)?;
        let body = req.into_inner();
        let sid_value = body
            .sid
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("missing sid"))?
            .value
            .clone();
        let fanout = self
            .routing
            .lookup_fanout(&aios_v1::SessionId {
                value: sid_value.clone(),
            })
            .ok_or_else(|| Status::not_found("session not found"))?;
        let stream = fanout.attach(64);
        // Sub-phase E: pool bracketing is inside the arcan-proxy `Pooled`
        // adapter; the pump only manages the one-pump-per-session slot
        // via `PumpGuard` (RAII).
        spawn_or_attach_fanout_pump(
            &fanout,
            Arc::clone(&self.arcan_call),
            sid_value,
            body.content,
        );
        Ok(Response::new(Box::pin(stream)))
    }

    async fn stream_session(
        &self,
        req: Request<pb::SessionRef>,
    ) -> Result<Response<Self::StreamSessionStream>, Status> {
        let _claims = Self::claims(&req)?;
        let body = req.get_ref();
        let sid_value = body
            .sid
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("missing sid"))?
            .value
            .clone();
        // M7 Sub-phase D (BRO-938 deviation D2): the WS upgrade carries
        // a resume cursor as `X-Life-Last-Seq-No: N`; lifegw forwards
        // that as `from_sequence` here. Spec C₃ §6.3 LOCKED L4-D3 +
        // Spec C₂ §3.2 SubscribeReq.from_sequence. The current mock
        // arcan / lago substrate wiring streams the live tail without
        // a replay window — propagating the cursor at the lifed seam
        // closes the wire-shape gap so the substrate-side replay pass
        // (Sub-phase E) is a one-line change. We log the cursor for
        // operator visibility into reconnect behaviour.
        let from_sequence = body.from_sequence.unwrap_or(0);
        if from_sequence > 0 {
            tracing::debug!(
                sid = %sid_value,
                from_sequence,
                "stream_session resume cursor received (lago tail replay deferred to E)"
            );
        }
        let fanout = self
            .routing
            .lookup_fanout(&aios_v1::SessionId {
                value: sid_value.clone(),
            })
            .ok_or_else(|| Status::not_found("session not found"))?;
        let stream = fanout.attach(64);
        // Sub-phase D6/E: per-session pump claim. Pool bracketing is now
        // inside arcan-proxy. Subsequent stream_session / send_message
        // calls reuse the active pump.
        spawn_or_attach_fanout_pump(
            &fanout,
            Arc::clone(&self.arcan_call),
            sid_value,
            String::new(),
        );
        Ok(Response::new(Box::pin(stream)))
    }

    async fn approve_dispatch(
        &self,
        req: Request<pb::ApprovalReq>,
    ) -> Result<Response<pb::Empty>, Status> {
        let _claims = Self::claims(&req)?;
        let body = req.into_inner();
        let sid = body
            .sid
            .ok_or_else(|| Status::invalid_argument("missing sid"))?;
        if body.dispatch_id.is_empty() {
            return Err(Status::invalid_argument("missing dispatch_id"));
        }
        // Spec C₂ §6.4: per-sid first-responder-wins. parking_lot::Mutex
        // — never held across an await.
        match self
            .approval_locks
            .try_acquire(&sid.value, &body.dispatch_id)
        {
            Ok(()) => Ok(Response::new(pb::Empty {})),
            Err(prior) => Err(Status::already_exists(format!(
                "dispatch {prior} already approved for session {}",
                sid.value
            ))),
        }
    }

    async fn cancel_dispatch(
        &self,
        req: Request<pb::DispatchRef>,
    ) -> Result<Response<pb::Empty>, Status> {
        let _claims = Self::claims(&req)?;
        let body = req.into_inner();
        let sid = body
            .sid
            .ok_or_else(|| Status::invalid_argument("missing sid"))?;
        // Releasing the slot lets a subsequent re-approval succeed; this
        // matches Spec C₂ §6.4's "approver may revoke + re-approve" loop.
        self.approval_locks.release(&sid.value);
        Ok(Response::new(pb::Empty {}))
    }

    async fn list_skills(
        &self,
        _req: Request<pb::ListSkillsReq>,
    ) -> Result<Response<pb::SkillCatalog>, Status> {
        Ok(Response::new(pb::SkillCatalog { items: vec![] }))
    }

    async fn list_models(
        &self,
        _req: Request<pb::ListModelsReq>,
    ) -> Result<Response<pb::ModelCatalog>, Status> {
        Ok(Response::new(pb::ModelCatalog { items: vec![] }))
    }

    async fn list_tools(
        &self,
        _req: Request<pb::ListToolsReq>,
    ) -> Result<Response<pb::ToolCatalog>, Status> {
        Ok(Response::new(pb::ToolCatalog { items: vec![] }))
    }

    async fn spawn_child(
        &self,
        _req: Request<pb::SpawnChildReq>,
    ) -> Result<Response<pb::SpawnChildResp>, Status> {
        Err(Status::unimplemented(
            "Agent.SpawnChild ships in Spec C₇ (recursive embedding, post-MVS). \
             Tracking ticket: BRO-926.",
        ))
    }
}
