//! Property-based tests on the [`GateChain`] state machine.
//!
//! Asserts three invariants over thousands of random gate-outcome
//! combinations:
//!
//! 1. **Determinism** — two `GateChain`s built with identical inputs
//!    return identical [`GateDecision`]s.
//! 2. **Monotonicity** — tightening a gate (loose → strict, or `Allow`
//!    → `Deny`/`RequireApproval`) never relaxes the overall decision. A
//!    chain that only tightens can never produce `Allow` where the
//!    all-`Allow` baseline would have.
//! 3. **Attribution** — when the chain returns
//!    [`GateDecision::Deny`], the surfaced `gate_id` matches the stub
//!    gate that actually returned `Deny`, in the short-circuit order
//!    declared by [`GateChain::check_dispatch`] (policy → budget).
//!
//! The `GateChain` in this worktree runs gates in order `policy →
//! budget` and short-circuits on the first non-`Allow` outcome from
//! either gate, lifting [`BudgetDecision::Deny`] to
//! [`GateDecision::Deny`] and [`BudgetDecision::RequireApproval`] to
//! [`GateDecision::RequireApproval`]. The property assertions below
//! reflect that exact ordering — they are not independent of it.

use std::sync::Arc;

use aios_protocol::budget::{BudgetDecision, BudgetGatePort, ResourceBudget};
use aios_protocol::hypervisor::{BackendId, ForkSpec, VmHandle, VmId, VmStatus};
use aios_protocol::ids::{AgentId, ApprovalId, SessionId};
use aios_protocol::kernel::{ChainId, KernelContext, WalletAttribution};
use aios_protocol::kernel::{GateKind, KernelResult};
use aios_protocol::network_isolation::{EgressTarget, NetworkIsolationPort};
use aios_protocol::policy::Capability;
use aios_protocol::ports::ApprovalTicket;
use aios_protocol::sandbox::NetworkPolicy;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use life_kernel_core::gate_chain::{GateChain, GateDecision};
use proptest::prelude::*;

// ── Stub gates parameterised by outcome ────────────────────────────────

/// Simple ternary outcome mirroring the three [`BudgetDecision`]
/// variants that [`GateChain`] is sensitive to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateOutcome {
    Allow,
    Deny,
    Approval,
}

/// [`BudgetGatePort`] stub whose responses are fully determined by the
/// [`GateOutcome`] passed at construction. `id` doubles as the
/// `gate_id` surfaced in [`BudgetDecision::Deny`] so attribution tests
/// can match on it.
struct StubBudgetGate {
    id: &'static str,
    outcome: GateOutcome,
}

#[async_trait]
impl BudgetGatePort for StubBudgetGate {
    async fn check_dispatch(
        &self,
        _ctx: &KernelContext,
        _cost_hint: &ResourceBudget,
    ) -> BudgetDecision {
        match self.outcome {
            GateOutcome::Allow => BudgetDecision::Allow,
            GateOutcome::Deny => BudgetDecision::Deny {
                reason: format!("stub-{} deny", self.id),
                gate_id: self.id.to_string(),
            },
            GateOutcome::Approval => BudgetDecision::RequireApproval {
                ticket: test_ticket(self.id),
            },
        }
    }

    async fn check_fork(
        &self,
        _parent: &VmHandle,
        _spec: &ForkSpec,
        _ctx: &KernelContext,
    ) -> BudgetDecision {
        // Same deterministic behaviour as `check_dispatch` — the proptest
        // suite exercises `check_dispatch`, but returning a coherent
        // response here keeps the stub safe for any caller.
        match self.outcome {
            GateOutcome::Allow => BudgetDecision::Allow,
            GateOutcome::Deny => BudgetDecision::Deny {
                reason: format!("stub-{} deny", self.id),
                gate_id: self.id.to_string(),
            },
            GateOutcome::Approval => BudgetDecision::RequireApproval {
                ticket: test_ticket(self.id),
            },
        }
    }
}

/// No-op [`NetworkIsolationPort`] — `check_dispatch` never invokes it,
/// so these methods exist only to satisfy the builder.
struct StubNetwork;

#[async_trait]
impl NetworkIsolationPort for StubNetwork {
    async fn apply(&self, _vm: &VmHandle, _policy: &NetworkPolicy) -> KernelResult<()> {
        Ok(())
    }

    async fn record_egress(
        &self,
        _vm: &VmHandle,
        _bytes: u64,
        _dst: &EgressTarget,
    ) -> KernelResult<()> {
        Ok(())
    }
}

