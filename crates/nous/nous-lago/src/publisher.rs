//! Publishes Nous evaluation events to the Lago journal.

use std::sync::Arc;

use lago_core::event::EventEnvelope;
use lago_core::id::{BranchId, EventId, SeqNo, SessionId};
use lago_core::journal::Journal;
use nous_core::events::NousEvent;
use nous_core::score::EvalScore;
use tracing::{debug, instrument, warn};

fn write_trace_context(metadata: &mut std::collections::HashMap<String, String>) {
    if let Some((trace_id, span_id)) = life_vigil::spans::current_trace_context() {
        metadata.insert("trace_id".to_string(), trace_id);
        metadata.insert("span_id".to_string(), span_id);
    }
}

/// Static helpers that convert Nous types into `EventKind` payloads.
///
/// Thin adapter that converts `EvalScore` / `NousEvent` to
/// `EventKind::Custom` with `"eval."` prefix and appends to the journal.
pub struct NousPublisher;

impl NousPublisher {
    /// Convert an `EvalScore` into a `NousEvent` and then into an `EventKind`.
    pub fn score_to_event_kind(score: &EvalScore) -> aios_protocol::event::EventKind {
        let event = NousEvent::from_inline_score(score);
        event.into_event_kind()
    }

    /// Convert a `NousEvent` directly into an `EventKind`.
    pub fn event_to_event_kind(event: NousEvent) -> aios_protocol::event::EventKind {
        debug!(
            event_type = std::any::type_name::<NousEvent>(),
            "publishing nous event to lago"
        );
        event.into_event_kind()
    }
}

/// Live publisher that holds a `Journal` arc and appends eval events directly.
///
/// Follows the same pattern as `autonomic-lago`'s publisher.
pub struct LivePublisher {
    journal: Arc<dyn Journal>,
    session_id: String,
    agent_id: String,
}

