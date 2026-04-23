//! Static policy gate: wraps [`aios_protocol::ports::PolicyGatePort`] to
//! produce [`aios_protocol::budget::BudgetDecision`].
//!
//! Enforces session-level `aios-policy` rules at the kernel boundary.
//! When the underlying policy flags any capability as requiring
//! approval, the gate delegates to an injected
//! [`aios_protocol::ports::ApprovalPort`] and forwards the resulting
//! [`aios_protocol::ports::ApprovalTicket`] back on
//! [`BudgetDecision::RequireApproval`].
//!
//! ## Mapping rules
//!
//! | `PolicyGateDecision`                       | `BudgetDecision`                                     |
//! |--------------------------------------------|------------------------------------------------------|
//! | `denied` non-empty                         | `Deny { gate_id: "policy-static", reason }`          |
//! | `requires_approval` non-empty, `denied` ok | `RequireApproval { ticket }` via `ApprovalPort::enqueue` |
//! | otherwise                                  | `Allow`                                              |
//!
//! Evaluation failures from the wrapped policy surface as a `Deny` with
//! `gate_id = "policy-static"` so the engine always gets a stable gate
//! identifier on the rejection path.
//!
//! ## Phase 1 simplification
//!
//! [`KernelContext`] does not yet carry the tool-call's declared
//! capability set — that lands in a later ABI bump. For Phase 1 the
//! gate evaluates with an empty `Vec<Capability>` (baseline access).
//! Deny-only and approval-only policies still exercise the full mapping
//! because they are computed from the session policy rather than the
//! per-call requested set.

use std::sync::Arc;

use aios_protocol::budget::{BudgetDecision, BudgetGatePort, ResourceBudget};
use aios_protocol::hypervisor::{ForkSpec, VmHandle};
use aios_protocol::kernel::KernelContext;
use aios_protocol::policy::Capability;
use aios_protocol::ports::{ApprovalPort, ApprovalRequest, PolicyGatePort};
use async_trait::async_trait;

/// Stable gate identifier attached to every
/// [`BudgetDecision::Deny`] raised by this gate.
///
/// Matches the `gate_id` contract documented on
/// [`aios_protocol::budget::BudgetDecision::Deny`].
pub const STATIC_POLICY_GATE_ID: &str = "policy-static";

/// [`BudgetGatePort`] impl that maps
/// [`aios_protocol::ports::PolicyGateDecision`] onto the kernel-tier
/// [`BudgetDecision`].
///
/// Construction takes any pair of `P: PolicyGatePort` and
/// `A: ApprovalPort` wrapped in [`Arc`] and produces a gate suitable
/// for installation in the kernel engine's gate chain. Both
/// collaborators are called at most once per `check_dispatch` /
/// `check_fork`, so the gate is cheap on the hot path.
pub struct StaticPolicyGate<P, A>
where
    P: PolicyGatePort + ?Sized + 'static,
    A: ApprovalPort + ?Sized + 'static,
{
    policy: Arc<P>,
    approvals: Arc<A>,
}

