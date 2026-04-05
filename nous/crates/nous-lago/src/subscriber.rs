//! Subscribes to Nous evaluation events from the Lago journal.

use aios_protocol::event::EventKind;
use lago_core::error::LagoResult;
use lago_core::event::EventEnvelope;
use lago_core::projection::Projection;
use nous_core::events::NousEvent;

/// Processes eval events from the Lago journal stream.
///
/// Filters for `"eval."` prefixed custom events and deserializes
/// them back into `NousEvent` variants for downstream processing.
pub struct NousSubscriber;

impl NousSubscriber {
    /// Try to extract a `NousEvent` from an `EventKind`.
    ///
    /// Returns `None` if the event is not a Nous evaluation event.
    pub fn try_extract(kind: &EventKind) -> Option<NousEvent> {
        if let EventKind::Custom { event_type, data } = kind {
            NousEvent::from_custom(event_type, data)
        } else {
            None
        }
    }

    /// Check if an `EventKind` is a Nous evaluation event.
    pub fn is_eval_event(kind: &EventKind) -> bool {
        matches!(kind, EventKind::Custom { event_type, .. } if NousEvent::is_eval_event(event_type))
    }
}

/// Exponential moving average smoothing factor for eval quality scores.
const EVAL_EMA_ALPHA: f64 = 0.3;

/// In-memory evaluation state maintained by the projection.
///
/// Mirrors the same fields as `autonomic_core::gating::EvalState`
/// but is owned by the Nous projection for independent tracking.
#[derive(Debug, Clone)]
pub struct EvalState {
    /// Count of inline evaluations completed.
    pub inline_eval_count: u32,
    /// Count of async evaluations completed.
    pub async_eval_count: u32,
    /// Aggregate quality score (0.0..1.0), exponential moving average.
    pub aggregate_quality_score: f64,
    /// Quality trend (positive = improving, negative = degrading).
    pub quality_trend: f64,
    /// Timestamp of the last evaluation (ms since epoch).
    pub last_eval_ms: u64,
    /// Most recent evaluator scores by name.
    pub evaluator_scores: std::collections::HashMap<String, f64>,
}

impl Default for EvalState {
    fn default() -> Self {
        Self {
            inline_eval_count: 0,
            async_eval_count: 0,
            aggregate_quality_score: 1.0, // Optimistic start, same as autonomic-core
            quality_trend: 0.0,
            last_eval_ms: 0,
            evaluator_scores: std::collections::HashMap::new(),
        }
    }
}

/// Projection that processes eval events from the Lago journal
/// and maintains an in-memory `EvalState`.
///
/// Implements `lago_core::projection::Projection` for use with
/// Lago's journal subscription infrastructure.
pub struct EvalProjection {
    state: EvalState,
}

impl EvalProjection {
    /// Create a new eval projection with default state.
    pub fn new() -> Self {
        Self {
            state: EvalState::default(),
        }
    }

    /// Get a reference to the current eval state.
    pub fn state(&self) -> &EvalState {
        &self.state
    }

    /// Apply an eval event to the projection state.
    ///
    /// Uses the same EMA logic as `autonomic-controller`'s projection fold.
    fn apply_eval_event(&mut self, event_type: &str, data: &serde_json::Value, ts_ms: u64) {
        self.state.last_eval_ms = ts_ms;

        match event_type {
            "eval.InlineCompleted" => {
                self.state.inline_eval_count += 1;

                if let Some(evaluator) = data.get("evaluator").and_then(serde_json::Value::as_str) {
                    if let Some(score) = data.get("score").and_then(serde_json::Value::as_f64) {
                        self.state
                            .evaluator_scores
                            .insert(evaluator.to_owned(), score);

                        let prev = self.state.aggregate_quality_score;
                        self.state.aggregate_quality_score =
                            EVAL_EMA_ALPHA * score + (1.0 - EVAL_EMA_ALPHA) * prev;
                        self.state.quality_trend = self.state.aggregate_quality_score - prev;
                    }
                }
            }
            "eval.AsyncCompleted" => {
                self.state.async_eval_count += 1;

                if let Some(scores) = data.get("scores").and_then(|v| v.as_array()) {
                    for score_obj in scores {
                        if let Some(evaluator) = score_obj
                            .get("evaluator")
                            .and_then(serde_json::Value::as_str)
                        {
                            if let Some(score) =
                                score_obj.get("value").and_then(serde_json::Value::as_f64)
                            {
                                self.state
                                    .evaluator_scores
                                    .insert(evaluator.to_owned(), score);

                                let prev = self.state.aggregate_quality_score;
                                self.state.aggregate_quality_score =
                                    EVAL_EMA_ALPHA * score + (1.0 - EVAL_EMA_ALPHA) * prev;
                                self.state.quality_trend =
                                    self.state.aggregate_quality_score - prev;
                            }
                        }
                    }
                }
            }
            "eval.QualityChanged" => {
                if let Some(quality) = data
                    .get("aggregate_quality")
                    .and_then(serde_json::Value::as_f64)
                {
                    let prev = self.state.aggregate_quality_score;
                    self.state.aggregate_quality_score = quality;
                    self.state.quality_trend = quality - prev;
                }
            }
            _ => {
                // Unknown eval event subtype — timestamp updated above.
            }
        }
    }
}

