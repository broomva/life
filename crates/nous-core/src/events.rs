//! Nous event constructors.
//!
//! Eval events use `EventKind::Custom` with `"eval."` prefix,
//! following the same pattern as `"autonomic."` and `"strategy."` events.

use aios_protocol::event::EventKind;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::score::{EvalScore, ScoreLabel};
use crate::taxonomy::EvalLayer;

/// Prefix for all Nous evaluation events.
pub const EVAL_EVENT_PREFIX: &str = "eval.";

/// Nous-specific event types that wrap as `EventKind::Custom`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
pub enum NousEvent {
    /// An inline evaluator completed.
    InlineCompleted {
        evaluator: String,
        score: f64,
        label: ScoreLabel,
        layer: EvalLayer,
        session_id: String,
        run_id: Option<String>,
        explanation: Option<String>,
    },
    /// An async evaluator completed.
    AsyncCompleted {
        evaluator: String,
        scores: Vec<ScoreSummary>,
        session_id: String,
        run_id: Option<String>,
        duration_ms: u64,
    },
    /// Aggregate quality changed (emitted when EMA updates).
    QualityChanged {
        session_id: String,
        aggregate_quality: f64,
        trend: f64,
        inline_count: u32,
        async_count: u32,
    },
    /// EGRI outcome published after async judge evaluation.
    EgriOutcome {
        session_id: String,
        trial_id: Option<String>,
        outcome: serde_json::Value,
    },
}

/// Summary of a score for event serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreSummary {
    pub evaluator: String,
    pub value: f64,
    pub label: ScoreLabel,
    pub layer: EvalLayer,
}

impl From<&EvalScore> for ScoreSummary {
    fn from(score: &EvalScore) -> Self {
        Self {
            evaluator: score.evaluator.clone(),
            value: score.value,
            label: score.label,
            layer: score.layer,
        }
    }
}

impl NousEvent {
    /// Create an inline-completed event from an `EvalScore`.
    pub fn from_inline_score(score: &EvalScore) -> Self {
        Self::InlineCompleted {
            evaluator: score.evaluator.clone(),
            score: score.value,
            label: score.label,
            layer: score.layer,
            session_id: score.session_id.clone(),
            run_id: score.run_id.clone(),
            explanation: score.explanation.clone(),
        }
    }

    /// Convert this event into a canonical `EventKind::Custom`.
    pub fn into_event_kind(self) -> EventKind {
        let (event_type, data) = match &self {
            Self::InlineCompleted {
                evaluator,
                score,
                label,
                layer,
                session_id,
                run_id,
                explanation,
            } => (
                "eval.InlineCompleted",
                json!({
                    "evaluator": evaluator,
                    "score": score,
                    "label": label,
                    "layer": layer,
                    "session_id": session_id,
                    "run_id": run_id,
                    "explanation": explanation,
                }),
            ),
            Self::AsyncCompleted {
                evaluator,
                scores,
                session_id,
                run_id,
                duration_ms,
            } => (
                "eval.AsyncCompleted",
                json!({
                    "evaluator": evaluator,
                    "scores": scores,
                    "session_id": session_id,
                    "run_id": run_id,
                    "duration_ms": duration_ms,
                }),
            ),
            Self::QualityChanged {
                session_id,
                aggregate_quality,
                trend,
                inline_count,
                async_count,
            } => (
                "eval.QualityChanged",
                json!({
                    "session_id": session_id,
                    "aggregate_quality": aggregate_quality,
                    "trend": trend,
                    "inline_count": inline_count,
                    "async_count": async_count,
                }),
            ),
            Self::EgriOutcome {
                session_id,
                trial_id,
                outcome,
            } => (
                "eval.egri_outcome",
                json!({
                    "session_id": session_id,
                    "trial_id": trial_id,
                    "outcome": outcome,
                }),
            ),
        };
        EventKind::Custom {
            event_type: event_type.to_owned(),
            data,
        }
    }

