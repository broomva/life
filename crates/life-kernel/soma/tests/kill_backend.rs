//! Integration test: kill -9 backend resilience.
//!
//! Proves that when a [`HypervisorBackend::exec`] fails with a
//! "backend disappeared" error mid-dispatch, the daemon:
//!
//! (a) Returns the error to the client as `Status::internal`.
//! (b) Does **not** silently remove the VM from the `live_vms` index —
//!     lifecycle under client control, not the server.
//! (c) After the client calls `destroy`, the VM is removed from the index
//!     and a `KernelVmDestroyed` event is emitted to the journal.
//!
//! ## Why no tonic transport?
//!
//! Driving the test through a real Unix socket + tonic client adds transport
//! latency and setup friction without adding meaningful signal for the
//! semantics under test.  Instead we drive the service layer directly:
//!
//! - A [`KillableBackend`] hand-rolled stub implements `HypervisorBackend`
//!   and uses an `AtomicBool alive` flag to toggle from "running" to
//!   "disappeared" mid-test.
//! - We build a `KernelEngine` via `KernelEngineBuilder` (same path as
//!   `bootstrap.rs`) over a real in-memory Lago journal backed by `TempDir`.
//! - We wrap the engine in `LifeKernelService` and call `create_vm` /
//!   `dispatch` / `destroy` directly (as tonic `Request` values — the same
//!   path the transport would take).
//!
//! ## Phase limitation: auto-eviction on backend disappearance
//!
//! The engine does **not** auto-remove the VM from `live_vms` when a dispatch
//! fails with a `KernelError::Backend` error.  This is intentional and
//! correct: the engine cannot distinguish "backend crashed" from "transient
//! I/O hiccup".  Lifecycle remains under client control until Phase 4
//! introduces a health-probe path (BRO-903 or later).
//!
//! The test documents and asserts this semantics explicitly — it is NOT a bug.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aios_protocol::{
    hypervisor::{
        BackendCapabilitySet, BackendError, BackendId, ExecRequest, ExecResult, VmHandle, VmId,
        VmSnapshotId, VmSpec, VmStatus,
    },
    ids::{AgentId, SessionId},
};
use async_trait::async_trait;
use chrono::Utc;
use lago_aios_eventstore_adapter::LagoAiosEventStoreAdapter;
use lago_journal::RedbJournal;
use life_kernel_core::KernelEngine;
use life_kernel_gate::{budget::NoOpBudgetGate, network::NoOpNetworkIsolation};
use life_kernel_proto::pb::{self, kernel_service_server::KernelService as _};
use soma::server::LifeKernelService;
use tempfile::TempDir;
use tokio_stream::StreamExt as _;
use tonic::Request;

// ── KillableBackend ───────────────────────────────────────────────────────────

/// A `HypervisorBackend` whose `exec` (and therefore `dispatch`) fails with
/// `BackendError::Internal("backend disappeared")` when `alive` is set to
/// `false`.  All other methods succeed unconditionally.
///
/// `create` always returns a fixed handle regardless of the `alive` flag —
/// the VM can be created while the backend is up; the backend then "dies"
/// before the first dispatch.
struct KillableBackend {
    alive: Arc<AtomicBool>,
}