impl Default for EvalProjection {
    fn default() -> Self {
        Self::new()
    }
}

impl Projection for EvalProjection {
    fn on_event(&mut self, event: &EventEnvelope) -> LagoResult<()> {
        if let EventKind::Custom { event_type, data } = &event.payload {
            if NousEvent::is_eval_event(event_type) {
                // Convert timestamp from micros to millis for consistency
                let ts_ms = event.timestamp / 1000;
                self.apply_eval_event(event_type, data, ts_ms);
            }
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "nous_eval"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lago_core::id::{BranchId, EventId, SessionId};
    use nous_core::score::ScoreLabel;
    use nous_core::taxonomy::EvalLayer;
    use std::collections::HashMap;

    fn make_eval_envelope(event_type: &str, data: serde_json::Value) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId::new(),
            session_id: SessionId::from_string("sess-1"),
            branch_id: BranchId::from_string("main"),
            run_id: None,
            seq: 1,
            timestamp: 5_000_000, // 5000ms in micros
            parent_id: None,
            payload: EventKind::Custom {
                event_type: event_type.to_owned(),
                data,
            },
            metadata: HashMap::new(),
            schema_version: 1,
        }
    }

    #[test]
    fn extract_inline_completed() {
        let event = NousEvent::InlineCompleted {
            evaluator: "test".into(),
            score: 0.9,
            label: ScoreLabel::Good,
            layer: EvalLayer::Execution,
            session_id: "s".into(),
            run_id: None,
            explanation: None,
        };
        let kind = event.into_event_kind();

        let extracted = NousSubscriber::try_extract(&kind).unwrap();
        assert!(
            matches!(extracted, NousEvent::InlineCompleted { evaluator, .. } if evaluator == "test")
        );
    }

    #[test]
    fn non_eval_event_returns_none() {
        let kind = EventKind::RunFinished {
            reason: "done".into(),
            total_iterations: 1,
            final_answer: None,
            usage: None,
        };
        assert!(NousSubscriber::try_extract(&kind).is_none());
    }

    #[test]
    fn is_eval_event_checks_prefix() {
        let eval_kind = EventKind::Custom {
            event_type: "eval.InlineCompleted".into(),
            data: serde_json::json!({}),
        };
        assert!(NousSubscriber::is_eval_event(&eval_kind));

        let other_kind = EventKind::Custom {
            event_type: "autonomic.CostCharged".into(),
            data: serde_json::json!({}),
        };
        assert!(!NousSubscriber::is_eval_event(&other_kind));
    }

    // ── EvalProjection tests ──

    #[test]
    fn projection_name() {
        let proj = EvalProjection::new();
        assert_eq!(proj.name(), "nous_eval");
    }

    #[test]
    fn projection_default_state() {
        let proj = EvalProjection::new();
        let state = proj.state();
        assert_eq!(state.inline_eval_count, 0);
        assert_eq!(state.async_eval_count, 0);
        assert!((state.aggregate_quality_score - 1.0).abs() < f64::EPSILON);
        assert!((state.quality_trend).abs() < f64::EPSILON);
    }

    #[test]
    fn projection_processes_inline_completed() {
        let mut proj = EvalProjection::new();
        let envelope = make_eval_envelope(
            "eval.InlineCompleted",
            serde_json::json!({
                "evaluator": "token_efficiency",
                "score": 0.8,
                "label": "good",
                "layer": "execution",
                "session_id": "s1"
            }),
        );

        proj.on_event(&envelope).unwrap();

        let state = proj.state();
        assert_eq!(state.inline_eval_count, 1);
        // EMA: 0.3 * 0.8 + 0.7 * 1.0 = 0.94
        assert!((state.aggregate_quality_score - 0.94).abs() < 0.001);
        assert_eq!(state.last_eval_ms, 5000); // 5_000_000 micros / 1000
        assert_eq!(state.evaluator_scores.get("token_efficiency"), Some(&0.8));
    }

    #[test]
    fn projection_processes_async_completed() {
        let mut proj = EvalProjection::new();
        let envelope = make_eval_envelope(
            "eval.AsyncCompleted",
            serde_json::json!({
                "evaluator": "plan_quality",
                "scores": [
                    {"evaluator": "plan_quality", "value": 0.7, "label": "warning", "layer": "reasoning"}
                ],
                "session_id": "s1",
                "duration_ms": 500
            }),
        );

        proj.on_event(&envelope).unwrap();

        let state = proj.state();
        assert_eq!(state.async_eval_count, 1);
        // EMA: 0.3 * 0.7 + 0.7 * 1.0 = 0.91
        assert!((state.aggregate_quality_score - 0.91).abs() < 0.001);
        assert_eq!(state.evaluator_scores.get("plan_quality"), Some(&0.7));
    }

