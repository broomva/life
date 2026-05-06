//! `LagoSink` — durable replay via `lago_core::Journal`.
//!
//! Every [`StreamEvent`] flowing through the autonomous loop is appended
//! to the lago event journal as an
//! [`aios_protocol::EventKind::Custom`] event with `event_type =
//! "ergon.stream"`. The full event payload is serialized to JSON in the
//! `data` field, so a future replay is byte-for-byte reconstructable.
//!
//! ## Why `Custom` instead of a first-class EventKind variant
//!
//! Spec D4 says first-class variants for stable cross-cutting events,
//! `Custom` for layer-specific extensions. `ergon.stream` is
//! ergon-specific — it doesn't make sense to bake the streaming-event
//! taxonomy into `aios-protocol::EventKind`. Using `Custom` keeps the
//! kernel contract clean while still providing forward-compatible
//! durability.
//!
//! ## Failure semantics
//!
//! `LagoSink::emit` returns [`ergon::ErgonError::Internal`] if the
//! journal append fails. The autonomous loop will see this and bubble
//! up — because durable replay is critical to ergon's value
//! proposition, lost events are NOT silently swallowed.
//!
//! ## Performance
//!
//! Every `emit` call results in a single journal `append`. For a
//! verbose stream (one event per token), this can be high-frequency.
//! Future optimisation: batch via `Journal::append_batch` over a
//! short tumbling window (e.g., 100ms). For v0.1 the simple per-event
//! path is correct and predictable.

use aios_protocol::EventKind;
use async_trait::async_trait;
use ergon::{ErgonError, Result, StreamEvent, StreamSink};
use lago_core::id::{BranchId, EventId, SessionId};
use lago_core::{EventEnvelope, Journal};
use std::sync::Arc;

/// Stable `event_type` tag for ergon stream events stored in the journal.
pub const ERGON_STREAM_EVENT_TYPE: &str = "ergon.stream";

/// A [`StreamSink`] that appends every [`StreamEvent`] to a
/// [`Journal`] as a [`EventKind::Custom`] event.
pub struct LagoSink {
    journal: Arc<dyn Journal>,
    session_id: SessionId,
    branch_id: BranchId,
}

impl LagoSink {
    /// Construct a sink against the given journal + session, defaulting
    /// to the `"main"` branch.
    ///
    /// `session_id` is the ergon-canonical `aios_protocol::SessionId`
    /// (same type ergon uses everywhere). It's converted internally to
    /// `lago_core::SessionId` for the journal append. The conversion is
    /// lossless and infallible (both types wrap a `String`).
    pub fn new(journal: Arc<dyn Journal>, session_id: aios_protocol::ids::SessionId) -> Self {
        Self {
            journal,
            session_id: session_id.into(),
            branch_id: BranchId::from("main"),
        }
    }

    /// Override the branch — useful for branched-session workflows.
    /// Accepts the ergon-canonical `aios_protocol::BranchId`.
    #[must_use]
    pub fn with_branch(mut self, branch_id: aios_protocol::ids::BranchId) -> Self {
        self.branch_id = branch_id.into();
        self
    }

    /// Read-only handle to the (lago-internal) session id.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Build the canonical `EventEnvelope` for a stream event.
    ///
    /// Exposed (rather than inlined) so tests can verify the shape and
    /// future extensions (e.g., parent_id correlation) have a single
    /// place to add them.
    fn envelope_for(&self, event: &StreamEvent) -> std::result::Result<EventEnvelope, ErgonError> {
        let data = serde_json::to_value(event)?;
        Ok(EventEnvelope {
            event_id: EventId::new(),
            session_id: self.session_id.clone(),
            branch_id: self.branch_id.clone(),
            run_id: None,
            seq: 0, // Journal assigns the real sequence number on append.
            timestamp: EventEnvelope::now_micros(),
            parent_id: None,
            payload: EventKind::Custom {
                event_type: ERGON_STREAM_EVENT_TYPE.to_string(),
                data,
            },
            metadata: std::collections::HashMap::new(),
            schema_version: 1,
        })
    }
}

