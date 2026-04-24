//! Typed handle over `life.Policy`.

use crate::connect::LifeClient;
use crate::error::{LifeClientError, LifeResult};
use aios_protocol::ids::SessionId;
use aios_protocol::policy::{Capability, PolicySet};
use aios_protocol::ports::PolicyGateDecision;
use life_kernel_proto::{common, policy as pb};
use pb::policy_service_client::PolicyServiceClient;
use serde::{Deserialize, Serialize};

/// Private wire struct for the `Evaluate` RPC request body — mirrors
/// the `PolicyGatePort::evaluate` argument shape `(SessionId, Vec<Capability>)`.
#[derive(Serialize, Deserialize)]
struct PolicyQueryWire {
    session_id: SessionId,
    requested: Vec<Capability>,
}

/// Typed handle over the `life.Policy` service.
pub struct Policy<'a> {
    client: &'a LifeClient,
}

impl<'a> Policy<'a> {
    /// Construct a new handle. Called from `LifeClient::policy`.
    pub(crate) fn new(client: &'a LifeClient) -> Self {
        Self { client }
    }

    /// Evaluate a capability request against the session's policy.
    pub async fn evaluate(
        &self,
        session_id: SessionId,
        requested: Vec<Capability>,
    ) -> LifeResult<PolicyGateDecision> {
        let mut c = PolicyServiceClient::new(self.client.channel());
        let query = PolicyQueryWire {
            session_id,
            requested,
        };
        let wire = c
            .evaluate(pb::EvaluateRequest {
                attribution: None,
                query_json: serde_json::to_vec(&query)
                    .map_err(|e| LifeClientError::Rpc(format!("query: {e}")))?,
            })
            .await
            .map_err(|e| LifeClientError::Rpc(e.to_string()))?
            .into_inner();
        serde_json::from_slice(&wire.decision_json)
            .map_err(|e| LifeClientError::Rpc(format!("decision: {e}")))
    }

    /// Install a new policy set for a session.
    pub async fn set_policy(&self, session: SessionId, policy: PolicySet) -> LifeResult<()> {
        let mut c = PolicyServiceClient::new(self.client.channel());
        c.set_policy(pb::SetPolicyRequest {
            session: Some(common::SessionId {
                value: session.to_string(),
            }),
            policy_set_json: serde_json::to_vec(&policy)
                .map_err(|e| LifeClientError::Rpc(format!("policy: {e}")))?,
        })
        .await
        .map(|_| ())
        .map_err(|e| LifeClientError::Rpc(e.to_string()))
    }
}
