//! life.admin.v1.Saga — inflight saga introspection.
//!
//! Per Spec C₂ §3.5 the admin Saga service exposes the saga registry to
//! operators (and post-MVS, to autonomic). `ForceCompensate` ships in
//! sub-phase C as a documented `Status::unimplemented` carve-out per the
//! M5 plan acceptance criteria: re-entrant compensation needs a
//! saga-driver entrypoint that exposes the steps + last-completed index
//! of an inflight saga. Tracked as a follow-up ticket under the Spec C
//! umbrella (companion to BRO-934 sub-phase D); until then operators
//! force-evict via `RoutingCache.Evict` + `Runtime.SessionsForceClose`.

use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use futures::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use life_runtime_proto::life::admin::v1 as adm;

use crate::auth::peercred::PeerCred;
use crate::listener::admin::AdminConnInfo;
use crate::saga::registry::{SagaRecord, SagaRegistry};
use crate::services::admin::policy::{AdminOp, AdminPolicy};

pub struct SagaAdminService {
    pub policy: Arc<AdminPolicy>,
    pub registry: Arc<SagaRegistry>,
}

impl SagaAdminService {
    pub fn new(policy: Arc<AdminPolicy>, registry: Arc<SagaRegistry>) -> Self {
        Self { policy, registry }
    }

    fn cred<T>(req: &Request<T>) -> Result<PeerCred, Status> {
        req.extensions()
            .get::<AdminConnInfo>()
            .map(|c| c.cred)
            .ok_or_else(|| Status::internal("admin connection lacks PeerCred"))
    }
}

fn record_to_pb(r: SagaRecord) -> adm::SagaState {
    adm::SagaState {
        saga_id: r.saga_id,
        saga_kind: r.saga_kind,
        sid: Some(r.sid),
        // Sub-phase C: started_at on the wire is a wall-clock approximation
        // emitted at snapshot time. The in-memory `Instant` cannot be
        // converted directly; consumers that need exact wall-clock should
        // read the lago `system/lifed/saga/<id>` namespace where each
        // event carries an explicit timestamp_ms.
        started_at: Some(prost_types::Timestamp::from(SystemTime::now())),
        current_step: r.current_step,
        completed_steps: r.completed_steps,
        compensations_applied: r.compensations_applied,
        status: r.status.as_str().to_string(),
    }
}

#[tonic::async_trait]
impl adm::saga_server::Saga for SagaAdminService {
    type ListInflightStream = Pin<Box<dyn Stream<Item = Result<adm::SagaState, Status>> + Send>>;

    async fn list_inflight(
        &self,
        req: Request<adm::ListInflightReq>,
    ) -> Result<Response<Self::ListInflightStream>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SagaListInflight)?;
        let limit = req.get_ref().limit as usize;
        let limit = if limit == 0 { usize::MAX } else { limit };
        let records = self.registry.snapshot_inflight(limit);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            for r in records {
                if tx.send(Ok(record_to_pb(r))).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn show(&self, req: Request<adm::SagaRef>) -> Result<Response<adm::SagaState>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SagaShow)?;
        let saga_id = req.get_ref().saga_id.clone();
        if saga_id.is_empty() {
            return Err(Status::invalid_argument("saga_id"));
        }
        let rec = self
            .registry
            .get(&saga_id)
            .ok_or_else(|| Status::not_found(format!("saga {saga_id} not found")))?;
        Ok(Response::new(record_to_pb(rec)))
    }

    async fn force_compensate(
        &self,
        req: Request<adm::SagaRef>,
    ) -> Result<Response<adm::SagaEmpty>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SagaForceCompensate)?;
        // Sub-phase C: documented carve-out per the M5 plan acceptance
        // criteria. Re-entrant compensation needs a saga-driver
        // entrypoint that exposes the steps + last-completed index of an
        // inflight saga. Tracked as a follow-up ticket under the Spec C
        // umbrella; until then operators force-evict via
        // RoutingCache.Evict + SessionsForceClose.
        Err(Status::unimplemented(
            "Saga.ForceCompensate is a documented carve-out — needs a \
             saga-driver re-entrant entrypoint (post-Sub-phase-C \
             follow-up under BRO-921 / BRO-934). Workaround: combine \
             RoutingCache.Evict + Runtime.SessionsForceClose.",
        ))
    }
}
