//! Deterministic gate chain composing policy, budget, fork-λ, and network
//! isolation checks in a fixed order.
//!
//! The chain is pure: every decision is a total function of `(inputs,
//! injected ports)`. No field mutation; any observable state change
//! (denials, approvals) is surfaced to the caller through
//! [`GateDecision`] and emitted as an event by the engine via its
//! [`crate::event_emitter::EventEmitter`], not stored here.
//!
//! ## Ordering
//!
//! `check_dispatch` runs **policy → budget**. Gates are short-circuiting:
//! the first non-[`BudgetDecision::Allow`] decision wins. A
//! [`BudgetDecision::Deny`] surfaces as [`GateDecision::Deny`] carrying
//! the appropriate [`GateKind`] discriminator and the gate's own
//! `gate_id`/`reason`. A [`BudgetDecision::RequireApproval`] surfaces as
//! [`GateDecision::RequireApproval`] — the engine is responsible for
//! deciding whether to pause, fail, or route through an approval port.
//!
//! `check_fork` runs **policy → budget → fork-λ** (fork-λ is optional).
//! Denials from the fork-λ gate carry [`GateKind::ForkLambda`].
//!
//! `apply_network` is separate from dispatch/fork — it is called once at
//! [`crate::engine::KernelEngine::create_vm`] time to materialise the
//! VM's network policy through the injected
//! [`NetworkIsolationPort`]. Per-dispatch egress recording is performed
//! by [`crate::metering::MeteringWrapper`] and not by this chain.
//!
//! ## Purity
//!
//! [`GateChain`] has no interior mutability. Every collaborator is
//! `Arc`-owned, so the chain itself is cheap to clone and safe to share
//! across concurrent tasks.

use std::sync::Arc;

use aios_protocol::budget::{BudgetDecision, BudgetGatePort, ResourceBudget};
use aios_protocol::hypervisor::{ForkSpec, VmHandle};
use aios_protocol::kernel::{GateKind, KernelContext, KernelResult};
use aios_protocol::network_isolation::NetworkIsolationPort;
use aios_protocol::ports::ApprovalTicket;
use aios_protocol::sandbox::NetworkPolicy;

/// Unified decision returned by the gate chain.
///
/// Variants are deliberately flat so callers can match on them without
/// further unwrapping. Serialisation is intentionally not derived —
/// decisions are ephemeral; only the kernel events they trigger are
/// durable.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GateDecision {
    /// Every stage in the chain returned `Allow`. Callers may proceed.
    Allow,
    /// A stage returned [`BudgetDecision::Deny`]. `gate` records which
    /// logical gate vetoed the call; `gate_id` is the underlying gate's
    /// stable identifier (e.g. `"policy-static"`); `reason` is
    /// human-readable context for audit events.
    Deny {
        /// Logical gate responsible for the denial.
        gate: GateKind,
        /// Human-readable reason for the denial.
        reason: String,
        /// Stable identifier of the gate that issued the denial.
        gate_id: String,
    },
    /// A stage returned [`BudgetDecision::RequireApproval`]. The
    /// enclosed ticket has already been enqueued on the approval queue
    /// by the gate that produced it; the engine routes this upward.
    RequireApproval {
        /// Ticket pre-enqueued by the gate that raised the approval
        /// requirement.
        ticket: ApprovalTicket,
    },
}

/// Ordered composition of the four gate ports the kernel consults.
///
/// Construction is guarded by [`GateChainBuilder`] so the required
/// collaborators (`policy`, `budget`, `network_isolation`) are present
/// before the chain becomes observable. `fork_lambda_gate` is optional
/// because the MVS wiring in Phase 1 omits it entirely.
#[derive(Clone)]
pub struct GateChain {
    /// Policy gate expressed as a [`BudgetGatePort`] (life-kernel-gate's
    /// `StaticPolicyGate` maps `PolicyGateDecision` onto
    /// [`BudgetDecision`]).
    policy: Arc<dyn BudgetGatePort>,
    /// Budget gate — session caps / RCS-λ in later phases, no-op in
    /// Phase 1.
    budget: Arc<dyn BudgetGatePort>,
    /// Optional fork-λ gate consulted only by [`check_fork`].
    fork_lambda: Option<Arc<dyn BudgetGatePort>>,
    /// Declarative network policy captured at chain construction; passed
    /// verbatim into [`NetworkIsolationPort::apply`] at VM start.
    network_policy: NetworkPolicy,
    /// Network isolation port (no-op in Phase 1, eBPF in Phase 6).
    network: Arc<dyn NetworkIsolationPort>,
}

