//! Pure wrapper around
//! [`aios_protocol::hypervisor::HypervisorBackend::exec`] that emits the
//! canonical `KernelDispatchStarted` → `KernelDispatchCompleted` →
//! `KernelUsageRecorded` sequence and computes per-dispatch
//! [`aios_protocol::budget::ResourceUsage`].
//!
//! The wrapper is deliberately thin: it holds only the wrapped backend
//! and a clone of the engine's [`EventEmitter`]. It never accumulates
//! cross-call state, so the engine's observable behaviour remains a
//! pure function of the event journal (see `crate`-level rustdoc).
//!
//! ## Usage semantics
//!
//! For each call to [`MeteringWrapper::dispatch`]:
//!
//! 1. `KernelDispatchStarted { vm_id, call_id, tool_name }` is emitted
//!    *before* dispatching to the inner backend.
//! 2. Wall-clock elapsed is measured via [`std::time::Instant`] across
//!    the `inner.exec(...)` call.
//! 3. A [`ResourceUsage`] is constructed with
//!    [`UsageConfidence::Estimated`]. `duration_ms` reflects the
//!    measured wall clock; `cpu_ms`, `mem_peak_kb`, `egress_bytes`, and
//!    `syscall_count` are populated with `0` in Phase 1 — real rusage /
//!    `/proc/self/stat` integration is a separate Phase 1.5 ticket, see
//!    the inline TODO.
//! 4. On the success path, `KernelDispatchCompleted { call_id, usage,
//!    exit_code }` is emitted, followed by `KernelUsageRecorded {
//!    session_id, wallet, usage }`.
//! 5. On the error path, `KernelDispatchCompleted` is still emitted
//!    with `exit_code = -1` and the elapsed duration so downstream
//!    replay tooling always sees a matched started/completed pair; the
//!    original `BackendError` is then propagated as
//!    [`KernelError::Backend`].

use std::sync::Arc;
use std::time::Instant;

use aios_protocol::budget::{ResourceUsage, UsageConfidence};
use aios_protocol::event::{
    EventKind, KernelDispatchCompleted, KernelDispatchStarted, KernelUsageRecorded,
};
use aios_protocol::hypervisor::{ExecRequest, ExecResult, HypervisorBackend, VmHandle};
use aios_protocol::kernel::{KernelContext, KernelError, KernelResult};

use crate::event_emitter::EventEmitter;

/// Metering wrapper around a concrete
/// [`HypervisorBackend`] — see the module-level rustdoc for the
/// emission protocol.
///
/// Generic over `B: HypervisorBackend` so the underlying backend can be
/// an owned value or any type that implements the trait. The engine
/// wires one wrapper per backend registration.
#[derive(Clone)]
pub struct MeteringWrapper<B: HypervisorBackend> {
    inner: Arc<B>,
    emitter: Arc<EventEmitter>,
}

impl<B: HypervisorBackend> MeteringWrapper<B> {
    /// Wrap `inner` so that every dispatch emits the canonical event
    /// trio through `emitter`.
    pub fn new(inner: Arc<B>, emitter: Arc<EventEmitter>) -> Self {
        Self { inner, emitter }
    }

    /// Borrow the wrapped backend.
    pub fn inner(&self) -> &B {
        &self.inner
    }

