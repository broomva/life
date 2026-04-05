//! Evaluation score types.
//!
//! `EvalScore` is the atomic unit of evaluation output.
//! `EvalResult` groups multiple scores from a single evaluator invocation.
//! Both are OTel-aligned for emission as `gen_ai.evaluation.result` span events.

use serde::{Deserialize, Serialize};

use crate::error::{NousError, NousResult};
use crate::taxonomy::{EvalLayer, EvalTiming};

/// A single evaluation score.
///
/// Designed to map directly to an OpenTelemetry `gen_ai.evaluation.result` span event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalScore {
    /// Name of the evaluator that produced this score (e.g. `token_efficiency`).
    pub evaluator: String,
    /// Normalized score value in `[0.0, 1.0]`. Higher is better.
    pub value: f64,
    /// Categorical label (e.g. "good", "warning", "critical").
    pub label: ScoreLabel,
    /// Which layer of agent behavior this evaluates.
    pub layer: EvalLayer,
    /// Whether this was computed inline or async.
    pub timing: EvalTiming,
    /// Optional human-readable explanation.
    pub explanation: Option<String>,
    /// Session ID this score belongs to.
    pub session_id: String,
    /// Run ID within the session (if applicable).
    pub run_id: Option<String>,
}

impl EvalScore {
    /// Create a new score, validating the value is in `[0.0, 1.0]`.
    pub fn new(
        evaluator: impl Into<String>,
        value: f64,
        layer: EvalLayer,
        timing: EvalTiming,
        session_id: impl Into<String>,
    ) -> NousResult<Self> {
        if !(0.0..=1.0).contains(&value) {
            return Err(NousError::ScoreOutOfRange { value });
        }
        Ok(Self {
            evaluator: evaluator.into(),
            value,
            label: ScoreLabel::from_value(value),
            layer,
            timing,
            explanation: None,
            session_id: session_id.into(),
            run_id: None,
        })
    }

    /// Set the explanation.
    pub fn with_explanation(mut self, explanation: impl Into<String>) -> Self {
        self.explanation = Some(explanation.into());
        self
    }

    /// Set the run ID.
    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }
}

/// Categorical score label derived from the numeric value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoreLabel {
    /// Score >= 0.8 — excellent quality.
    Good,
    /// Score >= 0.5 — acceptable but could improve.
    Warning,
    /// Score < 0.5 — needs attention.
    Critical,
}

impl ScoreLabel {
    /// Derive label from a normalized score value.
    pub fn from_value(value: f64) -> Self {
        if value >= 0.8 {
            Self::Good
        } else if value >= 0.5 {
            Self::Warning
        } else {
            Self::Critical
        }
    }

    /// String representation for OpenTelemetry attributes.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// A collection of scores from a single evaluator invocation.
///
/// Async evaluators (like LLM-as-judge) may produce multiple scores
/// in a single call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    /// The evaluator that produced these scores.
    pub evaluator: String,
    /// Individual scores.
    pub scores: Vec<EvalScore>,
    /// Timestamp of evaluation (ms since epoch).
    pub timestamp_ms: u64,
    /// Duration of the evaluation (ms).
    pub duration_ms: u64,
}

impl EvalResult {
    /// Aggregate quality score: mean of all score values.
    pub fn aggregate_score(&self) -> f64 {
        if self.scores.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.scores.iter().map(|s| s.value).sum();
        sum / self.scores.len() as f64
    }

