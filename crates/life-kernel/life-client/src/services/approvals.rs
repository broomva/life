//! Typed handle over `life.Approvals`.

use crate::connect::LifeClient;
use crate::error::{LifeClientError, LifeResult};
use aios_protocol::ids::{ApprovalId, SessionId};
use aios_protocol::ports::{ApprovalRequest, ApprovalResolution, ApprovalTicket};
use life_kernel_proto::{approvals as pb, common};
use pb::approvals_service_client::ApprovalsServiceClient;

/// Typed handle over the `life.Approvals` service.
pub struct Approvals<'a> {
    client: &'a LifeClient,
}

impl<'a> Approvals<'a> {
    /// Construct a new handle. Called from `LifeClient::approvals`.
    pub(crate) fn new(client: &'a LifeClient) -> Self {
        Self { client }
    }

    /// Enqueue a new approval request.
    pub async fn enqueue(&self, request: ApprovalRequest) -> LifeResult<ApprovalTicket> {
        let mut c = ApprovalsServiceClient::new(self.client.channel());
        let body = pb::EnqueueRequest {
            attribution: None,
            request_json: serde_json::to_vec(&request)
                .map_err(|e| LifeClientError::Rpc(format!("request: {e}")))?,
        };
        let wire = c
            .enqueue(body)
            .await
            .map_err(|e| LifeClientError::Rpc(e.to_string()))?
            .into_inner();
        serde_json::from_slice(&wire.ticket_json)
            .map_err(|e| LifeClientError::Rpc(format!("ticket: {e}")))
    }

    /// List pending approvals for a session.
    pub async fn list_pending(&self, session: SessionId) -> LifeResult<Vec<ApprovalTicket>> {
        let mut c = ApprovalsServiceClient::new(self.client.channel());
        let wire = c
            .list_pending(pb::ListPendingRequest {
                session: Some(common::SessionId {
                    value: session.to_string(),
                }),
            })
            .await
            .map_err(|e| LifeClientError::Rpc(e.to_string()))?
            .into_inner();
        wire.tickets
            .into_iter()
            .map(|t| {
                serde_json::from_slice(&t.ticket_json)
                    .map_err(|e| LifeClientError::Rpc(format!("ticket: {e}")))
            })
            .collect()
    }

    /// Resolve an approval with an approved/denied decision.
    pub async fn resolve(
        &self,
        approval_id: ApprovalId,
        approved: bool,
        actor: String,
    ) -> LifeResult<ApprovalResolution> {
        let mut c = ApprovalsServiceClient::new(self.client.channel());
        let wire = c
            .resolve(pb::ResolveRequest {
                approval_id: Some(common::ApprovalId {
                    value: approval_id.to_string(),
                }),
                approved,
                actor,
            })
            .await
            .map_err(|e| LifeClientError::Rpc(e.to_string()))?
            .into_inner();
        serde_json::from_slice(&wire.resolution_json)
            .map_err(|e| LifeClientError::Rpc(format!("resolution: {e}")))
    }
}