impl std::fmt::Debug for LagoSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LagoSink")
            .field("session_id", &self.session_id.as_str())
            .field("branch_id", &self.branch_id.as_str())
            .finish()
    }
}

#[async_trait]
impl StreamSink for LagoSink {
    async fn emit(&self, event: StreamEvent) -> Result<()> {
        let envelope = self.envelope_for(&event)?;
        self.journal
            .append(envelope)
            .await
            .map_err(|e| ErgonError::Internal(format!("lago journal append failed: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ergon::StopReason;
    use lago_core::Journal;
    use lago_core::error::{LagoError, LagoResult};
    use lago_core::id::SeqNo;
    use lago_core::journal::{EventQuery, EventStream};
    use lago_core::session::Session;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

    fn ergon_session() -> aios_protocol::ids::SessionId {
        aios_protocol::ids::SessionId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV")
    }

    fn lago_session() -> SessionId {
        SessionId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV")
    }

    /// In-memory mock Journal that records every appended envelope.
    #[derive(Default)]
    struct MockJournal {
        appended: Mutex<Vec<EventEnvelope>>,
        next_seq: AtomicU64,
    }

    impl MockJournal {
        fn appended(&self) -> Vec<EventEnvelope> {
            self.appended.lock().expect("lock").clone()
        }
    }

    impl Journal for MockJournal {
        fn append(&self, mut event: EventEnvelope) -> BoxFuture<'_, LagoResult<SeqNo>> {
            Box::pin(async move {
                let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
                event.seq = seq;
                self.appended.lock().expect("lock").push(event);
                Ok(seq)
            })
        }

        fn append_batch(&self, events: Vec<EventEnvelope>) -> BoxFuture<'_, LagoResult<SeqNo>> {
            Box::pin(async move {
                let mut last = 0;
                for mut e in events {
                    let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
                    e.seq = seq;
                    self.appended.lock().expect("lock").push(e);
                    last = seq;
                }
                Ok(last)
            })
        }

        fn read(&self, _query: EventQuery) -> BoxFuture<'_, LagoResult<Vec<EventEnvelope>>> {
            Box::pin(async move { Ok(Vec::new()) })
        }

        fn get_event(
            &self,
            _event_id: &EventId,
        ) -> BoxFuture<'_, LagoResult<Option<EventEnvelope>>> {
            Box::pin(async move { Ok(None) })
        }

        fn head_seq(
            &self,
            _session_id: &SessionId,
            _branch_id: &BranchId,
        ) -> BoxFuture<'_, LagoResult<SeqNo>> {
            Box::pin(async move { Ok(0) })
        }

