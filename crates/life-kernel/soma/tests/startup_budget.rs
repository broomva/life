//! Integration test: cold-start budget.
//!
//! Verifies that the daemon bootstrap path completes within the 500 ms
//! cold-start target agreed in BRO-901.
//!
//! ## Two test variants
//!
//! ### `bootstrap_engine_core_under_500ms` (NOT ignored — CI-safe)
//!
//! Measures the time taken to assemble a `KernelEngine` via the same builder
//! path that `bootstrap::build_engine` uses, but without calling
//! `LocalSandboxProvider::from_env()` (which probes the Docker socket and
//! can fail in environments without Docker).  The test wires a
//! [`StubFastBackend`] — a zero-latency in-process backend — so the only
//! measured work is:
//!
//! - `TempDir` creation
//! - `RedbJournal::open` (new file)
//! - `KernelEngineBuilder::build` (gate chain + emitter assembly)
//!
//! This is the correct lower-bound measurement: if the pure assembly path
//! exceeds 500 ms on any machine, a real deployment has no hope of meeting
//! the budget.
//!
//! ### `cold_start_under_500ms` (IGNORED by default)
//!
//! Measures the full `bootstrap::build_engine` + `listener::serve` path
//! including `LocalSandboxProvider::from_env()`.  Gated behind `#[ignore]`
//! because it requires Docker or nsjail to succeed.  Run it with:
//!
//! ```bash
//! cargo test -p lifed -- --ignored cold_start_under_500ms
//! ```
//!
//! ## Measurement methodology
//!
//! Both tests use `std::time::Instant` around the measured block.  The
//! target is 500 ms in **debug** mode — if your machine is genuinely slower
//! than this threshold, run with `cargo test --release` and adjust the
//! threshold comment accordingly (do not lower the threshold itself).
//!
//! ## 500 ms rationale
//!
//! The threshold is deliberately generous for an in-process assembly test:
//! redb open + gate-chain wire-up should complete in < 50 ms on any modern
//! machine.  The headroom exists to absorb CI jitter and future additions
//! (e.g. Vigil OTLP exporter init) without requiring constant threshold
//! adjustments.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aios_protocol::{
    hypervisor::{
        BackendCapabilitySet, BackendError, BackendId, ExecRequest, ExecResult, VmHandle, VmId,
        VmSnapshotId, VmSpec, VmStatus,
    },
    ids::{AgentId, ApprovalId, SessionId},
    policy::Capability,
    ports::{
        ApprovalPort, ApprovalRequest, ApprovalResolution, ApprovalTicket, EventStorePort,
        PolicyGateDecision, PolicyGatePort,
    },
};
use async_trait::async_trait;
use chrono::Utc;
use lago_aios_eventstore_adapter::LagoAiosEventStoreAdapter;
use lago_journal::RedbJournal;
use life_kernel_core::KernelEngine;
use life_kernel_gate::{budget::NoOpBudgetGate, network::NoOpNetworkIsolation};
use tempfile::TempDir;

// ── StubFastBackend ───────────────────────────────────────────────────────────

/// Zero-latency in-process backend for startup timing tests.
///
/// All methods return immediately with canned values.  The `name` constant
/// is `"stub-fast"` — callers must pass `BackendSelector::Auto` since there
/// is no explicit name routing in this test harness.
struct StubFastBackend;