impl<P, A> StaticPolicyGate<P, A>
where
    P: PolicyGatePort + ?Sized + 'static,
    A: ApprovalPort + ?Sized + 'static,
{
    /// Construct a gate that queries `policy` and, on an approval
    /// requirement, enqueues onto `approvals`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use aios_protocol::ports::{ApprovalPort, PolicyGatePort};
    /// use life_kernel_gate::StaticPolicyGate;
    ///
    /// # fn demo(
    /// #     policy_port: Arc<dyn PolicyGatePort>,
    /// #     approvals_port: Arc<dyn ApprovalPort>,
    /// # ) {
    /// let gate = StaticPolicyGate::new(policy_port, approvals_port);
    /// # let _ = gate;
    /// # }
    /// ```
    pub fn new(policy: Arc<P>, approvals: Arc<A>) -> Self {
        Self { policy, approvals }
    }

    /// Shared evaluation used by both `check_dispatch` and
    /// `check_fork`. Splitting it out keeps the two trait methods as
    /// thin, delegated wrappers and makes "fork uses the same policy
    /// path" a single-line implementation.
    async fn evaluate(&self, ctx: &KernelContext) -> BudgetDecision {
        // Phase 1: we evaluate an empty capability set because
        // KernelContext does not surface the caller's declared
        // capabilities yet. See module-level rustdoc.
        let requested: Vec<Capability> = Vec::new();
        let decision = match self
            .policy
            .evaluate(ctx.session_id.clone(), requested)
            .await
        {
            Ok(d) => d,
            Err(e) => {
                return BudgetDecision::Deny {
                    reason: format!("policy evaluation failed: {e}"),
                    gate_id: STATIC_POLICY_GATE_ID.into(),
                };
            }
        };

        if !decision.denied.is_empty() {
            return BudgetDecision::Deny {
                reason: format!("capabilities denied: {:?}", decision.denied),
                gate_id: STATIC_POLICY_GATE_ID.into(),
            };
        }

        if !decision.requires_approval.is_empty() {
            // Enqueue a ticket for the first capability flagged for
            // approval. Batch approval — one ticket per capability —
            // is a Phase 4 refinement.
            let capability = decision
                .requires_approval
                .into_iter()
                .next()
                .expect("non-empty slice guarded above");
            return match self
                .approvals
                .enqueue(ApprovalRequest {
                    session_id: ctx.session_id.clone(),
                    call_id: String::new(),
                    tool_name: String::new(),
                    capability,
                    reason: "policy-static approval required".into(),
                })
                .await
            {
                Ok(ticket) => BudgetDecision::RequireApproval { ticket },
                Err(e) => BudgetDecision::Deny {
                    reason: format!("approval enqueue failed: {e}"),
                    gate_id: STATIC_POLICY_GATE_ID.into(),
                },
            };
        }

        BudgetDecision::Allow
    }
}