    /// Check if a `Custom` event is a Nous evaluation event by its prefix.
    pub fn is_eval_event(event_type: &str) -> bool {
        event_type.starts_with(EVAL_EVENT_PREFIX)
    }

    /// Try to parse an `EventKind::Custom` back into a `NousEvent`.
    pub fn from_custom(event_type: &str, data: &serde_json::Value) -> Option<Self> {
        if !Self::is_eval_event(event_type) {
            return None;
        }

        match event_type {
            "eval.InlineCompleted" => {
                let evaluator = data.get("evaluator")?.as_str()?.to_owned();
                let score = data.get("score")?.as_f64()?;
                let label: ScoreLabel = serde_json::from_value(data.get("label")?.clone()).ok()?;
                let layer: EvalLayer = serde_json::from_value(data.get("layer")?.clone()).ok()?;
                let session_id = data.get("session_id")?.as_str()?.to_owned();
                let run_id = data
                    .get("run_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                let explanation = data
                    .get("explanation")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                Some(Self::InlineCompleted {
                    evaluator,
                    score,
                    label,
                    layer,
                    session_id,
                    run_id,
                    explanation,
                })
            }
            "eval.AsyncCompleted" => {
                let evaluator = data.get("evaluator")?.as_str()?.to_owned();
                let scores: Vec<ScoreSummary> =
                    serde_json::from_value(data.get("scores")?.clone()).ok()?;
                let session_id = data.get("session_id")?.as_str()?.to_owned();
                let run_id = data
                    .get("run_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                let duration_ms = data.get("duration_ms")?.as_u64()?;
                Some(Self::AsyncCompleted {
                    evaluator,
                    scores,
                    session_id,
                    run_id,
                    duration_ms,
                })
            }
            "eval.QualityChanged" => {
                let session_id = data.get("session_id")?.as_str()?.to_owned();
                let aggregate_quality = data.get("aggregate_quality")?.as_f64()?;
                let trend = data.get("trend")?.as_f64()?;
                let inline_count = data.get("inline_count")?.as_u64()? as u32;
                let async_count = data.get("async_count")?.as_u64()? as u32;
                Some(Self::QualityChanged {
                    session_id,
                    aggregate_quality,
                    trend,
                    inline_count,
                    async_count,
                })
            }
            "eval.egri_outcome" => {
                let session_id = data.get("session_id")?.as_str()?.to_owned();
                let trial_id = data
                    .get("trial_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                let outcome = data.get("outcome")?.clone();
                Some(Self::EgriOutcome {
                    session_id,
                    trial_id,
                    outcome,
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taxonomy::EvalTiming;

    #[test]
    fn inline_completed_to_event_kind() {
        let event = NousEvent::InlineCompleted {
            evaluator: "token_efficiency".into(),
            score: 0.85,
            label: ScoreLabel::Good,
            layer: EvalLayer::Execution,
            session_id: "sess-1".into(),
            run_id: Some("run-1".into()),
            explanation: Some("good ratio".into()),
        };
        let kind = event.into_event_kind();
        if let EventKind::Custom { event_type, data } = &kind {
            assert_eq!(event_type, "eval.InlineCompleted");
            assert_eq!(data["evaluator"], "token_efficiency");
            assert_eq!(data["score"], 0.85);
        } else {
            panic!("expected Custom variant");
        }
    }

    #[test]
    fn event_kind_roundtrip_inline() {
        let event = NousEvent::InlineCompleted {
            evaluator: "budget_adherence".into(),
            score: 0.92,
            label: ScoreLabel::Good,
            layer: EvalLayer::Cost,
            session_id: "sess-1".into(),
            run_id: None,
            explanation: None,
        };
        let kind = event.into_event_kind();

        let json = serde_json::to_string(&kind).unwrap();
        let back: EventKind = serde_json::from_str(&json).unwrap();

        if let EventKind::Custom { event_type, data } = back {
            assert_eq!(event_type, "eval.InlineCompleted");
            let parsed = NousEvent::from_custom(&event_type, &data).unwrap();
            assert!(matches!(
                parsed,
                NousEvent::InlineCompleted {
                    evaluator,
                    ..
                } if evaluator == "budget_adherence"
            ));
        } else {
            panic!("expected Custom variant after roundtrip");
        }
    }

    #[test]
    fn quality_changed_roundtrip() {
        let event = NousEvent::QualityChanged {
            session_id: "sess-1".into(),
            aggregate_quality: 0.78,
            trend: 0.02,
            inline_count: 15,
            async_count: 3,
        };
        let kind = event.into_event_kind();
        if let EventKind::Custom { event_type, data } = kind {
            let parsed = NousEvent::from_custom(&event_type, &data).unwrap();
            assert!(matches!(
                parsed,
                NousEvent::QualityChanged {
                    aggregate_quality,
                    ..
                } if (aggregate_quality - 0.78).abs() < f64::EPSILON
            ));
        } else {
            panic!("expected Custom");
        }
    }

    #[test]
    fn is_eval_event_prefix() {
        assert!(NousEvent::is_eval_event("eval.InlineCompleted"));
        assert!(NousEvent::is_eval_event("eval.Anything"));
        assert!(!NousEvent::is_eval_event("autonomic.CostCharged"));
        assert!(!NousEvent::is_eval_event("InlineCompleted"));
    }

    #[test]
    fn from_custom_returns_none_for_non_eval() {
        let result = NousEvent::from_custom("autonomic.CostCharged", &json!({}));
        assert!(result.is_none());
    }

    #[test]
    fn from_inline_score_creates_event() {
        let score = EvalScore::new(
            "test_eval",
            0.75,
            EvalLayer::Action,
            EvalTiming::Inline,
            "sess-1",
        )
        .unwrap()
        .with_explanation("some explanation");

        let event = NousEvent::from_inline_score(&score);
        assert!(matches!(
            event,
            NousEvent::InlineCompleted {
                evaluator,
                score: s,
                ..
            } if evaluator == "test_eval" && (s - 0.75).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn score_summary_from_eval_score() {
        let score =
            EvalScore::new("test", 0.8, EvalLayer::Safety, EvalTiming::Inline, "s").unwrap();
        let summary = ScoreSummary::from(&score);
        assert_eq!(summary.evaluator, "test");
        assert!((summary.value - 0.8).abs() < f64::EPSILON);
        assert_eq!(summary.layer, EvalLayer::Safety);
    }

    #[test]
    fn egri_outcome_roundtrip() {
        let outcome_data = json!({
            "score": {"aggregate": 0.85, "plan_quality": 0.9},
            "constraints_passed": true,
            "constraint_violations": [],
        });
        let event = NousEvent::EgriOutcome {
            session_id: "sess-1".into(),
            trial_id: Some("trial-001".into()),
            outcome: outcome_data,
        };
        let kind = event.into_event_kind();
        if let EventKind::Custom { event_type, data } = kind {
            assert_eq!(event_type, "eval.egri_outcome");
            let parsed = NousEvent::from_custom(&event_type, &data).unwrap();
            match parsed {
                NousEvent::EgriOutcome {
                    session_id,
                    trial_id,
                    outcome,
                } => {
                    assert_eq!(session_id, "sess-1");
                    assert_eq!(trial_id.as_deref(), Some("trial-001"));
                    assert_eq!(outcome["score"]["aggregate"], 0.85);
                }
                _ => panic!("expected EgriOutcome variant"),
            }
        } else {
            panic!("expected Custom variant");
        }
    }

    #[test]
    fn egri_outcome_without_trial_id() {
        let event = NousEvent::EgriOutcome {
            session_id: "sess-2".into(),
            trial_id: None,
            outcome: json!({"score": {"aggregate": 0.5}}),
        };
        let kind = event.into_event_kind();
        if let EventKind::Custom { event_type, data } = kind {
            let parsed = NousEvent::from_custom(&event_type, &data).unwrap();
            assert!(matches!(
                parsed,
                NousEvent::EgriOutcome { trial_id: None, .. }
            ));
        } else {
            panic!("expected Custom variant");
        }
    }
}
