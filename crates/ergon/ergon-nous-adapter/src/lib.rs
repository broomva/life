//! Nous-backed implementation of [`ergon::ResponseScorer`].
//!
//! See `docs/architecture/adr/2026-05-22-nous-adapter-for-ergon-scoring.md` (BRO-1225)
//! for the design rationale + open questions.
//!
//! This crate ships the **skeleton only** — the `score` method body is
//! deliberately unimplemented and returns an `Err(...)` pointing back at
//! the ADR. The implementation lands in a follow-up ticket once the open
//! questions §1-3 (HookCtx access, async/sync boundary, metadata keys)
//! are resolved on review.

use std::sync::Arc;

use async_trait::async_trait;
use ergon::ModelResponse;
use ergon_life_hooks::ResponseScorer;
use nous_core::{EvalHook, EvaluatorRegistry};
use serde_json::Value;

/// Nous-backed `ResponseScorer` — translates per-call ergon hook events
/// into a fan-out over the evaluators registered for `self.hook` in the
/// Nous registry.
///
/// Construct with [`NousAdapter::new`]; default hook is
/// [`EvalHook::AfterModelCall`]. Override with [`Self::with_hook`].
///
/// See ADR §1 for why the adapter lives on the ergon side (not inside Nous).
pub struct NousAdapter {
    registry: Arc<EvaluatorRegistry>,
    hook: EvalHook,
}

impl NousAdapter {
    /// Construct an adapter that fans out to evaluators registered for
    /// `EvalHook::AfterModelCall` (the canonical post-inference hook).
    pub fn new(registry: Arc<EvaluatorRegistry>) -> Self {
        Self {
            registry,
            hook: EvalHook::AfterModelCall,
        }
    }

    /// Override the hook the adapter dispatches against. Use sparingly —
    /// the response-scoring path is canonically `AfterModelCall`.
    pub fn with_hook(mut self, hook: EvalHook) -> Self {
        self.hook = hook;
        self
    }

    /// Number of evaluators currently registered for this adapter's hook
    /// in the underlying registry. Useful for fail-open instrumentation
    /// — see ADR §4.
    pub fn evaluator_count(&self) -> usize {
        self.registry.evaluators_for(self.hook).len()
    }
}

#[async_trait]
impl ResponseScorer for NousAdapter {
    async fn score(&self, _response: &ModelResponse) -> Result<Value, String> {
        // Implementation follow-up tracked in BRO-1225 implementation
        // ticket (filed after the ADR review pass).
        //
        // The implementation will:
        //   1. Build an EvalContext from (&HookCtx, &ModelResponse) — see
        //      ADR §2 + Open Question §1 on HookCtx access.
        //   2. Iterate self.registry.evaluators_for(self.hook), calling
        //      evaluator.evaluate(&ctx) on each.
        //   3. Flatten Vec<Vec<EvalScore>> → serde_json::Value array.
        //   4. Handle the four failure-mode branches from ADR §4.
        Err(format!(
            "NousAdapter::score not yet implemented \
             (hook={:?}, evaluators={}); see ADR §1",
            self.hook,
            self.evaluator_count()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_constructs_with_empty_registry() {
        let registry = Arc::new(EvaluatorRegistry::new());
        let adapter = NousAdapter::new(registry);
        assert_eq!(adapter.evaluator_count(), 0);
    }

    #[test]
    fn with_hook_overrides_default() {
        let registry = Arc::new(EvaluatorRegistry::new());
        let adapter = NousAdapter::new(registry).with_hook(EvalHook::BeforeModelCall);
        assert_eq!(adapter.evaluator_count(), 0);
    }
}
