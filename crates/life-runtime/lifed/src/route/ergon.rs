//! Ergon route — name-keyed dispatch for `Agent.StreamSession` invocations
//! that target an `ergon::Workflow` running inside the substrate.
//!
//! ## Why this lives in lifed
//!
//! `lifed` is the public-plane facade for the Life Runtime. When a
//! caller hits `Agent.StreamSession({agent: "bookkeeping.promotion-judge", ...})`
//! lifed must:
//!
//! 1. Authenticate the capability token (handled by [`crate::auth`]).
//! 2. Look up the named workflow in a registry.
//! 3. Start it and stream its [`ergon::StreamEvent`] outputs back to
//!    the caller as `life.v1.AgentEvent` frames.
//!
//! Step 2 is the resolution this module owns. Step 3 is the execution
//! glue: this module defines a minimal [`ErgonWorkflowHandle`] trait
//! whose `start` method returns a stream of `life.v1.AgentEvent`. The
//! concrete implementation that runs an `ergon::Workflow` body inside
//! arcan-ergon lives outside this crate (Spec C₂ §11 forbids lifed from
//! depending on `arcan-core` / `aios-runtime`); that out-of-tree
//! implementation supplies a concrete [`ErgonRegistry`] at daemon
//! bootstrap.
//!
//! ## Wire shape
//!
//! The route emits `life.v1.AgentEvent` records — the same wire type
//! produced by [`crate::services::agent::AgentService::stream_session`].
//! That keeps downstream consumers (`lifegw` SSE encoding, the JS SDK,
//! `chatOS`) on one canonical stream type regardless of whether the
//! agent body is a free-form arcan dispatch or a typed ergon workflow.
//!
//! For BRO-1002 the route returns the canonical
//! [`pb::AgentEventKind::Token`] / `Finish` / `Error` taxonomy — the
//! full `StreamEvent` → `AgentEvent` mapping (reasoning, tool-use,
//! citation, structured output, usage) lands with the bookkeeping-judge
//! port in BRO-1003 once a concrete workflow exercises every variant.
//! `StreamEvent` carries a `serde_json::Value`-ready payload in every
//! variant, so the mapping is a straight `serde_json::to_vec` into
//! `EventRecord.payload`; this route stays additive when that mapping
//! lands.
//!
//! ## Spec & tracker
//!
//! - Spec: `docs/superpowers/specs/2026-05-05-ergon-v0.1.md` §12.8
//! - Linear: [BRO-1002](https://linear.app/broomva/issue/BRO-1002)

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use tonic::Status;

use aios_proto::aios::v1 as aios_v1;
use ergon::StreamEvent;
use life_runtime_proto::life::v1 as pb;

/// Server-streamed event flow returned by [`handle_stream_session`].
///
/// One canonical alias for every site that produces or consumes a
/// substrate-bridged stream of [`pb::AgentEvent`].
pub type EventStream = Pin<Box<dyn Stream<Item = Result<pb::AgentEvent, Status>> + Send>>;

/// Errors that may surface while resolving and starting an ergon
/// workflow. Each variant maps to a single canonical [`tonic::Status`]
/// code at the RPC boundary; the mapping is the only place an HTTP-ish
/// status lookup happens for ergon-routed sessions.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RouteError {
    /// No workflow registered under the requested name.
    #[error("ergon workflow `{name}` not found")]
    NotFound {
        /// The requested workflow name.
        name: String,
    },

    /// Caller supplied a malformed input — for ergon workflows this is
    /// almost always a `serde_json::Value` that fails to deserialize
    /// into the workflow's typed `Input`. Reported by the substrate-side
    /// implementation; lifed surfaces it verbatim.
    #[error("ergon workflow `{name}` invalid input: {reason}")]
    InvalidInput {
        /// Workflow name.
        name: String,
        /// Human-readable reason (typically a serde-json error string).
        reason: String,
    },

    /// The workflow started but failed during execution. lifed does not
    /// attempt to classify between hook denials, provider errors, or
    /// internal failures — the substrate-side adapter is responsible
    /// for that classification when it constructs the message. Maps to
    /// `Status::aborted` at the RPC boundary so the caller can
    /// distinguish from a transient `unavailable`.
    #[error("ergon workflow `{name}` failed: {message}")]
    WorkflowFailed {
        /// Workflow name.
        name: String,
        /// Human-readable failure message.
        message: String,
    },

    /// The substrate is unreachable (e.g. the arcan adapter isn't
    /// installed, the dispatcher channel is closed). This is the
    /// transient peer of [`Self::WorkflowFailed`] — callers should
    /// retry.
    #[error("ergon substrate unavailable: {0}")]
    SubstrateUnavailable(String),
}