    /// Dispatch `req` against `vm`, emitting the canonical metering
    /// events either side of the inner `exec` call.
    ///
    /// Returns `(ExecResult, ResourceUsage)` on success. On backend
    /// failure the `KernelDispatchCompleted` event is still emitted
    /// (with `exit_code = -1`) before the error propagates, so the
    /// event journal always contains a matched started/completed pair
    /// per `call_id`.
    pub async fn dispatch(
        &self,
        vm: &VmHandle,
        req: ExecRequest,
        ctx: &KernelContext,
        call_id: String,
        tool_name: String,
    ) -> KernelResult<(ExecResult, ResourceUsage)> {
        let vm_id = vm.vm_id.clone();

        // 1. Announce the dispatch before we reach the backend.
        let started = self
            .emitter
            .emit(
                EventKind::KernelDispatchStarted(KernelDispatchStarted {
                    vm_id: vm_id.clone(),
                    call_id: call_id.clone(),
                    tool_name: tool_name.clone(),
                }),
                None,
            )
            .await?;

        // 2. Measure wall clock across the backend call.
        let started_at = Instant::now();
        let result = self.inner.exec(vm, req).await;
        let elapsed = started_at.elapsed();
        // Saturate to `u64::MAX` on the theoretical overflow rather
        // than panicking — a single dispatch longer than 584 million
        // years is still a bug, but we prefer the caller sees the
        // event stream than a runtime abort.
        let duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);

        // TODO(Phase 1.5): populate `cpu_ms`, `mem_peak_kb`,
        // `syscall_count` from `getrusage` / `/proc/self/stat`
        // (Linux) and flip `confidence` to `UsageConfidence::Measured`
        // when the data is available. Egress accounting will be wired
        // through `NetworkIsolationPort` once the Phase 1 no-op lands.
        match result {
            Ok(exec_result) => {
                let exit_code = exec_result.exit_code;
                // Prefer the backend's self-reported duration when it
                // carries one (some backends time inside the sandbox
                // more precisely than our outer wall clock); otherwise
                // fall back to the measured wall clock.
                let reported_duration_ms = if exec_result.duration_ms != 0 {
                    exec_result.duration_ms
                } else {
                    duration_ms
                };
                let usage = ResourceUsage {
                    cpu_ms: 0,
                    mem_peak_kb: 0,
                    egress_bytes: 0,
                    duration_ms: reported_duration_ms,
                    syscall_count: 0,
                    confidence: UsageConfidence::Estimated,
                };

                // 4. Completed.
                let completed = self
                    .emitter
                    .emit(
                        EventKind::KernelDispatchCompleted(KernelDispatchCompleted {
                            call_id: call_id.clone(),
                            usage: usage.clone(),
                            exit_code,
                        }),
                        Some(started.event_id.clone()),
                    )
                    .await?;

                // 5. Usage recorded (attribute to ctx.wallet).
                self.emitter
                    .emit(
                        EventKind::KernelUsageRecorded(KernelUsageRecorded {
                            session_id: ctx.session_id.clone(),
                            wallet: ctx.wallet.clone(),
                            usage: usage.clone(),
                        }),
                        Some(completed.event_id.clone()),
                    )
                    .await?;

                Ok((exec_result, usage))
            }
            Err(backend_err) => {
                // On backend error we still emit a completion so the
                // started/completed pair is always balanced.
                // `exit_code = -1` is the sentinel documented in the
                // module rustdoc; duration reflects the measured wall
                // clock up to the backend failure.
                let usage = ResourceUsage {
                    cpu_ms: 0,
                    mem_peak_kb: 0,
                    egress_bytes: 0,
                    duration_ms,
                    syscall_count: 0,
                    confidence: UsageConfidence::Estimated,
                };
                // Best-effort emission — if the store itself fails we
                // propagate the backend error (which is the more useful
                // diagnostic) rather than swallowing it behind a
                // secondary store error.
                let _ = self
                    .emitter
                    .emit(
                        EventKind::KernelDispatchCompleted(KernelDispatchCompleted {
                            call_id,
                            usage,
                            exit_code: -1,
                        }),
                        Some(started.event_id),
                    )
                    .await;

                Err(KernelError::Backend(backend_err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    use std::time::Duration;

    use aios_protocol::error::KernelResult as LegacyKernelResult;
    use aios_protocol::event::EventRecord;
    use aios_protocol::hypervisor::{
        BackendCapabilitySet, BackendError, BackendId, VmId, VmSnapshotId, VmSpec, VmStatus,
    };
    use aios_protocol::ids::{AgentId, BranchId, SeqNo, SessionId};
    use aios_protocol::kernel::{ChainId, WalletAttribution};
    use aios_protocol::ports::{EventRecordStream, EventStorePort};
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};

    use crate::event_emitter::{Clock, EventEmitter};

    // ── StubEventStore (mirrors the one in event_emitter.rs) ────────

    struct StubEventStore {
        events: Mutex<Vec<EventRecord>>,
    }

    impl StubEventStore {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Vec::new()),
            })
        }

