//! Integration — run the full conformance suite against
//! [`arcan_provider_local::LocalSandboxProvider`].
//!
//! The test wires a minimal `ConformanceHarness` around a
//! [`life_kernel_core::KernelEngine`] composed of the NoOp gates (so
//! the engine short-circuits through the gate chain into the backend)
//! plus an in-memory capturing event store.
//!
//! Because the local backend probes the host at construction
//! ([`LocalSandboxProvider::from_env`] looks for Docker / nsjail), the
//! test degrades gracefully when neither is available: it emits a skip
//! note and returns success. That matches the plan's policy for
//! capability-gated scenarios — we never fail a machine for a
//! dependency it does not have installed.

use std::sync::{Arc, Mutex};

use aios_protocol::budget::{BudgetDecision, BudgetGatePort, ResourceBudget};
use aios_protocol::error::KernelResult as LegacyKernelResult;
use aios_protocol::event::EventRecord;
use aios_protocol::hypervisor::{ForkSpec, VmHandle};
use aios_protocol::ids::{AgentId, BranchId, SeqNo, SessionId};
use aios_protocol::kernel::KernelContext;
use aios_protocol::ports::{EventRecordStream, EventStorePort};
use async_trait::async_trait;
use life_kernel_conformance::{CapturingEventStore, ConformanceHarness, run_all_conformance_tests};
use life_kernel_core::KernelEngine;
use life_kernel_gate::{NoOpBudgetGate, NoOpNetworkIsolation};

// ── In-memory capturing store ────────────────────────────────────────

/// Append-only in-memory event store that assigns monotonic
/// sequence numbers and lets the conformance scenarios read the trail
/// back through [`CapturingEventStore::stored_events`].
#[derive(Default)]
struct InMemoryCapturingStore {
    events: Mutex<Vec<EventRecord>>,
}

impl InMemoryCapturingStore {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl EventStorePort for InMemoryCapturingStore {
    async fn append(&self, mut event: EventRecord) -> LegacyKernelResult<EventRecord> {
        let mut buf = self.events.lock().expect("poisoned event store mutex");
        event.sequence = buf.len() as SeqNo;
        buf.push(event.clone());
        Ok(event)
    }

    async fn read(
        &self,
        _session_id: SessionId,
        _branch_id: BranchId,
        from_sequence: u64,
        limit: usize,
    ) -> LegacyKernelResult<Vec<EventRecord>> {
        let events = self.events.lock().expect("poisoned event store mutex");
        Ok(events
            .iter()
            .filter(|e| e.sequence >= from_sequence)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn head(&self, _session_id: SessionId, _branch_id: BranchId) -> LegacyKernelResult<u64> {
        Ok(self
            .events
            .lock()
            .expect("poisoned event store mutex")
            .len() as u64)
    }

    async fn subscribe(
        &self,
        _session_id: SessionId,
        _branch_id: BranchId,
        _after_sequence: u64,
    ) -> LegacyKernelResult<EventRecordStream> {
        unimplemented!("subscribe not used in conformance tests")
    }
}

impl CapturingEventStore for InMemoryCapturingStore {
    fn stored_events(&self) -> Vec<EventRecord> {
        self.events
            .lock()
            .expect("poisoned event store mutex")
            .clone()
    }
}

// ── Always-deny policy gate ──────────────────────────────────────────

/// Minimal [`BudgetGatePort`] impl that vetoes every dispatch and
/// fork. Used exclusively by
/// [`ConformanceHarness::build_engine_with_deny_policy`].
struct AlwaysDenyPolicy;

#[async_trait]
impl BudgetGatePort for AlwaysDenyPolicy {
    async fn check_dispatch(
        &self,
        _ctx: &KernelContext,
        _cost_hint: &ResourceBudget,
    ) -> BudgetDecision {
        BudgetDecision::Deny {
            reason: "conformance-harness-always-deny".into(),
            gate_id: "conformance-policy-deny".into(),
        }
    }

    async fn check_fork(
        &self,
        _parent: &VmHandle,
        _spec: &ForkSpec,
        _ctx: &KernelContext,
    ) -> BudgetDecision {
        BudgetDecision::Deny {
            reason: "conformance-harness-always-deny".into(),
            gate_id: "conformance-policy-deny".into(),
        }
    }
}

// ── Harness ──────────────────────────────────────────────────────────

/// Harness that drives `arcan-provider-local` through the
/// `life-kernel-core` engine. Gate chain is permissive (NoOp budget,
/// NoOp network) so the engine short-circuits into the backend.
struct LocalHarness;

impl LocalHarness {
    /// Try to construct a local provider; returns `None` when the host
    /// has neither Docker nor nsjail available (dev-only path).
    fn try_local_provider() -> Option<arcan_provider_local::LocalSandboxProvider> {
        arcan_provider_local::LocalSandboxProvider::from_env().ok()
    }

    async fn build(
        &self,
        policy_gate: Arc<dyn BudgetGatePort>,
    ) -> (KernelEngine, Arc<dyn CapturingEventStore>) {
        let provider = Self::try_local_provider()
            .expect("LocalSandboxProvider::from_env returned None — see the test skip path");
        let store = InMemoryCapturingStore::new();
        let store_port: Arc<dyn EventStorePort> = store.clone();

        let engine = KernelEngine::builder()
            .policy_gate(policy_gate)
            .budget_gate(Arc::new(NoOpBudgetGate::new()))
            .network_isolation(Arc::new(NoOpNetworkIsolation::new()))
            .event_store(store_port)
            .session(
                SessionId::from_string("sess-conformance"),
                AgentId::from_string("agent-conformance"),
            )
            .register_backend(Arc::new(provider))
            .await
            .build()
            .await
            .expect("engine builds");

        let capturing: Arc<dyn CapturingEventStore> = store;
        (engine, capturing)
    }
}

#[async_trait]
impl ConformanceHarness for LocalHarness {
    async fn build_engine(&self) -> (KernelEngine, Arc<dyn CapturingEventStore>) {
        self.build(Arc::new(NoOpBudgetGate::new())).await
    }

    async fn build_engine_with_deny_policy(
        &self,
    ) -> Option<(KernelEngine, Arc<dyn CapturingEventStore>)> {
        Some(self.build(Arc::new(AlwaysDenyPolicy)).await)
    }
}

// ── Integration test ─────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn conformance_suite_passes_on_local_backend() {
    if LocalHarness::try_local_provider().is_none() {
        eprintln!(
            "[conformance] conformance_suite_passes_on_local_backend: neither Docker nor nsjail \
             is available on this host; skipping the full local-backend run. The suite still \
             exercises every scenario's compile path via this harness and the unit tests in \
             life-kernel-conformance."
        );
        return;
    }
    run_all_conformance_tests(&LocalHarness)
        .await
        .expect("conformance suite must pass on the local backend");
}
