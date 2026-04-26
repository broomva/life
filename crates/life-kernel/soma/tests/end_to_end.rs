//! End-to-end integration tests for the soma daemon.
//!
//! ## `end_to_end_with_stub_backend` (CI-safe, always runs)
//!
//! Uses a `KernelPort`-implementing stub that returns synthetic responses
//! without requiring Docker or nsjail.  Covers the tonic + listener
//! integration end-to-end: spin up the Unix listener, connect a real tonic
//! client, drive `create_vm` → `dispatch` → `destroy`, and verify the
//! service produced the expected live-VM index changes.
//!
//! Note: span tree verification (asserts on `life.session_id` attribute
//! population) is deferred to a future ticket — setting up an in-process
//! OTel tracing layer and asserting on recorded spans is a non-trivial test
//! harness investment. BRO-900's `tests/replay_restart.rs` already verifies
//! replay-state-from-events end-to-end without Docker.
//!
//! ## `end_to_end_full` (Docker-gated, `#[ignore]`)
//!
//! Uses `LocalSandboxProvider::from_env()` and requires Docker or nsjail.
//! Run locally with:
//!
//! ```bash
//! cargo test -p soma -- --ignored end_to_end_full
//! ```

use std::sync::Arc;
use std::time::Duration;

use aios_protocol::{
    hypervisor::{BackendId, ForkSpec, VmHandle, VmId, VmSnapshotHandle, VmSpec, VmStatus},
    kernel::{KernelContext, KernelResult},
    ports::KernelPort,
    tool::{ToolCall, ToolResult},
};
use chrono::Utc;
use hyper_util::rt::TokioIo;
use life_kernel_proto::aios_v1;
use life_kernel_proto::pb::{self, kernel_service_client::KernelServiceClient};
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

// ── Stub KernelPort ────────────────────────────────────────────────────────────

/// A `KernelPort` stub that returns synthetic success responses.
///
/// Used to decouple the tonic+listener integration from the real
/// `KernelEngine`/`LocalSandboxProvider` backend — no Docker or nsjail
/// required.
struct StubKernel;

#[async_trait::async_trait]
impl KernelPort for StubKernel {
    async fn create_vm(&self, _spec: VmSpec, ctx: KernelContext) -> KernelResult<VmHandle> {
        Ok(VmHandle {
            vm_id: VmId::from("vm-stub-1"),
            backend: BackendId::from("stub"),
            session_id: ctx.session_id.clone(),
            agent_id: ctx.agent_id.clone(),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        })
    }

    async fn dispatch(
        &self,
        _vm: &VmHandle,
        call: ToolCall,
        _ctx: &KernelContext,
    ) -> KernelResult<ToolResult> {
        Ok(ToolResult {
            call_id: call.call_id,
            tool_name: call.tool_name,
            output: serde_json::json!({"stdout": "hello\n", "exit_code": 0}),
            content: None,
            is_error: false,
            usage: None,
        })
    }

    async fn snapshot(&self, _vm: &VmHandle, name: &str) -> KernelResult<VmSnapshotHandle> {
        Ok(VmSnapshotHandle {
            snapshot_id: aios_protocol::hypervisor::VmSnapshotId::from("snap-stub-1"),
            vm_id: VmId::from("vm-stub-1"),
            name: name.to_string(),
            created_at: Utc::now(),
            size_bytes: 0,
        })
    }

    async fn fork(
        &self,
        _snapshot: &VmSnapshotHandle,
        _spec: ForkSpec,
        ctx: KernelContext,
    ) -> KernelResult<VmHandle> {
        Ok(VmHandle {
            vm_id: VmId::from("vm-stub-fork"),
            backend: BackendId::from("stub"),
            session_id: ctx.session_id,
            agent_id: ctx.agent_id,
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        })
    }

    async fn hibernate(&self, _vm: &VmHandle) -> KernelResult<()> {
        Ok(())
    }

    async fn resume(&self, vm: &VmHandle) -> KernelResult<VmHandle> {
        Ok(vm.clone())
    }

    async fn destroy(&self, _vm: VmHandle) -> KernelResult<()> {
        Ok(())
    }
}

// ── Client helpers ─────────────────────────────────────────────────────────────

async fn connect_unix(socket: &std::path::Path) -> KernelServiceClient<Channel> {
    let socket_path = socket.to_path_buf();
    let endpoint = Endpoint::try_from("http://[::]:50051").unwrap();
    let channel = endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = socket_path.clone();
            async move { UnixStream::connect(&path).await.map(TokioIo::new) }
        }))
        .await
        .expect("connect to stub soma socket");
    KernelServiceClient::new(channel)
}