        fn stored_events(&self) -> Vec<EventRecord> {
            self.events.lock().expect("poisoned mutex").clone()
        }
    }

    #[async_trait]
    impl EventStorePort for StubEventStore {
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
            Ok(self.stored_events())
        }

        async fn head(
            &self,
            _session_id: SessionId,
            _branch_id: BranchId,
        ) -> LegacyKernelResult<u64> {
            Ok(self.stored_events().len() as u64)
        }

        async fn subscribe(
            &self,
            _session_id: SessionId,
            _branch_id: BranchId,
            _after_sequence: u64,
        ) -> LegacyKernelResult<EventRecordStream> {
            unimplemented!("subscribe not used in metering tests")
        }
    }

    // ── StubBackend: sleeps 50ms, returns a canned ExecResult ───────

    struct StubBackend {
        behaviour: StubBehaviour,
    }

    enum StubBehaviour {
        Ok {
            stdout: Vec<u8>,
            exit_code: i32,
            /// If `Some`, the backend puts this value on
            /// `ExecResult::duration_ms`. Used to verify the metering
            /// wrapper prefers the backend's self-reported duration
            /// when present.
            backend_reported_duration_ms: Option<u64>,
        },
        Err(&'static str),
    }

    impl StubBackend {
        fn ok(stdout: &'static [u8], exit_code: i32) -> Arc<Self> {
            Arc::new(Self {
                behaviour: StubBehaviour::Ok {
                    stdout: stdout.to_vec(),
                    exit_code,
                    backend_reported_duration_ms: None,
                },
            })
        }

        fn err(msg: &'static str) -> Arc<Self> {
            Arc::new(Self {
                behaviour: StubBehaviour::Err(msg),
            })
        }
    }

    #[async_trait]
    impl HypervisorBackend for StubBackend {
        fn name(&self) -> &'static str {
            "stub"
        }

        fn capabilities(&self) -> BackendCapabilitySet {
            BackendCapabilitySet::FILESYSTEM_READ
        }

        async fn create(&self, _spec: VmSpec) -> Result<VmHandle, BackendError> {
            Ok(canned_handle())
        }

        async fn exec(
            &self,
            _vm: &VmHandle,
            _req: ExecRequest,
        ) -> Result<ExecResult, BackendError> {
            // Sleep so the wall-clock duration is measurably nonzero
            // — 50ms per the ticket spec.
            tokio::time::sleep(Duration::from_millis(50)).await;
            match &self.behaviour {
                StubBehaviour::Ok {
                    stdout,
                    exit_code,
                    backend_reported_duration_ms,
                } => Ok(ExecResult {
                    stdout: stdout.clone(),
                    stderr: Vec::new(),
                    exit_code: *exit_code,
                    duration_ms: backend_reported_duration_ms.unwrap_or(0),
                }),
                StubBehaviour::Err(msg) => Err(BackendError::Internal((*msg).into())),
            }
        }

        async fn snapshot(&self, _vm: &VmHandle) -> Result<VmSnapshotId, BackendError> {
            Ok(VmSnapshotId::from("stub-snap"))
        }

        async fn restore(&self, _snapshot: &VmSnapshotId) -> Result<VmHandle, BackendError> {
            Ok(canned_handle())
        }

        async fn destroy(&self, _vm: &VmHandle) -> Result<(), BackendError> {
            Ok(())
        }
    }

    // ── Shared fixtures ─────────────────────────────────────────────

    fn canned_handle() -> VmHandle {
        VmHandle {
            vm_id: VmId::from("vm-1"),
            backend: BackendId::from("stub"),
            session_id: SessionId::from_string("sess-1"),
            agent_id: AgentId::from_string("agent-1"),
            status: VmStatus::Running,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }

    fn canned_ctx() -> KernelContext {
        KernelContext {
            session_id: SessionId::from_string("sess-1"),
            agent_id: AgentId::from_string("agent-1"),
            wallet: WalletAttribution {
                address: "0xabc".into(),
                chain: ChainId::base(),
            },
            cost_hint: None,
            trace_ctx: None,
        }
    }

    fn frozen_clock() -> Clock {
        let fixed = Utc.with_ymd_and_hms(2026, 4, 23, 12, 0, 0).unwrap();
        Arc::new(move || fixed)
    }

    fn build_emitter(store: Arc<StubEventStore>) -> Arc<EventEmitter> {
        match EventEmitter::builder(store)
            .session(SessionId::from_string("sess-1"))
            .agent(AgentId::from_string("agent-1"))
            .clock(frozen_clock())
            .build()
        {
            Ok(e) => Arc::new(e),
            Err(e) => panic!("builder failed: {e:?}"),
        }
    }

    fn ok_result<T, E: std::fmt::Debug>(label: &str, r: Result<T, E>) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("{label}: expected Ok, got {e:?}"),
        }
    }

    // ── Tests ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn metering_emits_started_then_completed() {
        let store = StubEventStore::new();
        let emitter = build_emitter(store.clone());
        let wrapper = MeteringWrapper::new(StubBackend::ok(b"ok", 0), emitter);

        ok_result(
            "dispatch",
            wrapper
                .dispatch(
                    &canned_handle(),
                    ExecRequest::shell("true"),
                    &canned_ctx(),
                    "call-1".into(),
                    "tool.test".into(),
                )
                .await,
        );

        let stored = store.stored_events();
        // Expect 3 events in order: Started, Completed, UsageRecorded.
        assert_eq!(stored.len(), 3, "stored events: {stored:#?}");
        match &stored[0].kind {
            EventKind::KernelDispatchStarted(p) => {
                assert_eq!(p.call_id, "call-1");
                assert_eq!(p.tool_name, "tool.test");
                assert_eq!(p.vm_id, VmId::from("vm-1"));
            }
            other => panic!("expected KernelDispatchStarted, got {other:?}"),
        }
        match &stored[1].kind {
            EventKind::KernelDispatchCompleted(p) => {
                assert_eq!(p.call_id, "call-1");
                assert_eq!(p.exit_code, 0);
            }
            other => panic!("expected KernelDispatchCompleted, got {other:?}"),
        }
        match &stored[2].kind {
            EventKind::KernelUsageRecorded(_) => {}
            other => panic!("expected KernelUsageRecorded, got {other:?}"),
        }
        // Causation chain: completed ← started, usage ← completed.
        assert_eq!(stored[1].causation_id, Some(stored[0].event_id.clone()));
        assert_eq!(stored[2].causation_id, Some(stored[1].event_id.clone()));
    }

    #[tokio::test]
    async fn metering_populates_duration_ms() {
        let store = StubEventStore::new();
        let emitter = build_emitter(store.clone());
        let wrapper = MeteringWrapper::new(StubBackend::ok(b"ok", 0), emitter);

        let (_exec, usage) = ok_result(
            "dispatch",
            wrapper
                .dispatch(
                    &canned_handle(),
                    ExecRequest::shell("true"),
                    &canned_ctx(),
                    "call-1".into(),
                    "tool.test".into(),
                )
                .await,
        );

        // StubBackend sleeps 50ms, so duration_ms must be >= 50 and <
        // some generous ceiling (anti-flake).
        assert!(
            usage.duration_ms >= 40,
            "duration_ms {} below the 40ms floor",
            usage.duration_ms
        );
        assert!(
            usage.duration_ms < 5_000,
            "duration_ms {} too large — timing bug?",
            usage.duration_ms
        );
        assert_eq!(usage.confidence, UsageConfidence::Estimated);
        assert_eq!(usage.cpu_ms, 0);
        assert_eq!(usage.mem_peak_kb, 0);
        assert_eq!(usage.egress_bytes, 0);
        assert_eq!(usage.syscall_count, 0);

        // The same duration should surface on the Completed event.
        let stored = store.stored_events();
        match &stored[1].kind {
            EventKind::KernelDispatchCompleted(p) => {
                assert_eq!(p.usage.duration_ms, usage.duration_ms);
            }
            other => panic!("expected KernelDispatchCompleted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn metering_propagates_backend_error_and_emits_completion() {
        let store = StubEventStore::new();
        let emitter = build_emitter(store.clone());
        let wrapper = MeteringWrapper::new(StubBackend::err("boom"), emitter);

        let result = wrapper
            .dispatch(
                &canned_handle(),
                ExecRequest::shell("false"),
                &canned_ctx(),
                "call-err".into(),
                "tool.fail".into(),
            )
            .await;

        match result {
            Err(KernelError::Backend(BackendError::Internal(msg))) => {
                assert_eq!(msg, "boom");
            }
            Err(other) => panic!("expected Backend(Internal), got {other:?}"),
            Ok(_) => panic!("expected an error"),
        }

        // Even on error the started/completed pair must be emitted;
        // UsageRecorded is intentionally NOT emitted on the error
        // path (nothing valid to attribute).
        let stored = store.stored_events();
        assert_eq!(stored.len(), 2, "events: {stored:#?}");
        match &stored[0].kind {
            EventKind::KernelDispatchStarted(p) => assert_eq!(p.call_id, "call-err"),
            other => panic!("expected Started, got {other:?}"),
        }
        match &stored[1].kind {
            EventKind::KernelDispatchCompleted(p) => {
                assert_eq!(p.call_id, "call-err");
                // Sentinel exit code on error path.
                assert_eq!(p.exit_code, -1);
                // Duration still populated from the wall clock.
                assert!(p.usage.duration_ms >= 40);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn metering_attributes_wallet_in_usage_event() {
        let store = StubEventStore::new();
        let emitter = build_emitter(store.clone());
        let wrapper = MeteringWrapper::new(StubBackend::ok(b"ok", 0), emitter);

        let mut ctx = canned_ctx();
        ctx.wallet = WalletAttribution {
            address: "0xdead".into(),
            chain: ChainId::from_caip2("eip155:10"),
        };
        ctx.session_id = SessionId::from_string("sess-wallet");

        ok_result(
            "dispatch",
            wrapper
                .dispatch(
                    &canned_handle(),
                    ExecRequest::shell("true"),
                    &ctx,
                    "call-w".into(),
                    "tool.wallet".into(),
                )
                .await,
        );

        let stored = store.stored_events();
        assert_eq!(stored.len(), 3);
        match &stored[2].kind {
            EventKind::KernelUsageRecorded(p) => {
                assert_eq!(p.wallet.address, "0xdead");
                assert_eq!(p.wallet.chain, ChainId::from_caip2("eip155:10"));
                assert_eq!(p.session_id, SessionId::from_string("sess-wallet"));
                // The usage payload carried on the UsageRecorded event
                // must mirror the one on the preceding Completed event
                // — the wrapper emits a consistent snapshot.
                let completed_usage = match &stored[1].kind {
                    EventKind::KernelDispatchCompleted(c) => c.usage.clone(),
                    other => panic!("expected Completed at index 1, got {other:?}"),
                };
                assert_eq!(p.usage, completed_usage);
            }
            other => panic!("expected UsageRecorded at index 2, got {other:?}"),
        }
    }
}
