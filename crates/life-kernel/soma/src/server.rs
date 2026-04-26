//! tonic server that fans RPCs onto a shared `KernelPort` implementation.
//!
//! Each RPC is a thin adapter: convert the proto request to canonical
//! `aios_protocol` types, call the engine, convert the result back.
//! `KernelError → tonic::Status` mapping lives here. Every handler is
//! decorated with `#[tracing::instrument]` and emits `kernel.*` metrics
//! via `KernelMetrics` (BRO-899).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use aios_protocol::hypervisor::{VmHandle, VmInfo};
use aios_protocol::ports::KernelPort;
use life_kernel_proto::pb::{
    self,
    kernel_service_server::{KernelService, KernelServiceServer},
};
use std::sync::RwLock;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::observability::KernelMetrics;

/// Production `KernelService` impl backed by any `KernelPort` implementation.
///
/// Generic so tests can inject a mock; production always passes
/// `Arc<KernelEngine>`.
pub struct LifeKernelService<E: KernelPort> {
    engine: Arc<E>,
    /// Live-VM index, keyed by stringified `VmId`. Seeded from
    /// `ReplayedState::snapshot_vm_handles()` at daemon start and kept
    /// consistent by `create_vm` / `destroy`. `list_vms` reads from it.
    ///
    /// Uses `std::sync::RwLock` — all reads/writes are short and synchronous,
    /// so there is no need for the `tokio::sync::RwLock` overhead.
    live_vms: Arc<RwLock<HashMap<String, VmHandle>>>,
    /// Number of dispatches currently in-flight.  Bracketed by
    /// `fetch_add(1)` on entry and `fetch_sub(1)` on exit (including early
    /// returns) via [`InFlightGuard`].  Exposed to the drain logic through
    /// [`Self::in_flight`].
    in_flight: Arc<AtomicUsize>,
    /// Canonical `kernel.*` metric handles. Cheap to clone — backed by
    /// `Arc`-wrapped OTel instrument handles internally.
    metrics: KernelMetrics,
}

// Hand-written `Clone` so the bound is `E: KernelPort` and NOT
// `E: KernelPort + Clone`. `KernelEngine` itself is not `Clone` (it holds
// `Arc<GateChain>` + `Arc<EventEmitter>` internally), so `#[derive(Clone)]`
// would silently fail to provide `LifeKernelService<KernelEngine>: Clone`
// — which `tonic::transport::Server::add_service` and any multi-threaded
// service wiring will require.
impl<E: KernelPort> Clone for LifeKernelService<E> {
    fn clone(&self) -> Self {
        Self {
            engine: Arc::clone(&self.engine),
            live_vms: Arc::clone(&self.live_vms),
            in_flight: Arc::clone(&self.in_flight),
            metrics: self.metrics.clone(),
        }
    }
}

impl<E: KernelPort + 'static> LifeKernelService<E> {
    /// Construct a service over the given engine with an empty live-VM index.
    pub fn new(engine: Arc<E>) -> Self {
        Self::with_seed(engine, Vec::new())
    }

    /// Construct a service and seed the live-VM index from a prior replay.
    ///
    /// Callers pass `bootstrap.replayed.snapshot_vm_handles()` to restore
    /// the VM index after a daemon restart without replaying from scratch at
    /// the RPC layer.
    ///
    /// `KernelMetrics::register()` is called internally; callers do not need
    /// to construct metrics separately.
    pub fn with_seed(engine: Arc<E>, seed: Vec<VmHandle>) -> Self {
        let mut map = HashMap::new();
        for handle in seed {
            map.insert(handle.vm_id.to_string(), handle);
        }
        Self {
            engine,
            live_vms: Arc::new(RwLock::new(map)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            metrics: KernelMetrics::register(),
        }
    }

    /// Wrap the service in a tonic `KernelServiceServer` ready to add to a router.
    pub fn into_server(self) -> KernelServiceServer<Self> {
        KernelServiceServer::new(self)
    }

    /// Return a reference-counted handle to the in-flight counter.
    ///
    /// The shutdown / drain logic calls this to wait for all in-flight
    /// dispatches to complete before the process exits.
    pub fn in_flight(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.in_flight)
    }
}

// ── RAII guard for in-flight dispatch accounting ─────────────────────────────

