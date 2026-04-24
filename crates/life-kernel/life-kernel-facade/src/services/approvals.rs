//! `life.Approvals` service adapter.

use crate::convert::{kernel_err_to_status, to_json};
use aios_protocol::ids::{ApprovalId, SessionId};
use aios_protocol::ports::{ApprovalPort, ApprovalRequest};
use life_kernel_proto::approvals as pb;
use pb::approvals_service_server::ApprovalsService as TonicApprovals;
use std::pin::Pin;
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// Generic tonic service adapter for `life.Approvals`.
pub struct ApprovalsService<P: ApprovalPort + 'static> {
    port: Arc<P>,
}

impl<P: ApprovalPort> ApprovalsService<P> {
    /// Wrap a port impl in this adapter.
    pub fn new(port: Arc<P>) -> Self {
        Self { port }
    }
}

type SubscribeStream =
    Pin<Box<dyn futures::Stream<Item = Result<pb::ApprovalTicket, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl<P: ApprovalPort + Send + Sync + 'static> TonicApprovals for ApprovalsService<P> {
    async fn enqueue(
        &self,
        req: Request<pb::EnqueueRequest>,
    ) -> Result<Response<pb::ApprovalTicket>, Status> {
        let r = req.into_inner();
        let request: ApprovalRequest =
            serde_json::from_slice(&r.request_json)
                .map_err(|e| Status::invalid_argument(format!("request_json: {e}")))?;
        let ticket = self.port.enqueue(request).await.map_err(kernel_err_to_status)?;
        Ok(Response::new(pb::ApprovalTicket {
            ticket_json: to_json(&ticket, "ticket")?,
        }))
    }

    async fn list_pending(
        &self,
        req: Request<pb::ListPendingRequest>,
    ) -> Result<Response<pb::ListPendingResponse>, Status> {
        let sid = SessionId::from(req.into_inner().session.unwrap_or_default().value);
        let tickets = self.port.list_pending(sid).await.map_err(kernel_err_to_status)?;
        let wire = tickets
            .iter()
            .map(|t| {
                Ok(pb::ApprovalTicket { ticket_json: to_json(t, "ticket")? })
            })
            .collect::<Result<Vec<_>, Status>>()?;
        Ok(Response::new(pb::ListPendingResponse { tickets: wire }))
    }

    async fn resolve(
        &self,
        req: Request<pb::ResolveRequest>,
    ) -> Result<Response<pb::ResolveResponse>, Status> {
        let r = req.into_inner();
        let aid = ApprovalId::from(r.approval_id.unwrap_or_default().value);
        let resolution = self
            .port
            .resolve(aid, r.approved, r.actor)
            .await
            .map_err(kernel_err_to_status)?;
        Ok(Response::new(pb::ResolveResponse {
            resolution_json: to_json(&resolution, "resolution")?,
        }))
    }

    /// Subscription stream not yet wired — `ApprovalPort` has no `subscribe` method.
    /// Returns `Status::unimplemented` until the port grows streaming support.
    type SubscribeStream = SubscribeStream;

    async fn subscribe(
        &self,
        _req: Request<pb::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        Err(Status::unimplemented(
            "approval subscription stream not yet available in v0; \
             poll list_pending instead",
        ))
    }
}