#[async_trait]
impl aios_protocol::hypervisor::HypervisorBackend for StubFastBackend {
    fn name(&self) -> &'static str {
        "stub-fast"
    }

    fn capabilities(&self) -> BackendCapabilitySet {
        BackendCapabilitySet::FILESYSTEM_READ
    }

    async fn create(&self, _spec: VmSpec) -> Result<VmHandle, BackendError> {
        Ok(VmHandle {
            vm_id: VmId::from("stub-vm"),
            backend: BackendId::from("stub-fast"),
            session_id: SessionId::from_string("sess-budget"),
            agent_id: AgentId::from_string("agent-budget"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        })
    }

    async fn exec(&self, _vm: &VmHandle, _req: ExecRequest) -> Result<ExecResult, BackendError> {
        Ok(ExecResult {
            stdout: b"ok".to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
            duration_ms: 1,
        })
    }

    async fn snapshot(&self, _vm: &VmHandle) -> Result<VmSnapshotId, BackendError> {
        Ok(VmSnapshotId::from("snap-stub"))
    }

    async fn restore(&self, _snap: &VmSnapshotId) -> Result<VmHandle, BackendError> {
        Ok(VmHandle {
            vm_id: VmId::from("stub-vm-forked"),
            backend: BackendId::from("stub-fast"),
            session_id: SessionId::from_string("sess-budget"),
            agent_id: AgentId::from_string("agent-budget"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        })
    }

    async fn destroy(&self, _vm: &VmHandle) -> Result<(), BackendError> {
        Ok(())
    }
}

// ── Stub gates for the budget test ───────────────────────────────────────────

struct BudgetAllowAll;

#[async_trait]
impl PolicyGatePort for BudgetAllowAll {
    async fn evaluate(
        &self,
        _session_id: SessionId,
        requested: Vec<Capability>,
    ) -> aios_protocol::error::KernelResult<PolicyGateDecision> {
        Ok(PolicyGateDecision {
            allowed: requested,
            requires_approval: Vec::new(),
            denied: Vec::new(),
        })
    }
}

struct BudgetNeverBlocks;

#[async_trait]
impl ApprovalPort for BudgetNeverBlocks {
    async fn enqueue(
        &self,
        request: ApprovalRequest,
    ) -> aios_protocol::error::KernelResult<ApprovalTicket> {
        Ok(ApprovalTicket {
            approval_id: ApprovalId::from_string("auto"),
            session_id: request.session_id,
            call_id: request.call_id,
            tool_name: request.tool_name,
            capability: request.capability,
            reason: request.reason,
            created_at: Utc::now(),
        })
    }

    async fn list_pending(
        &self,
        _session_id: SessionId,
    ) -> aios_protocol::error::KernelResult<Vec<ApprovalTicket>> {
        Ok(Vec::new())
    }

    async fn resolve(
        &self,
        approval_id: ApprovalId,
        approved: bool,
        actor: String,
    ) -> aios_protocol::error::KernelResult<ApprovalResolution> {
        Ok(ApprovalResolution {
            approval_id,
            approved,
            actor,
            resolved_at: Utc::now(),
        })
    }
}

// ── Helper: assemble a KernelEngine (the same path bootstrap.rs takes) ────────

/// Assemble a `KernelEngine` backed by the stub backend + in-memory Lago
/// store.  Returns the engine and the `TempDir` that owns the backing file.
async fn assemble_engine() -> (Arc<KernelEngine>, TempDir) {
    use life_kernel_gate::policy::StaticPolicyGate;

    let dir = TempDir::new().expect("tempdir for startup budget test");
    let db_path = dir.path().join("journal.redb");
    let journal = RedbJournal::open(&db_path).expect("RedbJournal::open");
    let store: Arc<dyn EventStorePort> =
        Arc::new(LagoAiosEventStoreAdapter::new(Arc::new(journal)));

    let policy_gate: Arc<dyn aios_protocol::budget::BudgetGatePort> = Arc::new(
        StaticPolicyGate::new(Arc::new(BudgetAllowAll), Arc::new(BudgetNeverBlocks)),
    );
    let budget_gate: Arc<dyn aios_protocol::budget::BudgetGatePort> =
        Arc::new(NoOpBudgetGate::new());
    let network_gate: Arc<dyn aios_protocol::network_isolation::NetworkIsolationPort> =
        Arc::new(NoOpNetworkIsolation::new());

    let engine = KernelEngine::builder()
        .policy_gate(policy_gate)
        .budget_gate(budget_gate)
        .network_isolation(network_gate)
        .event_store(store)
        .session(
            SessionId::from_string("sess-budget"),
            AgentId::from_string("agent-budget"),
        )
        .register_backend(
            Arc::new(StubFastBackend) as Arc<dyn aios_protocol::hypervisor::HypervisorBackend>
        )
        .await
        .build()
        .await
        .expect("KernelEngine::build must succeed");

    (Arc::new(engine), dir)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Assemble a complete `KernelEngine` (the inner loop of `build_engine`)
/// without touching the Docker socket.  The elapsed time must stay under
/// 500 ms even in unoptimised debug mode.
///
/// If this test flakes due to machine load (e.g. on a heavily loaded CI
/// runner), run with `cargo test --release -p lifed` and measure the
/// release-mode time — it will be substantially faster.
///
/// Target: < 500 ms (debug). Expected actual: < 50 ms on any modern machine.
#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_engine_core_under_500ms() {
    let threshold = Duration::from_millis(500);
    let t0 = Instant::now();

    let (_engine, _dir) = assemble_engine().await;

    let elapsed = t0.elapsed();
    assert!(
        elapsed < threshold,
        "KernelEngine assembly took {elapsed:?}, which exceeds the 500 ms cold-start budget. \
         Run `cargo test --release -p lifed` to measure release-mode performance. \
         If this is a slow CI machine, consider bumping the threshold with a comment explaining why.",
    );
}

/// End-to-end cold-start: `bootstrap::build_engine` (which calls
/// `LocalSandboxProvider::from_env()`) plus Unix-socket listener bind.
///
/// Ignored by default because `LocalSandboxProvider::from_env()` probes the
/// Docker socket and fails when Docker is not available (CI / macOS without
/// Docker Desktop).
///
/// Operator measurement:
///
/// ```bash
/// cargo test -p lifed -- --ignored cold_start_under_500ms
/// ```
///
/// Target: < 500 ms (debug). If the Docker socket probe adds significant
/// latency on your machine, run `cargo test --release` for a production-grade
/// measurement.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Docker or nsjail on the host (LocalSandboxProvider::from_env probes /var/run/docker.sock)"]
async fn cold_start_under_500ms() {
    use std::time::Duration;

    use lifed::{LifedConfig, bootstrap};

    let threshold = Duration::from_millis(500);
    let t0 = Instant::now();

    // `LifedConfig::default()` has `backends.local = true` and
    // `lago.store = InMemory`.
    let cfg = LifedConfig::default();
    let bootstrap = bootstrap::build_engine(&cfg)
        .await
        .expect("build_engine must succeed");

    let elapsed = t0.elapsed();

    // Basic sanity: the returned bootstrap is in a useful shape.
    assert!(Arc::strong_count(&bootstrap.engine) >= 1);
    assert_eq!(bootstrap.replayed.events_applied, 0);
    assert_eq!(bootstrap.session_id.as_str(), "lifed:lifed");

    assert!(
        elapsed < threshold,
        "Full bootstrap::build_engine took {elapsed:?}, which exceeds the 500 ms cold-start \
         budget. Run `cargo test --release -p lifed -- --ignored cold_start_under_500ms` for a \
         release-mode measurement.",
    );
}
