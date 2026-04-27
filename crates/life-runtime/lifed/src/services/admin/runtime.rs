//! life.admin.v1.Runtime — operator + autonomic introspection.
//!
//! Per Spec C₂ §3.5 the admin Runtime service exposes liveness + the
//! routing-cache view + idempotency-key lookup. Each handler:
//!
//! 1. Pulls the connection's `PeerCred` out of the request extensions
//!    (placed there by `listener::admin::AdminAcceptor`).
//! 2. Authorises via `AdminPolicy::check`.
//! 3. Reads from in-memory state and returns. No business logic.
//!
//! Admin handlers may exceed the public-plane ≤20-LOC budget — dump and
//! filter ops naturally take more lines — but never hold a lock across
//! an `await`.

use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use futures::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use life_runtime_proto::life::admin::v1 as adm;

use crate::auth::peercred::PeerCred;
use crate::idempotency::{IdemKey, IdempotencyStore};
use crate::listener::admin::AdminConnInfo;
use crate::routing::cache::RoutingCache;
use crate::services::admin::policy::{AdminOp, AdminPolicy};

pub struct RuntimeAdminService {
    pub policy: Arc<AdminPolicy>,
    pub routing: Arc<RoutingCache>,
    pub idem: Arc<dyn IdempotencyStore>,
}

impl RuntimeAdminService {
    pub fn new(
        policy: Arc<AdminPolicy>,
        routing: Arc<RoutingCache>,
        idem: Arc<dyn IdempotencyStore>,
    ) -> Self {
        Self {
            policy,
            routing,
            idem,
        }
    }

    fn cred<T>(req: &Request<T>) -> Result<PeerCred, Status> {
        req.extensions()
            .get::<AdminConnInfo>()
            .map(|c| c.cred)
            .ok_or_else(|| Status::internal("admin connection lacks PeerCred"))
    }
}

#[tonic::async_trait]
impl adm::runtime_server::Runtime for RuntimeAdminService {
    type SessionsListAllStream =
        Pin<Box<dyn Stream<Item = Result<adm::SessionSummary, Status>> + Send>>;

    async fn health_check(
        &self,
        req: Request<adm::HealthReq>,
    ) -> Result<Response<adm::HealthResp>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::HealthCheck)?;
        Ok(Response::new(adm::HealthResp {
            ok: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            cache_size: self.routing.size() as u64,
            substrates: vec![],
        }))
    }

    async fn sessions_list_all(
        &self,
        req: Request<adm::ListAllReq>,
    ) -> Result<Response<Self::SessionsListAllStream>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SessionsListAll)?;
        let limit = req.get_ref().limit as usize;
        let limit = if limit == 0 { usize::MAX } else { limit };
        let summaries = self.routing.snapshot_summaries(limit);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            for s in summaries {
                let summary = adm::SessionSummary {
                    sid: Some(s.sid),
                    user_id: s.user_id,
                    project_id: s.project_id,
                    created_at: Some(prost_types::Timestamp::from(SystemTime::now())),
                    status: format!("{:?}", s.status).to_lowercase(),
                    attached_streams: s.attached_streams,
                };
                if tx.send(Ok(summary)).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn sessions_force_close(
        &self,
        req: Request<adm::ForceCloseReq>,
    ) -> Result<Response<adm::RuntimeEmpty>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SessionsForceClose)?;
        let sid = req
            .get_ref()
            .sid
            .clone()
            .ok_or_else(|| Status::invalid_argument("sid"))?;
        self.routing.evict(&sid);
        Ok(Response::new(adm::RuntimeEmpty {}))
    }

    async fn sessions_suspend(
        &self,
        req: Request<adm::SuspendReq>,
    ) -> Result<Response<adm::RuntimeEmpty>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::SessionsSuspend)?;
        let sid = req
            .get_ref()
            .sid
            .clone()
            .ok_or_else(|| Status::invalid_argument("sid"))?;
        self.routing
            .mark_status(&sid, crate::routing::cache::SessionStatus::Detached);
        Ok(Response::new(adm::RuntimeEmpty {}))
    }

    async fn idempotency_lookup(
        &self,
        req: Request<adm::IdemReq>,
    ) -> Result<Response<adm::IdemResult>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::IdempotencyLookup)?;
        let body = req.into_inner();
        // Sub-phase C: lookup is best-effort — admins query by
        // (idempotency-key, method) only. We pass empty user/project so
        // backends that key on those fields will treat this as a miss
        // unless the backend supports key-only lookup. The lago-backed
        // store keys on the full pipe-encoded tuple, so this returns
        // None unless the original handler used an empty user/project
        // (which it never does).
        let key = IdemKey {
            user_id: String::new(),
            project_id: String::new(),
            key: body.idempotency_key,
            method: body.method,
        };
        let bytes = self.idem.lookup(&key).await?;
        let found = bytes.is_some();
        Ok(Response::new(adm::IdemResult {
            found,
            at: None,
            original_response: bytes.unwrap_or_default(),
        }))
    }
}
