//! `nous_compare` praxis tool — compare two scalar scores with a
//! configurable equality threshold.
//!
//! Used by promoter/judge agents that need a typed verdict
//! (`greater` / `lesser` / `equal` / `within_threshold`) rather than a
//! raw delta. Default threshold is `0.001`, chosen to absorb f64
//! rounding noise on aggregated scores while still catching meaningful
//! differences. Callers that want strict equality can pass `0.0`.

use aios_protocol::tool::{
    Tool, ToolAnnotations, ToolCall, ToolContext, ToolDefinition, ToolError, ToolResult,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Default equality tolerance (sub-thousandth of a score point).
pub const DEFAULT_THRESHOLD: f64 = 0.001;

/// Verdict returned by [`NousCompareTool`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompareVerdict {
    Greater,
    Lesser,
    Equal,
    WithinThreshold,
}

/// Output payload of [`NousCompareTool`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompareOutput {
    /// Signed delta `a - b`.
    pub delta: f64,
    /// `|delta|`.
    pub abs_delta: f64,
    pub verdict: CompareVerdict,
}

/// Praxis tool implementing `nous_compare`.
///
/// Input shape: `{ "a": f64, "b": f64, "threshold": Option<f64> }`.
#[derive(Debug, Clone, Default)]
pub struct NousCompareTool;

