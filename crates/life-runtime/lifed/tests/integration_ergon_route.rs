//! Integration test: drive `route::ergon::handle_stream_session` against
//! a mock [`ErgonRegistry`] end-to-end and assert that events flow.
//!
//! This is the smallest exercise of the route's critical path the spec
//! §12.8 success criteria call out: a mock workflow registers a handle,
//! the route resolves it, the workflow emits a fixed sequence of
//! `StreamEvent`s, and the test client collects them as
//! `life.v1.AgentEvent` frames in order with monotone sequence numbers.
//!
//! The substrate-side adapter (arcan-ergon) ships outside this crate
//! per Spec C₂ §11; this test stands in for that adapter using an
//! in-process [`ergon::WorkflowExecutor`]. The pattern is the one the
//! production adapter will use: wrap a real `Workflow` impl, map its
//! `StreamEvent` outputs to `AgentEvent` frames, and return a
//! `Stream<Item = Result<AgentEvent, Status>>`.
//!
//! Linear: [BRO-1002](https://linear.app/broomva/issue/BRO-1002)

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt;
use futures::stream;
use tonic::Status;

use aios_proto::aios::v1 as aios_v1;
use ergon::{StopReason, StreamEvent};
use life_runtime_proto::life::v1 as pb;
use lifed::route::ergon::{
    ErgonRegistry, ErgonWorkflowHandle, InMemoryErgonRegistry, LifedContext, RouteError,
    StreamSessionRequest, handle_stream_session, stream_event_to_agent_event,
};

type EventStream = Pin<Box<dyn Stream<Item = Result<pb::AgentEvent, Status>> + Send>>;

/// Reusable mock workflow handle: records the input it received and
/// emits a canned sequence of `StreamEvent`s mapped to `AgentEvent`.
struct MockHandle {
    name: String,
    events: Vec<StreamEvent>,
}

impl MockHandle {
    fn new(name: &str, events: Vec<StreamEvent>) -> Self {
        Self {
            name: name.to_owned(),
            events,
        }
    }
}

#[async_trait]
impl ErgonWorkflowHandle for MockHandle {
    fn name(&self) -> &str {
        &self.name
    }

    async fn start(
        &self,
        sid: aios_v1::SessionId,
        _input: serde_json::Value,
    ) -> Result<EventStream, RouteError> {
        let mut seq = 0u64;
        let frames: Vec<Result<pb::AgentEvent, Status>> = self
            .events
            .iter()
            .map(|e| {
                let r = stream_event_to_agent_event(e, &sid, seq);
                seq += 1;
                r
            })
            .collect();
        Ok(Box::pin(stream::iter(frames)))
    }
}