impl GateChain {
    /// Start building a [`GateChain`].
    ///
    /// Use the resulting [`GateChainBuilder`] to wire collaborators and
    /// call [`GateChainBuilder::build`] to finalise the chain.
    pub fn builder() -> GateChainBuilder {
        GateChainBuilder::default()
    }

    /// Run the dispatch chain: **policy → budget**.
    ///
    /// Returns the first non-[`BudgetDecision::Allow`] decision it
    /// observes. When every stage returns [`BudgetDecision::Allow`],
    /// returns [`GateDecision::Allow`].
    pub async fn check_dispatch(
        &self,
        _vm: &VmHandle,
        ctx: &KernelContext,
        cost_hint: &ResourceBudget,
    ) -> GateDecision {
        // 1. Policy gate.
        if let Some(decision) = lift_decision(
            self.policy.check_dispatch(ctx, cost_hint).await,
            GateKind::Policy,
        ) {
            return decision;
        }

        // 2. Budget gate.
        if let Some(decision) = lift_decision(
            self.budget.check_dispatch(ctx, cost_hint).await,
            GateKind::Budget,
        ) {
            return decision;
        }

        GateDecision::Allow
    }

    /// Run the fork chain: **policy → budget → fork-λ**.
    ///
    /// The fork-λ stage runs only when a fork-λ gate was configured.
    /// Semantics match [`check_dispatch`] otherwise.
    pub async fn check_fork(
        &self,
        parent: &VmHandle,
        spec: &ForkSpec,
        ctx: &KernelContext,
    ) -> GateDecision {
        // 1. Policy gate.
        if let Some(decision) = lift_decision(
            self.policy.check_fork(parent, spec, ctx).await,
            GateKind::Policy,
        ) {
            return decision;
        }

        // 2. Budget gate.
        if let Some(decision) = lift_decision(
            self.budget.check_fork(parent, spec, ctx).await,
            GateKind::Budget,
        ) {
            return decision;
        }

        // 3. Fork-λ gate (optional).
        if let Some(fork_lambda) = &self.fork_lambda
            && let Some(decision) = lift_decision(
                fork_lambda.check_fork(parent, spec, ctx).await,
                GateKind::ForkLambda,
            )
        {
            return decision;
        }

        GateDecision::Allow
    }

    /// Materialise the chain's declared network policy on `vm`.
    ///
    /// Called once at VM start, after the backend returns its handle.
    /// A failure aborts bring-up — the engine is expected to propagate
    /// the error back to the originating [`crate::engine::KernelEngine::create_vm`]
    /// call.
    pub async fn apply_network(&self, vm: &VmHandle) -> KernelResult<()> {
        self.network.apply(vm, &self.network_policy).await
    }

    /// Access the declared network policy (useful for logs / tests).
    pub fn network_policy(&self) -> &NetworkPolicy {
        &self.network_policy
    }
}

/// Map a [`BudgetDecision`] produced by an inner gate into an optional
/// [`GateDecision`].
///
/// Returns `None` for [`BudgetDecision::Allow`] (the caller should move
/// on to the next stage) and `Some(_)` for [`BudgetDecision::Deny`] or
/// [`BudgetDecision::RequireApproval`]. The `kind` argument supplies
/// the [`GateKind`] discriminator attached to [`GateDecision::Deny`].
///
/// The `_` arm is present because [`BudgetDecision`] is `#[non_exhaustive]`;
/// a future variant surfaces as an opaque `Deny` carrying the discriminator
/// so the engine sees a stable error path instead of a panic.
fn lift_decision(decision: BudgetDecision, kind: GateKind) -> Option<GateDecision> {
    match decision {
        BudgetDecision::Allow => None,
        BudgetDecision::Deny { reason, gate_id } => Some(GateDecision::Deny {
            gate: kind,
            reason,
            gate_id,
        }),
        BudgetDecision::RequireApproval { ticket } => {
            Some(GateDecision::RequireApproval { ticket })
        }
        // Forward-compat — an unknown variant is treated as a Deny so the
        // engine never observes a hidden Allow that could undermine the
        // chain's safety posture.
        _ => Some(GateDecision::Deny {
            gate: kind,
            reason: "unknown BudgetDecision variant".into(),
            gate_id: "gate-chain-unknown-variant".into(),
        }),
    }
}

