//! Step efficiency evaluator.
//!
//! Measures how many iterations the agent used relative to
//! the maximum allowed. Fewer iterations for successful completion
//! indicates higher efficiency.

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

/// Evaluates step efficiency: `1.0 - (iteration / max_iterations)`.
///
/// Score of 1.0 means the agent completed in the first iteration.
/// Score of 0.0 means it used all available iterations.
pub struct StepEfficiency;

impl NousEvaluator for StepEfficiency {
    fn name(&self) -> &str {
        "step_efficiency"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Execution
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Inline
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        let (Some(iteration), Some(max_iterations)) = (ctx.iteration, ctx.max_iterations) else {
            return Ok(vec![]);
        };

        if max_iterations == 0 {
            return Ok(vec![]);
        }

        let value = 1.0 - (iteration as f64 / max_iterations as f64);
        // Clamp to [0.0, 1.0]
        let value = value.clamp(0.0, 1.0);

        let score = EvalScore::new(
            self.name(),
            value,
            self.layer(),
            self.timing(),
            &ctx.session_id,
        )?
        .with_explanation(format!("iteration {iteration}/{max_iterations}"));

        Ok(vec![score])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with_iterations(current: u32, max: u32) -> EvalContext {
        let mut ctx = EvalContext::new("test");
        ctx.iteration = Some(current);
        ctx.max_iterations = Some(max);
        ctx
    }

    #[test]
    fn first_iteration_scores_high() {
        let eval = StepEfficiency;
        let ctx = ctx_with_iterations(1, 24);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        // 1.0 - 1/24 ≈ 0.958
        assert!(scores[0].value > 0.9);
    }

    #[test]
    fn last_iteration_scores_zero() {
        let eval = StepEfficiency;
        let ctx = ctx_with_iterations(24, 24);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value).abs() < f64::EPSILON);
    }

    #[test]
    fn middle_iteration_scores_half() {
        let eval = StepEfficiency;
        let ctx = ctx_with_iterations(12, 24);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_data_returns_empty() {
        let eval = StepEfficiency;
        let ctx = EvalContext::new("test");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }
}