    /// Worst score label across all scores.
    pub fn worst_label(&self) -> ScoreLabel {
        self.scores
            .iter()
            .map(|s| s.label)
            .min_by_key(|l| match l {
                ScoreLabel::Critical => 0,
                ScoreLabel::Warning => 1,
                ScoreLabel::Good => 2,
            })
            .unwrap_or(ScoreLabel::Good)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_score_new_valid() {
        let score = EvalScore::new(
            "token_efficiency",
            0.85,
            EvalLayer::Execution,
            EvalTiming::Inline,
            "sess-1",
        )
        .unwrap();
        assert_eq!(score.evaluator, "token_efficiency");
        assert!((score.value - 0.85).abs() < f64::EPSILON);
        assert_eq!(score.label, ScoreLabel::Good);
    }

    #[test]
    fn eval_score_rejects_out_of_range() {
        let result = EvalScore::new("test", 1.5, EvalLayer::Cost, EvalTiming::Inline, "s");
        assert!(result.is_err());

        let result = EvalScore::new("test", -0.1, EvalLayer::Cost, EvalTiming::Inline, "s");
        assert!(result.is_err());
    }

    #[test]
    fn eval_score_boundary_values() {
        assert!(EvalScore::new("test", 0.0, EvalLayer::Cost, EvalTiming::Inline, "s").is_ok());
        assert!(EvalScore::new("test", 1.0, EvalLayer::Cost, EvalTiming::Inline, "s").is_ok());
    }

    #[test]
    fn score_label_from_value() {
        assert_eq!(ScoreLabel::from_value(0.95), ScoreLabel::Good);
        assert_eq!(ScoreLabel::from_value(0.80), ScoreLabel::Good);
        assert_eq!(ScoreLabel::from_value(0.79), ScoreLabel::Warning);
        assert_eq!(ScoreLabel::from_value(0.50), ScoreLabel::Warning);
        assert_eq!(ScoreLabel::from_value(0.49), ScoreLabel::Critical);
        assert_eq!(ScoreLabel::from_value(0.0), ScoreLabel::Critical);
    }

    #[test]
    fn eval_score_with_explanation() {
        let score = EvalScore::new("test", 0.7, EvalLayer::Action, EvalTiming::Inline, "s")
            .unwrap()
            .with_explanation("tool error rate elevated");
        assert_eq!(
            score.explanation.as_deref(),
            Some("tool error rate elevated")
        );
    }

    #[test]
    fn eval_score_with_run_id() {
        let score = EvalScore::new("test", 0.7, EvalLayer::Action, EvalTiming::Inline, "s")
            .unwrap()
            .with_run_id("run-1");
        assert_eq!(score.run_id.as_deref(), Some("run-1"));
    }

    #[test]
    fn eval_score_serde_roundtrip() {
        let score = EvalScore::new("test", 0.75, EvalLayer::Reasoning, EvalTiming::Async, "s")
            .unwrap()
            .with_explanation("decent reasoning");
        let json = serde_json::to_string(&score).unwrap();
        let back: EvalScore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.evaluator, "test");
        assert!((back.value - 0.75).abs() < f64::EPSILON);
        assert_eq!(back.label, ScoreLabel::Warning);
    }

    #[test]
    fn eval_result_aggregate_score() {
        let result = EvalResult {
            evaluator: "test".into(),
            scores: vec![
                EvalScore::new("a", 0.8, EvalLayer::Action, EvalTiming::Inline, "s").unwrap(),
                EvalScore::new("b", 0.6, EvalLayer::Action, EvalTiming::Inline, "s").unwrap(),
            ],
            timestamp_ms: 1000,
            duration_ms: 5,
        };
        assert!((result.aggregate_score() - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_result_aggregate_empty() {
        let result = EvalResult {
            evaluator: "test".into(),
            scores: vec![],
            timestamp_ms: 0,
            duration_ms: 0,
        };
        assert!((result.aggregate_score()).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_result_worst_label() {
        let result = EvalResult {
            evaluator: "test".into(),
            scores: vec![
                EvalScore::new("a", 0.9, EvalLayer::Action, EvalTiming::Inline, "s").unwrap(),
                EvalScore::new("b", 0.3, EvalLayer::Action, EvalTiming::Inline, "s").unwrap(),
            ],
            timestamp_ms: 1000,
            duration_ms: 5,
        };
        assert_eq!(result.worst_label(), ScoreLabel::Critical);
    }
}