/// Staging builder for [`GateChain`].
///
/// Required fields: [`policy`](Self::policy), [`budget`](Self::budget),
/// [`network_isolation`](Self::network_isolation). Optional fields:
/// [`fork_lambda`](Self::fork_lambda) (default `None`) and
/// [`network_policy`](Self::network_policy) (default
/// [`NetworkPolicy::Disabled`]).
#[derive(Default)]
pub struct GateChainBuilder {
    policy: Option<Arc<dyn BudgetGatePort>>,
    budget: Option<Arc<dyn BudgetGatePort>>,
    fork_lambda: Option<Arc<dyn BudgetGatePort>>,
    network_policy: NetworkPolicy,
    network: Option<Arc<dyn NetworkIsolationPort>>,
}

/// Error returned by [`GateChainBuilder::build`] when a required
/// collaborator was not set.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GateChainBuildError {
    /// The `policy` collaborator was not supplied.
    #[error("GateChainBuilder: `policy` is required")]
    MissingPolicy,
    /// The `budget` collaborator was not supplied.
    #[error("GateChainBuilder: `budget` is required")]
    MissingBudget,
    /// The `network_isolation` collaborator was not supplied.
    #[error("GateChainBuilder: `network_isolation` is required")]
    MissingNetworkIsolation,
}

impl GateChainBuilder {
    /// Install the policy gate (required).
    pub fn policy(mut self, policy: Arc<dyn BudgetGatePort>) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Install the budget gate (required).
    pub fn budget(mut self, budget: Arc<dyn BudgetGatePort>) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Install the optional fork-λ gate.
    pub fn fork_lambda(mut self, fork_lambda: Arc<dyn BudgetGatePort>) -> Self {
        self.fork_lambda = Some(fork_lambda);
        self
    }

    /// Override the default [`NetworkPolicy::Disabled`] applied at VM
    /// start.
    pub fn network_policy(mut self, policy: NetworkPolicy) -> Self {
        self.network_policy = policy;
        self
    }

    /// Install the network isolation port (required).
    pub fn network_isolation(mut self, network: Arc<dyn NetworkIsolationPort>) -> Self {
        self.network = Some(network);
        self
    }

