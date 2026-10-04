//! Capability-gating hook — vetoes tool calls that exceed the agent's
//! granted capabilities.
//!
//! Production wiring: the arcan adapter (BRO-1001) implements
//! [`CapabilityResolver`] against `aios_protocol::PolicySet`, evaluating
//! the call's required capability against the session's granted set.

use async_trait::async_trait;
use ergon::{
    Hook, HookCtx, HookOutcome, InferenceHookOutcome, ModelRequest, ModelResponse, Result,
    ToolCall, ToolHookOutcome, ToolResult,
};
use std::sync::Arc;

/// Adapter trait — implementer answers "may this tool call proceed?".
///
/// In production: implemented in the arcan adapter against
/// `aios_protocol::PolicySet`, walking `gate_capabilities` and matching
/// the tool name plus its arguments to known capability patterns.
///
/// The trait is **deliberately small** — one async method. Substrate
/// integration (PolicySet evaluation, capability glob matching, etc.)
/// happens behind this seam, not in this crate.
#[async_trait]
pub trait CapabilityResolver: Send + Sync {
    /// Decide whether the given tool invocation is allowed.
    ///
    /// - `Ok(())` ⇒ allow the call to dispatch.
    /// - `Err(reason)` ⇒ deny; the reason is surfaced as the
    ///   [`ToolHookOutcome::Deny`] message.
    async fn can_invoke(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> std::result::Result<(), String>;
}

/// Hook that fires `on_pre_tool_use` and consults a [`CapabilityResolver`]
/// before allowing dispatch.
///
/// On `Deny`, the autonomous loop synthesizes a model-visible error
/// `ToolResult` (per `step.rs::dispatch_tool`), so the model sees the
/// denial reason and can recover on the next turn rather than the
/// workflow aborting.
pub struct PraxisCapabilityHook {
    resolver: Arc<dyn CapabilityResolver>,
}

impl PraxisCapabilityHook {
    /// Construct from any [`CapabilityResolver`].
    pub fn new(resolver: Arc<dyn CapabilityResolver>) -> Self {
        Self { resolver }
    }
}

#[async_trait]
impl Hook for PraxisCapabilityHook {
    fn name(&self) -> &str {
        "praxis-capability"
    }

    async fn on_pre_tool_use(
        &self,
        _ctx: &HookCtx<'_>,
        call: &mut ToolCall,
    ) -> Result<ToolHookOutcome> {
        match self.resolver.can_invoke(&call.name, &call.input).await {
            Ok(()) => Ok(ToolHookOutcome::Continue),
            Err(reason) => Ok(ToolHookOutcome::Deny(format!(
                "capability denied for `{}`: {reason}",
                call.name
            ))),
        }
    }

    // Other 7 events default to Continue.
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
    async fn on_post_inference(&self, _: &HookCtx<'_>, _: &ModelResponse) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
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
    use ergon::SessionId;

    /// Mock resolver that allows everything except the listed names.
    struct DenyList(Vec<&'static str>);

    #[async_trait]
    impl CapabilityResolver for DenyList {
        async fn can_invoke(
            &self,
            tool_name: &str,
            _input: &serde_json::Value,
        ) -> std::result::Result<(), String> {
            if self.0.contains(&tool_name) {
                Err(format!("`{tool_name}` is on the deny list"))
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
    async fn allowed_tool_continues() {
        let hook = PraxisCapabilityHook::new(
            Arc::new(DenyList(vec!["shell"])) as Arc<dyn CapabilityResolver>
        );
        let span = tracing::Span::current();
        let hook_ctx = ctx(&span);
        let mut call = ToolCall::new("c1", "fs_read", serde_json::json!({"path": "/x"}));
        let outcome = hook.on_pre_tool_use(&hook_ctx, &mut call).await.unwrap();
        assert!(matches!(outcome, ToolHookOutcome::Continue));
    }

    #[tokio::test]
    async fn denied_tool_returns_deny_with_reason() {
        let hook = PraxisCapabilityHook::new(
            Arc::new(DenyList(vec!["shell"])) as Arc<dyn CapabilityResolver>
        );
        let span = tracing::Span::current();
        let hook_ctx = ctx(&span);
        let mut call = ToolCall::new("c1", "shell", serde_json::json!({"cmd": "rm -rf /"}));
        let outcome = hook.on_pre_tool_use(&hook_ctx, &mut call).await.unwrap();
        match outcome {
            ToolHookOutcome::Deny(reason) => {
                assert!(reason.contains("shell"));
                assert!(reason.contains("deny list"));
            }
            _ => panic!("expected Deny"),
        }
    }

    #[tokio::test]
    async fn other_events_default_continue() {
        let hook =
            PraxisCapabilityHook::new(Arc::new(DenyList(vec![])) as Arc<dyn CapabilityResolver>);
        let span = tracing::Span::current();
        let hook_ctx = ctx(&span);
        // Spot-check a few — exhaustive coverage is in ergon::hook tests.
        assert!(matches!(
            hook.on_workflow_start(&hook_ctx).await.unwrap(),
            HookOutcome::Continue
        ));
        assert!(matches!(
            hook.on_step_start(&hook_ctx, "s").await.unwrap(),
            HookOutcome::Continue
        ));
    }

    #[test]
    fn name_is_kebab_case() {
        let hook =
            PraxisCapabilityHook::new(Arc::new(DenyList(vec![])) as Arc<dyn CapabilityResolver>);
        assert_eq!(hook.name(), "praxis-capability");
    }
}
