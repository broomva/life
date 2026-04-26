//! life.v1.Agent — public-plane agent namespace.
//!
//! Sub-phase A wires the 11 RPCs against mock substrate clients. Sub-phase
//! B replaces the mock dispatch with real `arcan-proxy` + saga driver.
//!
//! Per Spec C₂ §1 and §3.1 this implementation MUST:
//! - Read CapabilityClaims from request extensions (set by AuthLayer middleware).
//! - Dispatch a single substrate call OR initiate one saga.
//! - Return.
//!
//! ≤20 LOC per handler is a hard constraint. Anything more is a sign that
//! business logic is leaking into lifed.

use std::pin::Pin;
use std::sync::Arc;

use chrono::Utc;
use futures::Stream;
use sha2::{Digest, Sha256};
use tonic::{Request, Response, Status};

use aios_proto::aios::v1 as aios_v1;
use life_runtime_proto::life::v1 as pb;

use crate::auth::capability::CapabilityClaims;
use crate::routing::cache::RoutingCache;

/// Trait abstracting the dispatch surface lifed needs from arcan.
/// Sub-phase A backs this with `MockArcan`; sub-phase B wires `ArcanProxy`.
#[async_trait::async_trait]
pub trait ArcanDispatch: Send + Sync + 'static {
    async fn create_agent(&self, sid: &str) -> Result<String, Status>;
    async fn destroy_agent(&self, sid: &str) -> Result<(), Status>;
    async fn dispatch_message(
        &self,
        sid: &str,
        content: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<pb::AgentEvent, Status>> + Send>>, Status>;
}

#[async_trait::async_trait]
pub trait LagoDispatch: Send + Sync + 'static {
    async fn open_namespace(&self, sid: &str) -> Result<String, Status>;
    async fn close_namespace(&self, ns: &str) -> Result<(), Status>;
}

#[async_trait::async_trait]
pub trait HaimaDispatch: Send + Sync + 'static {
    async fn bind_wallet(&self, sid: &str, project_id: &str) -> Result<String, Status>;
    async fn unbind_wallet(&self, wallet_id: &str) -> Result<(), Status>;
}

#[async_trait::async_trait]
pub trait AnimaDispatch: Send + Sync + 'static {
    async fn register_session(&self, sid: &str, user_id: &str) -> Result<(), Status>;
    async fn mark_session_closed(&self, sid: &str) -> Result<(), Status>;
}

/// `Agent` service implementation.
pub struct AgentService {
    pub arcan: Arc<dyn ArcanDispatch>,
    pub lago: Arc<dyn LagoDispatch>,
    pub haima: Arc<dyn HaimaDispatch>,
    pub anima: Arc<dyn AnimaDispatch>,
    pub routing: Arc<RoutingCache>,
}

impl AgentService {
    pub fn new(
        arcan: Arc<dyn ArcanDispatch>,
        lago: Arc<dyn LagoDispatch>,
        haima: Arc<dyn HaimaDispatch>,
        anima: Arc<dyn AnimaDispatch>,
        routing: Arc<RoutingCache>,
    ) -> Self {
        Self {
            arcan,
            lago,
            haima,
            anima,
            routing,
        }
    }

    fn claims<T>(req: &Request<T>) -> Result<&CapabilityClaims, Status> {
        req.extensions()
            .get::<CapabilityClaims>()
            .ok_or_else(|| Status::unauthenticated("missing capability claims"))
    }
}

/// Sub-phase A: one-shot sid generator. Real saga in sub-phase B replaces this
/// with the master-spec sid formula.
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
        // Deterministic-enough randomness for the sid mint;
        // sub-phase B replaces with `getrandom`.
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

        // Sub-phase A: serial dispatch. Sub-phase B replaces with SagaDriver.
        let _agent_id = self.arcan.create_agent(&sid_value).await?;
        let _ns = self.lago.open_namespace(&sid_value).await?;
        let _wallet = self.haima.bind_wallet(&sid_value, &body.project_id).await?;
        self.anima
            .register_session(&sid_value, &body.user_id)
            .await?;
        self.routing
            .insert_minimal(&sid, &body.user_id, &body.project_id);
        Ok(Response::new(pb::Session {
            sid: Some(sid),
            agent_id: Some(aios_v1::AgentId {
                value: format!("agent-{sid_value}"),
            }),
            user_id: claims.user_id,
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
        self.anima.mark_session_closed(&sid.value).await?;
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
        let stream = self.arcan.dispatch_message(&sid, &body.content).await?;
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
        // Sub-phase A: re-use SendMessage's stub stream with empty content.
        let stream = self.arcan.dispatch_message(&sid, "").await?;
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
