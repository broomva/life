//! Persists Nous evaluation scores as canonical Lago events.
//!
//! Each `EvalScore` becomes an `EventKind::Custom` with `"eval."` prefix,
//! making it visible to Autonomic's projection fold and the entire
//! control stack.
//!
//! The [`ScoreSink`] wraps a [`LivePublisher`] and provides a
//! `ScoreCallback`-compatible closure via [`ScoreSink::callback()`],
//! bridging the `NousMiddleware` on_score hook to Lago persistence.

use std::sync::Arc;

use lago_core::journal::Journal;
use nous_core::score::EvalScore;
use tokio::runtime::Handle;
use tracing::warn;

use crate::LivePublisher;

/// Bridge between the `NousMiddleware` score callback and Lago journal persistence.
///
/// Wraps a [`LivePublisher`] and provides a synchronous `Fn(&EvalScore)` callback
/// that can be passed to `NousMiddleware::with_on_score()`. Each score is published
/// to the Lago journal as an `eval.InlineCompleted` event, making it visible to
/// Autonomic's projection fold.
///
/// # Usage
///
/// ```rust,ignore
/// let sink = ScoreSink::new(journal, "session-1", "agent-1");
/// let middleware = NousMiddleware::with_on_score(registry, sink.callback());
/// ```
pub struct ScoreSink {
    publisher: Arc<LivePublisher>,
}

impl ScoreSink {
    /// Create a new score sink backed by a Lago journal.
    pub fn new(
        journal: Arc<dyn Journal>,
        session_id: impl Into<String>,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            publisher: Arc::new(LivePublisher::new(journal, session_id, agent_id)),
        }
    }

    /// Create a callback compatible with `NousMiddleware::with_on_score()`.
    ///
    /// The returned closure captures an `Arc<LivePublisher>` and a Tokio
    /// `Handle`. Each invocation spawns a non-blocking task to persist
    /// the score, so the synchronous evaluator hot path is not blocked.
    ///
    /// If no Tokio runtime is available when this method is called,
    /// the callback will log a warning and drop scores silently.
    pub fn callback(self) -> Arc<dyn Fn(&EvalScore) + Send + Sync> {
        let publisher = self.publisher;
        let handle = Handle::try_current().ok();

        Arc::new(move |score: &EvalScore| {
            let Some(ref handle) = handle else {
                warn!("score_sink: no tokio runtime, dropping score");
                return;
            };

            let publisher = publisher.clone();
            let score = score.clone();

            handle.spawn(async move {
                if let Err(e) = publisher.publish_score(&score).await {
                    warn!(
                        evaluator = %score.evaluator,
                        error = %e,
                        "score_sink: failed to persist eval score to Lago (non-fatal)"
                    );
                }
            });
        })
    }
}

