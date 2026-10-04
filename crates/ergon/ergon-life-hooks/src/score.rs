//! Response-scoring hook — observes model output and records a score
//! against the metacognitive evaluator.
//!
//! Production wiring: the arcan adapter (BRO-1001) implements
//! [`ResponseScorer`] against `nous_core::NousEvaluator`, scoring each
//! turn's response and persisting the result via lago events.
//!
//! ## v0.1 scope: observe-only
//!
//! `NousScoreHook` does **not** veto inference. It only observes. The
//! response is scored, the score is recorded, and the outcome is always
//! [`HookOutcome::Continue`]. Score-based denial is a v0.2 capability —
//! it would require a richer return type from [`ResponseScorer`] (e.g.,
//! a threshold + reason).

use async_trait::async_trait;
use ergon::{
    Hook, HookCtx, HookOutcome, InferenceHookOutcome, ModelRequest, ModelResponse, Result,
    ToolCall, ToolHookOutcome, ToolResult,
};
use std::sync::Arc;

/// Adapter trait — implementer scores a model response.
///
/// In production: implemented in the arcan adapter against
/// `nous_core::NousEvaluator`. The returned JSON value is the canonical
/// score representation (e.g. `{"novelty": 2, "specificity": 3,
/// "relevance": 3, "total": 8}`); ergon does not impose a schema.
#[async_trait]
pub trait ResponseScorer: Send + Sync {
    /// Score the response. Errors are non-fatal; the hook records the
    /// failure on the trace span and returns [`HookOutcome::Continue`].
    async fn score(
        &self,
        response: &ModelResponse,
    ) -> std::result::Result<serde_json::Value, String>;
}

/// Hook that fires `on_post_inference` and records a metacognitive
/// score for the response.
pub struct NousScoreHook {
    scorer: Arc<dyn ResponseScorer>,
}

impl NousScoreHook {
    /// Construct from any [`ResponseScorer`].
    pub fn new(scorer: Arc<dyn ResponseScorer>) -> Self {
        Self { scorer }
    }
}

#[async_trait]
impl Hook for NousScoreHook {
    fn name(&self) -> &str {
        "nous-score"
    }

    async fn on_post_inference(
        &self,
        ctx: &HookCtx<'_>,
        response: &ModelResponse,
    ) -> Result<HookOutcome> {
        match self.scorer.score(response).await {
            Ok(score) => {
                tracing::info!(
                    parent: ctx.trace,
                    workflow = ctx.workflow_name,
                    score = ?score,
                    "nous-score recorded",
                );
            }
            Err(reason) => {
                tracing::warn!(
                    parent: ctx.trace,
                    workflow = ctx.workflow_name,
                    error = %reason,
                    "nous-score failed (non-fatal)",
                );
            }
        }
        Ok(HookOutcome::Continue)
    }

    async fn on_workflow_start(&self, _: &HookCtx<'_>) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }
    async fn on_workflow_end(&self, _: &HookCtx<'_>, _: bool) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }
    async fn on_step_start(&self, _: &HookCtx<'_>, _: &str) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }
    async fn on_step_end(&self, _: &HookCtx<'_>, _: &str, _: bool) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }
    async fn on_pre_inference(
        &self,
        _: &HookCtx<'_>,
        _: &mut ModelRequest,
    ) -> Result<InferenceHookOutcome> {
        Ok(InferenceHookOutcome::Continue)
    }
    async fn on_pre_tool_use(&self, _: &HookCtx<'_>, _: &mut ToolCall) -> Result<ToolHookOutcome> {
        Ok(ToolHookOutcome::Continue)
    }
    async fn on_post_tool_use(
        &self,
        _: &HookCtx<'_>,
        _: &ToolCall,
        _: &mut ToolResult,
    ) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ergon::{ContentBlock, SessionId, StopReason};
    use std::sync::Mutex;

    /// Mock scorer that records every call and returns a canned score.
    struct RecorderScorer {
        score: serde_json::Value,
        calls: Mutex<u32>,
    }

    #[async_trait]
    impl ResponseScorer for RecorderScorer {
        async fn score(
            &self,
            _response: &ModelResponse,
        ) -> std::result::Result<serde_json::Value, String> {
            *self.calls.lock().expect("lock") += 1;
            Ok(self.score.clone())
        }
    }

    /// Mock scorer that always errors.
    struct FailingScorer;

    #[async_trait]
    impl ResponseScorer for FailingScorer {
        async fn score(
            &self,
            _response: &ModelResponse,
        ) -> std::result::Result<serde_json::Value, String> {
            Err("evaluator offline".into())
        }
    }

    fn ctx<'a>(span: &'a tracing::Span) -> HookCtx<'a> {
        HookCtx::new(
            SessionId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            "wf",
            span,
        )
    }

    fn sample_response() -> ModelResponse {
        ModelResponse::new(vec![ContentBlock::text("hi")], StopReason::EndTurn)
    }

    #[tokio::test]
    async fn successful_score_continues_and_invokes_scorer() {
        let scorer = Arc::new(RecorderScorer {
            score: serde_json::json!({"total": 8}),
            calls: Mutex::new(0),
        });
        let hook = NousScoreHook::new(scorer.clone() as Arc<dyn ResponseScorer>);
        let span = tracing::Span::current();
        let outcome = hook
            .on_post_inference(&ctx(&span), &sample_response())
            .await
            .unwrap();
        assert!(matches!(outcome, HookOutcome::Continue));
        assert_eq!(*scorer.calls.lock().expect("lock"), 1);
    }

    #[tokio::test]
    async fn scorer_failure_is_non_fatal_and_continues() {
        let hook = NousScoreHook::new(Arc::new(FailingScorer) as Arc<dyn ResponseScorer>);
        let span = tracing::Span::current();
        let outcome = hook
            .on_post_inference(&ctx(&span), &sample_response())
            .await
            .unwrap();
        // Even when the scorer errors, the hook MUST return Continue —
        // observability is observe-only in v0.1.
        assert!(matches!(outcome, HookOutcome::Continue));
    }

    #[test]
    fn name_is_kebab_case() {
        let hook = NousScoreHook::new(Arc::new(RecorderScorer {
            score: serde_json::json!(null),
            calls: Mutex::new(0),
        }) as Arc<dyn ResponseScorer>);
        assert_eq!(hook.name(), "nous-score");
    }
}
