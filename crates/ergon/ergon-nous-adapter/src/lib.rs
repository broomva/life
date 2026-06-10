//! Nous-backed implementation of [`ergon_life_hooks::ResponseScorer`].
//!
//! See `docs/architecture/adr/2026-05-22-nous-adapter-for-ergon-scoring.md` (BRO-1225)
//! for the design rationale.
//!
//! Implemented 2026-06-10 (harness Phase-2 gap closure): `score` fans
//! the response out over the evaluators registered for the adapter's
//! hook, fail-open per evaluator (ADR §4 — a broken evaluator is
//! recorded in the score object's `failures` array, never aborts the
//! workflow). The ADR's Open §1 (HookCtx/session access) resolved as:
//! the ergon `ResponseScorer` boundary stays narrow; session-series
//! evaluators run via `NousToolObserver` on the Direct path instead.

use std::sync::Arc;

use async_trait::async_trait;
use ergon::ModelResponse;
use ergon_life_hooks::ResponseScorer;
use nous_core::{EvalContext, EvalHook, EvaluatorRegistry};
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
    /// Fan the response out over the evaluators registered for
    /// `self.hook` (ADR §2).
    ///
    /// The `EvalContext` is built from what the `ResponseScorer`
    /// boundary actually exposes — the response's token accounting and
    /// shape. Session identity is not available at this boundary (ADR
    /// Open §1 resolved as: don't widen the ergon trait; evaluators
    /// that need session-level series run via `NousToolObserver` on
    /// the Direct path instead). Failure handling per ADR §4:
    /// individual evaluator errors are collected and reported in the
    /// score object (fail-open) — the call itself only errs on
    /// serialization failure, and the hook layer treats even that as
    /// non-fatal.
    async fn score(&self, response: &ModelResponse) -> Result<Value, String> {
        let evaluators = self.registry.evaluators_for(self.hook);
        let mut ctx = EvalContext::new("ergon-workflow");
        ctx.input_tokens = Some(u64::from(response.usage.input_tokens));
        ctx.output_tokens = Some(u64::from(response.usage.output_tokens));
        ctx.metadata.insert(
            "stop_reason".to_owned(),
            format!("{:?}", response.stop_reason),
        );
        ctx.metadata.insert(
            "content_blocks".to_owned(),
            response.content.len().to_string(),
        );

        let mut scores = Vec::new();
        let mut failures = Vec::new();
        for evaluator in evaluators {
            match evaluator.evaluate(&ctx) {
                Ok(mut produced) => scores.append(&mut produced),
                Err(e) => {
                    tracing::warn!(
                        evaluator = evaluator.name(),
                        error = %e,
                        "nous evaluator failed (fail-open, recorded in score object)"
                    );
                    failures.push(serde_json::json!({
                        "evaluator": evaluator.name(),
                        "error": e.to_string(),
                    }));
                }
            }
        }

        let scores =
            serde_json::to_value(&scores).map_err(|e| format!("serialize EvalScores: {e}"))?;
        Ok(serde_json::json!({
            "hook": format!("{:?}", self.hook),
            "scores": scores,
            "failures": failures,
        }))
    }
}

#[cfg(test)]
mod tests {
    use ergon::{ContentBlock, ModelResponse, StopReason, Usage};
    use nous_core::{EvalLayer, EvalScore, EvalTiming, NousEvaluator, NousResult};

    use super::*;

    struct FixedEvaluator;

    impl NousEvaluator for FixedEvaluator {
        fn name(&self) -> &str {
            "fixed"
        }
        fn layer(&self) -> EvalLayer {
            EvalLayer::Execution
        }
        fn timing(&self) -> EvalTiming {
            EvalTiming::Inline
        }
        fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
            assert_eq!(ctx.output_tokens, Some(7), "usage flows into the context");
            Ok(vec![EvalScore::new(
                "fixed",
                0.9,
                EvalLayer::Execution,
                EvalTiming::Inline,
                ctx.session_id.clone(),
            )?])
        }
    }

    struct BrokenEvaluator;

    impl NousEvaluator for BrokenEvaluator {
        fn name(&self) -> &str {
            "broken"
        }
        fn layer(&self) -> EvalLayer {
            EvalLayer::Execution
        }
        fn timing(&self) -> EvalTiming {
            EvalTiming::Inline
        }
        fn evaluate(&self, _ctx: &EvalContext) -> NousResult<Vec<EvalScore>> {
            Err(nous_core::NousError::Registry("boom".to_owned()))
        }
    }

    fn response() -> ModelResponse {
        let mut usage = Usage::default();
        usage.input_tokens = 3;
        usage.output_tokens = 7;
        ModelResponse::new(vec![ContentBlock::text("hi")], StopReason::EndTurn).with_usage(usage)
    }

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

    #[tokio::test]
    async fn score_fans_out_to_registered_evaluators() {
        let mut registry = EvaluatorRegistry::new();
        registry
            .register(EvalHook::AfterModelCall, Arc::new(FixedEvaluator))
            .expect("register");
        let adapter = NousAdapter::new(Arc::new(registry));
        let value = adapter.score(&response()).await.expect("score ok");
        let scores = value["scores"].as_array().expect("scores array");
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0]["evaluator"], "fixed");
        assert_eq!(scores[0]["value"], 0.9);
        assert_eq!(value["failures"].as_array().map(Vec::len), Some(0));
    }

    #[tokio::test]
    async fn broken_evaluator_fails_open() {
        let mut registry = EvaluatorRegistry::new();
        registry
            .register(EvalHook::AfterModelCall, Arc::new(FixedEvaluator))
            .expect("register fixed");
        registry
            .register(EvalHook::AfterModelCall, Arc::new(BrokenEvaluator))
            .expect("register broken");
        let adapter = NousAdapter::new(Arc::new(registry));
        let value = adapter
            .score(&response())
            .await
            .expect("fail-open: call still succeeds");
        assert_eq!(value["scores"].as_array().map(Vec::len), Some(1));
        let failures = value["failures"].as_array().expect("failures array");
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0]["evaluator"], "broken");
    }

    #[tokio::test]
    async fn empty_registry_scores_empty() {
        let adapter = NousAdapter::new(Arc::new(EvaluatorRegistry::new()));
        let value = adapter.score(&response()).await.expect("score ok");
        assert_eq!(value["scores"].as_array().map(Vec::len), Some(0));
    }
}
