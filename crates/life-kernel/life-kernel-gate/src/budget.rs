//! Permissive budget gate: allows every check, logs the cost hint.
//!
//! Phase 1 ships this impl as the default wiring in `life-kernel-core`'s
//! gate chain so the engine dispatch path compiles and runs end-to-end
//! with no real enforcement. Real budget enforcement (session caps,
//! RCS-λ fork gate) lands in Phase 4 / Phase 6.

use aios_protocol::budget::{BudgetDecision, BudgetGatePort, ResourceBudget};
use aios_protocol::hypervisor::{ForkSpec, VmHandle};
use aios_protocol::kernel::KernelContext;
use async_trait::async_trait;

/// [`BudgetGatePort`] impl that permits every dispatch and fork.
///
/// Every call returns [`BudgetDecision::Allow`] and logs the incoming
/// [`ResourceBudget`] / [`KernelContext`] via `tracing::debug!` under the
/// target `life_kernel_gate::budget::noop`. The gate holds no state; it
/// is safe to share a single instance across arbitrarily many concurrent
/// dispatches.
///
/// Intended as the MVS default. Real budget enforcement is Phase 4.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use aios_protocol::budget::BudgetGatePort;
/// use life_kernel_gate::NoOpBudgetGate;
///
/// // Share the same permissive gate across every KernelEngine wiring
/// // slot (policy, budget, fork-λ) until the real gates ship.
/// let gate: Arc<dyn BudgetGatePort> = Arc::new(NoOpBudgetGate::default());
/// # let _ = gate;
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOpBudgetGate;

impl NoOpBudgetGate {
    /// Construct a new permissive gate. Equivalent to
    /// [`NoOpBudgetGate::default`].
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl BudgetGatePort for NoOpBudgetGate {
    async fn check_dispatch(
        &self,
        ctx: &KernelContext,
        cost_hint: &ResourceBudget,
    ) -> BudgetDecision {
        tracing::debug!(
            target: "life_kernel_gate::budget::noop",
            session_id = %ctx.session_id.as_str(),
            agent_id = %ctx.agent_id.as_str(),
            max_cpu_ms = ?cost_hint.max_cpu_ms,
            max_mem_kb = ?cost_hint.max_mem_kb,
            max_egress_bytes = ?cost_hint.max_egress_bytes,
            max_duration_ms = ?cost_hint.max_duration_ms,
            max_syscalls = ?cost_hint.max_syscalls,
            "NoOpBudgetGate::check_dispatch permit"
        );
        BudgetDecision::Allow
    }

    async fn check_fork(
        &self,
        _parent: &VmHandle,
        _spec: &ForkSpec,
        ctx: &KernelContext,
    ) -> BudgetDecision {
        tracing::debug!(
            target: "life_kernel_gate::budget::noop",
            session_id = %ctx.session_id.as_str(),
            agent_id = %ctx.agent_id.as_str(),
            "NoOpBudgetGate::check_fork permit"
        );
        BudgetDecision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use aios_protocol::hypervisor::{
        BackendId, ForkSpec, VmHandle, VmId, VmSnapshotId, VmSpecOverrides, VmStatus,
    };
    use aios_protocol::ids::{AgentId, SessionId};
    use aios_protocol::kernel::{ChainId, WalletAttribution};
    use chrono::Utc;

    /// Build a minimal, valid [`KernelContext`] for tests.
    fn ctx() -> KernelContext {
        KernelContext {
            session_id: SessionId::from_string("sess-budget-noop"),
            agent_id: AgentId::from_string("agent-budget-noop"),
            wallet: WalletAttribution {
                address: "0x0000000000000000000000000000000000000000".into(),
                chain: ChainId::base(),
            },
            cost_hint: None,
            trace_ctx: None,
        }
    }

    /// Build a minimal [`VmHandle`] for fork tests.
    fn vm_handle() -> VmHandle {
        VmHandle {
            vm_id: VmId::from("vm-noop-budget"),
            backend: BackendId::from("stub"),
            session_id: SessionId::from_string("sess-budget-noop"),
            agent_id: AgentId::from_string("agent-budget-noop"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }

    /// Build a minimal [`ForkSpec`] for fork tests.
    fn fork_spec() -> ForkSpec {
        ForkSpec {
            parent_snapshot: VmSnapshotId::from("snap-noop-budget"),
            overrides: VmSpecOverrides::default(),
        }
    }

    #[tokio::test]
    async fn noop_dispatch_returns_allow() {
        let gate = NoOpBudgetGate::new();
        let cost_hint = ResourceBudget::default();
        let decision = gate.check_dispatch(&ctx(), &cost_hint).await;
        assert_eq!(decision, BudgetDecision::Allow);
    }

    #[tokio::test]
    async fn noop_fork_returns_allow() {
        let gate = NoOpBudgetGate::new();
        let decision = gate.check_fork(&vm_handle(), &fork_spec(), &ctx()).await;
        assert_eq!(decision, BudgetDecision::Allow);
    }

    #[tokio::test]
    async fn noop_dispatch_returns_allow_with_partial_cost_hint() {
        // Exercises the debug-logging path with a populated cost hint to
        // lock in that non-None fields do not change the decision.
        let gate = NoOpBudgetGate;
        let cost_hint = ResourceBudget {
            max_cpu_ms: Some(2_000),
            max_duration_ms: Some(10_000),
            max_egress_bytes: Some(1 << 20),
            ..Default::default()
        };
        let decision = gate.check_dispatch(&ctx(), &cost_hint).await;
        assert_eq!(decision, BudgetDecision::Allow);
    }

    #[tokio::test]
    async fn noop_is_dyn_compatible() {
        // The engine's gate chain stores `Arc<dyn BudgetGatePort>`, so
        // verify the impl upholds dyn compatibility at runtime.
        let gate: std::sync::Arc<dyn BudgetGatePort> = std::sync::Arc::new(NoOpBudgetGate::new());
        let decision = gate
            .check_dispatch(&ctx(), &ResourceBudget::default())
            .await;
        assert_eq!(decision, BudgetDecision::Allow);
    }
}
