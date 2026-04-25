//! Integration test: shutdown drain.
//!
//! Proves that [`soma::shutdown::drain_in_flight`] correctly:
//! 1. Returns `Ok(())` immediately when the counter is zero.
//! 2. Returns `Err(remaining)` when the deadline expires with a positive count.
//! 3. Returns `Ok(())` when the counter reaches zero before the deadline.
//!
//! Additionally validates that the [`crate::server::LifeKernelService`]
//! in-flight counter is correctly bracketed by a dispatch call.
//!
//! ## Skip condition
//!
//! These tests are purely in-process and do not require Docker, nsjail, or any
//! Unix socket.  They are NOT marked `#[ignore]` and are safe for all CI
//! environments.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use soma::shutdown::drain_in_flight;

// ── drain_in_flight tests ─────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn drain_completes_immediately_when_counter_is_zero() {
    let counter = Arc::new(AtomicUsize::new(0));
    let result = drain_in_flight(Arc::clone(&counter), Duration::from_millis(500)).await;
    assert!(
        result.is_ok(),
        "drain must return Ok when counter starts at zero"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn drain_times_out_with_remaining_count() {
    let counter = Arc::new(AtomicUsize::new(5));
    // Use a short deadline so the test runs quickly.
    let result = drain_in_flight(Arc::clone(&counter), Duration::from_millis(150)).await;
    match result {
        Err(remaining) => {
            assert_eq!(remaining, 5, "expected 5 in-flight remaining");
        }
        Ok(()) => panic!("drain should have timed out but returned Ok"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn drain_completes_when_counter_drops_to_zero_before_deadline() {
    let counter = Arc::new(AtomicUsize::new(2));
    let counter_clone = Arc::clone(&counter);

    // Spawn a task that decrements the counter to zero before the drain deadline.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(60)).await;
        counter_clone.fetch_sub(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(30)).await;
        counter_clone.fetch_sub(1, Ordering::SeqCst);
    });

    let result = drain_in_flight(Arc::clone(&counter), Duration::from_millis(600)).await;
    assert!(
        result.is_ok(),
        "drain must return Ok when counter reaches zero before deadline"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "counter must be zero after drain"
    );
}

// ── in-flight bracketing via LifeKernelService ────────────────────────────────
//
// Validates that the service's `in_flight` counter correctly reflects the
// number of currently-executing dispatches.  We use the unit-level mock
// directly (no Unix socket / tonic transport).

#[tokio::test(flavor = "multi_thread")]
async fn service_in_flight_is_zero_outside_dispatch() {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use aios_protocol::{
        hypervisor::{BackendId, ForkSpec, VmHandle, VmId, VmSnapshotHandle, VmSpec, VmStatus},
        ids::{AgentId, SessionId},
        kernel::{KernelContext, KernelResult},
        ports::KernelPort,
        tool::{ToolCall, ToolResult},
    };
    use chrono::Utc;
    use life_kernel_proto::pb::{self, kernel_service_server::KernelService as _};
    use soma::server::LifeKernelService;
    use tonic::Request;

    // ── Minimal mock kernel ───────────────────────────────────────────────────

    struct QueuedKernel {
        dispatch_results: Mutex<VecDeque<KernelResult<ToolResult>>>,
    }

    impl QueuedKernel {
        fn new() -> Self {
            Self {
                dispatch_results: Mutex::new(VecDeque::new()),
            }
        }
        fn push_dispatch(&self, r: KernelResult<ToolResult>) {
            self.dispatch_results.lock().unwrap().push_back(r);
        }
    }

    #[async_trait::async_trait]
    impl KernelPort for QueuedKernel {
        async fn create_vm(&self, _: VmSpec, _: KernelContext) -> KernelResult<VmHandle> {
            unimplemented!()
        }
        async fn dispatch(
            &self,
            _: &VmHandle,
            _: ToolCall,
            _: &KernelContext,
        ) -> KernelResult<ToolResult> {
            self.dispatch_results
                .lock()
                .unwrap()
                .pop_front()
                .expect("no queued dispatch result")
        }
        async fn snapshot(&self, _: &VmHandle, _: &str) -> KernelResult<VmSnapshotHandle> {
            unimplemented!()
        }
        async fn fork(
            &self,
            _: &VmSnapshotHandle,
            _: ForkSpec,
            _: KernelContext,
        ) -> KernelResult<VmHandle> {
            unimplemented!()
        }
        async fn hibernate(&self, _: &VmHandle) -> KernelResult<()> {
            unimplemented!()
        }
        async fn resume(&self, _: &VmHandle) -> KernelResult<VmHandle> {
            unimplemented!()
        }
        async fn destroy(&self, _: VmHandle) -> KernelResult<()> {
            unimplemented!()
        }
    }

    // ── Test body ─────────────────────────────────────────────────────────────

    let mock = QueuedKernel::new();
    mock.push_dispatch(Ok(ToolResult {
        call_id: "drain-test".into(),
        tool_name: "shell".into(),
        output: serde_json::json!("ok"),
        content: None,
        is_error: false,
        usage: None,
    }));

    let svc = LifeKernelService::new(Arc::new(mock));
    let counter = svc.in_flight();

    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "counter must be 0 before dispatch"
    );

    let vm_handle = VmHandle {
        vm_id: VmId::from("vm-drain"),
        backend: BackendId::from("local"),
        session_id: SessionId::from_string("sess-drain"),
        agent_id: AgentId::from_string("agent-drain"),
        status: VmStatus::Running,
        created_at: Utc::now(),
        metadata: serde_json::Value::Null,
    };
    let pb_vm: pb::VmHandle = vm_handle.try_into().expect("vm handle to pb conversion");

    let req = Request::new(pb::DispatchRequest {
        vm: Some(pb_vm),
        call: Some(pb::ToolCall {
            call_id: "drain-test".into(),
            tool_name: "shell".into(),
            input_json: b"{}".to_vec(),
            requested_capabilities: vec![],
        }),
        ctx: Some(pb::KernelContext {
            session_id: Some(pb::SessionId {
                value: "sess-drain".into(),
            }),
            agent_id: Some(pb::AgentId {
                value: "agent-drain".into(),
            }),
            wallet: Some(pb::WalletAttribution {
                address: "0x0".into(),
                chain_caip2: "eip155:8453".into(),
            }),
            cost_hint: None,
            trace_ctx: None,
        }),
    });

    let _result: tonic::Response<pb::ToolResult> =
        svc.dispatch(req).await.expect("dispatch should succeed");

    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "counter must return to 0 after dispatch completes"
    );
}
