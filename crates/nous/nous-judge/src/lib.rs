//! Async LLM-as-judge evaluators for Nous.
//!
//! These evaluators run asynchronously after agent runs complete.
//! They use a separate model call to assess quality dimensions
//! that require language understanding.

pub mod anthropic_judge;
pub mod judge_provider;
pub mod knowledge_utilization;
pub mod plan_adherence;
pub mod plan_quality;
pub mod reasoning_coherence;
pub mod task_completion;

use std::sync::Arc;

use nous_core::{EvalHook, EvaluatorRegistry, NousResult};

pub use anthropic_judge::AnthropicJudgeProvider;
pub use judge_provider::{JudgeProvider, MockJudgeProvider, parse_judge_scores};
pub use knowledge_utilization::KnowledgeUtilization;
pub use plan_adherence::PlanAdherence;
pub use plan_quality::PlanQuality;
pub use reasoning_coherence::ReasoningCoherence;
pub use task_completion::TaskCompletion;

/// Build a registry containing the async reasoning judges.
///
/// This keeps the LLM-as-judge set separate from the inline heuristics so hosts
/// can opt into async evaluation deliberately.
pub fn registry_with_reasoning(provider: Arc<dyn JudgeProvider>) -> NousResult<EvaluatorRegistry> {
    let mut registry = EvaluatorRegistry::new();
    registry.register(
        EvalHook::OnRunFinished,
        Arc::new(PlanQuality::new(provider.clone())),
    )?;
    registry.register(
        EvalHook::OnRunFinished,
        Arc::new(TaskCompletion::new(provider.clone())),
    )?;
    registry.register(
        EvalHook::OnRunFinished,
        Arc::new(PlanAdherence::new(provider.clone())),
    )?;
    registry.register(
        EvalHook::OnRunFinished,
        Arc::new(ReasoningCoherence::new(provider.clone())),
    )?;
    registry.register(
        EvalHook::OnRunFinished,
        Arc::new(KnowledgeUtilization::new(provider)),
    )?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_with_reasoning_registers_five_async_judges() {
        let registry = registry_with_reasoning(Arc::new(MockJudgeProvider {
            response: "{\"score\": 1.0}".into(),
        }))
        .unwrap();

        assert_eq!(registry.len(), 5);
        assert_eq!(registry.evaluators_for(EvalHook::OnRunFinished).len(), 5);
    }
}