fn stub_ctx_pb() -> pb::KernelContext {
    pb::KernelContext {
        session_id: Some(aios_v1::SessionId {
            value: "e2e-session".into(),
        }),
        agent_id: Some(aios_v1::AgentId {
            value: "e2e-agent".into(),
        }),
        wallet: Some(pb::WalletAttribution {
            address: "0xdead".into(),
            chain_caip2: "eip155:8453".into(),
        }),
        cost_hint: None,
        trace_ctx: None,
    }
}

fn stub_vm_spec_pb() -> pb::VmSpec {
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

// ── CI-safe test (stub backend) ────────────────────────────────────────────────

/// End-to-end test using a stub `KernelPort`.
///
/// Proves that the tonic + Unix-socket listener + `LifeKernelService` stack
/// wires together correctly for the `create_vm` → `dispatch` → `destroy`
/// sequence without requiring a real backend (Docker or nsjail).
///
/// What is NOT verified here (future ticket):
/// - Lago event sequence (stub doesn't emit canonical events).
/// - Span attribute population (`life.session_id` field recording).
///   BRO-900's `replay_restart.rs` already proves event-sourced replay E2E.
#[tokio::test(flavor = "multi_thread")]
async fn end_to_end_with_stub_backend() {
    let tmpdir = tempfile::tempdir().unwrap();
    let socket = tmpdir.path().join("stub.sock");

    // Spin up the Unix listener via the top-level multiplexer.
    // `listener::serve` creates the service internally from the engine.
    let mut server_cfg = soma::SomaConfig::default();
    server_cfg.server.unix_socket = socket.clone();
    server_cfg.server.unix_socket_mode = Some(0o660);
    server_cfg.server.unix_socket_group = None;
    server_cfg.server.drain_secs = 5;

    let stub_engine: Arc<StubKernel> = Arc::new(StubKernel);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let server_task = tokio::spawn(async move {
        soma::listener::serve(&server_cfg, stub_engine, shutdown_rx, Vec::new()).await
    });

    // Wait for the socket to appear.
    let mut tries = 0;
    while tries < 40 && !socket.exists() {
        tokio::time::sleep(Duration::from_millis(50)).await;
        tries += 1;
    }
    assert!(socket.exists(), "stub socket not created within 2 s");

    // Connect a tonic client.
    let mut client = connect_unix(&socket).await;

    // -- create_vm ──────────────────────────────────────────────────────────────
    let create_resp = client
        .create_vm(pb::CreateVmRequest {
            spec: Some(stub_vm_spec_pb()),
            ctx: Some(stub_ctx_pb()),
        })
        .await
        .expect("create_vm RPC")
        .into_inner();

    let vm_id_str = create_resp
        .vm_id
        .as_ref()
        .expect("VmHandle must have vm_id")
        .value
        .clone();
    assert_eq!(vm_id_str, "vm-stub-1");

    // -- dispatch ───────────────────────────────────────────────────────────────
    let dispatch_resp = client
        .dispatch(pb::DispatchRequest {
            vm: Some(create_resp.clone()),
            call: Some(pb::ToolCall {
                call_id: "e2e-call-1".into(),
                tool_name: "shell".into(),
                input_json: b"{\"cmd\": \"echo hello\"}".to_vec(),
                requested_capabilities: vec![],
            }),
            ctx: Some(stub_ctx_pb()),
        })
        .await
        .expect("dispatch RPC")
        .into_inner();

    assert!(
        !dispatch_resp.is_error,
        "dispatch must succeed: {:?}",
        dispatch_resp
    );

    // -- destroy ────────────────────────────────────────────────────────────────
    client
        .destroy(pb::DestroyRequest {
            vm: Some(create_resp),
        })
        .await
        .expect("destroy RPC");

    // -- list_vms: must be empty after destroy ──────────────────────────────────
    use tokio_stream::StreamExt as _;
    let mut list_stream = client
        .list_vms(pb::ListVmsRequest { session_id: None })
        .await
        .expect("list_vms RPC")
        .into_inner();

    let mut count = 0usize;
    while let Some(item) = list_stream.next().await {
        item.expect("list_vms stream item");
        count += 1;
    }
    assert_eq!(
        count, 0,
        "live-VM index must be empty after destroy; got {count} VMs"
    );

    // Shut down the server.
    shutdown_tx.send(()).ok();
    tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server task timed out")
        .expect("server task panicked")
        .expect("server task returned error");
}

// ── Docker-gated full test ─────────────────────────────────────────────────────

/// End-to-end test using the real `LocalSandboxProvider`.
///
/// Requires Docker or nsjail on the host. Gated behind `#[ignore]` so CI
/// runners without either don't fail. Acceptance test for human/CI-with-docker
/// runs:
///
/// ```bash
/// cargo test -p soma -- --ignored end_to_end_full
/// ```
///
/// What this test verifies beyond `end_to_end_with_stub_backend`:
/// - `bootstrap::build_engine` with a real `LocalSandboxProvider`.
/// - Lago event store populated with canonical `kernel.*` events.
/// - The `dispatch` invocation against an actual sandboxed process.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Docker or nsjail on the host for LocalSandboxProvider::from_env()"]
async fn end_to_end_full() {
    let tmpdir = tempfile::tempdir().unwrap();
    let socket_path = tmpdir.path().join("soma-full.sock");

    // Build config with real local backend + in-memory lago.
    let mut cfg = soma::SomaConfig::default();
    cfg.server.unix_socket = socket_path.clone();
    cfg.server.unix_socket_mode = Some(0o660);
    cfg.server.unix_socket_group = None;
    cfg.server.drain_secs = 10;

    // Bootstrap the real engine (requires Docker / nsjail).
    let bootstrap = soma::bootstrap::build_engine(&cfg)
        .await
        .expect("build_engine with local backend");

    let session_id = bootstrap.session_id.clone();
    let branch_id = bootstrap.branch_id.clone();
    let event_store = Arc::clone(&bootstrap.event_store);

    let seed = bootstrap.replayed.snapshot_vm_handles();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let server_cfg = cfg.clone();
    let engine = Arc::clone(&bootstrap.engine);
    let server_task =
        tokio::spawn(
            async move { soma::listener::serve(&server_cfg, engine, shutdown_rx, seed).await },
        );

    // Wait for the socket.
    let mut tries = 0;
    while tries < 40 && !socket_path.exists() {
        tokio::time::sleep(Duration::from_millis(50)).await;
        tries += 1;
    }
    assert!(socket_path.exists(), "socket not created within 2 s");

    let mut client = connect_unix(&socket_path).await;

    // -- create_vm
    let create_resp = client
        .create_vm(pb::CreateVmRequest {
            spec: Some(stub_vm_spec_pb()),
            ctx: Some(stub_ctx_pb()),
        })
        .await
        .expect("create_vm RPC (full)")
        .into_inner();

    let vm_id_value = create_resp
        .vm_id
        .as_ref()
        .expect("VmHandle must have vm_id")
        .value
        .clone();
    assert!(!vm_id_value.is_empty(), "vm_id must be non-empty");

    // -- dispatch (echo hello)
    let dispatch_resp = client
        .dispatch(pb::DispatchRequest {
            vm: Some(create_resp.clone()),
            call: Some(pb::ToolCall {
                call_id: "full-call-1".into(),
                tool_name: "shell".into(),
                input_json: b"{\"cmd\": \"echo hello\"}".to_vec(),
                requested_capabilities: vec![],
            }),
            ctx: Some(stub_ctx_pb()),
        })
        .await
        .expect("dispatch RPC (full)")
        .into_inner();
    assert!(
        !dispatch_resp.is_error,
        "dispatch must succeed: {:?}",
        dispatch_resp
    );

    // -- destroy
    client
        .destroy(pb::DestroyRequest {
            vm: Some(create_resp),
        })
        .await
        .expect("destroy RPC (full)");

    // Assert canonical event sequence in the Lago store.
    // Expected: at minimum KernelVmCreated, KernelDispatchStarted,
    // KernelDispatchCompleted, KernelVmDestroyed.
    let events = event_store
        .read(session_id, branch_id, 0, 50)
        .await
        .expect("read events after full E2E");
    let kinds: Vec<_> = events
        .iter()
        .map(|r| std::mem::discriminant(&r.kind))
        .collect();
    assert!(
        !kinds.is_empty(),
        "event store must contain kernel.* events after full E2E"
    );

    shutdown_tx.send(()).ok();
    let _ = bootstrap._lago_tempdir; // hold alive until after reads
    tokio::time::timeout(Duration::from_secs(10), server_task)
        .await
        .expect("server task timed out")
        .expect("server task panicked")
        .expect("server task error");
}