        fn stream(
            &self,
            _session_id: SessionId,
            _branch_id: BranchId,
            _after_seq: SeqNo,
        ) -> BoxFuture<'_, LagoResult<EventStream>> {
            Box::pin(async move { Err(LagoError::Internal("stream not implemented".into())) })
        }

        fn put_session(&self, _session: Session) -> BoxFuture<'_, LagoResult<()>> {
            Box::pin(async { Ok(()) })
        }

        fn get_session(
            &self,
            _session_id: &SessionId,
        ) -> BoxFuture<'_, LagoResult<Option<Session>>> {
            Box::pin(async { Ok(None) })
        }

        fn list_sessions(&self) -> BoxFuture<'_, LagoResult<Vec<Session>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    /// Failing journal — every append returns an error.
    struct FailingJournal;

    impl Journal for FailingJournal {
        fn append(&self, _event: EventEnvelope) -> BoxFuture<'_, LagoResult<SeqNo>> {
            Box::pin(async { Err(LagoError::Internal("disk full".into())) })
        }
        fn append_batch(&self, _events: Vec<EventEnvelope>) -> BoxFuture<'_, LagoResult<SeqNo>> {
            Box::pin(async { Err(LagoError::Internal("disk full".into())) })
        }
        fn read(&self, _query: EventQuery) -> BoxFuture<'_, LagoResult<Vec<EventEnvelope>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn get_event(
            &self,
            _event_id: &EventId,
        ) -> BoxFuture<'_, LagoResult<Option<EventEnvelope>>> {
            Box::pin(async { Ok(None) })
        }
        fn head_seq(&self, _: &SessionId, _: &BranchId) -> BoxFuture<'_, LagoResult<SeqNo>> {
            Box::pin(async { Ok(0) })
        }
        fn stream(
            &self,
            _: SessionId,
            _: BranchId,
            _: SeqNo,
        ) -> BoxFuture<'_, LagoResult<EventStream>> {
            Box::pin(async { Err(LagoError::Internal("not impl".into())) })
        }
        fn put_session(&self, _: Session) -> BoxFuture<'_, LagoResult<()>> {
            Box::pin(async { Ok(()) })
        }
        fn get_session(&self, _: &SessionId) -> BoxFuture<'_, LagoResult<Option<Session>>> {
            Box::pin(async { Ok(None) })
        }
        fn list_sessions(&self) -> BoxFuture<'_, LagoResult<Vec<Session>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    fn done_event() -> StreamEvent {
        StreamEvent::Done {
            stop_reason: StopReason::EndTurn,
        }
    }

    #[tokio::test]
    async fn emit_appends_event_with_custom_payload() {
        let journal = Arc::new(MockJournal::default());
        let sink = LagoSink::new(journal.clone() as Arc<dyn Journal>, ergon_session());

        sink.emit(done_event()).await.expect("emit ok");

        let appended = journal.appended();
        assert_eq!(appended.len(), 1);
        let env = &appended[0];
        assert_eq!(env.session_id, lago_session());
        assert_eq!(env.branch_id.as_str(), "main");
        match &env.payload {
            EventKind::Custom { event_type, data } => {
                assert_eq!(event_type, ERGON_STREAM_EVENT_TYPE);
                // Round-trip the data back to a StreamEvent.
                let back: StreamEvent = serde_json::from_value(data.clone()).expect("round-trip");
                match back {
                    StreamEvent::Done { stop_reason } => {
                        assert_eq!(stop_reason, StopReason::EndTurn);
                    }
                    _ => panic!("variant mismatch"),
                }
            }
            other => panic!("expected Custom payload, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn emit_with_branch_overrides_default_main() {
        let journal = Arc::new(MockJournal::default());
        let sink = LagoSink::new(journal.clone() as Arc<dyn Journal>, ergon_session())
            .with_branch(aios_protocol::ids::BranchId::from("experiment-2"));

        sink.emit(done_event()).await.expect("emit ok");

        let appended = journal.appended();
        assert_eq!(appended[0].branch_id.as_str(), "experiment-2");
    }

    #[tokio::test]
    async fn journal_failure_surfaces_as_internal_error() {
        let journal: Arc<dyn Journal> = Arc::new(FailingJournal);
        let sink = LagoSink::new(journal, ergon_session());
        let err = sink.emit(done_event()).await.expect_err("should fail");
        match err {
            ErgonError::Internal(msg) => {
                assert!(msg.contains("lago journal append failed"));
                assert!(msg.contains("disk full"));
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multiple_emits_accumulate_in_journal_in_order() {
        let journal = Arc::new(MockJournal::default());
        let sink = LagoSink::new(journal.clone() as Arc<dyn Journal>, ergon_session());
        sink.emit(StreamEvent::TextDelta {
            id: "t".into(),
            delta: "hello ".into(),
        })
        .await
        .expect("ok");
        sink.emit(StreamEvent::TextDelta {
            id: "t".into(),
            delta: "world".into(),
        })
        .await
        .expect("ok");
        sink.emit(done_event()).await.expect("ok");

        let appended = journal.appended();
        assert_eq!(appended.len(), 3);
        // Sequence numbers monotonically increase (mock assigns).
        assert_eq!(appended[0].seq, 0);
        assert_eq!(appended[1].seq, 1);
        assert_eq!(appended[2].seq, 2);
    }

    #[test]
    fn debug_print_contains_session_and_branch() {
        let journal: Arc<dyn Journal> = Arc::new(MockJournal::default());
        let sink = LagoSink::new(journal, ergon_session());
        let s = format!("{sink:?}");
        assert!(s.contains("session_id"));
        assert!(s.contains("branch_id"));
    }
}
