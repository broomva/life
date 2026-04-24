//! Typed handle over `life.Session`.

use crate::connect::LifeClient;
use crate::error::{LifeClientError, LifeResult};
use aios_protocol::ids::SessionId;
use aios_protocol::session::{
    CreateSessionRequest, SessionFilter, SessionManifest, TickInput, TickOutput,
};
use life_kernel_proto::{common, session as pb};
use pb::session_service_client::SessionServiceClient;

/// Typed handle over the `life.Session` service.
pub struct Session<'a> {
    client: &'a LifeClient,
}

impl<'a> Session<'a> {
    /// Construct a new handle. Called from `LifeClient::session`.
    pub(crate) fn new(client: &'a LifeClient) -> Self {
        Self { client }
    }

    /// Create a new session.
    pub async fn create(&self, req: CreateSessionRequest) -> LifeResult<SessionManifest> {
        let mut c = SessionServiceClient::new(self.client.channel());
        let body = pb::CreateRequest {
            attribution: None,
            request_json: serde_json::to_vec(&req)
                .map_err(|e| LifeClientError::Rpc(format!("request_json: {e}")))?,
        };
        let res = c
            .create(body)
            .await
            .map_err(|e| LifeClientError::Rpc(e.to_string()))?;
        let wire = res.into_inner();
        serde_json::from_slice(&wire.manifest_json)
            .map_err(|e| LifeClientError::Rpc(format!("manifest: {e}")))
    }

    /// Fetch an existing session by id.
    pub async fn get(&self, session: SessionId) -> LifeResult<SessionManifest> {
        let mut c = SessionServiceClient::new(self.client.channel());
        let res = c
            .get(pb::GetRequest {
                session: Some(common::SessionId {
                    value: session.to_string(),
                }),
            })
            .await
            .map_err(|e| LifeClientError::Rpc(e.to_string()))?;
        let wire = res.into_inner();
        serde_json::from_slice(&wire.manifest_json)
            .map_err(|e| LifeClientError::Rpc(format!("manifest: {e}")))
    }

    /// List sessions matching the given filter.
    pub async fn list(&self, filter: SessionFilter) -> LifeResult<Vec<SessionManifest>> {
        let mut c = SessionServiceClient::new(self.client.channel());
        let res = c
            .list(pb::ListRequest {
                filter: Some(pb::SessionFilterWire {
                    filter_json: serde_json::to_vec(&filter)
                        .map_err(|e| LifeClientError::Rpc(format!("filter: {e}")))?,
                }),
            })
            .await
            .map_err(|e| LifeClientError::Rpc(e.to_string()))?
            .into_inner();
        res.manifests
            .into_iter()
            .map(|m| {
                serde_json::from_slice(&m.manifest_json)
                    .map_err(|e| LifeClientError::Rpc(format!("manifest: {e}")))
            })
            .collect()
    }

    /// Run one agent tick for the given session.
    pub async fn tick(&self, session: SessionId, input: TickInput) -> LifeResult<TickOutput> {
        let mut c = SessionServiceClient::new(self.client.channel());
        let body = pb::TickRequest {
            session: Some(common::SessionId {
                value: session.to_string(),
            }),
            input_json: serde_json::to_vec(&input)
                .map_err(|e| LifeClientError::Rpc(format!("input: {e}")))?,
        };
        let res = c
            .tick(body)
            .await
            .map_err(|e| LifeClientError::Rpc(e.to_string()))?
            .into_inner();
        serde_json::from_slice(&res.output_json)
            .map_err(|e| LifeClientError::Rpc(format!("output: {e}")))
    }

    /// Close the session with the given reason.
    pub async fn close(&self, session: SessionId, reason: String) -> LifeResult<()> {
        let mut c = SessionServiceClient::new(self.client.channel());
        c.close(pb::CloseRequest {
            session: Some(common::SessionId {
                value: session.to_string(),
            }),
            reason,
        })
        .await
        .map(|_| ())
        .map_err(|e| LifeClientError::Rpc(e.to_string()))
    }
}
