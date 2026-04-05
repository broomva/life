//! Budget adherence evaluator.
//!
//! Measures how well the agent is staying within its token budget.
//! Score drops as the agent consumes a larger fraction of remaining budget.

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

/// Evaluates budget adherence: remaining tokens as fraction of total budget.
///
/// Score = `tokens_remaining / (tokens_used + tokens_remaining)`.
pub struct BudgetAdherence;

impl NousEvaluator for BudgetAdherence {
    fn name(&self) -> &str {
        "budget_adherence"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Cost
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Inline
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        let (Some(remaining), Some(used)) = (ctx.tokens_remaining, ctx.total_tokens_used) else {
            return Ok(vec![]);
        };

        let total = remaining + used;
        if total == 0 {
            return Ok(vec![]);
        }

        let value = remaining as f64 / total as f64;

        let explanation = format!(
            "{remaining} tokens remaining of {total} total ({:.0}% used)",
            (used as f64 / total as f64) * 100.0
        );

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

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with_budget(used: u64, remaining: u64) -> EvalContext {
        let mut ctx = EvalContext::new("test");
        ctx.total_tokens_used = Some(used);
        ctx.tokens_remaining = Some(remaining);
        ctx
    }

    #[test]
    fn full_budget_scores_one() {
        let eval = BudgetAdherence;
        let ctx = ctx_with_budget(0, 100_000);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn half_budget_scores_half() {
        let eval = BudgetAdherence;
        let ctx = ctx_with_budget(50_000, 50_000);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn depleted_budget_scores_zero() {
        let eval = BudgetAdherence;
        let ctx = ctx_with_budget(100_000, 0);
        let scores = eval.evaluate(&ctx).unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].value).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_budget_returns_empty() {
        let eval = BudgetAdherence;
        let ctx = EvalContext::new("test");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }
}