impl From<RouteError> for Status {
    fn from(err: RouteError) -> Self {
        match err {
            RouteError::NotFound { .. } => Status::not_found(err.to_string()),
            RouteError::InvalidInput { .. } => Status::invalid_argument(err.to_string()),
            RouteError::WorkflowFailed { .. } => Status::aborted(err.to_string()),
            RouteError::SubstrateUnavailable(_) => Status::unavailable(err.to_string()),
        }
    }
}

/// Request the route handler operates on.
///
/// This is a route-layer DTO — it does not extend the wire `SessionRef`
/// type from `life.v1`. When the proto evolves to carry an agent name
/// alongside the session id (BRO-1003 / spec criterion 7), the service
/// handler in [`crate::services::agent`] will extract the relevant
/// fields and build this struct.
#[derive(Debug, Clone)]
pub struct StreamSessionRequest {
    /// Stable name of the registered workflow (e.g.
    /// `"bookkeeping.promotion-judge"`).
    pub agent_name: String,
    /// Session id the caller is binding to.
    pub sid: aios_v1::SessionId,
    /// Typed input the substrate-side adapter passes to
    /// [`ergon::Workflow::execute`] via `serde_json::from_value`.
    /// Validation that the JSON deserializes is deferred to the
    /// concrete [`ErgonWorkflowHandle`] implementation (it owns the
    /// associated `Input` type).
    pub input: serde_json::Value,
}

/// Boxed handle to a registered workflow.
///
/// Type-erases over `ergon::WorkflowExecutor<W>`'s generic associated
/// types so the registry can store any workflow under a string name.
/// The substrate-side implementation (in arcan-ergon, out of tree) wraps
/// each concrete `ergon::Workflow` and returns the JSON-in / stream-out
/// surface this trait exposes.
#[async_trait]
pub trait ErgonWorkflowHandle: Send + Sync {
    /// Stable workflow name. Used in tracing spans + the
    /// [`RouteError::NotFound`] message.
    fn name(&self) -> &str;

    /// Start the workflow against the supplied session and input,
    /// returning a server-stream of `life.v1.AgentEvent` frames.
    ///
    /// The substrate-side implementation typically:
    ///
    /// 1. `serde_json::from_value::<W::Input>(input)` — surfacing
    ///    [`RouteError::InvalidInput`] on failure.
    /// 2. Spawns a tokio task that drives the workflow body, mapping
    ///    [`ergon::StreamEvent`]s to [`pb::AgentEvent`]s on the way out.
    /// 3. Returns the receiver end as a `Stream`.
    ///
    /// If the substrate isn't installed, return
    /// [`RouteError::SubstrateUnavailable`].
    async fn start(
        &self,
        sid: aios_v1::SessionId,
        input: serde_json::Value,
    ) -> Result<EventStream, RouteError>;
}

/// Name-keyed registry of [`ErgonWorkflowHandle`]s.
///
/// The lifed daemon holds an `Arc<dyn ErgonRegistry>` in its
/// [`LifedContext`]; at boot the daemon receives a concrete registry
/// from the substrate-side adapter (or an [`InMemoryErgonRegistry`] in
/// tests).
pub trait ErgonRegistry: Send + Sync {
    /// Look up a registered workflow handle by name.
    ///
    /// Returns [`RouteError::NotFound`] when no workflow is registered
    /// under that name.
    fn resolve(&self, name: &str) -> Result<Arc<dyn ErgonWorkflowHandle>, RouteError>;

    /// Snapshot of every registered name, sorted. Used by operator
    /// introspection (`admin/runtime` list-skills surface) and by
    /// tests that want to assert wiring.
    fn known_names(&self) -> Vec<String>;
}

