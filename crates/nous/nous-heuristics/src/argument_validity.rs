//! Argument validity evaluator.
//!
//! Validates tool call arguments against a JSON schema provided in
//! `ctx.metadata["tool_input_schema"]`. Checks required fields and
//! basic type constraints. If no schema is available, returns empty
//! (no penalty for missing metadata).

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};
use serde_json::Value;

/// Evaluates argument validity against a tool's input schema.
///
/// Score interpretation:
/// - 1.0: all required fields present and types match
/// - Degrades per violation (each missing required field or type mismatch
///   reduces score by `1 / total_checks`)
/// - Returns empty if no schema is available in metadata
pub struct ArgumentValidity;

impl NousEvaluator for ArgumentValidity {
    fn name(&self) -> &str {
        "argument_validity"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Action
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Inline
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        // Retrieve schema string from metadata; if absent, skip evaluation.
        let Some(schema_str) = ctx.metadata.get("tool_input_schema") else {
            return Ok(vec![]);
        };

        // Parse schema JSON. If malformed, skip rather than penalize.
        let Ok(schema) = serde_json::from_str::<Value>(schema_str) else {
            return Ok(vec![]);
        };

        // Retrieve the tool arguments; if absent, skip.
        let Some(args_str) = ctx.metadata.get("tool_args") else {
            return Ok(vec![]);
        };

        // Parse arguments JSON. Malformed args is a violation.
        let Ok(args) = serde_json::from_str::<Value>(args_str) else {
            let score = EvalScore::new(
                self.name(),
                0.0,
                self.layer(),
                self.timing(),
                &ctx.session_id,
            )?
            .with_explanation("tool_args is not valid JSON".to_string());
            return Ok(vec![score]);
        };

        let Some(args_obj) = args.as_object() else {
            let score = EvalScore::new(
                self.name(),
                0.0,
                self.layer(),
                self.timing(),
                &ctx.session_id,
            )?
            .with_explanation("tool_args is not a JSON object".to_string());
            return Ok(vec![score]);
        };

        let mut violations: Vec<String> = Vec::new();
        let mut total_checks: u32 = 0;

        // Check required fields.
        if let Some(Value::Array(required)) = schema.get("required") {
            for req in required {
                if let Some(field_name) = req.as_str() {
                    total_checks += 1;
                    if !args_obj.contains_key(field_name) {
                        violations.push(format!("missing required field '{field_name}'"));
                    }
                }
            }
        }

        // Check basic type constraints from "properties".
        if let Some(Value::Object(properties)) = schema.get("properties") {
            for (prop_name, prop_schema) in properties {
                if let Some(arg_value) = args_obj.get(prop_name)
                    && let Some(Value::String(expected_type)) = prop_schema.get("type")
                {
                    total_checks += 1;
                    if !value_matches_type(arg_value, expected_type) {
                        violations.push(format!(
                            "field '{prop_name}' expected type '{expected_type}'"
                        ));
                    }
                }
            }
        }

        // If no checks were performed, nothing to score.
        if total_checks == 0 {
            return Ok(vec![]);
        }

        let violation_count = violations.len() as f64;
        let value = (1.0 - violation_count / total_checks as f64).max(0.0);

        let explanation = if violations.is_empty() {
            format!("{total_checks} checks passed")
        } else {
            format!(
                "{} violation(s): {}",
                violations.len(),
                violations.join("; ")
            )
        };

        let score = EvalScore::new(
            self.name(),
            value,
            self.layer(),
            self.timing(),
            &ctx.session_id,
        )?
        .with_explanation(explanation);

        Ok(vec![score])
    }
}

/// Check if a JSON value matches the expected JSON Schema type string.
fn value_matches_type(value: &Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true, // Unknown type — don't penalize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_with_required(required: &[&str], properties: &str) -> String {
        let req_json: Vec<String> = required.iter().map(|r| format!("\"{r}\"")).collect();
        format!(
            r#"{{"type":"object","required":[{}],"properties":{}}}"#,
            req_json.join(","),
            properties
        )
    }

    fn ctx_with_schema_and_args(schema: &str, args: &str) -> EvalContext {
        let mut ctx = EvalContext::new("test");
        ctx.metadata
            .insert("tool_input_schema".to_string(), schema.to_string());
        ctx.metadata
            .insert("tool_args".to_string(), args.to_string());
        ctx
    }

    #[test]
    fn valid_args_scores_one() {
        let eval = ArgumentValidity;
        let schema = schema_with_required(
            &["name", "count"],
            r#"{"name":{"type":"string"},"count":{"type":"integer"}}"#,
        );
        let args = r#"{"name":"hello","count":42}"#;
        let ctx = ctx_with_schema_and_args(&schema, args);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!(
            (scores[0].value - 1.0).abs() < f64::EPSILON,
            "expected 1.0, got {}",
            scores[0].value
        );
    }

    #[test]
    fn missing_required_field_degrades_score() {
        let eval = ArgumentValidity;
        let schema = schema_with_required(
            &["name", "count"],
            r#"{"name":{"type":"string"},"count":{"type":"integer"}}"#,
        );
        // Missing "count"
        let args = r#"{"name":"hello"}"#;
        let ctx = ctx_with_schema_and_args(&schema, args);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        // 1 violation out of 3 checks (2 required + 1 type check for "name")
        assert!(
            scores[0].value < 1.0,
            "expected < 1.0, got {}",
            scores[0].value
        );
        assert!(
            scores[0].value > 0.0,
            "expected > 0.0, got {}",
            scores[0].value
        );
    }

    #[test]
    fn type_mismatch_degrades_score() {
        let eval = ArgumentValidity;
        let schema = schema_with_required(&["name"], r#"{"name":{"type":"string"}}"#);
        // "name" is a number instead of string
        let args = r#"{"name":123}"#;
        let ctx = ctx_with_schema_and_args(&schema, args);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!(
            scores[0].value < 1.0,
            "expected < 1.0, got {}",
            scores[0].value
        );
    }

    #[test]
    fn no_schema_in_metadata_returns_empty() {
        let eval = ArgumentValidity;
        let mut ctx = EvalContext::new("test");
        ctx.metadata
            .insert("tool_args".to_string(), r#"{"a":1}"#.to_string());
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn empty_metadata_returns_empty() {
        let eval = ArgumentValidity;
        let ctx = EvalContext::new("test");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn malformed_args_scores_zero() {
        let eval = ArgumentValidity;
        let schema = schema_with_required(&["name"], r#"{"name":{"type":"string"}}"#);
        let mut ctx = EvalContext::new("test");
        ctx.metadata.insert("tool_input_schema".to_string(), schema);
        ctx.metadata
            .insert("tool_args".to_string(), "not valid json".to_string());
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value).abs() < f64::EPSILON);
    }

    #[test]
    fn all_required_missing_scores_zero() {
        let eval = ArgumentValidity;
        let schema = schema_with_required(
            &["a", "b", "c"],
            r#"{"a":{"type":"string"},"b":{"type":"string"},"c":{"type":"string"}}"#,
        );
        let args = r#"{}"#;
        let ctx = ctx_with_schema_and_args(&schema, args);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value).abs() < f64::EPSILON);
    }
}