impl NousCompareTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for NousCompareTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "nous_compare".into(),
            description: "Compare two scalar scores. Returns delta, abs_delta, and a verdict \
                 (greater / lesser / equal / within_threshold). The optional \
                 'threshold' parameter (default 0.001) collapses near-equal values \
                 into 'within_threshold'."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "a": { "type": "number" },
                    "b": { "type": "number" },
                    "threshold": { "type": "number", "minimum": 0 }
                },
                "required": ["a", "b"]
            }),
            title: Some("Nous Compare".into()),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "delta": { "type": "number" },
                    "abs_delta": { "type": "number", "minimum": 0 },
                    "verdict": {
                        "type": "string",
                        "enum": ["greater", "lesser", "equal", "within_threshold"]
                    }
                },
                "required": ["delta", "abs_delta", "verdict"]
            })),
            annotations: Some(ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            }),
            category: Some("nous".into()),
            tags: vec!["nous".into(), "compare".into()],
            timeout_secs: Some(5),
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let a = extract_number(&call.input, "a")?;
        let b = extract_number(&call.input, "b")?;

        let threshold = match call.input.get("threshold") {
            None => DEFAULT_THRESHOLD,
            Some(v) => {
                let t = v.as_f64().ok_or_else(|| ToolError::InvalidInput {
                    message: "'threshold' must be a non-negative number".into(),
                })?;
                if !t.is_finite() || t < 0.0 {
                    return Err(ToolError::InvalidInput {
                        message: "'threshold' must be a finite non-negative number".into(),
                    });
                }
                t
            }
        };

        let result = compare_scores(a, b, threshold);
        let output = serde_json::to_value(&result).map_err(|e| ToolError::ExecutionFailed {
            tool_name: "nous_compare".into(),
            message: format!("failed to serialize compare output: {e}"),
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

/// Pure comparison helper. Exposed for direct use by other crates.
pub fn compare_scores(a: f64, b: f64, threshold: f64) -> CompareOutput {
    let delta = a - b;
    let abs_delta = delta.abs();
    let verdict = if abs_delta <= threshold {
        // Strict equality wins so the verdict is informative even
        // when threshold is the default 0.001.
        if a == b {
            CompareVerdict::Equal
        } else {
            CompareVerdict::WithinThreshold
        }
    } else if delta > 0.0 {
        CompareVerdict::Greater
    } else {
        CompareVerdict::Lesser
    };
    CompareOutput {
        delta,
        abs_delta,
        verdict,
    }
}

fn extract_number(input: &serde_json::Value, field: &str) -> Result<f64, ToolError> {
    let v = input.get(field).ok_or_else(|| ToolError::InvalidInput {
        message: format!("Missing required field '{field}'"),
    })?;
    let f = v.as_f64().ok_or_else(|| ToolError::InvalidInput {
        message: format!("'{field}' must be a number"),
    })?;
    if !f.is_finite() {
        return Err(ToolError::InvalidInput {
            message: format!("'{field}' must be finite (NaN/inf forbidden)"),
        });
    }
    Ok(f)
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
            tool_name: "nous_compare".into(),
            input,
            requested_capabilities: vec![],
        }
    }

    #[test]
    fn nous_compare_within_threshold_marks_equal() {
        let tool = NousCompareTool::new();
        // 0.0001 difference < 0.001 default threshold → within_threshold.
        let result = tool
            .execute(&call(json!({"a": 1.0001, "b": 1.0})), &ctx())
            .unwrap();
        let out: CompareOutput = serde_json::from_value(result.output).unwrap();
        assert!((out.delta - 0.0001).abs() < 1e-12);
        assert!((out.abs_delta - 0.0001).abs() < 1e-12);
        assert_eq!(out.verdict, CompareVerdict::WithinThreshold);

        // True equality returns the more informative `Equal` verdict.
        let result = tool
            .execute(&call(json!({"a": 1.0, "b": 1.0})), &ctx())
            .unwrap();
        let out: CompareOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(out.delta, 0.0);
        assert_eq!(out.verdict, CompareVerdict::Equal);
    }

    #[test]
    fn nous_compare_greater_returns_positive_delta() {
        let tool = NousCompareTool::new();
        let result = tool
            .execute(&call(json!({"a": 7.5, "b": 4.0})), &ctx())
            .unwrap();
        let out: CompareOutput = serde_json::from_value(result.output).unwrap();
        assert!((out.delta - 3.5).abs() < 1e-12);
        assert!((out.abs_delta - 3.5).abs() < 1e-12);
        assert_eq!(out.verdict, CompareVerdict::Greater);
    }

    #[test]
    fn nous_compare_lesser_returns_negative_delta() {
        let tool = NousCompareTool::new();
        let result = tool
            .execute(&call(json!({"a": 2.0, "b": 9.0})), &ctx())
            .unwrap();
        let out: CompareOutput = serde_json::from_value(result.output).unwrap();
        assert!((out.delta + 7.0).abs() < 1e-12);
        assert!((out.abs_delta - 7.0).abs() < 1e-12);
        assert_eq!(out.verdict, CompareVerdict::Lesser);
    }

    #[test]
    fn nous_compare_custom_threshold_widens_equal_band() {
        let tool = NousCompareTool::new();
        // Difference = 0.5 < threshold 1.0 → within_threshold.
        let result = tool
            .execute(&call(json!({"a": 5.5, "b": 5.0, "threshold": 1.0})), &ctx())
            .unwrap();
        let out: CompareOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(out.verdict, CompareVerdict::WithinThreshold);
    }

    #[test]
    fn nous_compare_zero_threshold_demands_strict_equality() {
        let tool = NousCompareTool::new();
        let result = tool
            .execute(&call(json!({"a": 1.0, "b": 1.0, "threshold": 0.0})), &ctx())
            .unwrap();
        let out: CompareOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(out.verdict, CompareVerdict::Equal);

        let result = tool
            .execute(
                &call(json!({"a": 1.0001, "b": 1.0, "threshold": 0.0})),
                &ctx(),
            )
            .unwrap();
        let out: CompareOutput = serde_json::from_value(result.output).unwrap();
        assert_eq!(out.verdict, CompareVerdict::Greater);
    }

    #[test]
    fn nous_compare_rejects_negative_threshold() {
        let tool = NousCompareTool::new();
        let err = tool
            .execute(
                &call(json!({"a": 1.0, "b": 2.0, "threshold": -0.1})),
                &ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput { .. }));
    }

    #[test]
    fn nous_compare_rejects_missing_fields() {
        let tool = NousCompareTool::new();
        let err = tool.execute(&call(json!({"a": 1.0})), &ctx()).unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput { .. }));
    }
}
