//! Plan quality evaluator — LLM-as-judge for reasoning coherence.
//!
//! Assesses whether the agent's plan is logically sound,
//! complete, and well-structured.

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

/// Evaluates the quality of the agent's planning/reasoning.
///
/// Uses an LLM call to assess coherence, completeness, and logical soundness.
/// Runs asynchronously after a run completes.
pub struct PlanQuality;

impl NousEvaluator for PlanQuality {
    fn name(&self) -> &str {
        "plan_quality"
    }

    fn layer(&self) -> EvalLayer {
        EvalLayer::Reasoning
    }

    fn timing(&self) -> EvalTiming {
        EvalTiming::Async
    }

    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
        // Placeholder: async evaluation requires JudgeProvider integration.
        // For now, return empty to indicate insufficient data.
        let _ = ctx;
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_quality_returns_empty_without_provider() {
        let eval = PlanQuality;
        let ctx = EvalContext::new("test");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }
}
