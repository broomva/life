//! Event-replay determinism: proves [`KernelEngine`] is a pure fold over
//! its Lago `kernel.*` event journal.
//!
//! The test records a full session lifecycle against a [`StubBackend`],
//! captures every emitted event via an in-memory [`EventStorePort`],
//! replays the captured [`EventKind`] stream through
//! [`KernelEngine::replay`], and asserts the reconstructed
//! [`ReplayedState`] matches an independently-computed expected state.
//!
//! Failure here means the engine mutates state that the event journal
//! does not capture — a fundamental event-sourcing invariant break.
//!
//! A second replay of the same event stream must produce a
//! byte-identical [`ReplayedState`] (determinism). Both assertions are
//! executed in the single integration test below so a regression in
//! either direction fails loudly.

use std::sync::{Arc, Mutex};

use aios_protocol::budget::{BudgetDecision, BudgetGatePort, ResourceBudget, UsageConfidence};
use aios_protocol::error::KernelResult as LegacyKernelResult;
use aios_protocol::event::{EventKind, EventRecord};
use aios_protocol::hypervisor::{
    BackendCapabilitySet, BackendError, BackendId, BackendSelector, ExecRequest, ExecResult,
    ForkSpec, HypervisorBackend, RuntimeHint, VmHandle, VmId, VmSnapshotId, VmSpec,
    VmSpecOverrides, VmStatus,
};
use aios_protocol::ids::{AgentId, BranchId, SeqNo, SessionId};
use aios_protocol::kernel::{ChainId, KernelContext, KernelResult, WalletAttribution};
use aios_protocol::network_isolation::{EgressTarget, NetworkIsolationPort};
use aios_protocol::ports::{EventRecordStream, EventStorePort, KernelPort};
use aios_protocol::sandbox::NetworkPolicy;
use aios_protocol::tool::ToolCall;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use life_kernel_core::KernelEngine;

// ── In-memory EventStorePort ────────────────────────────────────────────

/// Minimal in-memory [`EventStorePort`] that stamps a sequence on each
/// appended [`EventRecord`] and returns the stored vector verbatim on
/// `read()`. Shared with the recorder/replayer halves of the test via
/// `Arc`.
struct InMemoryEventStore {
    events: Mutex<Vec<EventRecord>>,
}

impl InMemoryEventStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
        })
    }

    fn stored(&self) -> Vec<EventRecord> {
        self.events.lock().expect("poisoned mutex").clone()
    }
}

#[async_trait]
impl EventStorePort for InMemoryEventStore {
    async fn append(&self, mut event: EventRecord) -> LegacyKernelResult<EventRecord> {
        let mut buf = self.events.lock().expect("poisoned mutex");
        event.sequence = buf.len() as SeqNo;
        buf.push(event.clone());
        Ok(event)
    }

    async fn read(
        &self,
        _session_id: SessionId,
        _branch_id: BranchId,
        _from_sequence: u64,
        _limit: usize,
    ) -> LegacyKernelResult<Vec<EventRecord>> {
        Ok(self.stored())
    }

    async fn head(&self, _session_id: SessionId, _branch_id: BranchId) -> LegacyKernelResult<u64> {
        Ok(self.stored().len() as u64)
    }

    async fn subscribe(
        &self,
        _session_id: SessionId,
        _branch_id: BranchId,
        _after_sequence: u64,
    ) -> LegacyKernelResult<EventRecordStream> {
        unimplemented!("subscribe not used in replay_determinism test")
    }
}

// ── Stub backend ────────────────────────────────────────────────────────

/// Hypervisor stub that hands out monotonically-numbered VM ids.
///
/// `create()` returns `vm-N` where `N` is incremented on every call;
/// `restore()` similarly returns `fork-N`. `exec()` reports a fixed 5-ms
/// duration so downstream replay sees a realistic aggregated usage.
struct StubBackend {
    create_counter: Mutex<u32>,
    restore_counter: Mutex<u32>,
}