impl LivePublisher {
    /// Create a new live publisher.
    pub fn new(
        journal: Arc<dyn Journal>,
        session_id: impl Into<String>,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            journal,
            session_id: session_id.into(),
            agent_id: agent_id.into(),
        }
    }

    /// Publish an `EvalScore` to the Lago journal.
    ///
    /// Builds an `EventEnvelope` from the score with `eval.` prefix
    /// and appends it to the journal.
    #[instrument(skip(self, score), fields(lago.stream_id = %self.session_id, nous.agent_id = %self.agent_id))]
    pub async fn publish_score(
        &self,
        score: &EvalScore,
    ) -> Result<SeqNo, lago_core::error::LagoError> {
        let event_kind = NousPublisher::score_to_event_kind(score);

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("agent_id".to_string(), self.agent_id.clone());
        write_trace_context(&mut metadata);

        let envelope = EventEnvelope {
            event_id: EventId::new(),
            session_id: SessionId::from_string(&self.session_id),
            branch_id: BranchId::from_string("main"),
            run_id: None,
            seq: 0, // Journal assigns the actual sequence number
            timestamp: EventEnvelope::now_micros(),
            parent_id: None,
            payload: event_kind,
            metadata,
            schema_version: 1,
        };

        self.journal.append(envelope).await
    }

    /// Publish a `NousEvent` to the Lago journal.
    #[instrument(skip(self, event), fields(lago.stream_id = %self.session_id, nous.agent_id = %self.agent_id))]
    pub async fn publish_event(
        &self,
        event: NousEvent,
    ) -> Result<SeqNo, lago_core::error::LagoError> {
        let event_kind = NousPublisher::event_to_event_kind(event);

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("agent_id".to_string(), self.agent_id.clone());
        write_trace_context(&mut metadata);

        let envelope = EventEnvelope {
            event_id: EventId::new(),
            session_id: SessionId::from_string(&self.session_id),
            branch_id: BranchId::from_string("main"),
            run_id: None,
            seq: 0,
            timestamp: EventEnvelope::now_micros(),
            parent_id: None,
            payload: event_kind,
            metadata,
            schema_version: 1,
        };

        self.journal.append(envelope).await
    }

    /// Publish a batch of eval events atomically.
    #[instrument(skip(self, events), fields(lago.stream_id = %self.session_id, nous.event_count = events.len()))]
    pub async fn publish_events(
        &self,
        events: Vec<NousEvent>,
    ) -> Result<SeqNo, lago_core::error::LagoError> {
        if events.is_empty() {
            warn!("publish_events called with empty event list");
            return Ok(0);
        }

        let envelopes: Vec<EventEnvelope> = events
            .into_iter()
            .map(|event| {
                let event_kind = NousPublisher::event_to_event_kind(event);
                let mut metadata = std::collections::HashMap::new();
                metadata.insert("agent_id".to_string(), self.agent_id.clone());
                write_trace_context(&mut metadata);

                EventEnvelope {
                    event_id: EventId::new(),
                    session_id: SessionId::from_string(&self.session_id),
                    branch_id: BranchId::from_string("main"),
                    run_id: None,
                    seq: 0,
                    timestamp: EventEnvelope::now_micros(),
                    parent_id: None,
                    payload: event_kind,
                    metadata,
                    schema_version: 1,
                }
            })
            .collect();

        self.journal.append_batch(envelopes).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::event::EventKind;
    use lago_core::journal::EventQuery;
    use lago_journal::RedbJournal;
    use nous_core::{EvalLayer, EvalTiming};

    fn open_journal(dir: &std::path::Path) -> Arc<dyn Journal> {
        let db_path = dir.join("test.redb");
        Arc::new(RedbJournal::open(db_path).unwrap()) as Arc<dyn Journal>
    }

    #[test]
    fn score_to_event_kind_produces_custom() {
        let score =
            EvalScore::new("test", 0.85, EvalLayer::Execution, EvalTiming::Inline, "s").unwrap();
        let kind = NousPublisher::score_to_event_kind(&score);
        assert!(
            matches!(kind, EventKind::Custom { event_type, .. } if event_type.starts_with("eval."))
        );
    }

    #[test]
    fn event_to_event_kind_produces_custom() {
        let event = NousEvent::QualityChanged {
            session_id: "sess-1".into(),
            aggregate_quality: 0.82,
            trend: 0.01,
            inline_count: 10,
            async_count: 2,
        };
        let kind = NousPublisher::event_to_event_kind(event);
        assert!(
            matches!(kind, EventKind::Custom { event_type, .. } if event_type == "eval.QualityChanged")
        );
    }

    #[tokio::test]
    async fn live_publisher_publish_score_creates_valid_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let journal = open_journal(dir.path());
        let publisher = LivePublisher::new(journal.clone(), "sess-1", "agent-1");

        let score = EvalScore::new(
            "token_efficiency",
            0.85,
            EvalLayer::Execution,
            EvalTiming::Inline,
            "sess-1",
        )
        .unwrap();

        let seq = publisher.publish_score(&score).await.unwrap();
        assert!(seq > 0);

        // Read back and verify
        let query = EventQuery::new()
            .session(SessionId::from_string("sess-1"))
            .branch(BranchId::from_string("main"));
        let events = journal.read(query).await.unwrap();
        assert_eq!(events.len(), 1);

        if let EventKind::Custom { event_type, data } = &events[0].payload {
            assert!(event_type.starts_with("eval."));
            assert_eq!(event_type, "eval.InlineCompleted");
            assert_eq!(data["evaluator"], "token_efficiency");
            assert_eq!(data["score"], 0.85);
        } else {
            panic!("expected Custom event kind with eval. prefix");
        }

        // Verify metadata
        assert_eq!(events[0].metadata.get("agent_id").unwrap(), "agent-1");
    }

    #[tokio::test]
    async fn live_publisher_publish_event_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let journal = open_journal(dir.path());
        let publisher = LivePublisher::new(journal.clone(), "sess-2", "agent-1");

        let event = NousEvent::QualityChanged {
            session_id: "sess-2".into(),
            aggregate_quality: 0.72,
            trend: -0.05,
            inline_count: 10,
            async_count: 2,
        };

        let seq = publisher.publish_event(event).await.unwrap();
        assert!(seq > 0);

        let query = EventQuery::new()
            .session(SessionId::from_string("sess-2"))
            .branch(BranchId::from_string("main"));
        let events = journal.read(query).await.unwrap();
        assert_eq!(events.len(), 1);

        if let EventKind::Custom { event_type, data } = &events[0].payload {
            assert_eq!(event_type, "eval.QualityChanged");
            assert_eq!(data["aggregate_quality"], 0.72);
        } else {
            panic!("expected Custom event kind");
        }
    }

    #[tokio::test]
    async fn live_publisher_publish_batch_monotonic() {
        let dir = tempfile::tempdir().unwrap();
        let journal = open_journal(dir.path());
        let publisher = LivePublisher::new(journal.clone(), "sess-3", "agent-1");

        let events = vec![
            NousEvent::InlineCompleted {
                evaluator: "eval_a".into(),
                score: 0.9,
                label: nous_core::score::ScoreLabel::Good,
                layer: EvalLayer::Execution,
                session_id: "sess-3".into(),
                run_id: None,
                explanation: None,
            },
            NousEvent::InlineCompleted {
                evaluator: "eval_b".into(),
                score: 0.6,
                label: nous_core::score::ScoreLabel::Warning,
                layer: EvalLayer::Action,
                session_id: "sess-3".into(),
                run_id: None,
                explanation: None,
            },
        ];

        let final_seq = publisher.publish_events(events).await.unwrap();
        assert!(final_seq >= 2);

        let query = EventQuery::new()
            .session(SessionId::from_string("sess-3"))
            .branch(BranchId::from_string("main"));
        let stored = journal.read(query).await.unwrap();
        assert_eq!(stored.len(), 2);

        for window in stored.windows(2) {
            assert!(window[1].seq > window[0].seq, "sequences must be monotonic");
        }
    }

    #[tokio::test]
    async fn live_publisher_empty_batch_noop() {
        let dir = tempfile::tempdir().unwrap();
        let journal = open_journal(dir.path());
        let publisher = LivePublisher::new(journal, "sess-4", "agent-1");

        let seq = publisher.publish_events(vec![]).await.unwrap();
        assert_eq!(seq, 0);
    }
}
