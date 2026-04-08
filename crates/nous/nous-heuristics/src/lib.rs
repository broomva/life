//! Inline heuristic evaluators for Nous.
//!
//! Each evaluator completes in < 2ms with no I/O.
//! They measure different aspects of agent behavior quality.

pub mod argument_validity;
pub mod budget_adherence;
pub mod knowledge;
pub mod safety_compliance;
pub mod step_efficiency;
pub mod token_efficiency;
pub mod tool_correctness;

pub use argument_validity::ArgumentValidity;
pub use budget_adherence::BudgetAdherence;
pub use knowledge::{
    KnowledgeCoherenceEvaluator, KnowledgeCoverageEvaluator, KnowledgeFreshnessEvaluator,
};
pub use safety_compliance::SafetyCompliance;
pub use step_efficiency::StepEfficiency;
pub use token_efficiency::TokenEfficiency;
pub use tool_correctness::ToolCorrectness;

use std::sync::{Arc, RwLock};

use lago_knowledge::KnowledgeIndex;
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

    // pre_tool_call evaluators
    registry.register(EvalHook::PreToolCall, Arc::new(ArgumentValidity))?;

    // post_tool_call evaluators
    registry.register(EvalHook::PostToolCall, Arc::new(SafetyCompliance))?;

    // on_run_finished evaluators
    registry.register(EvalHook::OnRunFinished, Arc::new(ToolCorrectness))?;
    registry.register(EvalHook::OnRunFinished, Arc::new(StepEfficiency))?;

    Ok(registry)
}

/// Build a registry with the default heuristic evaluators plus knowledge evaluators.
///
/// Requires a [`KnowledgeIndex`] — call this when `wiki_dir` is configured
/// and a knowledge index is available. The knowledge evaluators run at
/// `OnRunFinished` and measure freshness, coherence, and coverage of
/// the agent's knowledge substrate.
///
/// This is the recommended registry for production Arcan instances
/// that have Lago knowledge integration enabled.
pub fn registry_with_knowledge(
    index: Arc<RwLock<KnowledgeIndex>>,
) -> NousResult<EvaluatorRegistry> {
    let mut registry = default_registry()?;

    registry.register(
        EvalHook::OnRunFinished,
        Arc::new(KnowledgeFreshnessEvaluator::with_defaults(index.clone())),
    )?;
    registry.register(
        EvalHook::OnRunFinished,
        Arc::new(KnowledgeCoherenceEvaluator::new(index.clone())),
    )?;
    registry.register(
        EvalHook::OnRunFinished,
        Arc::new(KnowledgeCoverageEvaluator::new(index)),
    )?;

    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lago_core::ManifestEntry;
    use lago_store::BlobStore;

    #[test]
    fn default_registry_has_six_evaluators() {
        let registry = default_registry().unwrap();
        assert_eq!(registry.len(), 6);
    }

    #[test]
    fn default_registry_hooks_wired() {
        let registry = default_registry().unwrap();
        assert_eq!(registry.evaluators_for(EvalHook::AfterModelCall).len(), 2);
        assert_eq!(registry.evaluators_for(EvalHook::PreToolCall).len(), 1);
        assert_eq!(registry.evaluators_for(EvalHook::PostToolCall).len(), 1);
        assert_eq!(registry.evaluators_for(EvalHook::OnRunFinished).len(), 2);
        assert!(
            registry
                .evaluators_for(EvalHook::BeforeModelCall)
                .is_empty()
        );
    }

    #[test]
    fn registry_with_knowledge_has_nine_evaluators() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::open(tmp.path()).unwrap();

        // Build a minimal knowledge index with two linked pages.
        let content_a = "# A\n\nSee [[B]].";
        let content_b = "# B\n\nSee [[A]].";
        let hash_a = store.put(content_a.as_bytes()).unwrap();
        let hash_b = store.put(content_b.as_bytes()).unwrap();

        let entries = vec![
            ManifestEntry {
                path: "/a.md".to_string(),
                blob_hash: hash_a,
                size_bytes: content_a.len() as u64,
                content_type: Some("text/markdown".to_string()),
                updated_at: 0,
            },
            ManifestEntry {
                path: "/b.md".to_string(),
                blob_hash: hash_b,
                size_bytes: content_b.len() as u64,
                content_type: Some("text/markdown".to_string()),
                updated_at: 0,
            },
        ];

        let index = KnowledgeIndex::build(&entries, &store).unwrap();
        let index = Arc::new(RwLock::new(index));

        let registry = registry_with_knowledge(index).unwrap();
        // 6 default + 3 knowledge = 9 total
        assert_eq!(registry.len(), 9);
    }

    #[test]
    fn registry_with_knowledge_adds_to_on_run_finished() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::open(tmp.path()).unwrap();
        let index = KnowledgeIndex::build(&[], &store).unwrap();
        let index = Arc::new(RwLock::new(index));

        let registry = registry_with_knowledge(index).unwrap();
        // default has 2 OnRunFinished + 3 knowledge = 5
        assert_eq!(registry.evaluators_for(EvalHook::OnRunFinished).len(), 5);
        // Other hooks unchanged
        assert_eq!(registry.evaluators_for(EvalHook::AfterModelCall).len(), 2);
        assert_eq!(registry.evaluators_for(EvalHook::PreToolCall).len(), 1);
        assert_eq!(registry.evaluators_for(EvalHook::PostToolCall).len(), 1);
    }
}