impl KillableBackend {
    fn new() -> (Arc<Self>, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(true));
        let backend = Arc::new(Self {
            alive: Arc::clone(&flag),
        });
        (backend, flag)
    }

    fn canned_handle() -> VmHandle {
        VmHandle {
            vm_id: VmId::from("vm-kill-test"),
            backend: BackendId::from("killable"),
            session_id: SessionId::from_string("sess-kill"),
            agent_id: AgentId::from_string("agent-kill"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }
}

#[async_trait]
impl aios_protocol::hypervisor::HypervisorBackend for KillableBackend {
    fn name(&self) -> &'static str {
        "killable"
    }

    fn capabilities(&self) -> BackendCapabilitySet {
        BackendCapabilitySet::FILESYSTEM_READ
    }

    async fn create(&self, _spec: VmSpec) -> Result<VmHandle, BackendError> {
        // `create` always succeeds — backend "dies" after the VM is up.
        Ok(Self::canned_handle())
    }

    async fn exec(&self, _vm: &VmHandle, _req: ExecRequest) -> Result<ExecResult, BackendError> {
        if self.alive.load(Ordering::SeqCst) {
            Ok(ExecResult {
                stdout: b"ok".to_vec(),
                stderr: Vec::new(),
                exit_code: 0,
                duration_ms: 1,
            })
        } else {
            Err(BackendError::Internal("backend disappeared".into()))
        }
    }

    async fn snapshot(&self, _vm: &VmHandle) -> Result<VmSnapshotId, BackendError> {
        Ok(VmSnapshotId::from("snap-kill"))
    }

    async fn restore(&self, _snapshot: &VmSnapshotId) -> Result<VmHandle, BackendError> {
        Ok(Self::canned_handle())
    }

    async fn destroy(&self, _vm: &VmHandle) -> Result<(), BackendError> {
        // Destroy "succeeds" even if the backend is dead —
        // the VM process is already gone.
        Ok(())
    }
}

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Build a `LifeKernelService` backed by a `KillableBackend` and a real
/// in-memory Lago journal.  Returns the service + the kill-flag + the
/// `TempDir` keeping the in-memory redb file alive.
async fn build_killable_service() -> (
    LifeKernelService<KernelEngine>,
    Arc<AtomicBool>, // kill flag
    TempDir,         // keeps the Lago tempdir alive
    Arc<dyn aios_protocol::ports::EventStorePort>,
) {
    use aios_protocol::error::KernelResult;
    use aios_protocol::ids::ApprovalId;
    use aios_protocol::policy::Capability;
    use aios_protocol::ports::{
        ApprovalPort, ApprovalRequest, ApprovalResolution, ApprovalTicket, PolicyGateDecision,
        PolicyGatePort,
    };

    struct AllowAll;

    #[async_trait]
    impl PolicyGatePort for AllowAll {
        async fn evaluate(
            &self,
            _session_id: SessionId,
            requested: Vec<Capability>,
        ) -> KernelResult<PolicyGateDecision> {
            Ok(PolicyGateDecision {
                allowed: requested,
                requires_approval: Vec::new(),
                denied: Vec::new(),
            })
        }
    }

    struct NeverBlocks;

    #[async_trait]
    impl ApprovalPort for NeverBlocks {
        async fn enqueue(&self, request: ApprovalRequest) -> KernelResult<ApprovalTicket> {
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
                resolved_at: Utc::now(),
            })
        }
    }

    use life_kernel_gate::policy::StaticPolicyGate;

    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("journal.redb");
    let journal = RedbJournal::open(&db_path).expect("open journal");
    let store: Arc<dyn aios_protocol::ports::EventStorePort> =
        Arc::new(LagoAiosEventStoreAdapter::new(Arc::new(journal)));

    let policy_gate: Arc<dyn aios_protocol::budget::BudgetGatePort> = Arc::new(
        StaticPolicyGate::new(Arc::new(AllowAll), Arc::new(NeverBlocks)),
    );
    let budget_gate: Arc<dyn aios_protocol::budget::BudgetGatePort> =
        Arc::new(NoOpBudgetGate::new());
    let network_gate: Arc<dyn aios_protocol::network_isolation::NetworkIsolationPort> =
        Arc::new(NoOpNetworkIsolation::new());

    let (backend, kill_flag) = KillableBackend::new();

    let engine = KernelEngine::builder()
        .policy_gate(policy_gate)
        .budget_gate(budget_gate)
        .network_isolation(network_gate)
        .event_store(Arc::clone(&store))
        .session(
            SessionId::from_string("sess-kill"),
            AgentId::from_string("agent-kill"),
        )
        .register_backend(backend as Arc<dyn aios_protocol::hypervisor::HypervisorBackend>)
        .await
        .build()
        .await
        .expect("engine build must succeed");

    let svc = LifeKernelService::new(Arc::new(engine));
    (svc, kill_flag, dir, store)
}

/// Build a `pb::KernelContext` for the kill-backend test session.
fn kill_ctx_pb() -> pb::KernelContext {
    pb::KernelContext {
        session_id: Some(pb::SessionId {
            value: "sess-kill".into(),
        }),
        agent_id: Some(pb::AgentId {
            value: "agent-kill".into(),
        }),
        wallet: Some(pb::WalletAttribution {
            address: "0xdead".into(),
            chain_caip2: "eip155:8453".into(),
        }),
        cost_hint: None,
        trace_ctx: None,
    }
}