/// Minimal route-level context handed to [`handle_stream_session`].
///
/// `lifed`'s service layer already aggregates a much broader context
/// (keystore, saga driver, routing cache, substrate proxies — see
/// [`crate::services::agent::AgentService`]). The route layer only needs
/// the registry plus the ambient tracing span so it stays
/// substrate-agnostic. The service handler (in a follow-up PR) is
/// responsible for assembling this struct.
pub struct LifedContext {
    /// Workflow registry installed at daemon bootstrap.
    pub ergon_registry: Arc<dyn ErgonRegistry>,
    /// Ambient tracing span. The handler attaches its own span as a
    /// child via [`tracing::Span::enter`]; provided here so callers can
    /// thread a request-scope span through.
    pub trace: tracing::Span,
}

impl LifedContext {
    /// Construct a context from a registry. The trace defaults to the
    /// current span — callers can override by mutating the field if
    /// they prefer their own.
    pub fn new(ergon_registry: Arc<dyn ErgonRegistry>) -> Self {
        Self {
            ergon_registry,
            trace: tracing::Span::current(),
        }
    }
}

/// Resolve the named ergon workflow and start its event stream.
///
/// This is the entry point spec §12.8 commits us to land. The function
/// body is the canonical three-step shape from the spec skeleton:
///
/// 1. Resolve the workflow handle from the registry.
/// 2. Start it against the request session id + input.
/// 3. Return the resulting [`EventStream`].
///
/// Errors propagate via [`RouteError`]; callers convert to
/// [`tonic::Status`] at the gRPC boundary.
pub async fn handle_stream_session(
    req: StreamSessionRequest,
    ctx: &LifedContext,
) -> Result<EventStream, RouteError> {
    let _enter = ctx.trace.enter();
    tracing::debug!(
        agent_name = %req.agent_name,
        sid = %req.sid.value,
        "ergon: resolving workflow",
    );
    let workflow = ctx.ergon_registry.resolve(&req.agent_name)?;
    let stream = workflow.start(req.sid, req.input).await?;
    Ok(stream)
}

// ─── In-memory registry (tests + dev) ────────────────────────────────

/// In-process registry used by tests and the dev-mode daemon.
///
/// Workflows are inserted via [`Self::register`]; lookups are O(1)
/// against a [`dashmap::DashMap`] — registration is rare (typically
/// happens once at boot) so a write lock is acceptable.
#[derive(Default)]
pub struct InMemoryErgonRegistry {
    entries: dashmap::DashMap<String, Arc<dyn ErgonWorkflowHandle>>,
}

impl InMemoryErgonRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a workflow handle. Returns the prior entry (if any) so
    /// callers can detect accidental shadowing. Most callers will treat
    /// `Some(_)` as a programmer error during bootstrap.
    pub fn register(
        &self,
        handle: Arc<dyn ErgonWorkflowHandle>,
    ) -> Option<Arc<dyn ErgonWorkflowHandle>> {
        let name = handle.name().to_owned();
        self.entries.insert(name, handle)
    }
}

impl ErgonRegistry for InMemoryErgonRegistry {
    fn resolve(&self, name: &str) -> Result<Arc<dyn ErgonWorkflowHandle>, RouteError> {
        self.entries
            .get(name)
            .map(|e| Arc::clone(e.value()))
            .ok_or_else(|| RouteError::NotFound {
                name: name.to_owned(),
            })
    }

    fn known_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.entries.iter().map(|e| e.key().clone()).collect();
        v.sort();
        v
    }
}

// ─── StreamEvent → AgentEvent mapping helper ─────────────────────────