#[tokio::test]
async fn end_to_end_route_drives_mock_workflow_and_streams_events() {
    // Substrate-side adapter would build the registry at boot. Here a
    // single mock workflow stands in for the production wiring.
    let canned_events = vec![
        StreamEvent::SessionStart {
            session_id: ergon::SessionId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            model: "stub-1".into(),
            provider: "stub".into(),
        },
        StreamEvent::TextStart { id: "t1".into() },
        StreamEvent::TextDelta {
            id: "t1".into(),
            delta: "hello ".into(),
        },
        StreamEvent::TextDelta {
            id: "t1".into(),
            delta: "world".into(),
        },
        StreamEvent::TextEnd { id: "t1".into() },
        StreamEvent::Usage {
            input: 42,
            output: 11,
            cached_input: None,
            reasoning: None,
        },
        StreamEvent::Done {
            stop_reason: StopReason::EndTurn,
        },
    ];

    let handle: Arc<dyn ErgonWorkflowHandle> = Arc::new(MockHandle::new(
        "bookkeeping.promotion-judge",
        canned_events,
    ));
    let registry_concrete = InMemoryErgonRegistry::new();
    registry_concrete.register(handle);
    let registry: Arc<dyn ErgonRegistry> = Arc::new(registry_concrete);

    // Assert wiring is visible to introspection — admin/runtime
    // surfaces will consume this in a follow-up.
    assert_eq!(
        registry.known_names(),
        vec!["bookkeeping.promotion-judge".to_owned()]
    );

    let ctx = LifedContext::new(registry);

    let req = StreamSessionRequest {
        agent_name: "bookkeeping.promotion-judge".into(),
        sid: aios_v1::SessionId {
            value: "session-abc".into(),
        },
        input: serde_json::json!({
            "raw_extract_path": "research/notes/2026-05-12-prompt-patterns-raw.md",
        }),
    };

    // `EventStream` is a trait object (no `Debug`); avoid `expect`.
    let stream = match handle_stream_session(req, &ctx).await {
        Ok(s) => s,
        Err(e) => panic!("happy path stream must start: {e:?}"),
    };

    let frames: Vec<Result<pb::AgentEvent, Status>> = stream.collect().await;

    // Every canned event surfaced as an `AgentEvent` frame.
    assert_eq!(frames.len(), 7, "all events propagated");

    // Sequence numbers are monotone starting at zero — the contract
    // chatOS / SDK reconnect logic expects (`SessionRef.from_sequence`).
    for (i, frame) in frames.iter().enumerate() {
        let frame = frame.as_ref().expect("frame ok");
        let record = frame.record.as_ref().expect("record present");
        assert_eq!(
            record.sequence, i as u64,
            "frame {i} carries monotone sequence"
        );
        assert_eq!(
            record.session_id.as_ref().expect("sid").value,
            "session-abc",
            "frame {i} carries the requested sid"
        );
    }

    // The Done frame must report Finish at the proto-kind level so SSE
    // emitters know to close the stream cleanly.
    let last = frames.last().expect("non-empty").as_ref().expect("ok");
    assert_eq!(last.kind, pb::AgentEventKind::Finish as i32);
    let last_record = last.record.as_ref().expect("record");
    assert_eq!(last_record.kind, "done");

    // The TextDelta frames must round-trip through the payload as JSON
    // matching `StreamEvent`'s `serde(tag = "event")` discriminator.
    let third = frames[2].as_ref().expect("ok");
    let third_record = third.record.as_ref().expect("record");
    let parsed: serde_json::Value =
        serde_json::from_slice(&third_record.payload).expect("payload is valid json");
    assert_eq!(parsed["event"], "text_delta");
    assert_eq!(parsed["delta"], "hello ");
}

#[tokio::test]
async fn unknown_agent_short_circuits_before_workflow_start() {
    let registry: Arc<dyn ErgonRegistry> = Arc::new(InMemoryErgonRegistry::new());
    let ctx = LifedContext::new(registry);
    let req = StreamSessionRequest {
        agent_name: "missing".into(),
        sid: aios_v1::SessionId {
            value: "session-x".into(),
        },
        input: serde_json::json!({}),
    };
    // `EventStream` is a trait object (no `Debug`); use a `match`.
    let err = match handle_stream_session(req, &ctx).await {
        Ok(_) => panic!("expected NotFound"),
        Err(e) => e,
    };
    let status: Status = err.into();
    assert_eq!(status.code(), tonic::Code::NotFound);
    assert!(status.message().contains("missing"));
}

#[tokio::test]
async fn workflow_start_failure_surfaces_aborted_status() {
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
                message: "hook denied".into(),
            })
        }
    }

    let r = InMemoryErgonRegistry::new();
    r.register(Arc::new(FailingHandle));
    let registry: Arc<dyn ErgonRegistry> = Arc::new(r);
    let ctx = LifedContext::new(registry);

    let req = StreamSessionRequest {
        agent_name: "failing".into(),
        sid: aios_v1::SessionId {
            value: "session-y".into(),
        },
        input: serde_json::json!({}),
    };
    // `EventStream` is a trait object (no `Debug`); use a `match`.
    let err = match handle_stream_session(req, &ctx).await {
        Ok(_) => panic!("expected WorkflowFailed"),
        Err(e) => e,
    };
    let status: Status = err.into();
    assert_eq!(status.code(), tonic::Code::Aborted);
    assert!(status.message().contains("hook denied"));
}
