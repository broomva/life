//! life.v1.Agent — public-plane agent namespace.
//!
//! Sub-phase B wires real `*-proxy` clients via the per-substrate `*Call`
//! traits. `CreateSession` runs the four-step saga (Spec C₂ §4.2) through
//! the new `SagaDriver`. Per Spec C₂ §1, every public RPC handler reads
//! the capability token, performs a single substrate route OR initiates
//! one saga, and returns. ≤20 LOC per handler is a hard constraint.

use std::pin::Pin;
use std::sync::Arc;

use chrono::Utc;
use futures::Stream;
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
use crate::saga::driver::{SagaCtx, SagaDriver, SagaStep};
use crate::saga::steps::{BindWallet, CreateAgent, OpenLagoNamespace, RegisterAnimaSession};

/// `Agent` service implementation. Holds typed substrate proxies, the
/// signing keystore, the saga driver, and the routing cache.
pub struct AgentService {
    pub arcan_call: Arc<dyn ArcanCall>,
    pub lago_call: Arc<dyn LagoCall>,
    pub haima_call: Arc<dyn HaimaCall>,
    pub anima_call: Arc<dyn AnimaCall>,
    pub routing: Arc<RoutingCache>,
    pub ks: Arc<Keystore>,
    pub saga: Arc<SagaDriver>,
}

impl AgentService {
    pub fn new(
        arcan_call: Arc<dyn ArcanCall>,
        lago_call: Arc<dyn LagoCall>,
        haima_call: Arc<dyn HaimaCall>,
        anima_call: Arc<dyn AnimaCall>,
        routing: Arc<RoutingCache>,
        ks: Arc<Keystore>,
        saga: Arc<SagaDriver>,
    ) -> Self {
        Self {
            arcan_call,
            lago_call,
            haima_call,
            anima_call,
            routing,
            ks,
            saga,
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
        let sid = body
            .sid
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("missing sid"))?
            .value
            .clone();
        let stream = self
            .arcan_call
            .dispatch_message(&sid, &body.content)
            .await
            .map_err(Status::from)?;
        Ok(Response::new(stream))
    }

    async fn stream_session(
        &self,
        req: Request<pb::SessionRef>,
    ) -> Result<Response<Self::StreamSessionStream>, Status> {
        let _claims = Self::claims(&req)?;
        let sid = req
            .get_ref()
            .sid
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("missing sid"))?
            .value
            .clone();
        let stream = self
            .arcan_call
            .dispatch_message(&sid, "")
            .await
            .map_err(Status::from)?;
        Ok(Response::new(stream))
    }

    async fn approve_dispatch(
        &self,
        _req: Request<pb::ApprovalReq>,
    ) -> Result<Response<pb::Empty>, Status> {
        // Sub-phase A: ack-only stub. B12 wires the real per-sid lock.
        Ok(Response::new(pb::Empty {}))
    }

    async fn cancel_dispatch(
        &self,
        _req: Request<pb::DispatchRef>,
    ) -> Result<Response<pb::Empty>, Status> {
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