#[async_trait]
impl<P, A> BudgetGatePort for StaticPolicyGate<P, A>
where
    P: PolicyGatePort + ?Sized + 'static,
    A: ApprovalPort + ?Sized + 'static,
{
    async fn check_dispatch(
        &self,
        ctx: &KernelContext,
        _cost_hint: &ResourceBudget,
    ) -> BudgetDecision {
        self.evaluate(ctx).await
    }

    async fn check_fork(
        &self,
        _parent: &VmHandle,
        _spec: &ForkSpec,
        ctx: &KernelContext,
    ) -> BudgetDecision {
        // Fork inherits the same policy check as dispatch. Policies
        // that need to treat forks more strictly can compose a
        // dedicated fork-λ gate downstream.
        self.evaluate(ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};

    use aios_protocol::error::{KernelError, KernelResult};
    use aios_protocol::hypervisor::{
        BackendId, ForkSpec, VmHandle, VmId, VmSnapshotId, VmSpecOverrides, VmStatus,
    };
    use aios_protocol::ids::{ApprovalId, SessionId};
    use aios_protocol::kernel::{ChainId, WalletAttribution};
    use aios_protocol::ports::{ApprovalResolution, ApprovalTicket, PolicyGateDecision};
    use chrono::{TimeZone, Utc};

    fn ctx() -> KernelContext {
        KernelContext {
            session_id: SessionId::from_string("sess-policy-static"),
            agent_id: aios_protocol::ids::AgentId::from_string("agent-policy-static"),
            wallet: WalletAttribution {
                address: "0x0".into(),
                chain: ChainId::base(),
            },
            cost_hint: None,
            trace_ctx: None,
        }
    }

    fn vm_handle() -> VmHandle {
        VmHandle {
            vm_id: VmId::from("vm-policy-static"),
            backend: BackendId::from("stub"),
            session_id: SessionId::from_string("sess-policy-static"),
            agent_id: aios_protocol::ids::AgentId::from_string("agent-policy-static"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }

    fn fork_spec() -> ForkSpec {
        ForkSpec {
            parent_snapshot: VmSnapshotId::from("snap-policy-static"),
            overrides: VmSpecOverrides::default(),
        }
    }

    fn deterministic_ticket(capability: Capability) -> ApprovalTicket {
        ApprovalTicket {
            approval_id: ApprovalId::from_string("app-policy-static"),
            session_id: SessionId::from_string("sess-policy-static"),
            call_id: String::new(),
            tool_name: String::new(),
            capability,
            reason: "policy-static approval required".into(),
            created_at: Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).unwrap(),
        }
    }

    /// Stub [`PolicyGatePort`] that returns a cloned canned decision
    /// on every `evaluate` call and rejects `set_policy` as
    /// unsupported (this gate only reads).
    struct StubPolicyGate {
        decision: PolicyGateDecision,
    }

    #[async_trait]
    impl PolicyGatePort for StubPolicyGate {
        async fn evaluate(
            &self,
            _session_id: SessionId,
            _requested: Vec<Capability>,
        ) -> KernelResult<PolicyGateDecision> {
            Ok(self.decision.clone())
        }
    }

    /// Stub [`ApprovalPort`] that returns a canned ticket on every
    /// `enqueue`, fails `list_pending` / `resolve` because they are
    /// unused by `StaticPolicyGate` but required by the trait.
    struct StubApprovalPort {
        ticket: ApprovalTicket,
        enqueue_calls: AtomicU64,
    }

    impl StubApprovalPort {
        fn new(ticket: ApprovalTicket) -> Self {
            Self {
                ticket,
                enqueue_calls: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl ApprovalPort for StubApprovalPort {
        async fn enqueue(&self, _request: ApprovalRequest) -> KernelResult<ApprovalTicket> {
            self.enqueue_calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.ticket.clone())
        }

        async fn list_pending(&self, _session_id: SessionId) -> KernelResult<Vec<ApprovalTicket>> {
            Ok(Vec::new())
        }

        async fn resolve(
            &self,
            approval_id: ApprovalId,
            approved: bool,
            actor: String,
        ) -> KernelResult<ApprovalResolution> {
            Ok(ApprovalResolution {
                approval_id,
                approved,
                actor,
                resolved_at: Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).unwrap(),
            })
        }
    }

    /// Stub approval port that always fails `enqueue`, used to assert
    /// that an enqueue error surfaces as a `Deny` with the static
    /// gate id.
    struct FailingApprovalPort;

    #[async_trait]
    impl ApprovalPort for FailingApprovalPort {
        async fn enqueue(&self, _request: ApprovalRequest) -> KernelResult<ApprovalTicket> {
            Err(KernelError::Runtime("approval backend offline".into()))
        }

        async fn list_pending(&self, _session_id: SessionId) -> KernelResult<Vec<ApprovalTicket>> {
            Ok(Vec::new())
        }

        async fn resolve(
            &self,
            approval_id: ApprovalId,
            approved: bool,
            actor: String,
        ) -> KernelResult<ApprovalResolution> {
            Ok(ApprovalResolution {
                approval_id,
                approved,
                actor,
                resolved_at: Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).unwrap(),
            })
        }
    }

    /// Stub policy port that always fails `evaluate`.
    struct FailingPolicyGate;

    #[async_trait]
    impl PolicyGatePort for FailingPolicyGate {
        async fn evaluate(
            &self,
            _session_id: SessionId,
            _requested: Vec<Capability>,
        ) -> KernelResult<PolicyGateDecision> {
            Err(KernelError::Runtime("policy backend offline".into()))
        }
    }

    fn gate_id_of(decision: &BudgetDecision) -> Option<&str> {
        match decision {
            BudgetDecision::Deny { gate_id, .. } => Some(gate_id.as_str()),
            _ => None,
        }
    }

    #[tokio::test]
    async fn static_policy_allows_when_all_capabilities_allowed() {
        let policy = Arc::new(StubPolicyGate {
            decision: PolicyGateDecision {
                allowed: vec![Capability::new("tool:echo")],
                requires_approval: Vec::new(),
                denied: Vec::new(),
            },
        });
        let approvals = Arc::new(StubApprovalPort::new(deterministic_ticket(
            Capability::new("unused"),
        )));
        let gate = StaticPolicyGate::new(policy, approvals.clone());

        let decision = gate
            .check_dispatch(&ctx(), &ResourceBudget::default())
            .await;
        assert_eq!(decision, BudgetDecision::Allow);
        // Approval port should not be touched on the allow path.
        assert_eq!(approvals.enqueue_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn static_policy_denies_when_any_capability_denied() {
        let denied = vec![Capability::new("secrets:read:prod")];
        let policy = Arc::new(StubPolicyGate {
            decision: PolicyGateDecision {
                allowed: Vec::new(),
                requires_approval: Vec::new(),
                denied: denied.clone(),
            },
        });
        let approvals = Arc::new(StubApprovalPort::new(deterministic_ticket(
            Capability::new("unused"),
        )));
        let gate = StaticPolicyGate::new(policy, approvals);

        let decision = gate
            .check_dispatch(&ctx(), &ResourceBudget::default())
            .await;
        match decision {
            BudgetDecision::Deny { gate_id, reason } => {
                assert_eq!(gate_id, STATIC_POLICY_GATE_ID);
                assert!(reason.contains("secrets:read:prod"), "reason={reason}");
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn static_policy_requires_approval_when_any_requires_approval() {
        let requested_cap = Capability::new("exec:cmd:rm -rf /");
        let ticket = deterministic_ticket(requested_cap.clone());
        let policy = Arc::new(StubPolicyGate {
            decision: PolicyGateDecision {
                allowed: Vec::new(),
                requires_approval: vec![requested_cap.clone()],
                denied: Vec::new(),
            },
        });
        let approvals = Arc::new(StubApprovalPort::new(ticket.clone()));
        let gate = StaticPolicyGate::new(policy, approvals.clone());

        let decision = gate
            .check_dispatch(&ctx(), &ResourceBudget::default())
            .await;
        match decision {
            BudgetDecision::RequireApproval { ticket: t } => assert_eq!(t, ticket),
            other => panic!("expected RequireApproval, got {other:?}"),
        }
        assert_eq!(approvals.enqueue_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn static_policy_fork_uses_same_policy_path() {
        let denied = vec![Capability::new("exec:cmd:sudo")];
        let policy = Arc::new(StubPolicyGate {
            decision: PolicyGateDecision {
                allowed: Vec::new(),
                requires_approval: Vec::new(),
                denied: denied.clone(),
            },
        });
        let approvals = Arc::new(StubApprovalPort::new(deterministic_ticket(
            Capability::new("unused"),
        )));
        let gate = StaticPolicyGate::new(policy, approvals);

        let decision = gate.check_fork(&vm_handle(), &fork_spec(), &ctx()).await;
        assert_eq!(gate_id_of(&decision), Some(STATIC_POLICY_GATE_ID));
        match decision {
            BudgetDecision::Deny { reason, .. } => {
                assert!(reason.contains("exec:cmd:sudo"));
            }
            other => panic!("expected Deny on fork, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn static_policy_denies_when_policy_evaluation_fails() {
        let policy = Arc::new(FailingPolicyGate);
        let approvals = Arc::new(StubApprovalPort::new(deterministic_ticket(
            Capability::new("unused"),
        )));
        let gate = StaticPolicyGate::new(policy, approvals);

        let decision = gate
            .check_dispatch(&ctx(), &ResourceBudget::default())
            .await;
        match decision {
            BudgetDecision::Deny { gate_id, reason } => {
                assert_eq!(gate_id, STATIC_POLICY_GATE_ID);
                assert!(
                    reason.contains("policy evaluation failed"),
                    "reason={reason}"
                );
            }
            other => panic!("expected Deny on evaluate error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn static_policy_denies_when_approval_enqueue_fails() {
        let requested_cap = Capability::new("exec:cmd:systemctl stop");
        let policy = Arc::new(StubPolicyGate {
            decision: PolicyGateDecision {
                allowed: Vec::new(),
                requires_approval: vec![requested_cap.clone()],
                denied: Vec::new(),
            },
        });
        let approvals = Arc::new(FailingApprovalPort);
        let gate = StaticPolicyGate::new(policy, approvals);

        let decision = gate
            .check_dispatch(&ctx(), &ResourceBudget::default())
            .await;
        match decision {
            BudgetDecision::Deny { gate_id, reason } => {
                assert_eq!(gate_id, STATIC_POLICY_GATE_ID);
                assert!(
                    reason.contains("approval enqueue failed"),
                    "reason={reason}"
                );
            }
            other => panic!("expected Deny on enqueue failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn static_policy_is_dyn_compatible() {
        let policy = Arc::new(StubPolicyGate {
            decision: PolicyGateDecision {
                allowed: Vec::new(),
                requires_approval: Vec::new(),
                denied: Vec::new(),
            },
        });
        let approvals = Arc::new(StubApprovalPort::new(deterministic_ticket(
            Capability::new("unused"),
        )));
        let gate: Arc<dyn BudgetGatePort> = Arc::new(StaticPolicyGate::new(policy, approvals));
        let decision = gate
            .check_dispatch(&ctx(), &ResourceBudget::default())
            .await;
        assert_eq!(decision, BudgetDecision::Allow);
    }
}