    #[test]
    fn projection_processes_quality_changed() {
        let mut proj = EvalProjection::new();
        let envelope = make_eval_envelope(
            "eval.QualityChanged",
            serde_json::json!({
                "session_id": "s1",
                "aggregate_quality": 0.72,
                "trend": -0.05,
                "inline_count": 10,
                "async_count": 2
            }),
        );

        proj.on_event(&envelope).unwrap();

        let state = proj.state();
        assert!((state.aggregate_quality_score - 0.72).abs() < f64::EPSILON);
        // trend: 0.72 - 1.0 = -0.28
        assert!((state.quality_trend - (-0.28)).abs() < f64::EPSILON);
    }

    #[test]
    fn projection_ignores_non_eval_events() {
        let mut proj = EvalProjection::new();
        let envelope = EventEnvelope {
            event_id: EventId::new(),
            session_id: SessionId::from_string("sess-1"),
            branch_id: BranchId::from_string("main"),
            run_id: None,
            seq: 1,
            timestamp: 5_000_000,
            parent_id: None,
            payload: EventKind::RunFinished {
                reason: "done".into(),
                total_iterations: 1,
                final_answer: None,
                usage: None,
            },
            metadata: HashMap::new(),
            schema_version: 1,
        };

        proj.on_event(&envelope).unwrap();

        let state = proj.state();
        assert_eq!(state.inline_eval_count, 0);
        assert_eq!(state.async_eval_count, 0);
        assert!((state.aggregate_quality_score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn projection_ignores_non_eval_custom_events() {
        let mut proj = EvalProjection::new();
        let envelope =
            make_eval_envelope("autonomic.CostCharged", serde_json::json!({"amount": 500}));

        proj.on_event(&envelope).unwrap();

        let state = proj.state();
        assert_eq!(state.inline_eval_count, 0);
        assert!((state.aggregate_quality_score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn projection_multiple_inline_events_ema_converges() {
        let mut proj = EvalProjection::new();

        // Feed 10 low-quality scores (0.3) to drive quality down
        for i in 0..10 {
            let envelope = EventEnvelope {
                event_id: EventId::new(),
                session_id: SessionId::from_string("sess-1"),
                branch_id: BranchId::from_string("main"),
                run_id: None,
                seq: i + 1,
                timestamp: (i + 1) * 1_000_000, // incrementing timestamps
                parent_id: None,
                payload: EventKind::Custom {
                    event_type: "eval.InlineCompleted".to_owned(),
                    data: serde_json::json!({
                        "evaluator": "tool_correctness",
                        "score": 0.3,
                        "label": "critical",
                        "layer": "action",
                        "session_id": "s1"
                    }),
                },
                metadata: HashMap::new(),
                schema_version: 1,
            };
            proj.on_event(&envelope).unwrap();
        }

        let state = proj.state();
        assert_eq!(state.inline_eval_count, 10);
        // After 10 events with score=0.3, EMA should be close to 0.3
        assert!(
            state.aggregate_quality_score < 0.5,
            "quality should have degraded below 0.5, got {}",
            state.aggregate_quality_score
        );
        assert!(state.quality_trend < 0.0, "trend should be negative");
        assert_eq!(state.last_eval_ms, 10_000); // 10_000_000 micros / 1000
    }

    #[test]
    fn projection_tracks_per_evaluator_scores() {
        let mut proj = EvalProjection::new();

        // Two different evaluators
        let envelope_a = make_eval_envelope(
            "eval.InlineCompleted",
            serde_json::json!({
                "evaluator": "token_efficiency",
                "score": 0.9,
                "label": "good",
                "layer": "execution",
                "session_id": "s1"
            }),
        );
        let envelope_b = make_eval_envelope(
            "eval.InlineCompleted",
            serde_json::json!({
                "evaluator": "safety_compliance",
                "score": 0.4,
                "label": "critical",
                "layer": "safety",
                "session_id": "s1"
            }),
        );

        proj.on_event(&envelope_a).unwrap();
        proj.on_event(&envelope_b).unwrap();

        let state = proj.state();
        assert_eq!(state.evaluator_scores.get("token_efficiency"), Some(&0.9));
        assert_eq!(state.evaluator_scores.get("safety_compliance"), Some(&0.4));
        assert_eq!(state.inline_eval_count, 2);
    }

    #[test]
    fn projection_evaluator_score_updates_on_newer_event() {
        let mut proj = EvalProjection::new();

        // First score for evaluator
        let envelope1 = make_eval_envelope(
            "eval.InlineCompleted",
            serde_json::json!({
                "evaluator": "token_efficiency",
                "score": 0.5,
                "label": "warning",
                "layer": "execution",
                "session_id": "s1"
            }),
        );
        // Updated score for same evaluator
        let envelope2 = make_eval_envelope(
            "eval.InlineCompleted",
            serde_json::json!({
                "evaluator": "token_efficiency",
                "score": 0.9,
                "label": "good",
                "layer": "execution",
                "session_id": "s1"
            }),
        );

        proj.on_event(&envelope1).unwrap();
        proj.on_event(&envelope2).unwrap();

        let state = proj.state();
        // Should reflect the latest score
        assert_eq!(state.evaluator_scores.get("token_efficiency"), Some(&0.9));
    }
}