/// Map a single [`ergon::StreamEvent`] to a [`pb::AgentEvent`].
///
/// The mapping is intentionally coarse for BRO-1002: every event maps
/// onto one of the canonical `AgentEventKind` variants, with the full
/// `StreamEvent` body serialized as JSON into `EventRecord.payload`.
/// Once a concrete workflow (bookkeeping-judge / BRO-1003) exercises
/// every `StreamEvent` variant in production, the substrate-side
/// adapter can refine the kind tagging per variant.
///
/// Returns `Err` only when JSON serialization fails — which is itself a
/// programmer error (every `StreamEvent` variant derives `Serialize`),
/// so the error is reported as `Status::internal`.
pub fn stream_event_to_agent_event(
    event: &StreamEvent,
    sid: &aios_v1::SessionId,
    sequence: u64,
) -> Result<pb::AgentEvent, Status> {
    let payload = serde_json::to_vec(event)
        .map_err(|e| Status::internal(format!("StreamEvent serialization failed: {e}")))?;
    let kind = match event {
        StreamEvent::TextStart { .. }
        | StreamEvent::TextDelta { .. }
        | StreamEvent::TextEnd { .. }
        | StreamEvent::ReasoningStart { .. }
        | StreamEvent::ReasoningDelta { .. }
        | StreamEvent::ReasoningEnd { .. }
        | StreamEvent::StructuredStart { .. }
        | StreamEvent::StructuredDelta { .. }
        | StreamEvent::StructuredEnd { .. }
        | StreamEvent::Citation { .. }
        | StreamEvent::Source { .. }
        | StreamEvent::Usage { .. }
        | StreamEvent::SessionStart { .. } => pb::AgentEventKind::Token,
        StreamEvent::ToolUseStart { .. }
        | StreamEvent::ToolUseInputDelta { .. }
        | StreamEvent::ToolUseEnd { .. } => pb::AgentEventKind::ToolCallPending,
        StreamEvent::Done { .. } => pb::AgentEventKind::Finish,
        StreamEvent::Error { .. } => pb::AgentEventKind::Error,
        // `StreamEvent` is `#[non_exhaustive]` — fall back to the
        // generic Token kind for variants we don't yet recognize.
        // The full body is still in `payload`, so future SDKs can
        // discriminate by inspecting the `event` field.
        _ => pb::AgentEventKind::Unspecified,
    };
    let record = pb::EventRecord {
        session_id: Some(sid.clone()),
        sequence,
        at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        kind: stream_event_kind_tag(event).to_owned(),
        payload,
    };
    Ok(pb::AgentEvent {
        record: Some(record),
        kind: kind.into(),
    })
}

