//! EGRI bridge — maps Nous eval results to autoany Outcome format.
//!
//! Produces `serde_json::Value` in the autoany `Outcome` schema
//! without depending on `autoany-core` directly. The JSON output
//! can be deserialized by any autoany consumer.

use serde_json::json;

use crate::score::EvalResult;

/// Convert an `EvalResult` into an autoany-compatible Outcome JSON value.
///
/// The output follows the `autoany_core::Outcome` schema:
/// ```json
/// {
///   "score": { "aggregate": 0.85, "coherence": 0.9, ... },
///   "constraints_passed": true,
///   "constraint_violations": [],
///   "evaluator_metadata": { "evaluator": "...", "duration_ms": 42 }
/// }
/// ```
///
/// Score is a vector with one entry per evaluator score, plus an
/// `aggregate` key with the mean quality score.
pub fn eval_result_to_outcome(result: &EvalResult) -> serde_json::Value {
    let mut score_map = serde_json::Map::new();
    let mut violations = Vec::new();

    // Add each evaluator's score to the vector.
    for score in &result.scores {
        score_map.insert(
            score.evaluator.clone(),
            serde_json::Value::from(score.value),
        );

        // Treat critical scores as constraint violations.
        if score.value < 0.3 {
            violations.push(format!(
                "{}: {:.2} (critical, layer: {})",
                score.evaluator, score.value, score.layer
            ));
        }
    }

    // Add aggregate score.
    let aggregate = result.aggregate_score();
    score_map.insert("aggregate".to_string(), serde_json::Value::from(aggregate));

    let constraints_passed = violations.is_empty();

    json!({
        "score": score_map,
        "constraints_passed": constraints_passed,
        "constraint_violations": violations,
        "evaluator_metadata": {
            "evaluator": result.evaluator,
            "duration_ms": result.duration_ms,
            "score_count": result.scores.len(),
            "timestamp_ms": result.timestamp_ms,
        }
    })
}

/// Convert an `EvalResult` into an EGRI trial event payload.
///
/// This produces the `eval.egri_outcome` custom event data
/// that can be persisted to Lago and consumed by the EGRI loop.
pub fn eval_result_to_trial_event(
    result: &EvalResult,
    session_id: &str,
    trial_id: Option<&str>,
) -> serde_json::Value {
    json!({
        "event_type": "eval.egri_outcome",
        "session_id": session_id,
        "trial_id": trial_id,
        "outcome": eval_result_to_outcome(result),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::{EvalResult, EvalScore};
    use crate::taxonomy::{EvalLayer, EvalTiming};

    fn make_result(scores: Vec<(f64, &str)>) -> EvalResult {
        EvalResult {
            evaluator: "test_evaluator".into(),
            scores: scores
                .into_iter()
                .map(|(value, name)| {
                    EvalScore::new(name, value, EvalLayer::Reasoning, EvalTiming::Async, "s")
                        .unwrap()
                })
                .collect(),
            timestamp_ms: 1000,
            duration_ms: 42,
        }
    }

    #[test]
    fn outcome_has_correct_structure() {
        let result = make_result(vec![(0.9, "coherence"), (0.8, "completeness")]);
        let outcome = eval_result_to_outcome(&result);

        assert!(outcome.get("score").is_some());
        assert!(outcome.get("constraints_passed").is_some());
        assert!(outcome.get("constraint_violations").is_some());
        assert!(outcome.get("evaluator_metadata").is_some());
    }

    #[test]
    fn outcome_score_is_vector() {
        let result = make_result(vec![(0.9, "coherence"), (0.8, "completeness")]);
        let outcome = eval_result_to_outcome(&result);

        let score = outcome.get("score").unwrap().as_object().unwrap();
        assert!((score["coherence"].as_f64().unwrap() - 0.9).abs() < f64::EPSILON);
        assert!((score["completeness"].as_f64().unwrap() - 0.8).abs() < f64::EPSILON);
        assert!((score["aggregate"].as_f64().unwrap() - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn critical_scores_become_violations() {
        let result = make_result(vec![(0.2, "safety"), (0.9, "coherence")]);
        let outcome = eval_result_to_outcome(&result);

        assert!(!outcome["constraints_passed"].as_bool().unwrap());
        let violations = outcome["constraint_violations"].as_array().unwrap();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].as_str().unwrap().contains("safety"));
    }

    #[test]
    fn all_good_scores_pass_constraints() {
        let result = make_result(vec![(0.9, "a"), (0.8, "b"), (0.7, "c")]);
        let outcome = eval_result_to_outcome(&result);

        assert!(outcome["constraints_passed"].as_bool().unwrap());
        assert!(
            outcome["constraint_violations"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn empty_result_produces_valid_outcome() {
        let result = EvalResult {
            evaluator: "empty".into(),
            scores: vec![],
            timestamp_ms: 0,
            duration_ms: 0,
        };
        let outcome = eval_result_to_outcome(&result);

        assert!(outcome["constraints_passed"].as_bool().unwrap());
        let score = outcome.get("score").unwrap().as_object().unwrap();
        assert!((score["aggregate"].as_f64().unwrap()).abs() < f64::EPSILON);
    }

    #[test]
    fn trial_event_has_correct_structure() {
        let result = make_result(vec![(0.85, "quality")]);
        let event = eval_result_to_trial_event(&result, "sess-1", Some("trial-001"));

        assert_eq!(event["event_type"], "eval.egri_outcome");
        assert_eq!(event["session_id"], "sess-1");
        assert_eq!(event["trial_id"], "trial-001");
        assert!(event.get("outcome").is_some());
    }

    #[test]
    fn trial_event_without_trial_id() {
        let result = make_result(vec![(0.85, "quality")]);
        let event = eval_result_to_trial_event(&result, "sess-1", None);

        assert!(event["trial_id"].is_null());
    }

    #[test]
    fn metadata_includes_evaluator_info() {
        let result = make_result(vec![(0.8, "test")]);
        let outcome = eval_result_to_outcome(&result);

        let meta = outcome.get("evaluator_metadata").unwrap();
        assert_eq!(meta["evaluator"], "test_evaluator");
        assert_eq!(meta["duration_ms"], 42);
        assert_eq!(meta["score_count"], 1);
    }
}
