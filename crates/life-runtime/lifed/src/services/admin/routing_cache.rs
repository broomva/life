//! life.admin.v1.RoutingCache — dump + evict + (stub) rebuild.
//!
//! Per Spec C₂ §3.5 the admin RoutingCache service exposes the in-memory
//! routing cache to operators. `RebuildFromLago` ships in sub-phase C as
//! a documented carve-out returning 0 entries: it depends on a lago
//! `ListNamespaces` RPC that doesn't exist yet. Real impl lands in
//! sub-phase D2 (BRO-934) alongside cold-start replay from lago.

use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use futures::Stream;
use lago_proxy::LagoCall;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use life_runtime_proto::life::admin::v1 as adm;

use crate::auth::peercred::PeerCred;
use crate::listener::admin::AdminConnInfo;
use crate::routing::cache::RoutingCache;
use crate::services::admin::policy::{AdminOp, AdminPolicy};

pub struct RoutingCacheAdminService {
    pub policy: Arc<AdminPolicy>,
    pub routing: Arc<RoutingCache>,
    /// Sub-phase D2: lago handle so `RebuildFromLago` can issue
    /// `lago.ListNamespaces` and warm the cache.
    pub lago: Arc<dyn LagoCall>,
}

impl RoutingCacheAdminService {
    pub fn new(
        policy: Arc<AdminPolicy>,
        routing: Arc<RoutingCache>,
        lago: Arc<dyn LagoCall>,
    ) -> Self {
        Self {
            policy,
            routing,
            lago,
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
impl adm::routing_cache_server::RoutingCache for RoutingCacheAdminService {
    type DumpStream = Pin<Box<dyn Stream<Item = Result<adm::RouteEntry, Status>> + Send>>;

    async fn dump(&self, req: Request<adm::DumpReq>) -> Result<Response<Self::DumpStream>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::RoutingCacheDump)?;
        let limit = req.get_ref().limit as usize;
        let limit = if limit == 0 { usize::MAX } else { limit };
        let summaries = self.routing.snapshot_summaries(limit);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            for s in summaries {
                let entry = adm::RouteEntry {
                    sid: Some(s.sid),
                    user_id: s.user_id,
                    project_id: s.project_id,
                    arcan_addr: s.agent_id,
                    lago_namespace: s.lago_namespace,
                    haima_wallet: s.haima_wallet,
                    anima_account: s.anima_account,
                    // Sub-phase C MVS: lifed doesn't yet pin a vm_id per
                    // session (the soma kernel facade does). Empty until
                    // Spec C₇.
                    vm_id: String::new(),
                    last_touched: Some(prost_types::Timestamp::from(SystemTime::now())),
                    attached_streams: s.attached_streams,
                };
                if tx.send(Ok(entry)).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn evict(
        &self,
        req: Request<adm::EvictReq>,
    ) -> Result<Response<adm::RoutingEmpty>, Status> {
        let cred = Self::cred(&req)?;
        self.policy.check(&cred, AdminOp::RoutingCacheEvict)?;
        let sid = req
            .get_ref()
            .sid
            .clone()
            .ok_or_else(|| Status::invalid_argument("sid"))?;
        self.routing.evict(&sid);
        Ok(Response::new(adm::RoutingEmpty {}))
    }

    async fn rebuild_from_lago(
        &self,
        req: Request<adm::RebuildReq>,
    ) -> Result<Response<adm::RebuildResp>, Status> {
        let cred = Self::cred(&req)?;
        self.policy
            .check(&cred, AdminOp::RoutingCacheRebuildFromLago)?;
        // Sub-phase D2: cold-start replay via lago.ListNamespaces. When
        // the wire RPC is available we enumerate `session/*` and warm
        // the routing cache. Until lagod ships ListNamespaces the proxy
        // returns an empty list and `loaded` is 0 — the cache populates
        // lazily as live traffic arrives, the documented Spec C₂ §6.3
        // fallback path.
        let loaded = self.routing.cold_start(Arc::clone(&self.lago)).await?;
        Ok(Response::new(adm::RebuildResp {
            sessions_loaded: loaded,
            lago_events_read: loaded as u64,
        }))
    }
}
