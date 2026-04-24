//! tonic server that fans RPCs onto a shared `KernelPort` implementation.
//!
//! Each RPC is a thin adapter: convert the proto request to canonical
//! `aios_protocol` types, call the engine, convert the result back.
//! `KernelError → tonic::Status` mapping lives here; tracing + metrics
//! instrumentation is added in a follow-up ticket (BRO-899).

use std::sync::Arc;

use aios_protocol::ports::KernelPort;
use life_kernel_proto::pb::{
    self,
    kernel_service_server::{KernelService, KernelServiceServer},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

/// Production `KernelService` impl backed by any `KernelPort` implementation.
///
/// Generic so tests can inject a mock; production always passes
/// `Arc<KernelEngine>`.
#[derive(Clone)]
pub struct LifeKernelService<E: KernelPort> {
    engine: Arc<E>,
}

impl<E: KernelPort + 'static> LifeKernelService<E> {
    /// Construct a service over the given engine.
    pub fn new(engine: Arc<E>) -> Self {
        Self { engine }
    }

    /// Wrap the service in a tonic `KernelServiceServer` ready to add to a router.
    pub fn into_server(self) -> KernelServiceServer<Self> {
        KernelServiceServer::new(self)
    }
}

// ── Error helpers ────────────────────────────────────────────────────────────

fn kernel_error_to_status(err: aios_protocol::kernel::KernelError) -> Status {
    use aios_protocol::kernel::KernelError::*;
    match err {
        VmNotFound(_) | BackendNotFound(_) | SnapshotNotFound(_) => {
            Status::not_found(err.to_string())
        }
        CapabilityUnavailable { .. } => Status::unimplemented(err.to_string()),
        Timeout { .. } => Status::deadline_exceeded(err.to_string()),
        GateDenied { .. } => Status::permission_denied(err.to_string()),
        Backend(_) | Internal(_) => Status::internal(err.to_string()),
        // `KernelError` is `#[non_exhaustive]`; future variants are also
        // treated as internal errors rather than silently misrouting them.
        _ => Status::internal(err.to_string()),
    }
}

fn convert_error_to_status(err: life_kernel_proto::ConvertError) -> Status {
    Status::invalid_argument(err.to_string())
}

// ── KernelService impl ───────────────────────────────────────────────────────

