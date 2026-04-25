//! `life.Policy` service adapter.
//!
//! Generic over `PolicyGatePort` so `lifed` can wire in any
//! `aios-policy` gate impl — the default static gate from
//! `life-kernel-gate` in Spec A Phase 1, or a future dynamic gate —
//! without touching the adapter.
//!
//! ## Wire shape
//!
//! The `Evaluate` RPC's request/response carry opaque `bytes *_json`
//! fields (see `policy.proto`) because the underlying port accepts a
//! `(SessionId, Vec<Capability>)` tuple and returns a
//! `PolicyGateDecision` struct. We define private wire structs
//! [`PolicyQueryWire`] / [`PolicyDecisionWire`] that mirror those types
//! one-to-one and JSON-encode them at the boundary.

use crate::convert::{from_json, kernel_err_to_status, to_json};
use aios_protocol::ids::SessionId;
use aios_protocol::policy::{Capability, PolicySet};
use aios_protocol::ports::{PolicyGateDecision, PolicyGatePort};
use life_kernel_proto::policy as pb;
use pb::policy_service_server::PolicyService as TonicPolicy;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// Private wire struct for the `Evaluate` RPC request body —
/// mirrors the `PolicyGatePort::evaluate` arguments.
#[derive(Serialize, Deserialize)]
struct PolicyQueryWire {
    session_id: SessionId,
    requested: Vec<Capability>,
}

/// Generic tonic service adapter for `life.Policy`.
pub struct PolicyService<P: PolicyGatePort + 'static> {
    port: Arc<P>,
}

impl<P: PolicyGatePort> PolicyService<P> {
    /// Wrap a `PolicyGatePort` impl in this adapter.
    pub fn new(port: Arc<P>) -> Self {
        Self { port }
    }
}

#[tonic::async_trait]
impl<P: PolicyGatePort + Send + Sync + 'static> TonicPolicy for PolicyService<P> {
    async fn evaluate(
        &self,
        req: Request<pb::EvaluateRequest>,
    ) -> Result<Response<pb::EvaluateResponse>, Status> {
        let r = req.into_inner();
        let query: PolicyQueryWire = from_json(&r.query_json, "query_json")?;
        let decision: PolicyGateDecision = self
            .port
            .evaluate(query.session_id, query.requested)
            .await
            .map_err(kernel_err_to_status)?;
        Ok(Response::new(pb::EvaluateResponse {
            decision_json: to_json(&decision, "decision")?,
        }))
    }

    async fn set_policy(
        &self,
        req: Request<pb::SetPolicyRequest>,
    ) -> Result<Response<pb::SetPolicyResponse>, Status> {
        let r = req.into_inner();
        let sid = SessionId::from(
            r.session
                .ok_or_else(|| Status::invalid_argument("session required"))?
                .value,
        );
        let set: PolicySet = from_json(&r.policy_set_json, "policy_set_json")?;
        self.port
            .set_policy(sid, set)
            .await
            .map_err(kernel_err_to_status)?;
        Ok(Response::new(pb::SetPolicyResponse {}))
    }
}
