//! Tool correctness evaluator.
//!
//! Measures tool error rate across the run.
//! A high error rate suggests the agent is misusing tools.

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

/// Evaluates tool correctness: `1.0 - (error_count / total_calls)`.
///
/// Score of 1.0 means no tool errors. Score of 0.0 means all calls errored.
pub struct ToolCorrectness;

impl NousEvaluator for ToolCorrectness {
    fn name(&self) -> &str {
        "tool_correctness"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Action
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Inline
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        let (Some(total), Some(errors)) = (ctx.tool_call_count, ctx.tool_error_count) else {
            return Ok(vec![]);
        };

        if total == 0 {
            return Ok(vec![]);
        }

        let error_rate = errors as f64 / total as f64;
        let value = 1.0 - error_rate;

        let score = EvalScore::new(
            self.name(),
            value,
            self.layer(),
            self.timing(),
            &ctx.session_id,
        )?
        .with_explanation(format!("{errors}/{total} tool calls errored"));

        Ok(vec![score])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with_tools(total: u32, errors: u32) -> EvalContext {
        let mut ctx = EvalContext::new("test");
        ctx.tool_call_count = Some(total);
        ctx.tool_error_count = Some(errors);
        ctx
    }

    #[test]
    fn no_errors_scores_one() {
        let eval = ToolCorrectness;
        let ctx = ctx_with_tools(10, 0);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn all_errors_scores_zero() {
        let eval = ToolCorrectness;
        let ctx = ctx_with_tools(5, 5);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value).abs() < f64::EPSILON);
    }

    #[test]
    fn partial_errors() {
        let eval = ToolCorrectness;
        let ctx = ctx_with_tools(10, 3);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn no_tool_calls_returns_empty() {
        let eval = ToolCorrectness;
        let ctx = ctx_with_tools(0, 0);
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn missing_data_returns_empty() {
        let eval = ToolCorrectness;
        let ctx = EvalContext::new("test");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }
}
