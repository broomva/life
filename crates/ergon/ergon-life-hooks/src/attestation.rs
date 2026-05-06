//! Soul-attestation hook — signs `SessionStart` / `SessionEnd` events
//! with the agent's identity material.
//!
//! Production wiring: the arcan adapter (BRO-1001) implements
//! [`SoulAttester`] against `anima_core::AgentSoul` (or the
//! anima-events sibling crate that owns the keypair). The signed events
//! flow into lago via the journal.
//!
//! ## v0.1 scope: best-effort attestation
//!
//! Failures from the attester are logged (via `tracing::warn`) but do
//! NOT abort the workflow. Rationale: a failed attestation should be
//! observable via telemetry; refusing to run the workflow because
//! attestation infrastructure is unavailable is a worse outcome than
//! running it without a signature.
//!
//! If a deployment's threat model demands hard-failure on attestation
//! errors, a custom hook can be added that wraps `SoulAttester` and
//! returns `HookOutcome::Deny` on error. The default crate-shipped
//! `AnimaAttestHook` stays tolerant.

use async_trait::async_trait;
use ergon::{
    Hook, HookCtx, HookOutcome, InferenceHookOutcome, ModelRequest, ModelResponse, Result,
    SessionId, ToolCall, ToolHookOutcome, ToolResult,
};
use std::sync::Arc;

/// Adapter trait — implementer signs session-boundary events.
///
/// In production: implemented in the arcan adapter against
/// `anima_core::AgentSoul`. The implementation owns the keypair and
/// emits the signed event onto the lago journal.
///
/// Errors are non-fatal — see module docs.
#[async_trait]
pub trait SoulAttester: Send + Sync {
    /// Sign a `SessionStart`-equivalent event for the given session.
    async fn sign_session_start(
        &self,
        session_id: &SessionId,
        workflow_name: &str,
    ) -> std::result::Result<(), String>;

    /// Sign a `SessionEnd`-equivalent event. `ok` indicates whether the
    /// workflow completed successfully.
    async fn sign_session_end(
        &self,
        session_id: &SessionId,
        workflow_name: &str,
        ok: bool,
    ) -> std::result::Result<(), String>;
}

/// Hook that fires `on_workflow_start` and `on_workflow_end`, attesting
/// each boundary via a [`SoulAttester`].
pub struct AnimaAttestHook {
    attester: Arc<dyn SoulAttester>,
}

impl AnimaAttestHook {
    /// Construct from any [`SoulAttester`].
    pub fn new(attester: Arc<dyn SoulAttester>) -> Self {
        Self { attester }
    }
}

#[async_trait]
impl Hook for AnimaAttestHook {
    fn name(&self) -> &str {
        "anima-attest"
    }

    async fn on_workflow_start(&self, ctx: &HookCtx<'_>) -> Result<HookOutcome> {
        if let Err(reason) = self
            .attester
            .sign_session_start(&ctx.session_id, ctx.workflow_name)
            .await
        {
            tracing::warn!(
                parent: ctx.trace,
                workflow = ctx.workflow_name,
                error = %reason,
                "anima-attest sign_session_start failed (non-fatal)",
            );
        }
        Ok(HookOutcome::Continue)
    }

    async fn on_workflow_end(&self, ctx: &HookCtx<'_>, ok: bool) -> Result<HookOutcome> {
        if let Err(reason) = self
            .attester
            .sign_session_end(&ctx.session_id, ctx.workflow_name, ok)
            .await
        {
            tracing::warn!(
                parent: ctx.trace,
                workflow = ctx.workflow_name,
                ok,
                error = %reason,
                "anima-attest sign_session_end failed (non-fatal)",
            );
        }
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
    async fn on_post_inference(&self, _: &HookCtx<'_>, _: &ModelResponse) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
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
    use std::sync::Mutex;

    #[derive(Default)]
    struct AttestRecorder {
        events: Mutex<Vec<String>>,
        fail_start: bool,
        fail_end: bool,
    }

    #[async_trait]
    impl SoulAttester for AttestRecorder {
        async fn sign_session_start(
            &self,
            session_id: &SessionId,
            workflow_name: &str,
        ) -> std::result::Result<(), String> {
            self.events
                .lock()
                .expect("lock")
                .push(format!("start:{workflow_name}:{}", session_id.as_str()));
            if self.fail_start {
                Err("kms unreachable".into())
            } else {
                Ok(())
            }
        }
        async fn sign_session_end(
            &self,
            session_id: &SessionId,
            workflow_name: &str,
            ok: bool,
        ) -> std::result::Result<(), String> {
            self.events
                .lock()
                .expect("lock")
                .push(format!("end:{workflow_name}:{}:{ok}", session_id.as_str()));
            if self.fail_end {
                Err("kms unreachable".into())
            } else {
                Ok(())
            }
        }
    }

    fn ctx<'a>(span: &'a tracing::Span) -> HookCtx<'a> {
        HookCtx::new(
            SessionId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            "wf",
            span,
        )
    }

    #[tokio::test]
    async fn workflow_start_attests_via_adapter() {
        let attester = Arc::new(AttestRecorder::default());
        let hook = AnimaAttestHook::new(attester.clone() as Arc<dyn SoulAttester>);
        let span = tracing::Span::current();
        let outcome = hook.on_workflow_start(&ctx(&span)).await.unwrap();
        assert!(matches!(outcome, HookOutcome::Continue));
        let events = attester.events.lock().expect("lock").clone();
        assert_eq!(events.len(), 1);
        assert!(events[0].starts_with("start:wf:"));
    }

    #[tokio::test]
    async fn workflow_end_attests_with_success_flag() {
        let attester = Arc::new(AttestRecorder::default());
        let hook = AnimaAttestHook::new(attester.clone() as Arc<dyn SoulAttester>);
        let span = tracing::Span::current();
        let _ = hook.on_workflow_end(&ctx(&span), true).await.unwrap();
        let _ = hook.on_workflow_end(&ctx(&span), false).await.unwrap();
        let events = attester.events.lock().expect("lock").clone();
        assert_eq!(events.len(), 2);
        assert!(events[0].ends_with(":true"));
        assert!(events[1].ends_with(":false"));
    }

    #[tokio::test]
    async fn attester_failure_is_non_fatal() {
        let attester = Arc::new(AttestRecorder {
            fail_start: true,
            fail_end: true,
            ..Default::default()
        });
        let hook = AnimaAttestHook::new(attester as Arc<dyn SoulAttester>);
        let span = tracing::Span::current();
        // Both start and end MUST return Continue even when attestation fails.
        let start = hook.on_workflow_start(&ctx(&span)).await.unwrap();
        let end = hook.on_workflow_end(&ctx(&span), true).await.unwrap();
        assert!(matches!(start, HookOutcome::Continue));
        assert!(matches!(end, HookOutcome::Continue));
    }

    #[test]
    fn name_is_kebab_case() {
        let hook =
            AnimaAttestHook::new(Arc::new(AttestRecorder::default()) as Arc<dyn SoulAttester>);
        assert_eq!(hook.name(), "anima-attest");
    }
}