/// Free-form string tag for the `EventRecord.kind` field. Mirrors the
/// `serde(tag = "event", rename_all = "snake_case")` discriminator on
/// [`StreamEvent`] so the wire and the typed enum stay aligned without
/// pulling serde across the FFI boundary.
fn stream_event_kind_tag(event: &StreamEvent) -> &'static str {
    match event {
        StreamEvent::SessionStart { .. } => "session_start",
        StreamEvent::TextStart { .. } => "text_start",
        StreamEvent::TextDelta { .. } => "text_delta",
        StreamEvent::TextEnd { .. } => "text_end",
        StreamEvent::ReasoningStart { .. } => "reasoning_start",
        StreamEvent::ReasoningDelta { .. } => "reasoning_delta",
        StreamEvent::ReasoningEnd { .. } => "reasoning_end",
        StreamEvent::ToolUseStart { .. } => "tool_use_start",
        StreamEvent::ToolUseInputDelta { .. } => "tool_use_input_delta",
        StreamEvent::ToolUseEnd { .. } => "tool_use_end",
        StreamEvent::StructuredStart { .. } => "structured_start",
        StreamEvent::StructuredDelta { .. } => "structured_delta",
        StreamEvent::StructuredEnd { .. } => "structured_end",
        StreamEvent::Citation { .. } => "citation",
        StreamEvent::Source { .. } => "source",
        StreamEvent::Usage { .. } => "usage",
        StreamEvent::Done { .. } => "done",
        StreamEvent::Error { .. } => "error",
        // `#[non_exhaustive]` — future variants surface as `unknown`
        // until the wire tagging is extended.
        _ => "unknown",
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ergon::StopReason;
    use futures::StreamExt;
    use futures::stream;

    /// Fake handle that emits a fixed sequence of events.
    struct EchoHandle {
        name: &'static str,
        events: Vec<StreamEvent>,
    }

    #[async_trait]
    impl ErgonWorkflowHandle for EchoHandle {
        fn name(&self) -> &str {
            self.name
        }

        async fn start(
            &self,
            sid: aios_v1::SessionId,
            _input: serde_json::Value,
        ) -> Result<EventStream, RouteError> {
            let events = self.events.clone();
            let mut seq = 0u64;
            let mapped: Vec<Result<pb::AgentEvent, Status>> = events
                .into_iter()
                .map(|e| {
                    let r = stream_event_to_agent_event(&e, &sid, seq);
                    seq += 1;
                    r
                })
                .collect();
            Ok(Box::pin(stream::iter(mapped)))
        }
    }

    /// Fake handle that always errors at start time.
    struct FailingHandle;

    #[async_trait]
    impl ErgonWorkflowHandle for FailingHandle {
        fn name(&self) -> &str {
            "failing"
        }

        async fn start(
            &self,
            _sid: aios_v1::SessionId,
            _input: serde_json::Value,
        ) -> Result<EventStream, RouteError> {
            Err(RouteError::WorkflowFailed {
                name: "failing".into(),
                message: "intentional".into(),
            })
        }
    }

    fn sid(s: &str) -> aios_v1::SessionId {
        aios_v1::SessionId {
            value: s.to_owned(),
        }
    }

    fn make_registry(handle: Arc<dyn ErgonWorkflowHandle>) -> Arc<dyn ErgonRegistry> {
        let r = InMemoryErgonRegistry::new();
        let prior = r.register(handle);
        assert!(prior.is_none(), "fresh registry must not shadow");
        Arc::new(r)
    }

    fn ctx_with(registry: Arc<dyn ErgonRegistry>) -> LifedContext {
        LifedContext::new(registry)
    }

    #[tokio::test]
    async fn happy_path_streams_events_in_order() {
        let events = vec![
            StreamEvent::TextStart { id: "t1".into() },
            StreamEvent::TextDelta {
                id: "t1".into(),
                delta: "hello".into(),
            },
            StreamEvent::TextEnd { id: "t1".into() },
            StreamEvent::Done {
                stop_reason: StopReason::EndTurn,
            },
        ];
        let handle: Arc<dyn ErgonWorkflowHandle> = Arc::new(EchoHandle {
            name: "echo",
            events,
        });
        let registry = make_registry(handle);
        let ctx = ctx_with(registry);

        let req = StreamSessionRequest {
            agent_name: "echo".into(),
            sid: sid("01ABC"),
            input: serde_json::json!({}),
        };
        // `EventStream` is a trait object (no `Debug`); avoid `expect`.
        let stream = match handle_stream_session(req, &ctx).await {
            Ok(s) => s,
            Err(e) => panic!("happy path stream must start: {e:?}"),
        };
        let collected: Vec<_> = stream.collect().await;
        assert_eq!(collected.len(), 4);
        // Every frame carries the same sid + a monotone sequence.
        for (i, frame) in collected.iter().enumerate() {
            let frame = frame.as_ref().expect("frame ok");
            let record = frame.record.as_ref().expect("record present");
            assert_eq!(record.sequence, i as u64);
            assert_eq!(record.session_id.as_ref().expect("sid").value, "01ABC");
        }
        // First frame is text_start; last is done.
        let first_kind = collected[0]
            .as_ref()
            .ok()
            .and_then(|f| f.record.as_ref())
            .map(|r| r.kind.clone())
            .unwrap_or_default();
        assert_eq!(first_kind, "text_start");
        let last_kind = collected
            .last()
            .and_then(|r| r.as_ref().ok())
            .and_then(|f| f.record.as_ref())
            .map(|r| r.kind.clone())
            .unwrap_or_default();
        assert_eq!(last_kind, "done");
        // Final frame should be marked Finish at the proto-kind level.
        let last_proto_kind = collected
            .last()
            .and_then(|r| r.as_ref().ok())
            .map(|f| f.kind)
            .unwrap_or_default();
        assert_eq!(last_proto_kind, pb::AgentEventKind::Finish as i32);
    }

    #[tokio::test]
    async fn unknown_agent_returns_not_found() {
        let registry: Arc<dyn ErgonRegistry> = Arc::new(InMemoryErgonRegistry::new());
        let ctx = ctx_with(registry);
        let req = StreamSessionRequest {
            agent_name: "ghost".into(),
            sid: sid("01ABC"),
            input: serde_json::json!({}),
        };
        // `EventStream` is a trait object (no `Debug`); use a match
        // instead of `expect_err`.
        let err = match handle_stream_session(req, &ctx).await {
            Ok(_) => panic!("expected NotFound"),
            Err(e) => e,
        };
        match &err {
            RouteError::NotFound { name } => assert_eq!(name, "ghost"),
            other => panic!("expected NotFound, got {other:?}"),
        }
        let status: Status = err.into();
        assert_eq!(status.code(), tonic::Code::NotFound);
        assert!(status.message().contains("ghost"));
    }

    #[tokio::test]
    async fn workflow_failure_propagates_as_aborted() {
        let handle: Arc<dyn ErgonWorkflowHandle> = Arc::new(FailingHandle);
        let registry = make_registry(handle);
        let ctx = ctx_with(registry);
        let req = StreamSessionRequest {
            agent_name: "failing".into(),
            sid: sid("01ABC"),
            input: serde_json::json!({}),
        };
        // `EventStream` is a trait object (no `Debug`); use a match.
        let err = match handle_stream_session(req, &ctx).await {
            Ok(_) => panic!("expected WorkflowFailed"),
            Err(e) => e,
        };
        match &err {
            RouteError::WorkflowFailed { name, message } => {
                assert_eq!(name, "failing");
                assert!(message.contains("intentional"));
            }
            other => panic!("expected WorkflowFailed, got {other:?}"),
        }
        let status: Status = err.into();
        assert_eq!(status.code(), tonic::Code::Aborted);
    }

    #[test]
    fn known_names_are_sorted_after_registration() {
        let r = InMemoryErgonRegistry::new();
        r.register(Arc::new(EchoHandle {
            name: "zeta",
            events: Vec::new(),
        }));
        r.register(Arc::new(EchoHandle {
            name: "alpha",
            events: Vec::new(),
        }));
        r.register(Arc::new(EchoHandle {
            name: "mu",
            events: Vec::new(),
        }));
        assert_eq!(r.known_names(), vec!["alpha", "mu", "zeta"]);
    }

    #[test]
    fn duplicate_registration_returns_prior_entry() {
        let r = InMemoryErgonRegistry::new();
        let h1: Arc<dyn ErgonWorkflowHandle> = Arc::new(EchoHandle {
            name: "echo",
            events: Vec::new(),
        });
        let h2: Arc<dyn ErgonWorkflowHandle> = Arc::new(EchoHandle {
            name: "echo",
            events: vec![StreamEvent::TextStart { id: "t".into() }],
        });
        assert!(r.register(h1).is_none());
        let prior = r.register(h2);
        assert!(prior.is_some(), "second insert returns prior");
    }

    #[test]
    fn invalid_input_maps_to_invalid_argument() {
        let err = RouteError::InvalidInput {
            name: "echo".into(),
            reason: "expected object".into(),
        };
        let status: Status = err.into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert!(status.message().contains("echo"));
        assert!(status.message().contains("expected object"));
    }

    #[test]
    fn substrate_unavailable_maps_to_unavailable() {
        let err = RouteError::SubstrateUnavailable("dispatcher closed".into());
        let status: Status = err.into();
        assert_eq!(status.code(), tonic::Code::Unavailable);
    }

    #[test]
    fn stream_event_to_agent_event_tags_tool_use_as_tool_call_pending() {
        let evt = StreamEvent::ToolUseStart {
            id: "t1".into(),
            name: "bash".into(),
        };
        let sid = sid("01ABC");
        let frame = stream_event_to_agent_event(&evt, &sid, 7).expect("ok");
        assert_eq!(frame.kind, pb::AgentEventKind::ToolCallPending as i32);
        let record = frame.record.expect("record");
        assert_eq!(record.sequence, 7);
        assert_eq!(record.kind, "tool_use_start");
        // Payload must round-trip through serde_json.
        let parsed: serde_json::Value =
            serde_json::from_slice(&record.payload).expect("payload is valid json");
        assert_eq!(parsed["event"], "tool_use_start");
        assert_eq!(parsed["id"], "t1");
        assert_eq!(parsed["name"], "bash");
    }

    #[test]
    fn stream_event_to_agent_event_tags_error_as_error_kind() {
        let evt = StreamEvent::Error {
            message: "boom".into(),
        };
        let sid = sid("01ABC");
        let frame = stream_event_to_agent_event(&evt, &sid, 0).expect("ok");
        assert_eq!(frame.kind, pb::AgentEventKind::Error as i32);
    }
}