    /// Finalise the builder. Returns an error if a required field was
    /// missing.
    pub fn build(self) -> Result<GateChain, GateChainBuildError> {
        Ok(GateChain {
            policy: self.policy.ok_or(GateChainBuildError::MissingPolicy)?,
            budget: self.budget.ok_or(GateChainBuildError::MissingBudget)?,
            fork_lambda: self.fork_lambda,
            network_policy: self.network_policy,
            network: self
                .network
                .ok_or(GateChainBuildError::MissingNetworkIsolation)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use aios_protocol::hypervisor::{BackendId, VmId, VmSnapshotId, VmSpecOverrides, VmStatus};
    use aios_protocol::ids::{AgentId, ApprovalId, SessionId};
    use aios_protocol::kernel::{ChainId, WalletAttribution};
    use aios_protocol::policy::Capability;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};

    // ── In-test gate stubs ──────────────────────────────────────────

    /// Trivial `BudgetGatePort` that always returns [`BudgetDecision::Allow`].
    struct AllowGate;

    #[async_trait]
    impl BudgetGatePort for AllowGate {
        async fn check_dispatch(
            &self,
            _ctx: &KernelContext,
            _cost_hint: &ResourceBudget,
        ) -> BudgetDecision {
            BudgetDecision::Allow
        }

        async fn check_fork(
            &self,
            _parent: &VmHandle,
            _spec: &ForkSpec,
            _ctx: &KernelContext,
        ) -> BudgetDecision {
            BudgetDecision::Allow
        }
    }

    /// `BudgetGatePort` stub that always returns
    /// [`BudgetDecision::Deny`] with a canned `reason` / `gate_id`.
    struct DenyGate {
        reason: &'static str,
        gate_id: &'static str,
    }

    #[async_trait]
    impl BudgetGatePort for DenyGate {
        async fn check_dispatch(
            &self,
            _ctx: &KernelContext,
            _cost_hint: &ResourceBudget,
        ) -> BudgetDecision {
            BudgetDecision::Deny {
                reason: self.reason.into(),
                gate_id: self.gate_id.into(),
            }
        }

        async fn check_fork(
            &self,
            _parent: &VmHandle,
            _spec: &ForkSpec,
            _ctx: &KernelContext,
        ) -> BudgetDecision {
            BudgetDecision::Deny {
                reason: self.reason.into(),
                gate_id: self.gate_id.into(),
            }
        }
    }

    /// `BudgetGatePort` stub that always returns
    /// [`BudgetDecision::RequireApproval`] with a canned ticket.
    struct RequireApprovalGate {
        ticket: ApprovalTicket,
    }

    #[async_trait]
    impl BudgetGatePort for RequireApprovalGate {
        async fn check_dispatch(
            &self,
            _ctx: &KernelContext,
            _cost_hint: &ResourceBudget,
        ) -> BudgetDecision {
            BudgetDecision::RequireApproval {
                ticket: self.ticket.clone(),
            }
        }

        async fn check_fork(
            &self,
            _parent: &VmHandle,
            _spec: &ForkSpec,
            _ctx: &KernelContext,
        ) -> BudgetDecision {
            BudgetDecision::RequireApproval {
                ticket: self.ticket.clone(),
            }
        }
    }

    /// `BudgetGatePort` stub that panics on every call. Used to assert
    /// that short-circuit ordering prevents a later stage from running.
    struct PanicGate;

    #[async_trait]
    impl BudgetGatePort for PanicGate {
        async fn check_dispatch(
            &self,
            _ctx: &KernelContext,
            _cost_hint: &ResourceBudget,
        ) -> BudgetDecision {
            panic!("PanicGate::check_dispatch must not be reached")
        }

        async fn check_fork(
            &self,
            _parent: &VmHandle,
            _spec: &ForkSpec,
            _ctx: &KernelContext,
        ) -> BudgetDecision {
            panic!("PanicGate::check_fork must not be reached")
        }
    }

    /// `BudgetGatePort` that increments a call counter on every
    /// invocation. Used to assert a gate *was* executed.
    struct CountingAllowGate {
        calls: Arc<AtomicU64>,
    }

    #[async_trait]
    impl BudgetGatePort for CountingAllowGate {
        async fn check_dispatch(
            &self,
            _ctx: &KernelContext,
            _cost_hint: &ResourceBudget,
        ) -> BudgetDecision {
            self.calls.fetch_add(1, Ordering::Relaxed);
            BudgetDecision::Allow
        }

        async fn check_fork(
            &self,
            _parent: &VmHandle,
            _spec: &ForkSpec,
            _ctx: &KernelContext,
        ) -> BudgetDecision {
            self.calls.fetch_add(1, Ordering::Relaxed);
            BudgetDecision::Allow
        }
    }

    // ── In-test NetworkIsolationPort stub ───────────────────────────

    struct RecordingNetwork {
        applied: AtomicBool,
    }

    impl RecordingNetwork {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                applied: AtomicBool::new(false),
            })
        }
    }

    #[async_trait]
    impl NetworkIsolationPort for RecordingNetwork {
        async fn apply(&self, _vm: &VmHandle, _policy: &NetworkPolicy) -> KernelResult<()> {
            self.applied.store(true, Ordering::Relaxed);
            Ok(())
        }

        async fn record_egress(
            &self,
            _vm: &VmHandle,
            _bytes: u64,
            _dst: &aios_protocol::network_isolation::EgressTarget,
        ) -> KernelResult<()> {
            Ok(())
        }
    }

    // ── Shared fixtures ─────────────────────────────────────────────

    fn ctx() -> KernelContext {
        KernelContext {
            session_id: SessionId::from_string("sess-gate-chain"),
            agent_id: AgentId::from_string("agent-gate-chain"),
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
            vm_id: VmId::from("vm-gate-chain"),
            backend: BackendId::from("stub"),
            session_id: SessionId::from_string("sess-gate-chain"),
            agent_id: AgentId::from_string("agent-gate-chain"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }

    fn fork_spec() -> ForkSpec {
        ForkSpec {
            parent_snapshot: VmSnapshotId::from("snap-gate-chain"),
            overrides: VmSpecOverrides::default(),
        }
    }

    fn canned_ticket() -> ApprovalTicket {
        ApprovalTicket {
            approval_id: ApprovalId::from_string("app-gate-chain"),
            session_id: SessionId::from_string("sess-gate-chain"),
            call_id: "call-x".into(),
            tool_name: "tool.x".into(),
            capability: Capability::new("exec:test"),
            reason: "approval required".into(),
            created_at: Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).unwrap(),
        }
    }

    // ── Tests ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn gate_chain_policy_allow_budget_allow_returns_allow() {
        let network = RecordingNetwork::new();
        let chain = GateChain::builder()
            .policy(Arc::new(AllowGate))
            .budget(Arc::new(AllowGate))
            .network_isolation(network)
            .build()
            .expect("builder should succeed");

        let decision = chain
            .check_dispatch(&vm_handle(), &ctx(), &ResourceBudget::default())
            .await;
        assert_eq!(decision, GateDecision::Allow);
    }

    #[tokio::test]
    async fn gate_chain_policy_deny_short_circuits_before_budget() {
        // The budget gate is `PanicGate`: if the chain ever invokes it
        // after a policy denial, the test panics. A passing test proves
        // the short-circuit.
        let network = RecordingNetwork::new();
        let chain = GateChain::builder()
            .policy(Arc::new(DenyGate {
                reason: "policy rejects",
                gate_id: "policy-static",
            }))
            .budget(Arc::new(PanicGate))
            .network_isolation(network)
            .build()
            .expect("builder should succeed");

        let decision = chain
            .check_dispatch(&vm_handle(), &ctx(), &ResourceBudget::default())
            .await;
        match decision {
            GateDecision::Deny {
                gate,
                reason,
                gate_id,
            } => {
                assert_eq!(gate, GateKind::Policy);
                assert_eq!(gate_id, "policy-static");
                assert_eq!(reason, "policy rejects");
            }
            other => panic!("expected Deny(Policy), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn gate_chain_budget_deny_reports_gate_budget() {
        let network = RecordingNetwork::new();
        let chain = GateChain::builder()
            .policy(Arc::new(AllowGate))
            .budget(Arc::new(DenyGate {
                reason: "over cap",
                gate_id: "session-budget",
            }))
            .network_isolation(network)
            .build()
            .expect("builder should succeed");

        let decision = chain
            .check_dispatch(&vm_handle(), &ctx(), &ResourceBudget::default())
            .await;
        match decision {
            GateDecision::Deny {
                gate,
                reason,
                gate_id,
            } => {
                assert_eq!(gate, GateKind::Budget);
                assert_eq!(gate_id, "session-budget");
                assert_eq!(reason, "over cap");
            }
            other => panic!("expected Deny(Budget), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn gate_chain_fork_runs_fork_lambda_when_present() {
        // policy + budget allow; fork-λ rejects. The resulting Deny
        // must carry GateKind::ForkLambda so downstream audit can
        // distinguish the reason.
        let network = RecordingNetwork::new();
        let chain = GateChain::builder()
            .policy(Arc::new(AllowGate))
            .budget(Arc::new(AllowGate))
            .fork_lambda(Arc::new(DenyGate {
                reason: "λ exhausted",
                gate_id: "rcs-lambda",
            }))
            .network_isolation(network)
            .build()
            .expect("builder should succeed");

        let decision = chain.check_fork(&vm_handle(), &fork_spec(), &ctx()).await;
        match decision {
            GateDecision::Deny {
                gate,
                reason,
                gate_id,
            } => {
                assert_eq!(gate, GateKind::ForkLambda);
                assert_eq!(gate_id, "rcs-lambda");
                assert_eq!(reason, "λ exhausted");
            }
            other => panic!("expected Deny(ForkLambda), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn gate_chain_fork_skips_fork_lambda_when_none() {
        // No fork-λ gate installed; the chain must still return Allow
        // when policy + budget allow. CountingAllowGate lets us assert
        // exactly two gate calls (one per stage).
        let policy_calls = Arc::new(AtomicU64::new(0));
        let budget_calls = Arc::new(AtomicU64::new(0));
        let policy = CountingAllowGate {
            calls: policy_calls.clone(),
        };
        let budget = CountingAllowGate {
            calls: budget_calls.clone(),
        };
        let network = RecordingNetwork::new();
        let chain = GateChain::builder()
            .policy(Arc::new(policy))
            .budget(Arc::new(budget))
            .network_isolation(network)
            .build()
            .expect("builder should succeed");

        let decision = chain.check_fork(&vm_handle(), &fork_spec(), &ctx()).await;
        assert_eq!(decision, GateDecision::Allow);
        assert_eq!(policy_calls.load(Ordering::Relaxed), 1);
        assert_eq!(budget_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn gate_chain_require_approval_surfaces_from_policy_stage() {
        // When the policy stage raises RequireApproval, later stages
        // must NOT run. PanicGate on budget proves the short-circuit.
        let ticket = canned_ticket();
        let network = RecordingNetwork::new();
        let chain = GateChain::builder()
            .policy(Arc::new(RequireApprovalGate {
                ticket: ticket.clone(),
            }))
            .budget(Arc::new(PanicGate))
            .network_isolation(network)
            .build()
            .expect("builder should succeed");

        let decision = chain
            .check_dispatch(&vm_handle(), &ctx(), &ResourceBudget::default())
            .await;
        match decision {
            GateDecision::RequireApproval { ticket: t } => assert_eq!(t, ticket),
            other => panic!("expected RequireApproval, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn gate_chain_apply_network_delegates_to_port() {
        let network = RecordingNetwork::new();
        let chain = GateChain::builder()
            .policy(Arc::new(AllowGate))
            .budget(Arc::new(AllowGate))
            .network_policy(NetworkPolicy::AllowAll)
            .network_isolation(network.clone())
            .build()
            .expect("builder should succeed");

        assert_eq!(chain.network_policy(), &NetworkPolicy::AllowAll);
        chain
            .apply_network(&vm_handle())
            .await
            .expect("apply_network should succeed");
        assert!(network.applied.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn gate_chain_builder_rejects_missing_fields() {
        // `GateChain` holds `Arc<dyn …>` fields that are not `Debug`, so
        // `expect_err` does not compile. Match manually instead.

        // Missing policy.
        match GateChain::builder()
            .budget(Arc::new(AllowGate))
            .network_isolation(RecordingNetwork::new())
            .build()
        {
            Err(GateChainBuildError::MissingPolicy) => {}
            Err(other) => panic!("expected MissingPolicy, got {other:?}"),
            Ok(_) => panic!("expected MissingPolicy, got Ok"),
        }

        // Missing budget.
        match GateChain::builder()
            .policy(Arc::new(AllowGate))
            .network_isolation(RecordingNetwork::new())
            .build()
        {
            Err(GateChainBuildError::MissingBudget) => {}
            Err(other) => panic!("expected MissingBudget, got {other:?}"),
            Ok(_) => panic!("expected MissingBudget, got Ok"),
        }

        // Missing network.
        match GateChain::builder()
            .policy(Arc::new(AllowGate))
            .budget(Arc::new(AllowGate))
            .build()
        {
            Err(GateChainBuildError::MissingNetworkIsolation) => {}
            Err(other) => panic!("expected MissingNetworkIsolation, got {other:?}"),
            Ok(_) => panic!("expected MissingNetworkIsolation, got Ok"),
        }
    }
}