/// Increments the in-flight counter on construction; decrements on drop.
///
/// Using a guard struct ensures the counter is decremented even when an RPC
/// handler returns early via `?`.
struct InFlightGuard(Arc<AtomicUsize>);

impl InFlightGuard {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(counter))
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
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
    #[tracing::instrument(
        skip(self, request),
        fields(
            life.session_id = tracing::field::Empty,
            kernel.vm_id = tracing::field::Empty,
        )
    )]
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
        let ctx: aios_protocol::kernel::KernelContext = inner
            .ctx
            .ok_or_else(|| Status::invalid_argument("missing ctx"))?
            .try_into()
            .map_err(convert_error_to_status)?;

        // Populate span fields now that we have a typed context.
        tracing::Span::current().record("life.session_id", ctx.session_id.as_str());

        let handle = self
            .engine
            .create_vm(spec, ctx)
            .await
            .map_err(kernel_error_to_status)?;

        tracing::Span::current().record("kernel.vm_id", handle.vm_id.0.as_str());

        // Insert into the live-VM index.
        {
            let mut map = self
                .live_vms
                .write()
                .expect("live_vms RwLock poisoned in create_vm");
            map.insert(handle.vm_id.to_string(), handle.clone());
        }

        self.metrics.record_lifecycle("create");

        let pb_handle: pb::VmHandle = handle
            .try_into()
            .map_err(|e: life_kernel_proto::ConvertError| Status::internal(e.to_string()))?;
        Ok(Response::new(pb_handle))
    }

    #[tracing::instrument(
        skip(self, request),
        fields(
            life.session_id = tracing::field::Empty,
            kernel.vm_id = tracing::field::Empty,
            kernel.tool_name = tracing::field::Empty,
        )
    )]
    async fn dispatch(
        &self,
        request: Request<pb::DispatchRequest>,
    ) -> Result<Response<pb::ToolResult>, Status> {
        // Bracket the entire handler with an in-flight guard so any early
        // return via `?` still decrements the counter.
        let _guard = InFlightGuard::new(&self.in_flight);

        let inner = request.into_inner();
        let pb_vm = inner
            .vm
            .ok_or_else(|| Status::invalid_argument("missing vm"))?;
        let vm: VmHandle = pb_vm.try_into().map_err(convert_error_to_status)?;
        let call: aios_protocol::tool::ToolCall = inner
            .call
            .ok_or_else(|| Status::invalid_argument("missing call"))?
            .try_into()
            .map_err(convert_error_to_status)?;
        let ctx: aios_protocol::kernel::KernelContext = inner
            .ctx
            .ok_or_else(|| Status::invalid_argument("missing ctx"))?
            .try_into()
            .map_err(convert_error_to_status)?;

        // Populate span fields before the await point.
        tracing::Span::current().record("life.session_id", ctx.session_id.as_str());
        tracing::Span::current().record("kernel.vm_id", vm.vm_id.0.as_str());
        tracing::Span::current().record("kernel.tool_name", call.tool_name.as_str());

        let tool_name = call.tool_name.clone();
        let started = Instant::now();
        let result = self.engine.dispatch(&vm, call, &ctx).await;
        let elapsed = started.elapsed();

        // Record dispatch duration regardless of success/failure.
        self.metrics.observe_dispatch(elapsed, tool_name);

        let result = result.map_err(kernel_error_to_status)?;
        let pb_result: pb::ToolResult = result
            .try_into()
            .map_err(|e: life_kernel_proto::ConvertError| Status::internal(e.to_string()))?;
        Ok(Response::new(pb_result))
    }

    #[tracing::instrument(
        skip(self, request),
        fields(
            kernel.vm_id = tracing::field::Empty,
        )
    )]
    async fn snapshot(
        &self,
        request: Request<pb::SnapshotRequest>,
    ) -> Result<Response<pb::VmSnapshotHandle>, Status> {
        let inner = request.into_inner();
        let pb_vm = inner
            .vm
            .ok_or_else(|| Status::invalid_argument("missing vm"))?;
        let vm: VmHandle = pb_vm.try_into().map_err(convert_error_to_status)?;

        tracing::Span::current().record("kernel.vm_id", vm.vm_id.0.as_str());

        let handle = self
            .engine
            .snapshot(&vm, &inner.name)
            .await
            .map_err(kernel_error_to_status)?;

        self.metrics.record_lifecycle("snapshot");

        // Infallible by design — see life-kernel-proto::convert line 548.
        // Promote to TryFrom + map_err if that ever changes.
        let pb_handle: pb::VmSnapshotHandle = handle.into();
        Ok(Response::new(pb_handle))
    }

    #[tracing::instrument(
        skip(self, request),
        fields(
            life.session_id = tracing::field::Empty,
            kernel.vm_id = tracing::field::Empty,
        )
    )]
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
        let ctx: aios_protocol::kernel::KernelContext = inner
            .ctx
            .ok_or_else(|| Status::invalid_argument("missing ctx"))?
            .try_into()
            .map_err(convert_error_to_status)?;

        tracing::Span::current().record("life.session_id", ctx.session_id.as_str());

        let handle = self
            .engine
            .fork(&snapshot, spec, ctx)
            .await
            .map_err(kernel_error_to_status)?;

        tracing::Span::current().record("kernel.vm_id", handle.vm_id.0.as_str());

        self.metrics.record_lifecycle("fork");

        let pb_handle: pb::VmHandle = handle
            .try_into()
            .map_err(|e: life_kernel_proto::ConvertError| Status::internal(e.to_string()))?;
        Ok(Response::new(pb_handle))
    }

    #[tracing::instrument(
        skip(self, request),
        fields(
            kernel.vm_id = tracing::field::Empty,
        )
    )]
    async fn hibernate(
        &self,
        request: Request<pb::LifecycleRequest>,
    ) -> Result<Response<pb::Empty>, Status> {
        let inner = request.into_inner();
        let pb_vm = inner
            .vm
            .ok_or_else(|| Status::invalid_argument("missing vm"))?;
        let vm: VmHandle = pb_vm.try_into().map_err(convert_error_to_status)?;

        tracing::Span::current().record("kernel.vm_id", vm.vm_id.0.as_str());

        self.engine
            .hibernate(&vm)
            .await
            .map_err(kernel_error_to_status)?;

        self.metrics.record_lifecycle("hibernate");

        Ok(Response::new(pb::Empty {}))
    }

    #[tracing::instrument(
        skip(self, request),
        fields(
            kernel.vm_id = tracing::field::Empty,
        )
    )]
    async fn resume(
        &self,
        request: Request<pb::LifecycleRequest>,
    ) -> Result<Response<pb::VmHandle>, Status> {
        let inner = request.into_inner();
        let pb_vm = inner
            .vm
            .ok_or_else(|| Status::invalid_argument("missing vm"))?;
        let vm: VmHandle = pb_vm.try_into().map_err(convert_error_to_status)?;

        tracing::Span::current().record("kernel.vm_id", vm.vm_id.0.as_str());

        let handle = self
            .engine
            .resume(&vm)
            .await
            .map_err(kernel_error_to_status)?;

        self.metrics.record_lifecycle("resume");

        let pb_handle: pb::VmHandle = handle
            .try_into()
            .map_err(|e: life_kernel_proto::ConvertError| Status::internal(e.to_string()))?;
        Ok(Response::new(pb_handle))
    }

    #[tracing::instrument(
        skip(self, request),
        fields(
            kernel.vm_id = tracing::field::Empty,
        )
    )]
    async fn destroy(
        &self,
        request: Request<pb::DestroyRequest>,
    ) -> Result<Response<pb::Empty>, Status> {
        let inner = request.into_inner();
        let pb_vm = inner
            .vm
            .ok_or_else(|| Status::invalid_argument("missing vm"))?;
        let vm: VmHandle = pb_vm.try_into().map_err(convert_error_to_status)?;

        // Capture vm_id before the engine consumes the handle.
        let vm_id_str = vm.vm_id.to_string();

        tracing::Span::current().record("kernel.vm_id", vm_id_str.as_str());

        self.engine
            .destroy(vm)
            .await
            .map_err(kernel_error_to_status)?;

        // Remove from live-VM index on successful destroy.
        {
            let mut map = self
                .live_vms
                .write()
                .expect("live_vms RwLock poisoned in destroy");
            map.remove(&vm_id_str);
        }

        self.metrics.record_lifecycle("destroy");

        Ok(Response::new(pb::Empty {}))
    }

    type ListVmsStream = ReceiverStream<Result<pb::VmInfo, Status>>;

    #[tracing::instrument(skip(self, request))]
    async fn list_vms(
        &self,
        request: Request<pb::ListVmsRequest>,
    ) -> Result<Response<Self::ListVmsStream>, Status> {
        let session_filter = request.into_inner().session_id.map(|id| id.value);

        // Snapshot the live-VM map under a short read-lock, then stream items
        // through a buffered channel (buffer 32, as per spec).  The channel
        // task runs independently so the RPC handler can return quickly.
        let snapshot: Vec<VmHandle> = {
            let map = self
                .live_vms
                .read()
                .expect("live_vms RwLock poisoned in list_vms");
            map.values()
                .filter(|h| {
                    // Apply optional session filter.
                    if let Some(ref filter) = session_filter {
                        h.session_id.as_str() == filter.as_str()
                    } else {
                        true
                    }
                })
                .cloned()
                .collect()
        };

        let (tx, rx) = mpsc::channel::<Result<pb::VmInfo, Status>>(32);

        tokio::spawn(async move {
            for handle in snapshot {
                // Convert VmHandle → VmInfo (dropping session_id/agent_id/metadata
                // which are not part of the pb::VmInfo projection).
                let vm_info = VmInfo {
                    vm_id: handle.vm_id,
                    backend: handle.backend,
                    status: handle.status,
                    created_at: handle.created_at,
                };
                let pb_info = match pb::VmInfo::try_from(vm_info) {
                    Ok(i) => i,
                    Err(e) => {
                        tracing::warn!(error = %e, "VmHandle→VmInfo conversion failed in list_vms — skipping");
                        continue;
                    }
                };
                if tx.send(Ok(pb_info)).await.is_err() {
                    // Receiver dropped — client disconnected.
                    break;
                }
            }
            // Channel sender drops here, closing the stream on the client side.
        });

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
    use life_kernel_proto::aios_v1;
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

    #[allow(dead_code)]
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

        fn push_snapshot(&self, r: aios_protocol::kernel::KernelResult<VmSnapshotHandle>) {
            self.snapshot_results.lock().unwrap().push_back(r);
        }

        fn push_fork(&self, r: aios_protocol::kernel::KernelResult<VmHandle>) {
            self.fork_results.lock().unwrap().push_back(r);
        }

        fn push_hibernate(&self, r: aios_protocol::kernel::KernelResult<()>) {
            self.hibernate_results.lock().unwrap().push_back(r);
        }

        fn push_resume(&self, r: aios_protocol::kernel::KernelResult<VmHandle>) {
            self.resume_results.lock().unwrap().push_back(r);
        }

        fn push_destroy(&self, r: aios_protocol::kernel::KernelResult<()>) {
            self.destroy_results.lock().unwrap().push_back(r);
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

    fn test_vm_handle_with_session(vm_id: &str, session: &str) -> VmHandle {
        VmHandle {
            vm_id: VmId::from(vm_id),
            backend: BackendId::from("local"),
            session_id: SessionId::from_string(session),
            agent_id: AgentId::from_string("agent-1"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }

    fn test_ctx_pb() -> pb::KernelContext {
        pb::KernelContext {
            session_id: Some(aios_v1::SessionId {
                value: "sess-1".into(),
            }),
            agent_id: Some(aios_v1::AgentId {
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

    /// Locks in the `Clone` guarantee: `LifeKernelService<E>` must be
    /// cloneable regardless of whether `E: Clone`. The production engine
    /// (`KernelEngine`) is NOT `Clone`; `#[derive(Clone)]` would silently
    /// constrain the impl to `E: Clone` and would require a refactor when
    /// `tonic::transport::Server::add_service` is wired in BRO-896.
    /// This test ensures the hand-written `Clone` impl stays in place.
    #[tokio::test]
    async fn service_is_clone_regardless_of_engine_clone() {
        let svc = LifeKernelService::new(Arc::new(MockKernel::new()));
        let _clone = svc.clone();
    }

    /// `list_vms` with an optional session filter must return `Ok(Response)`
    /// wrapping a stream that yields zero items before closing (empty index).
    #[tokio::test]
    async fn list_vms_returns_empty_stream_and_accepts_session_filter() {
        let mock = MockKernel::new();
        let svc = LifeKernelService::new(Arc::new(mock));

        let req = Request::new(pb::ListVmsRequest {
            session_id: Some(aios_v1::SessionId {
                value: "sess-filter".into(),
            }),
        });
        let resp = svc.list_vms(req).await.expect("list_vms should succeed");
        let mut stream = resp.into_inner();
        // The stream should close immediately (no items in empty index).
        let item = stream.next().await;
        assert!(
            item.is_none(),
            "expected empty stream for empty live-VM index, got {item:?}"
        );
    }

    // ── New tests for BRO-900 ─────────────────────────────────────────────────

    /// `create_vm` inserts the returned handle into the live-VM index.
    #[tokio::test]
    async fn create_vm_inserts_into_live_vms() {
        let mock = MockKernel::new();
        mock.push_create_vm(Ok(test_vm_handle()));
        let svc = LifeKernelService::new(Arc::new(mock));

        let req = Request::new(pb::CreateVmRequest {
            spec: Some(test_vm_spec_pb()),
            ctx: Some(test_ctx_pb()),
        });
        svc.create_vm(req).await.expect("create_vm should succeed");

        // The VM should now be in the live index.
        let map = svc.live_vms.read().unwrap();
        assert!(
            map.contains_key("vm-test"),
            "vm-test must be in live_vms after create_vm"
        );
    }

    /// `destroy` removes the handle from the live-VM index.
    #[tokio::test]
    async fn destroy_removes_from_live_vms() {
        let mock = MockKernel::new();
        mock.push_destroy(Ok(()));

        // Seed with a pre-existing handle.
        let svc = LifeKernelService::with_seed(Arc::new(mock), vec![test_vm_handle()]);

        // Confirm it's seeded.
        assert!(
            svc.live_vms.read().unwrap().contains_key("vm-test"),
            "vm-test must be in live_vms before destroy"
        );

        let req = Request::new(pb::DestroyRequest {
            vm: Some(test_vm_handle_pb()),
        });
        svc.destroy(req).await.expect("destroy should succeed");

        // Must be removed from the index.
        assert!(
            !svc.live_vms.read().unwrap().contains_key("vm-test"),
            "vm-test must be removed from live_vms after destroy"
        );
    }

    /// `list_vms` returns seeded handles and correctly filters by session_id.
    #[tokio::test]
    async fn list_vms_returns_seeded_handles_and_filters_by_session() {
        let mock = MockKernel::new();
        let seed = vec![
            test_vm_handle_with_session("vm-s1-a", "sess-alpha"),
            test_vm_handle_with_session("vm-s1-b", "sess-alpha"),
            test_vm_handle_with_session("vm-s2-a", "sess-beta"),
        ];
        let svc = LifeKernelService::with_seed(Arc::new(mock), seed);

        // No filter → all three.
        {
            let req = Request::new(pb::ListVmsRequest { session_id: None });
            let resp = svc.list_vms(req).await.expect("list_vms should succeed");
            let mut stream = resp.into_inner();
            let mut count = 0usize;
            while let Some(item) = stream.next().await {
                item.expect("stream item must be Ok");
                count += 1;
            }
            assert_eq!(count, 3, "no filter must return all 3 VMs");
        }

        // Filter by sess-alpha → two.
        {
            let req = Request::new(pb::ListVmsRequest {
                session_id: Some(aios_v1::SessionId {
                    value: "sess-alpha".into(),
                }),
            });
            let resp = svc.list_vms(req).await.expect("list_vms should succeed");
            let mut stream = resp.into_inner();
            let mut count = 0usize;
            while let Some(item) = stream.next().await {
                item.expect("stream item must be Ok");
                count += 1;
            }
            assert_eq!(count, 2, "filter sess-alpha must return 2 VMs");
        }

        // Filter by sess-beta → one.
        {
            let req = Request::new(pb::ListVmsRequest {
                session_id: Some(aios_v1::SessionId {
                    value: "sess-beta".into(),
                }),
            });
            let resp = svc.list_vms(req).await.expect("list_vms should succeed");
            let mut stream = resp.into_inner();
            let mut count = 0usize;
            while let Some(item) = stream.next().await {
                item.expect("stream item must be Ok");
                count += 1;
            }
            assert_eq!(count, 1, "filter sess-beta must return 1 VM");
        }
    }

    /// `dispatch` increments the in-flight counter on entry and decrements it
    /// on exit (including error paths).
    #[tokio::test]
    async fn dispatch_increments_and_decrements_in_flight() {
        let mock = MockKernel::new();
        mock.push_dispatch(Ok(ToolResult {
            call_id: "c-inflight".into(),
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
            "counter must start at zero"
        );

        let call_pb = pb::ToolCall {
            call_id: "c-inflight".into(),
            tool_name: "shell".into(),
            input_json: b"{}".to_vec(),
            requested_capabilities: vec![],
        };
        let req = Request::new(pb::DispatchRequest {
            vm: Some(test_vm_handle_pb()),
            call: Some(call_pb),
            ctx: Some(test_ctx_pb()),
        });
        svc.dispatch(req).await.expect("dispatch should succeed");

        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "counter must return to zero after dispatch completes"
        );
    }

    // ── BRO-899 metric smoke tests ────────────────────────────────────────────
    //
    // These tests verify that the metric handle construction and the
    // record_lifecycle / observe_dispatch call paths do not panic. End-to-end
    // metric pipeline verification (InMemoryMetricsExporter + reader) is done
    // in BRO-903 where the full OTel SDK test harness is wired.

    /// `KernelMetrics` registered inside `with_seed` is accessible via clone
    /// and the `record_lifecycle` call path does not panic.
    #[tokio::test]
    async fn metric_record_lifecycle_smoke() {
        let mock = MockKernel::new();
        mock.push_create_vm(Ok(test_vm_handle()));
        let svc = LifeKernelService::new(Arc::new(mock));

        // Drive create_vm → triggers record_lifecycle("create") internally.
        let req = Request::new(pb::CreateVmRequest {
            spec: Some(test_vm_spec_pb()),
            ctx: Some(test_ctx_pb()),
        });
        svc.create_vm(req)
            .await
            .expect("create_vm should succeed for metric smoke test");
        // No assertion needed — the test passes if no panic occurs.
    }

    /// `observe_dispatch` path is exercised via a successful dispatch call;
    /// verifies the timing path does not panic and the in-flight counter stays
    /// consistent.
    #[tokio::test]
    async fn metric_observe_dispatch_smoke() {
        let mock = MockKernel::new();
        mock.push_dispatch(Ok(ToolResult {
            call_id: "metric-smoke".into(),
            tool_name: "read_file".into(),
            output: serde_json::json!({"content": "hello"}),
            content: None,
            is_error: false,
            usage: None,
        }));
        let svc = LifeKernelService::new(Arc::new(mock));

        let call_pb = pb::ToolCall {
            call_id: "metric-smoke".into(),
            tool_name: "read_file".into(),
            input_json: b"{\"path\":\"/tmp/x\"}".to_vec(),
            requested_capabilities: vec![],
        };
        let req = Request::new(pb::DispatchRequest {
            vm: Some(test_vm_handle_pb()),
            call: Some(call_pb),
            ctx: Some(test_ctx_pb()),
        });
        svc.dispatch(req)
            .await
            .expect("dispatch should succeed for metric smoke test");

        // Counter back at zero after successful dispatch.
        assert_eq!(svc.in_flight().load(Ordering::SeqCst), 0);
    }

    /// All six lifecycle actions flow through `record_lifecycle` without panic.
    #[tokio::test]
    async fn metric_all_lifecycle_actions_smoke() {
        use crate::observability::KernelMetrics;
        let metrics = KernelMetrics::register();
        for action in &[
            "create",
            "destroy",
            "hibernate",
            "resume",
            "snapshot",
            "fork",
        ] {
            metrics.record_lifecycle(action);
        }
    }

    /// `LifeKernelService<E>: Clone` still holds after the metrics field was
    /// added in BRO-899.
    #[tokio::test]
    async fn service_clone_with_metrics_field() {
        let svc = LifeKernelService::new(Arc::new(MockKernel::new()));
        let _clone = svc.clone();
        // Drives `KernelMetrics: Clone` — test passes if no panic.
    }
}
