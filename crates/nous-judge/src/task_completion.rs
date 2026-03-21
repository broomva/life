//! Task completion evaluator — did the agent achieve its goal?

use nous_core::{EvalContext, EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

/// Evaluates whether the agent successfully completed its assigned task.
pub struct TaskCompletion;

impl NousEvaluator for TaskCompletion {
    fn name(&self) -> &str {
        "task_completion"
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
    fn task_completion_returns_empty_without_provider() {
        let eval = TaskCompletion;
        let ctx = EvalContext::new("test");
        let scores = eval.evaluate(&ctx).unwrap();
        assert!(scores.is_empty());
    }
}
