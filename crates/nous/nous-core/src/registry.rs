//! Evaluator registry — manages collections of evaluators by hook point.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{NousError, NousResult};
use crate::evaluator::{EvalHook, NousEvaluator};

/// Registry of evaluators organized by hook point.
///
/// Each evaluator is registered to one or more hooks where it should
/// be invoked during the agent lifecycle.
pub struct EvaluatorRegistry {
    /// Evaluators indexed by hook point.
    hooks: HashMap<EvalHook, Vec<Arc<dyn NousEvaluator>>>,
    /// All registered evaluator names (for dedup checking).
    names: Vec<String>,
}

impl EvaluatorRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
            names: Vec::new(),
        }
    }

    /// Register an evaluator for a specific hook point.
    ///
    /// Returns an error if an evaluator with the same name is already registered.
    pub fn register(
        &mut self,
        hook: EvalHook,
        evaluator: Arc<dyn NousEvaluator>,
    ) -> NousResult<()> {
        let name = evaluator.name().to_owned();
        if self.names.contains(&name) {
            return Err(NousError::Registry(format!(
                "evaluator '{name}' already registered"
            )));
        }
        self.names.push(name);
        self.hooks.entry(hook).or_default().push(evaluator);
        Ok(())
    }

    /// Get all evaluators registered for a hook point.
    pub fn evaluators_for(&self, hook: EvalHook) -> &[Arc<dyn NousEvaluator>] {
        self.hooks.get(&hook).map_or(&[], |v| v.as_slice())
    }

    /// Total number of registered evaluators.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// List all registered evaluator names.
    pub fn evaluator_names(&self) -> &[String] {
        &self.names
    }
}

impl Default for EvaluatorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluator::EvalContext;
    use crate::score::EvalScore;
    use crate::taxonomy::{EvalLayer, EvalTiming};

    struct StubEvaluator {
        name: String,
    }

    impl NousEvaluator for StubEvaluator {
        fn name(&self) -> &str {
            &self.name
        }

        fn layer(&self) -> EvalLayer {
            EvalLayer::Execution
        }

        fn timing(&self) -> EvalTiming {
            EvalTiming::Inline
        }

        fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
            Ok(vec![EvalScore::new(
                self.name(),
                0.9,
                self.layer(),
                self.timing(),
                &ctx.session_id,
            )?])
        }
    }

    #[test]
    fn register_and_lookup() {
        let mut registry = EvaluatorRegistry::new();
        let evaluator = Arc::new(StubEvaluator {
            name: "test_eval".into(),
        });
        registry
            .register(EvalHook::AfterModelCall, evaluator)
            .unwrap();

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.evaluators_for(EvalHook::AfterModelCall).len(), 1);
        assert!(registry.evaluators_for(EvalHook::PreToolCall).is_empty());
    }

    #[test]
    fn duplicate_name_rejected() {
        let mut registry = EvaluatorRegistry::new();
        let eval1 = Arc::new(StubEvaluator { name: "dup".into() });
        let eval2 = Arc::new(StubEvaluator { name: "dup".into() });
        registry.register(EvalHook::AfterModelCall, eval1).unwrap();
        let result = registry.register(EvalHook::PreToolCall, eval2);
        assert!(result.is_err());
    }

    #[test]
    fn empty_registry() {
        let registry = EvaluatorRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.evaluators_for(EvalHook::OnRunFinished).is_empty());
    }

    #[test]
    fn evaluator_names_listed() {
        let mut registry = EvaluatorRegistry::new();
        registry
            .register(
                EvalHook::AfterModelCall,
                Arc::new(StubEvaluator {
                    name: "alpha".into(),
                }),
            )
            .unwrap();
        registry
            .register(
                EvalHook::PostToolCall,
                Arc::new(StubEvaluator {
                    name: "beta".into(),
                }),
            )
            .unwrap();

        let names = registry.evaluator_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
    }
}
