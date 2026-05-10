//! `nous_aggregate` praxis tool — descriptive statistics over a vector
//! of f64 scores.
//!
//! Designed for authored agents (BRO-1011 nous-promoter, bookkeeping
//! scorers) that need a single quick summary line over historical
//! lineage scores. Hand-rolled because it would not be worth taking
//! a `statrs` / `medians` workspace dep for ~20 LOC; the tradeoff is
//! flagged at the top of [`aggregate_scores`].

use aios_protocol::tool::{
    Tool, ToolAnnotations, ToolCall, ToolContext, ToolDefinition, ToolError, ToolResult,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Statistics returned by [`NousAggregateTool`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AggregateOutput {
    pub count: usize,
    pub mean: f64,
    pub median: f64,
    pub min: f64,
    pub max: f64,
    /// Sample standard deviation (Bessel-corrected, n-1 in the
    /// denominator). Reported as `0.0` when `count <= 1`.
    pub stddev: f64,
}

/// Praxis tool implementing `nous_aggregate`.
///
/// Input shape: `{ "scores": [f64, ...] }`. Output is an
/// [`AggregateOutput`] serialized to JSON.
#[derive(Debug, Clone, Default)]
pub struct NousAggregateTool;

impl NousAggregateTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for NousAggregateTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "nous_aggregate".into(),
            description: "Compute count / mean / median / min / max / stddev over a vector of f64 \
                 scores. Empty input returns count=0 and zero-valued stats."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scores": {
                        "type": "array",
                        "items": { "type": "number" },
                        "description": "List of numeric scores to aggregate."
                    }
                },
                "required": ["scores"]
            }),
            title: Some("Nous Aggregate".into()),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "count": { "type": "integer", "minimum": 0 },
                    "mean": { "type": "number" },
                    "median": { "type": "number" },
                    "min": { "type": "number" },
                    "max": { "type": "number" },
                    "stddev": { "type": "number", "minimum": 0 }
                },
                "required": ["count", "mean", "median", "min", "max", "stddev"]
            })),
            annotations: Some(ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            }),
            category: Some("nous".into()),
            tags: vec!["nous".into(), "stats".into(), "aggregate".into()],
            timeout_secs: Some(5),
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let scores_val = call
            .input
            .get("scores")
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing required field 'scores'".into(),
            })?;

        let scores_arr = scores_val
            .as_array()
            .ok_or_else(|| ToolError::InvalidInput {
                message: "'scores' must be a JSON array of numbers".into(),
            })?;

        let mut scores: Vec<f64> = Vec::with_capacity(scores_arr.len());
        for (i, v) in scores_arr.iter().enumerate() {
            let f = v.as_f64().ok_or_else(|| ToolError::InvalidInput {
                message: format!("scores[{i}] is not a number"),
            })?;
            if !f.is_finite() {
                return Err(ToolError::InvalidInput {
                    message: format!("scores[{i}] is not finite (NaN/inf forbidden)"),
                });
            }
            scores.push(f);
        }

        let stats = aggregate_scores(&scores);
        let output = serde_json::to_value(&stats).map_err(|e| ToolError::ExecutionFailed {
            tool_name: "nous_aggregate".into(),
            message: format!("failed to serialize aggregate output: {e}"),
        })?;

        Ok(ToolResult {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            output,
            content: None,
            is_error: false,
            usage: None,
        })
    }
}

/// Pure-Rust statistics over an `&[f64]`. Hand-rolled to avoid taking
/// a dependency for ~20 LOC of arithmetic.
///
/// Empty input returns the zero summary; single-element input returns
/// `mean == median == min == max` and `stddev == 0.0`.
pub fn aggregate_scores(scores: &[f64]) -> AggregateOutput {
    if scores.is_empty() {
        return AggregateOutput {
            count: 0,
            mean: 0.0,
            median: 0.0,
            min: 0.0,
            max: 0.0,
            stddev: 0.0,
        };
    }

    let count = scores.len();
    let sum: f64 = scores.iter().sum();
    let mean = sum / count as f64;

    let mut sorted: Vec<f64> = scores.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let min = sorted[0];
    let max = sorted[count - 1];

    let median = if count.is_multiple_of(2) {
        (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0
    } else {
        sorted[count / 2]
    };

    let stddev = if count <= 1 {
        0.0
    } else {
        let var: f64 =
            scores.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / (count as f64 - 1.0);
        var.sqrt()
    };

    AggregateOutput {
        count,
        mean,
        median,
        min,
        max,
        stddev,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext {
            run_id: "test-run".into(),
            session_id: "S".into(),
            iteration: 0,
            ..Default::default()
        }
    }

    fn call(input: serde_json::Value) -> ToolCall {
        ToolCall {
            call_id: "call-1".into(),
            tool_name: "nous_aggregate".into(),
            input,
            requested_capabilities: vec![],
        }
    }

    #[test]
    fn nous_aggregate_empty_input_returns_zero_count() {
        let tool = NousAggregateTool::new();
        let result = tool.execute(&call(json!({"scores": []})), &ctx()).unwrap();
        assert!(!result.is_error);
        let out: AggregateOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(out.count, 0);
        assert_eq!(out.mean, 0.0);
        assert_eq!(out.median, 0.0);
        assert_eq!(out.min, 0.0);
        assert_eq!(out.max, 0.0);
        assert_eq!(out.stddev, 0.0);
    }

    #[test]
    fn nous_aggregate_single_score_mean_eq_median_eq_score() {
        let tool = NousAggregateTool::new();
        let result = tool
            .execute(&call(json!({"scores": [4.2]})), &ctx())
            .unwrap();
        let out: AggregateOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(out.count, 1);
        assert_eq!(out.mean, 4.2);
        assert_eq!(out.median, 4.2);
        assert_eq!(out.min, 4.2);
        assert_eq!(out.max, 4.2);
        // n=1 → stddev defined as 0 (Bessel correction undefined).
        assert_eq!(out.stddev, 0.0);
    }

    #[test]
    fn nous_aggregate_known_set_produces_known_stats() {
        // Set: [2, 4, 4, 4, 5, 5, 7, 9]
        // mean = 5.0, median = (4+5)/2 = 4.5, min = 2, max = 9
        // sample stddev (n-1) = sqrt(32/7) = 2.13808993...
        let tool = NousAggregateTool::new();
        let result = tool
            .execute(
                &call(json!({"scores": [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]})),
                &ctx(),
            )
            .unwrap();
        let out: AggregateOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(out.count, 8);
        assert!((out.mean - 5.0).abs() < 1e-9);
        assert!((out.median - 4.5).abs() < 1e-9);
        assert_eq!(out.min, 2.0);
        assert_eq!(out.max, 9.0);
        let expected_stddev = (32.0_f64 / 7.0_f64).sqrt();
        assert!(
            (out.stddev - expected_stddev).abs() < 1e-9,
            "stddev = {} expected = {}",
            out.stddev,
            expected_stddev
        );
    }

    #[test]
    fn nous_aggregate_rejects_non_array() {
        let tool = NousAggregateTool::new();
        let err = tool
            .execute(&call(json!({"scores": "not-an-array"})), &ctx())
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput { .. }));
    }

    #[test]
    fn nous_aggregate_rejects_nan() {
        let tool = NousAggregateTool::new();
        let err = tool
            .execute(&call(json!({"scores": [1.0, "not-a-number"]})), &ctx())
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput { .. }));
    }
}
