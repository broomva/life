//! Plan adherence evaluator — did the agent follow its stated plan?

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

/// Evaluates whether the agent's execution matched its stated plan.
pub struct PlanAdherence;

impl NousEvaluator for PlanAdherence {
    fn name(&self) -> &str {
        "plan_adherence"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Reasoning
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Async
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        let _ = ctx;
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_adherence_returns_empty_without_provider() {
        let eval = PlanAdherence;
        let ctx = EvalContext::new("test");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }
}