// ── Test fixtures ──────────────────────────────────────────────────────

/// Minimal [`ApprovalTicket`] keyed on the stub id so it remains stable
/// across property iterations.
fn test_ticket(gate_id: &str) -> ApprovalTicket {
    ApprovalTicket {
        approval_id: ApprovalId::from_string(format!("app-{gate_id}")),
        session_id: SessionId::from_string("sess-proptest"),
        call_id: "call-proptest".into(),
        tool_name: "tool.proptest".into(),
        capability: Capability::new("exec:proptest"),
        reason: format!("stub-{gate_id} approval"),
        created_at: Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).unwrap(),
    }
}

/// Canonical [`KernelContext`] — kept constant so any non-determinism
/// must originate in the chain itself, not the fixtures.
fn test_ctx() -> KernelContext {
    KernelContext {
        session_id: SessionId::from_string("sess-proptest"),
        agent_id: AgentId::from_string("agent-proptest"),
        wallet: WalletAttribution {
            address: "0x0".into(),
            chain: ChainId::base(),
        },
        cost_hint: None,
        trace_ctx: None,
    }
}

/// Canonical [`VmHandle`] — chain's dispatch path never inspects it,
/// but we need a real value to call [`GateChain::check_dispatch`].
fn test_vm() -> VmHandle {
    VmHandle {
        vm_id: VmId::from("vm-proptest"),
        backend: BackendId::from("stub"),
        session_id: SessionId::from_string("sess-proptest"),
        agent_id: AgentId::from_string("agent-proptest"),
        status: VmStatus::Running,
        created_at: Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).unwrap(),
        metadata: serde_json::Value::Null,
    }
}

/// Build a [`GateChain`] whose two `BudgetGatePort` stubs return the
/// supplied outcomes. The policy stub is keyed `"policy"`, the budget
/// stub `"budget"` — those ids flow through into
/// [`GateDecision::Deny::gate_id`] and anchor the attribution property.
fn build_chain(policy: GateOutcome, budget: GateOutcome) -> GateChain {
    GateChain::builder()
        .policy(Arc::new(StubBudgetGate {
            id: "policy",
            outcome: policy,
        }))
        .budget(Arc::new(StubBudgetGate {
            id: "budget",
            outcome: budget,
        }))
        .network_isolation(Arc::new(StubNetwork))
        .build()
        .expect("builder with policy+budget+network must succeed")
}

/// Strategy covering the full `GateOutcome` alphabet with equal
/// weights — no skew that could mask a regression in a minority branch.
fn arb_outcome() -> impl Strategy<Value = GateOutcome> {
    prop_oneof![
        Just(GateOutcome::Allow),
        Just(GateOutcome::Deny),
        Just(GateOutcome::Approval),
    ]
}

/// Run a fresh single-threaded Tokio runtime and drive `chain.check_dispatch`
/// to completion. A new runtime per case keeps the property synchronous
/// from proptest's perspective (which is not async-aware) and avoids
/// sharing any cross-case state.
fn run_check_dispatch(chain: &GateChain) -> GateDecision {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("single-thread tokio runtime must build");
    rt.block_on(chain.check_dispatch(&test_vm(), &test_ctx(), &ResourceBudget::default()))
}

// ── Property tests ────────────────────────────────────────────────────