#[tonic::async_trait]
impl<E: KernelPort + 'static> KernelService for LifeKernelService<E> {
    async fn create_vm(
        &self,
        request: Request<pb::CreateVmRequest>,
    ) -> Result<Response<pb::VmHandle>, Status> {
        let inner = request.into_inner();
        let spec = inner
            .spec
            .ok_or_else(|| Status::invalid_argument("missing spec"))?
            .try_into()
            .map_err(convert_error_to_status)?;
        let ctx = inner
            .ctx
            .ok_or_else(|| Status::invalid_argument("missing ctx"))?
            .try_into()
            .map_err(convert_error_to_status)?;
        let handle = self
            .engine
            .create_vm(spec, ctx)
            .await
            .map_err(kernel_error_to_status)?;
        let pb_handle: pb::VmHandle = handle
            .try_into()
            .map_err(|e: life_kernel_proto::ConvertError| Status::internal(e.to_string()))?;
        Ok(Response::new(pb_handle))
    }

    async fn dispatch(
        &self,
        request: Request<pb::DispatchRequest>,
    ) -> Result<Response<pb::ToolResult>, Status> {
        let inner = request.into_inner();
        let pb_vm = inner
            .vm
            .ok_or_else(|| Status::invalid_argument("missing vm"))?;
        let vm = pb_vm.try_into().map_err(convert_error_to_status)?;
        let call = inner
            .call
            .ok_or_else(|| Status::invalid_argument("missing call"))?
            .try_into()
            .map_err(convert_error_to_status)?;
        let ctx: aios_protocol::kernel::KernelContext = inner
            .ctx
            .ok_or_else(|| Status::invalid_argument("missing ctx"))?
            .try_into()
            .map_err(convert_error_to_status)?;
        let result = self
            .engine
            .dispatch(&vm, call, &ctx)
            .await
            .map_err(kernel_error_to_status)?;
        let pb_result: pb::ToolResult = result
            .try_into()
            .map_err(|e: life_kernel_proto::ConvertError| Status::internal(e.to_string()))?;
        Ok(Response::new(pb_result))
    }

    async fn snapshot(
        &self,
        request: Request<pb::SnapshotRequest>,
    ) -> Result<Response<pb::VmSnapshotHandle>, Status> {
        let inner = request.into_inner();
        let pb_vm = inner
            .vm
            .ok_or_else(|| Status::invalid_argument("missing vm"))?;
        let vm = pb_vm.try_into().map_err(convert_error_to_status)?;
        let handle = self
            .engine
            .snapshot(&vm, &inner.name)
            .await
            .map_err(kernel_error_to_status)?;
        let pb_handle: pb::VmSnapshotHandle = handle.into();
        Ok(Response::new(pb_handle))
    }

    async fn fork(
        &self,
        request: Request<pb::ForkRequest>,
    ) -> Result<Response<pb::VmHandle>, Status> {
        let inner = request.into_inner();
        let snapshot = inner
            .snapshot
            .ok_or_else(|| Status::invalid_argument("missing snapshot"))?
            .try_into()
            .map_err(convert_error_to_status)?;
        let spec = inner
            .spec
            .ok_or_else(|| Status::invalid_argument("missing spec"))?
            .try_into()
            .map_err(convert_error_to_status)?;
        let ctx = inner
            .ctx
            .ok_or_else(|| Status::invalid_argument("missing ctx"))?
            .try_into()
            .map_err(convert_error_to_status)?;
        let handle = self
            .engine
            .fork(&snapshot, spec, ctx)
            .await
            .map_err(kernel_error_to_status)?;
        let pb_handle: pb::VmHandle = handle
            .try_into()
            .map_err(|e: life_kernel_proto::ConvertError| Status::internal(e.to_string()))?;
        Ok(Response::new(pb_handle))
    }

    async fn hibernate(
        &self,
        request: Request<pb::LifecycleRequest>,
    ) -> Result<Response<pb::Empty>, Status> {
        let inner = request.into_inner();
        let pb_vm = inner
            .vm
            .ok_or_else(|| Status::invalid_argument("missing vm"))?;
        let vm = pb_vm.try_into().map_err(convert_error_to_status)?;
        self.engine
            .hibernate(&vm)
            .await
            .map_err(kernel_error_to_status)?;
        Ok(Response::new(pb::Empty {}))
    }

    async fn resume(
        &self,
        request: Request<pb::LifecycleRequest>,
    ) -> Result<Response<pb::VmHandle>, Status> {
        let inner = request.into_inner();
        let pb_vm = inner
            .vm
            .ok_or_else(|| Status::invalid_argument("missing vm"))?;
        let vm = pb_vm.try_into().map_err(convert_error_to_status)?;
        let handle = self
            .engine
            .resume(&vm)
            .await
            .map_err(kernel_error_to_status)?;
        let pb_handle: pb::VmHandle = handle
            .try_into()
            .map_err(|e: life_kernel_proto::ConvertError| Status::internal(e.to_string()))?;
        Ok(Response::new(pb_handle))
    }

    async fn destroy(
        &self,
        request: Request<pb::DestroyRequest>,
    ) -> Result<Response<pb::Empty>, Status> {
        let inner = request.into_inner();
        let pb_vm = inner
            .vm
            .ok_or_else(|| Status::invalid_argument("missing vm"))?;
        let vm = pb_vm.try_into().map_err(convert_error_to_status)?;
        self.engine
            .destroy(vm)
            .await
            .map_err(kernel_error_to_status)?;
        Ok(Response::new(pb::Empty {}))
    }

    type ListVmsStream = ReceiverStream<Result<pb::VmInfo, Status>>;

    async fn list_vms(
        &self,
        request: Request<pb::ListVmsRequest>,
    ) -> Result<Response<Self::ListVmsStream>, Status> {
        // Session filter is parsed so callers discover the filter surface
        // via their generated stubs. The live-VM index is seeded in BRO-900;
        // until then the daemon reports an empty result regardless of filter.
        let _session_filter = request.into_inner().session_id.map(|id| id.value);

        let (tx, rx) = mpsc::channel::<Result<pb::VmInfo, Status>>(32);
        drop(tx); // Close immediately — placeholder response until BRO-900.
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//                                   Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use aios_protocol::{
        hypervisor::{
            BackendError, BackendId, ForkSpec, VmHandle, VmId, VmSnapshotHandle, VmSpec, VmStatus,
        },
        ids::{AgentId, SessionId},
        kernel::{GateKind, KernelContext, KernelError},
        sandbox::NetworkPolicy,
        tool::{ToolCall, ToolResult},
    };
    use chrono::Utc;
    use tokio_stream::StreamExt as _;

    // ── MockKernel ───────────────────────────────────────────────────────────

    /// Hand-written mock that returns pre-queued results (or panics if the
    /// queue is empty and a call is made unexpectedly).
    struct MockKernel {
        create_vm_results: Mutex<VecDeque<aios_protocol::kernel::KernelResult<VmHandle>>>,
        dispatch_results: Mutex<VecDeque<aios_protocol::kernel::KernelResult<ToolResult>>>,
        snapshot_results: Mutex<VecDeque<aios_protocol::kernel::KernelResult<VmSnapshotHandle>>>,
        fork_results: Mutex<VecDeque<aios_protocol::kernel::KernelResult<VmHandle>>>,
        hibernate_results: Mutex<VecDeque<aios_protocol::kernel::KernelResult<()>>>,
        resume_results: Mutex<VecDeque<aios_protocol::kernel::KernelResult<VmHandle>>>,
        destroy_results: Mutex<VecDeque<aios_protocol::kernel::KernelResult<()>>>,
    }

    impl MockKernel {
        fn new() -> Self {
            Self {
                create_vm_results: Mutex::new(VecDeque::new()),
                dispatch_results: Mutex::new(VecDeque::new()),
                snapshot_results: Mutex::new(VecDeque::new()),
                fork_results: Mutex::new(VecDeque::new()),
                hibernate_results: Mutex::new(VecDeque::new()),
                resume_results: Mutex::new(VecDeque::new()),
                destroy_results: Mutex::new(VecDeque::new()),
            }
        }

        fn push_create_vm(&self, r: aios_protocol::kernel::KernelResult<VmHandle>) {
            self.create_vm_results.lock().unwrap().push_back(r);
        }

        fn push_dispatch(&self, r: aios_protocol::kernel::KernelResult<ToolResult>) {
            self.dispatch_results.lock().unwrap().push_back(r);
        }
    }

    #[async_trait::async_trait]
    impl KernelPort for MockKernel {
        async fn create_vm(
            &self,
            _spec: VmSpec,
            _ctx: KernelContext,
        ) -> aios_protocol::kernel::KernelResult<VmHandle> {
            self.create_vm_results
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockKernel: no queued create_vm result")
        }

        async fn dispatch(
            &self,
            _vm: &VmHandle,
            _call: ToolCall,
            _ctx: &KernelContext,
        ) -> aios_protocol::kernel::KernelResult<ToolResult> {
            self.dispatch_results
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockKernel: no queued dispatch result")
        }

        async fn snapshot(
            &self,
            _vm: &VmHandle,
            _name: &str,
        ) -> aios_protocol::kernel::KernelResult<VmSnapshotHandle> {
            self.snapshot_results
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockKernel: no queued snapshot result")
        }

        async fn fork(
            &self,
            _snapshot: &VmSnapshotHandle,
            _spec: ForkSpec,
            _ctx: KernelContext,
        ) -> aios_protocol::kernel::KernelResult<VmHandle> {
            self.fork_results
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockKernel: no queued fork result")
        }

        async fn hibernate(&self, _vm: &VmHandle) -> aios_protocol::kernel::KernelResult<()> {
            self.hibernate_results
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockKernel: no queued hibernate result")
        }

        async fn resume(&self, _vm: &VmHandle) -> aios_protocol::kernel::KernelResult<VmHandle> {
            self.resume_results
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockKernel: no queued resume result")
        }

        async fn destroy(&self, _vm: VmHandle) -> aios_protocol::kernel::KernelResult<()> {
            self.destroy_results
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockKernel: no queued destroy result")
        }
    }

    // ── Fixtures ─────────────────────────────────────────────────────────────

    fn test_vm_handle() -> VmHandle {
        VmHandle {
            vm_id: VmId::from("vm-test"),
            backend: BackendId::from("local"),
            session_id: SessionId::from_string("sess-1"),
            agent_id: AgentId::from_string("agent-1"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }

    fn test_ctx_pb() -> pb::KernelContext {
        pb::KernelContext {
            session_id: Some(pb::SessionId {
                value: "sess-1".into(),
            }),
            agent_id: Some(pb::AgentId {
                value: "agent-1".into(),
            }),
            wallet: Some(pb::WalletAttribution {
                address: "0xdead".into(),
                chain_caip2: "eip155:8453".into(),
            }),
            cost_hint: None,
            trace_ctx: None,
        }
    }

    fn test_vm_spec_pb() -> pb::VmSpec {
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

    fn test_vm_handle_pb() -> pb::VmHandle {
        test_vm_handle()
            .try_into()
            .expect("vm handle to pb should succeed")
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    /// `create_vm` with a valid spec/ctx returns the engine's `VmHandle`
    /// serialised as `pb::VmHandle` with the same vm_id.
    #[tokio::test]
    async fn create_vm_happy_path_returns_handle() {
        let mock = MockKernel::new();
        mock.push_create_vm(Ok(test_vm_handle()));
        let svc = LifeKernelService::new(Arc::new(mock));

        let req = Request::new(pb::CreateVmRequest {
            spec: Some(test_vm_spec_pb()),
            ctx: Some(test_ctx_pb()),
        });
        let resp = svc.create_vm(req).await.expect("create_vm should succeed");
        let handle = resp.into_inner();
        assert_eq!(
            handle.vm_id.as_ref().map(|id| id.value.as_str()),
            Some("vm-test")
        );
    }

    /// A request with no `spec` field must be rejected with `invalid_argument`.
    #[tokio::test]
    async fn create_vm_bad_spec_returns_invalid_argument() {
        let mock = MockKernel::new();
        let svc = LifeKernelService::new(Arc::new(mock));

        let req = Request::new(pb::CreateVmRequest {
            spec: None,
            ctx: Some(test_ctx_pb()),
        });
        let err = svc
            .create_vm(req)
            .await
            .expect_err("missing spec should fail");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    /// A `dispatch` where the engine returns a `Backend(Internal(…))` error
    /// must surface as `Status::internal`.
    #[tokio::test]
    async fn dispatch_backend_error_surfaces_as_internal() {
        let mock = MockKernel::new();
        mock.push_dispatch(Err(KernelError::Backend(BackendError::Internal(
            "boom".into(),
        ))));
        let svc = LifeKernelService::new(Arc::new(mock));

        let call_pb = pb::ToolCall {
            call_id: "c1".into(),
            tool_name: "shell".into(),
            input_json: b"{}".to_vec(),
            requested_capabilities: vec![],
        };
        let req = Request::new(pb::DispatchRequest {
            vm: Some(test_vm_handle_pb()),
            call: Some(call_pb),
            ctx: Some(test_ctx_pb()),
        });
        let err = svc
            .dispatch(req)
            .await
            .expect_err("backend error must propagate");
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("boom"));
    }

    /// A `dispatch` where the engine returns `GateDenied` must surface as
    /// `Status::permission_denied`.
    #[tokio::test]
    async fn dispatch_gate_denied_surfaces_as_permission_denied() {
        let mock = MockKernel::new();
        mock.push_dispatch(Err(KernelError::GateDenied {
            gate: GateKind::Policy,
            reason: "policy says no".into(),
        }));
        let svc = LifeKernelService::new(Arc::new(mock));

        let call_pb = pb::ToolCall {
            call_id: "c2".into(),
            tool_name: "shell".into(),
            input_json: b"{}".to_vec(),
            requested_capabilities: vec![],
        };
        let req = Request::new(pb::DispatchRequest {
            vm: Some(test_vm_handle_pb()),
            call: Some(call_pb),
            ctx: Some(test_ctx_pb()),
        });
        let err = svc
            .dispatch(req)
            .await
            .expect_err("gate denial must propagate");
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    /// `list_vms` with an optional session filter must return `Ok(Response)`
    /// wrapping a stream that yields zero items before closing.
    #[tokio::test]
    async fn list_vms_returns_empty_stream_and_accepts_session_filter() {
        let mock = MockKernel::new();
        let svc = LifeKernelService::new(Arc::new(mock));

        let req = Request::new(pb::ListVmsRequest {
            session_id: Some(pb::SessionId {
                value: "sess-filter".into(),
            }),
        });
        let resp = svc.list_vms(req).await.expect("list_vms should succeed");
        let mut stream = resp.into_inner();
        // The stream should close immediately (no items).
        let item = stream.next().await;
        assert!(
            item.is_none(),
            "expected empty stream before BRO-900, got {item:?}"
        );
    }
}
