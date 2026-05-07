//! Budget-gating hook — vetoes inference calls when the agent's economic /
//! cognitive / operational budget is exhausted.
//!
//! Production wiring: the arcan adapter (BRO-1001) implements
//! [`BudgetGate`] against `autonomic::AutonomicGatingProfile`. The
//! `economic` substate carries `allow_expensive_tools` and
//! `max_tokens_next_turn`; `operational` and `cognitive` substates
//! contribute mode-based gates.

use async_trait::async_trait;
use ergon::{
    Hook, HookCtx, HookOutcome, InferenceHookOutcome, ModelRequest, ModelResponse, Result,
    ToolCall, ToolHookOutcome, ToolResult,
};
use std::sync::Arc;

/// Adapter trait — implementer answers "may this inference call proceed?".
///
/// In production: implemented in the arcan adapter against
/// `autonomic::AutonomicGatingProfile`, surfacing budget rationale so
/// the model can see *why* the call was denied.
///
/// The trait deliberately operates on a [`ModelRequest`] (not just a
/// boolean) so a future implementation can also *narrow* the request —
/// for example, downgrading `max_tokens` when the budget is tight, or
/// stripping expensive tools — and return `Continue`. The hook re-checks
/// after potential mutation.
#[async_trait]
pub trait BudgetGate: Send + Sync {
    /// Decide whether the given inference call is allowed.
    ///
    /// Implementers **may mutate `req`** to narrow the call (e.g., reduce
    /// `max_tokens`, filter `tools`) and return `Ok(())` to continue.
    /// They may also return `Err(reason)` to deny outright; the reason is
    /// surfaced as the [`InferenceHookOutcome::Deny`] message.
    async fn allow_inference(&self, req: &mut ModelRequest) -> std::result::Result<(), String>;
}

/// Hook that fires `on_pre_inference` and consults a [`BudgetGate`]
/// before allowing the provider call.
pub struct AutonomicBudgetHook {
    gate: Arc<dyn BudgetGate>,
}

impl AutonomicBudgetHook {
    /// Construct from any [`BudgetGate`].
    pub fn new(gate: Arc<dyn BudgetGate>) -> Self {
        Self { gate }
    }
}

#[async_trait]
impl Hook for AutonomicBudgetHook {
    fn name(&self) -> &str {
        "autonomic-budget"
    }

    async fn on_pre_inference(
        &self,
        _ctx: &HookCtx<'_>,
        req: &mut ModelRequest,
    ) -> Result<InferenceHookOutcome> {
        match self.gate.allow_inference(req).await {
            Ok(()) => Ok(InferenceHookOutcome::Continue),
            Err(reason) => Ok(InferenceHookOutcome::Deny(format!(
                "budget exhausted: {reason}"
            ))),
        }
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
    use ergon::{Message, SessionId};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Mock gate that toggles between allow/deny per the atomic flag.
    struct ToggleGate {
        deny: AtomicBool,
        invocations: Mutex<u32>,
    }

    impl ToggleGate {
        fn new(deny: bool) -> Self {
            Self {
                deny: AtomicBool::new(deny),
                invocations: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl BudgetGate for ToggleGate {
        async fn allow_inference(
            &self,
            _req: &mut ModelRequest,
        ) -> std::result::Result<(), String> {
            *self.invocations.lock().expect("lock") += 1;
            if self.deny.load(Ordering::Relaxed) {
                Err("balance < 0".into())
            } else {
                Ok(())
            }
        }
    }

    /// Mutating gate that narrows max_tokens.
    struct NarrowGate;

    #[async_trait]
    impl BudgetGate for NarrowGate {
        async fn allow_inference(&self, req: &mut ModelRequest) -> std::result::Result<(), String> {
            req.max_tokens = Some(256);
            Ok(())
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
    async fn allowed_inference_continues() {
        let gate = Arc::new(ToggleGate::new(false));
        let hook = AutonomicBudgetHook::new(gate.clone() as Arc<dyn BudgetGate>);
        let span = tracing::Span::current();
        let mut req = ModelRequest::new("m", vec![Message::user_text("hi")]);
        let outcome = hook.on_pre_inference(&ctx(&span), &mut req).await.unwrap();
        assert!(matches!(outcome, InferenceHookOutcome::Continue));
        assert_eq!(*gate.invocations.lock().expect("lock"), 1);
    }

    #[tokio::test]
    async fn exhausted_budget_returns_deny_with_reason() {
        let gate = Arc::new(ToggleGate::new(true));
        let hook = AutonomicBudgetHook::new(gate as Arc<dyn BudgetGate>);
        let span = tracing::Span::current();
        let mut req = ModelRequest::new("m", vec![Message::user_text("hi")]);
        let outcome = hook.on_pre_inference(&ctx(&span), &mut req).await.unwrap();
        match outcome {
            InferenceHookOutcome::Deny(reason) => {
                assert!(reason.contains("budget exhausted"));
                assert!(reason.contains("balance"));
            }
            _ => panic!("expected Deny"),
        }
    }

    #[tokio::test]
    async fn gate_can_mutate_request_in_place() {
        let hook = AutonomicBudgetHook::new(Arc::new(NarrowGate) as Arc<dyn BudgetGate>);
        let span = tracing::Span::current();
        let mut req = ModelRequest::new("m", vec![Message::user_text("hi")]);
        assert_eq!(req.max_tokens, None);
        let outcome = hook.on_pre_inference(&ctx(&span), &mut req).await.unwrap();
        assert!(matches!(outcome, InferenceHookOutcome::Continue));
        assert_eq!(req.max_tokens, Some(256));
    }

    #[test]
    fn name_is_kebab_case() {
        let hook =
            AutonomicBudgetHook::new(Arc::new(ToggleGate::new(false)) as Arc<dyn BudgetGate>);
        assert_eq!(hook.name(), "autonomic-budget");
    }
}