proptest! {
    // 2_500 cases per property gives > 1e4 invocations across the three
    // properties while keeping wall-clock well under a minute on CI.
    // Raise selectively if statistical coverage needs lift; the stubs
    // are pure so the bound is almost entirely runtime-construction
    // overhead.
    #![proptest_config(ProptestConfig { cases: 2_500, .. ProptestConfig::default() })]

    /// Two chains built from the same `(policy, budget)` pair must
    /// produce the same [`GateDecision`]. Ruled out: hidden mutable
    /// state in the chain, non-deterministic ordering, clock-dependent
    /// branches.
    #[test]
    fn determinism(policy in arb_outcome(), budget in arb_outcome()) {
        let chain_a = build_chain(policy, budget);
        let chain_b = build_chain(policy, budget);
        let a = run_check_dispatch(&chain_a);
        let b = run_check_dispatch(&chain_b);
        prop_assert_eq!(a, b);
    }

    /// Monotonicity, expressed in two complementary claims:
    ///
    /// 1. The all-`Allow` baseline must always yield `Allow`. If this
    ///    ever fails, every other monotonicity statement is moot.
    /// 2. Any `(policy, budget)` pair that tightens the baseline must
    ///    match the short-circuit behaviour of
    ///    [`GateChain::check_dispatch`]:
    ///    * policy `Deny` ⇒ `Deny(Policy)` regardless of `budget`;
    ///    * policy `Approval` ⇒ `RequireApproval` regardless of `budget`;
    ///    * policy `Allow`, budget `Deny` ⇒ `Deny(Budget)`;
    ///    * policy `Allow`, budget `Approval` ⇒ `RequireApproval`;
    ///    * both `Allow` ⇒ `Allow`.
    ///
    /// In no case may a tightened input produce a *looser* outcome
    /// (e.g. `Allow` out of a `Deny` stub). The match below enumerates
    /// every reachable tuple so a future `GateOutcome` variant forces
    /// a compile error rather than a silent gap.
    #[test]
    fn monotonicity(policy in arb_outcome(), budget in arb_outcome()) {
        let baseline = build_chain(GateOutcome::Allow, GateOutcome::Allow);
        let baseline_result = run_check_dispatch(&baseline);
        prop_assert!(
            matches!(baseline_result, GateDecision::Allow),
            "all-Allow baseline must produce Allow, got {baseline_result:?}",
        );

        let chain = build_chain(policy, budget);
        let result = run_check_dispatch(&chain);

        match (policy, budget) {
            // Policy short-circuits: budget outcome is irrelevant once
            // policy returns anything non-Allow.
            (GateOutcome::Deny, _) => prop_assert!(
                matches!(
                    result,
                    GateDecision::Deny { gate: GateKind::Policy, .. }
                ),
                "policy=Deny should yield Deny(Policy); got {result:?}",
            ),
            (GateOutcome::Approval, _) => prop_assert!(
                matches!(result, GateDecision::RequireApproval { .. }),
                "policy=Approval should yield RequireApproval; got {result:?}",
            ),
            // Policy allows — decision devolves to the budget stage.
            (GateOutcome::Allow, GateOutcome::Deny) => prop_assert!(
                matches!(
                    result,
                    GateDecision::Deny { gate: GateKind::Budget, .. }
                ),
                "budget=Deny should yield Deny(Budget); got {result:?}",
            ),
            (GateOutcome::Allow, GateOutcome::Approval) => prop_assert!(
                matches!(result, GateDecision::RequireApproval { .. }),
                "budget=Approval should yield RequireApproval; got {result:?}",
            ),
            (GateOutcome::Allow, GateOutcome::Allow) => prop_assert!(
                matches!(result, GateDecision::Allow),
                "Allow+Allow should yield Allow; got {result:?}",
            ),
        }
    }

    /// Any [`GateDecision::Deny`] must name the real denying stub. In
    /// the current two-stage chain that means:
    ///
    /// * `policy == Deny` ⇒ `gate_id == "policy"` and `gate ==
    ///   GateKind::Policy` (policy runs first, short-circuits);
    /// * otherwise `budget == Deny` ⇒ `gate_id == "budget"` and `gate
    ///   == GateKind::Budget`.
    ///
    /// A `Deny` decision arising from any other `(policy, budget)`
    /// pair is a bug in either the chain or the lifting function.
    #[test]
    fn attribution(policy in arb_outcome(), budget in arb_outcome()) {
        let chain = build_chain(policy, budget);
        let result = run_check_dispatch(&chain);

        if let GateDecision::Deny {
            gate,
            gate_id,
            reason,
        } = &result
        {
            let (expected_gate, expected_id) = if matches!(policy, GateOutcome::Deny) {
                (GateKind::Policy, "policy")
            } else if matches!(policy, GateOutcome::Allow)
                && matches!(budget, GateOutcome::Deny)
            {
                (GateKind::Budget, "budget")
            } else {
                return Err(TestCaseError::fail(format!(
                    "Deny surfaced for non-denying inputs: policy={policy:?}, \
                     budget={budget:?}, decision={result:?}",
                )));
            };
            prop_assert_eq!(*gate, expected_gate);
            prop_assert_eq!(gate_id.as_str(), expected_id);
            // The stub's canned `reason` must also surface intact — the
            // chain must not rewrite it on its way through `lift_decision`.
            prop_assert!(
                reason.contains(expected_id),
                "reason {reason:?} should mention denying stub {expected_id:?}",
            );
        }
    }
}