/// Build a `pb::VmSpec` targeting the `killable` backend by name.
fn killable_vm_spec_pb() -> pb::VmSpec {
    use aios_protocol::sandbox::NetworkPolicy;
    use prost::bytes::Bytes;

    let policy_json = serde_json::to_vec(&NetworkPolicy::Disabled).expect("policy serialise");
    pb::VmSpec {
        backend_selector: Some(pb::BackendSelector {
            kind: Some(pb::backend_selector::Kind::Auto(pb::Empty {})),
        }),
        resources: Some(pb::VmResources {
            vcpus: 1,
            memory_kb: 512 * 1024,
            disk_kb: 1024 * 1024,
            timeout_secs: 60,
        }),
        network_policy_json: Bytes::from(policy_json).to_vec(),
        mounts: vec![],
        env: Default::default(),
        runtime_hint: Some(pb::RuntimeHint {
            kind: pb::RuntimeHintKind::RuntimeHintShell as i32,
            version_or_image: String::new(),
        }),
        labels: Default::default(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// When the backend disappears mid-dispatch, the service returns
/// `Status::internal` with a message containing "backend disappeared".
///
/// This test is the primary kill-9 resilience gate.
#[tokio::test(flavor = "multi_thread")]
async fn dispatch_returns_error_when_backend_disappears() {
    let (svc, kill_flag, _dir, _store) = build_killable_service().await;

    // Step 1 — create a VM while the backend is alive.
    let create_req = Request::new(pb::CreateVmRequest {
        spec: Some(killable_vm_spec_pb()),
        ctx: Some(kill_ctx_pb()),
    });
    let create_resp = svc
        .create_vm(create_req)
        .await
        .expect("create_vm must succeed while backend is alive");
    let pb_vm = create_resp.into_inner();

    // Step 2 — kill the backend (simulates kill -9 of a Docker container).
    kill_flag.store(false, Ordering::SeqCst);

    // Step 3 — dispatch must fail with Status::internal.
    let dispatch_req = Request::new(pb::DispatchRequest {
        vm: Some(pb_vm),
        call: Some(pb::ToolCall {
            call_id: "call-kill-1".into(),
            tool_name: "shell".into(),
            input_json: b"{\"command\":\"echo hi\"}".to_vec(),
            requested_capabilities: vec![],
        }),
        ctx: Some(kill_ctx_pb()),
    });
    let err = svc
        .dispatch(dispatch_req)
        .await
        .expect_err("dispatch must fail when backend is dead");

    assert_eq!(
        err.code(),
        tonic::Code::Internal,
        "BackendError::Internal must map to Status::internal; got {err:?}"
    );
    assert!(
        err.message().contains("backend disappeared"),
        "error message must contain 'backend disappeared'; got: {}",
        err.message()
    );
}

/// When a dispatch fails due to a backend error, the VM is NOT silently
/// removed from the `live_vms` index.  Lifecycle remains under client
/// control — the caller decides whether to retry or explicitly destroy.
///
/// We verify the VM is still visible via `list_vms` after the failure.
///
/// See the module-level documentation for the rationale behind this design.
#[tokio::test(flavor = "multi_thread")]
async fn live_vms_does_not_silently_drop_vm_on_backend_error() {
    let (svc, kill_flag, _dir, _store) = build_killable_service().await;

    // Create the VM.
    let create_req = Request::new(pb::CreateVmRequest {
        spec: Some(killable_vm_spec_pb()),
        ctx: Some(kill_ctx_pb()),
    });
    svc.create_vm(create_req)
        .await
        .expect("create_vm must succeed");

    // Kill the backend.
    kill_flag.store(false, Ordering::SeqCst);

    // The VM should be in the index — verify via list_vms before the dispatch.
    let list_before = Request::new(pb::ListVmsRequest { session_id: None });
    let vms_before: Vec<_> = svc
        .list_vms(list_before)
        .await
        .expect("list_vms should succeed")
        .into_inner()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        vms_before.len(),
        1,
        "expected 1 VM in index before dispatch; got {:?}",
        vms_before
    );

    // Dispatch fails (backend dead).  We need the handle proto for the request.
    // Re-create it from the canned handle the KillableBackend returns.
    let pb_vm: pb::VmHandle = KillableBackend::canned_handle()
        .try_into()
        .expect("handle to pb");
    let dispatch_req = Request::new(pb::DispatchRequest {
        vm: Some(pb_vm),
        call: Some(pb::ToolCall {
            call_id: "call-kill-2".into(),
            tool_name: "shell".into(),
            input_json: b"{}".to_vec(),
            requested_capabilities: vec![],
        }),
        ctx: Some(kill_ctx_pb()),
    });
    let err = svc
        .dispatch(dispatch_req)
        .await
        .expect_err("dispatch must fail");
    assert_eq!(
        err.code(),
        tonic::Code::Internal,
        "must be Status::internal"
    );

    // The VM must STILL be in the live-VM index after the failed dispatch.
    let list_after = Request::new(pb::ListVmsRequest { session_id: None });
    let vms_after: Vec<_> = svc
        .list_vms(list_after)
        .await
        .expect("list_vms should succeed after failed dispatch")
        .into_inner()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        vms_after.len(),
        1,
        "VM must NOT be silently removed from live_vms on backend error; \
         expected 1, got {:?}",
        vms_after
    );
}

/// After a failed dispatch (backend killed), the client can call `destroy`
/// and the VM is correctly removed from the live-VM index.  A
/// `KernelVmDestroyed` event must appear in the journal.
#[tokio::test(flavor = "multi_thread")]
async fn subsequent_explicit_destroy_after_failure_succeeds_or_emits_event() {
    let (svc, kill_flag, _dir, store) = build_killable_service().await;

    // Create VM while backend is alive.
    let create_req = Request::new(pb::CreateVmRequest {
        spec: Some(killable_vm_spec_pb()),
        ctx: Some(kill_ctx_pb()),
    });
    svc.create_vm(create_req)
        .await
        .expect("create_vm must succeed");

    // Kill the backend.
    kill_flag.store(false, Ordering::SeqCst);

    // Dispatch fails.
    let pb_vm: pb::VmHandle = KillableBackend::canned_handle()
        .try_into()
        .expect("handle to pb");
    let dispatch_req = Request::new(pb::DispatchRequest {
        vm: Some(pb_vm.clone()),
        call: Some(pb::ToolCall {
            call_id: "call-kill-3".into(),
            tool_name: "shell".into(),
            input_json: b"{}".to_vec(),
            requested_capabilities: vec![],
        }),
        ctx: Some(kill_ctx_pb()),
    });
    let err = svc
        .dispatch(dispatch_req)
        .await
        .expect_err("dispatch must fail when backend is dead");
    assert_eq!(err.code(), tonic::Code::Internal);

    // VM is still in the index after the failed dispatch.
    let list_req = Request::new(pb::ListVmsRequest { session_id: None });
    let vms: Vec<_> = svc
        .list_vms(list_req)
        .await
        .expect("list_vms")
        .into_inner()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(vms.len(), 1, "VM must still be in index before destroy");

    // Explicit destroy must succeed (KillableBackend::destroy returns Ok even
    // when the backend is dead — the VM process is already gone).
    let destroy_req = Request::new(pb::DestroyRequest { vm: Some(pb_vm) });
    svc.destroy(destroy_req)
        .await
        .expect("destroy must succeed even after backend died");

    // VM must now be gone from the index.
    let list_after = Request::new(pb::ListVmsRequest { session_id: None });
    let vms_after: Vec<_> = svc
        .list_vms(list_after)
        .await
        .expect("list_vms after destroy")
        .into_inner()
        .collect::<Vec<_>>()
        .await;
    assert!(
        vms_after.is_empty(),
        "VM must be removed from live_vms after explicit destroy; \
         got {:?}",
        vms_after
    );

    // The event journal must contain at minimum a KernelVmCreated and a
    // KernelVmDestroyed event, proving the lifecycle is fully observable
    // from the journal.
    let events = store
        .read(
            aios_protocol::ids::SessionId::from_string("sess-kill"),
            aios_protocol::ids::BranchId::from_string("main"),
            0,
            64,
        )
        .await
        .expect("event store read");

    let has_created = events
        .iter()
        .any(|r| matches!(&r.kind, aios_protocol::event::EventKind::KernelVmCreated(_)));
    let has_destroyed = events.iter().any(|r| {
        matches!(
            &r.kind,
            aios_protocol::event::EventKind::KernelVmDestroyed(_)
        )
    });

    assert!(
        has_created,
        "journal must contain KernelVmCreated; events: {events:#?}"
    );
    assert!(
        has_destroyed,
        "journal must contain KernelVmDestroyed after explicit destroy; events: {events:#?}"
    );
}