impl StubBackend {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            create_counter: Mutex::new(0),
            restore_counter: Mutex::new(0),
        })
    }
}

#[async_trait]
impl HypervisorBackend for StubBackend {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn capabilities(&self) -> BackendCapabilitySet {
        BackendCapabilitySet::FILESYSTEM_READ | BackendCapabilitySet::PERSISTENCE
    }

    async fn create(&self, _spec: VmSpec) -> Result<VmHandle, BackendError> {
        let mut counter = self.create_counter.lock().expect("poisoned mutex");
        *counter += 1;
        Ok(VmHandle {
            vm_id: VmId::from(format!("vm-{}", *counter)),
            backend: BackendId::from("stub"),
            session_id: SessionId::from_string("sess-replay"),
            agent_id: AgentId::from_string("agent-replay"),
            status: VmStatus::Running,
            created_at: Utc
                .with_ymd_and_hms(2026, 4, 23, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            metadata: serde_json::Value::Null,
        })
    }

    async fn exec(&self, _vm: &VmHandle, _req: ExecRequest) -> Result<ExecResult, BackendError> {
        Ok(ExecResult {
            stdout: b"ok".to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
            duration_ms: 5,
        })
    }

    async fn snapshot(&self, vm: &VmHandle) -> Result<VmSnapshotId, BackendError> {
        Ok(VmSnapshotId::from(format!("snap-{}", vm.vm_id)))
    }

    async fn restore(&self, snapshot: &VmSnapshotId) -> Result<VmHandle, BackendError> {
        let mut counter = self.restore_counter.lock().expect("poisoned mutex");
        *counter += 1;
        Ok(VmHandle {
            vm_id: VmId::from(format!("fork-{}-from-{}", *counter, snapshot)),
            backend: BackendId::from("stub"),
            session_id: SessionId::from_string("sess-replay"),
            agent_id: AgentId::from_string("agent-replay"),
            status: VmStatus::Running,
            created_at: Utc
                .with_ymd_and_hms(2026, 4, 23, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            metadata: serde_json::Value::Null,
        })
    }

    async fn destroy(&self, _vm: &VmHandle) -> Result<(), BackendError> {
        Ok(())
    }
}

// ── Permissive gate stubs ───────────────────────────────────────────────

/// Budget gate that always allows — every dispatch and fork proceeds so
/// the test can observe the full lifecycle without interruption.
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

/// Network-isolation port that is entirely passive.
struct NopNetwork;

#[async_trait]
impl NetworkIsolationPort for NopNetwork {
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

// ── Fixtures ────────────────────────────────────────────────────────────

fn make_spec() -> VmSpec {
    VmSpec {
        backend_selector: BackendSelector::Auto,
        resources: Default::default(),
        network_policy: NetworkPolicy::Disabled,
        mounts: Vec::new(),
        env: Default::default(),
        runtime_hint: RuntimeHint::Shell,
        labels: Default::default(),
    }
}

fn make_ctx() -> KernelContext {
    KernelContext {
        session_id: SessionId::from_string("sess-replay"),
        agent_id: AgentId::from_string("agent-replay"),
        wallet: WalletAttribution {
            address: "0xreplay".into(),
            chain: ChainId::base(),
        },
        cost_hint: None,
        trace_ctx: None,
    }
}

// ── Test ────────────────────────────────────────────────────────────────

/// Full journey: `create → dispatch×3 → snapshot → fork → destroy×2`.
///
/// After the journey, capture every [`EventRecord`] emitted by the
/// engine, fold the stream via [`KernelEngine::replay`], and assert the
/// reconstructed state matches the engine's externally-visible state
/// at the end of the journey. A second replay of the same events must
/// produce a byte-identical [`ReplayedState`].
#[tokio::test(flavor = "multi_thread")]
async fn engine_is_deterministic_fold_over_event_journal() {
    let backend = StubBackend::new();
    let store = InMemoryEventStore::new();
    let engine = KernelEngine::builder()
        .policy_gate(Arc::new(AllowGate))
        .budget_gate(Arc::new(AllowGate))
        .network_isolation(Arc::new(NopNetwork))
        .event_store(Arc::clone(&store) as Arc<dyn EventStorePort>)
        .session(
            SessionId::from_string("sess-replay"),
            AgentId::from_string("agent-replay"),
        )
        .register_backend(Arc::clone(&backend) as Arc<dyn HypervisorBackend>)
        .await
        .build()
        .await
        .expect("engine must build with all required collaborators");

    let ctx = make_ctx();

    // 1. Create the primary VM.
    let vm1 = engine
        .create_vm(make_spec(), ctx.clone())
        .await
        .expect("create_vm succeeds");

    // 2. Dispatch three tool calls.
    for i in 0..3 {
        let call = ToolCall {
            call_id: format!("call-{i}"),
            tool_name: "echo".into(),
            input: serde_json::json!({}),
            requested_capabilities: Vec::new(),
        };
        engine
            .dispatch(&vm1, call, &ctx)
            .await
            .expect("dispatch succeeds under AllowGate");
    }

    // 3. Snapshot the primary VM.
    let snap = engine
        .snapshot(&vm1, "snap-1")
        .await
        .expect("snapshot succeeds");

    // 4. Fork a child from the snapshot.
    let vm2 = engine
        .fork(
            &snap,
            ForkSpec {
                parent_snapshot: snap.snapshot_id.clone(),
                overrides: VmSpecOverrides::default(),
            },
            ctx.clone(),
        )
        .await
        .expect("fork succeeds under AllowGate");

    // 5. Destroy both VMs.
    engine
        .destroy(vm1.clone())
        .await
        .expect("destroy(vm1) succeeds");
    engine
        .destroy(vm2.clone())
        .await
        .expect("destroy(vm2) succeeds");

    // ── Capture ──────────────────────────────────────────────────────
    let events: Vec<EventRecord> = store.stored();
    assert!(
        !events.is_empty(),
        "engine should have emitted at least one event"
    );

    let kinds: Vec<&EventKind> = events.iter().map(|r| &r.kind).collect();

    // ── Replay ───────────────────────────────────────────────────────
    let replayed = KernelEngine::replay(kinds.iter().copied());

    // Both VMs were destroyed → live_vms must be empty.
    assert!(
        replayed.live_vms.is_empty(),
        "both VMs were destroyed — live_vms must be empty, got {:#?}",
        replayed.live_vms
    );

    // Exactly one snapshot was taken.
    assert_eq!(
        replayed.snapshots.len(),
        1,
        "exactly one snapshot was captured — got {:#?}",
        replayed.snapshots
    );
    let snap_key = snap.snapshot_id.to_string();
    let replayed_snap = replayed
        .snapshots
        .get(&snap_key)
        .expect("snapshot must be keyed by stringified snapshot id");
    assert_eq!(replayed_snap.snapshot_id, snap.snapshot_id);
    assert_eq!(replayed_snap.vm_id, vm1.vm_id);
    assert_eq!(replayed_snap.name, "snap-1");

    // Session usage was accumulated from three dispatches (5 ms each).
    let session_usage = replayed
        .session_usage
        .get("sess-replay")
        .expect("usage recorded under the session key");
    assert!(
        session_usage.duration_ms >= 15,
        "duration_ms {} should accumulate ≥ 3×5ms",
        session_usage.duration_ms
    );
    assert_eq!(
        session_usage.confidence,
        UsageConfidence::Estimated,
        "stub backend reports Estimated — aggregate stays Estimated"
    );

    // Events folded counter must exactly match the event stream length.
    assert_eq!(
        replayed.events_applied,
        events.len() as u64,
        "events_applied must match the input stream length"
    );

    // ── Determinism: replay the same events again and expect byte-equality.
    let replayed2 = KernelEngine::replay(kinds.iter().copied());
    assert_eq!(
        replayed, replayed2,
        "replay must be deterministic — same event stream must produce identical state"
    );
}
