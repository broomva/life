//! Inline heuristic evaluators for Nous.
//!
//! Each evaluator completes in < 2ms with no I/O.
//! They measure different aspects of agent behavior quality.

pub mod budget_adherence;
pub mod safety_compliance;
pub mod step_efficiency;
pub mod token_efficiency;
pub mod tool_correctness;

pub use budget_adherence::BudgetAdherence;
pub use safety_compliance::SafetyCompliance;
pub use step_efficiency::StepEfficiency;
pub use token_efficiency::TokenEfficiency;
pub use tool_correctness::ToolCorrectness;

use std::sync::Arc;

use nous_core::{EvalHook, EvaluatorRegistry, NousResult};

/// Build a registry with the default set of inline heuristic evaluators.
pub fn default_registry() -> NousResult<EvaluatorRegistry> {
    let mut registry = EvaluatorRegistry::new();

    // after_model_call evaluators
    registry.register(
        EvalHook::AfterModelCall,
        Arc::new(TokenEfficiency::default()),
    )?;
    registry.register(EvalHook::AfterModelCall, Arc::new(BudgetAdherence))?;

    // post_tool_call evaluators
    registry.register(EvalHook::PostToolCall, Arc::new(SafetyCompliance))?;

    // on_run_finished evaluators
    registry.register(EvalHook::OnRunFinished, Arc::new(ToolCorrectness))?;
    registry.register(EvalHook::OnRunFinished, Arc::new(StepEfficiency))?;

    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_has_five_evaluators() {
        let registry = default_registry().unwrap();
        assert_eq!(registry.len(), 5);
    }

    #[test]
    fn default_registry_hooks_wired() {
        let registry = default_registry().unwrap();
        assert_eq!(registry.evaluators_for(EvalHook::AfterModelCall).len(), 2);
        assert_eq!(registry.evaluators_for(EvalHook::PostToolCall).len(), 1);
        assert_eq!(registry.evaluators_for(EvalHook::OnRunFinished).len(), 2);
        assert!(
            registry
                .evaluators_for(EvalHook::BeforeModelCall)
                .is_empty()
        );
        assert!(registry.evaluators_for(EvalHook::PreToolCall).is_empty());
    }
}
