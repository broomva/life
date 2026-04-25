//! arcand ApprovalPort proxy.
//!
//! Route map (arcand):
//! - `POST /sessions/{session_id}/approvals/{approval_id}` → resolve (204)
//! - `GET  /sessions/{session_id}/queue`                   → queue status
//!
//! arcand does not expose a direct POST-to-enqueue REST endpoint; approvals
//! are created internally by the agent loop. `enqueue` returns an error for
//! now — callers that need server-side enqueue should submit via the agent
//! loop (tick). This will be revisited when arcand adds the endpoint.
//!
//! `list_pending` returns empty because arcand's `/queue` endpoint exposes
//! pending *messages* (not approval tickets) — the approval queue is tracked
//! inside the agent loop only. Phase 1 documents this as a known gap.

use crate::arcand::client::ArcanClient;
use crate::error::{FacadeError, FacadeResult};
use aios_protocol::error::{KernelError, KernelResult};
use aios_protocol::ids::{ApprovalId, SessionId};
use aios_protocol::ports::{ApprovalPort, ApprovalRequest, ApprovalResolution, ApprovalTicket};
use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;

/// Body for `POST /sessions/{id}/approvals/{aid}`.
#[derive(Serialize)]
struct ResolveBody<'a> {
    approved: bool,
    actor: &'a str,
}

/// HTTP proxy for `aios_protocol::ports::ApprovalPort` over arcand.
pub struct ApprovalsProxy {
    client: ArcanClient,
}

impl ApprovalsProxy {
    /// Construct from a configured [`ArcanClient`].
    pub fn new(client: ArcanClient) -> Self {
        Self { client }
    }

    async fn do_resolve(
        &self,
        session_id: &str,
        approval_id: &str,
        approved: bool,
        actor: &str,
    ) -> FacadeResult<()> {
        let path = format!("/sessions/{}/approvals/{}", session_id, approval_id);
        let body = ResolveBody { approved, actor };
        let res = self
            .client
            .request(reqwest::Method::POST, &path)
            .json(&body)
            .send()
            .await
            .map_err(|e| FacadeError::BackendUnavailable {
                daemon: "arcand",
                source: e.into(),
            })?;
        if !res.status().is_success() && res.status().as_u16() != 204 {
            let status = res.status().as_u16();
            let message = res.text().await.unwrap_or_default();
            return Err(FacadeError::BackendRejected {
                daemon: "arcand",
                status,
                message,
            });
        }
        Ok(())
    }
}

#[async_trait]
impl ApprovalPort for ApprovalsProxy {
    /// Not directly exposed by arcand's REST API in v0.
    ///
    /// Approvals are created internally by the agent loop when a tool call
    /// requires human authorisation. Returns `KernelError::InvalidState` to
    /// signal the caller that direct enqueue is not supported via the proxy.
    async fn enqueue(&self, _request: ApprovalRequest) -> KernelResult<ApprovalTicket> {
        Err(KernelError::InvalidState(
            "arcand does not expose a direct approval-enqueue REST endpoint; \
             approvals are created by the agent loop (use tick to trigger)"
                .into(),
        ))
    }

    /// Returns an empty list.
    ///
    /// arcand's `/queue` endpoint exposes the consciousness queue (pending
    /// messages), not the approval ticket list. The approval queue is tracked
    /// inside the agent loop. Phase 1 documents this as a known gap; it will
    /// be addressed when arcand adds a `GET /sessions/{id}/approvals` endpoint.
    async fn list_pending(&self, _session_id: SessionId) -> KernelResult<Vec<ApprovalTicket>> {
        Ok(vec![])
    }

    async fn resolve(
        &self,
        approval_id: ApprovalId,
        approved: bool,
        actor: String,
    ) -> KernelResult<ApprovalResolution> {
        // arcand's resolve endpoint is scoped to a session; the approval_id
        // encodes enough information for routing. We use a synthetic session
        // path derived from the approval_id (arcand resolves by UUID globally).
        // Use "default" session scope — arcand routes by approval UUID.
        self.do_resolve("default", approval_id.as_str(), approved, &actor)
            .await
            .map_err(|e: FacadeError| KernelError::from(e))?;
        Ok(ApprovalResolution {
            approval_id,
            approved,
            actor,
            resolved_at: Utc::now(),
        })
    }
}