/// Persist a batch of `EvalScore`s to the Lago journal.
///
/// Convenience function for one-shot batch persistence (e.g., from
/// `NousMiddleware::scores()` after a run completes). Each score is
/// published individually to preserve per-score metadata and timestamps.
///
/// Returns the number of scores successfully persisted.
pub async fn persist_scores(
    journal: &Arc<dyn Journal>,
    session_id: &str,
    agent_id: &str,
    scores: &[EvalScore],
) -> usize {
    if scores.is_empty() {
        return 0;
    }

    let publisher = LivePublisher::new(journal.clone(), session_id, agent_id);
    let mut persisted = 0;

    for score in scores {
        match publisher.publish_score(score).await {
            Ok(seq) => {
                tracing::debug!(
                    evaluator = %score.evaluator,
                    value = score.value,
                    seq,
                    "persisted eval score to Lago"
                );
                persisted += 1;
            }
            Err(e) => {
                warn!(
                    evaluator = %score.evaluator,
                    error = %e,
                    "failed to persist eval score to Lago"
                );
            }
        }
    }

    persisted
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::event::EventKind;
    use lago_core::id::{BranchId, SessionId};
    use lago_core::journal::EventQuery;
    use lago_journal::RedbJournal;
    use nous_core::{EvalLayer, EvalTiming};

    fn open_journal(dir: &std::path::Path) -> Arc<dyn Journal> {
        let db_path = dir.join("test.redb");
        Arc::new(RedbJournal::open(db_path).unwrap()) as Arc<dyn Journal>
    }

    #[test]
    fn score_sink_creates_without_panic() {
        let dir = tempfile::tempdir().unwrap();
        let journal = open_journal(dir.path());
        let _sink = ScoreSink::new(journal, "sess-1", "agent-1");
    }

    #[tokio::test]
    async fn persist_scores_empty_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let journal = open_journal(dir.path());

        let count = persist_scores(&journal, "sess-1", "agent-1", &[]).await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn persist_scores_writes_correct_event_types() {
        let dir = tempfile::tempdir().unwrap();
        let journal = open_journal(dir.path());

        let scores = vec![
            EvalScore::new(
                "token_efficiency",
                0.85,
                EvalLayer::Execution,
                EvalTiming::Inline,
                "sess-1",
            )
            .unwrap(),
            EvalScore::new(
                "tool_correctness",
                0.5,
                EvalLayer::Action,
                EvalTiming::Inline,
                "sess-1",
            )
            .unwrap(),
        ];

        let count = persist_scores(&journal, "sess-1", "agent-1", &scores).await;
        assert_eq!(count, 2);

        // Read back events and verify structure.
        let query = EventQuery::new()
            .session(SessionId::from_string("sess-1"))
            .branch(BranchId::from_string("main"));
        let events = journal.read(query).await.unwrap();
        assert_eq!(events.len(), 2);

        // First event: token_efficiency
        if let EventKind::Custom { event_type, data } = &events[0].payload {
            assert_eq!(event_type, "eval.InlineCompleted");
            assert_eq!(data["evaluator"], "token_efficiency");
            assert_eq!(data["score"], 0.85);
        } else {
            panic!("expected Custom event kind with eval. prefix");
        }

        // Second event: tool_correctness
        if let EventKind::Custom { event_type, data } = &events[1].payload {
            assert_eq!(event_type, "eval.InlineCompleted");
            assert_eq!(data["evaluator"], "tool_correctness");
            assert_eq!(data["score"], 0.5);
        } else {
            panic!("expected Custom event kind with eval. prefix");
        }

        // Verify metadata
        for event in &events {
            assert_eq!(event.metadata.get("agent_id").unwrap(), "agent-1");
        }
    }

    #[tokio::test]
    async fn persist_scores_preserves_async_timing() {
        let dir = tempfile::tempdir().unwrap();
        let journal = open_journal(dir.path());

        // An async-timed score should still become eval.InlineCompleted
        // because NousPublisher::score_to_event_kind uses from_inline_score
        // which maps based on the NousEvent variant, not the timing field.
        let scores = vec![
            EvalScore::new(
                "plan_quality",
                0.7,
                EvalLayer::Reasoning,
                EvalTiming::Async,
                "sess-2",
            )
            .unwrap(),
        ];

        let count = persist_scores(&journal, "sess-2", "agent-1", &scores).await;
        assert_eq!(count, 1);

        let query = EventQuery::new()
            .session(SessionId::from_string("sess-2"))
            .branch(BranchId::from_string("main"));
        let events = journal.read(query).await.unwrap();
        assert_eq!(events.len(), 1);

        if let EventKind::Custom { event_type, data } = &events[0].payload {
            assert!(event_type.starts_with("eval."));
            assert_eq!(data["evaluator"], "plan_quality");
            assert_eq!(data["score"], 0.7);
        } else {
            panic!("expected Custom event kind");
        }
    }

    #[tokio::test]
    async fn score_sink_callback_persists_via_tokio() {
        let dir = tempfile::tempdir().unwrap();
        let journal = open_journal(dir.path());

        let sink = ScoreSink::new(journal.clone(), "sess-cb", "agent-cb");
        let callback = sink.callback();

        let score = EvalScore::new(
            "safety_compliance",
            1.0,
            EvalLayer::Safety,
            EvalTiming::Inline,
            "sess-cb",
        )
        .unwrap();

        // Invoke the callback (fire-and-forget)
        callback(&score);

        // Give the spawned task time to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let query = EventQuery::new()
            .session(SessionId::from_string("sess-cb"))
            .branch(BranchId::from_string("main"));
        let events = journal.read(query).await.unwrap();
        assert_eq!(events.len(), 1);

        if let EventKind::Custom { event_type, data } = &events[0].payload {
            assert_eq!(event_type, "eval.InlineCompleted");
            assert_eq!(data["evaluator"], "safety_compliance");
            assert_eq!(data["score"], 1.0);
        } else {
            panic!("expected Custom event kind");
        }
    }
}
