//! Lifecycle hooks — observe, deny, or rewrite events in the autonomous loop.
//!
//! A [`Hook`] is the single extension point ergon exposes for crosscutting
//! behaviour: capability gating, budget enforcement, scoring, attestation,
//! approval flows, telemetry, and any user-supplied policy. The trait
//! defines **eight** events fired at the seams of the workflow lifecycle:
//!
//! | Event | Fired when |
//! |---|---|
//! | [`Hook::on_workflow_start`] | Just before [`crate::Workflow::execute`] runs (before any user code) |
//! | [`Hook::on_workflow_end`]   | After `execute` returns or errors (always fires) |
//! | [`Hook::on_step_start`]     | Before each `Step::run` invocation |
//! | [`Hook::on_step_end`]       | After each `Step::run` returns |
//! | [`Hook::on_pre_inference`]  | Before each provider streaming call (request is mutable) |
//! | [`Hook::on_post_inference`] | After each provider streaming call (response is read-only) |
//! | [`Hook::on_pre_tool_use`]   | Before dispatching a tool call (call is mutable) |
//! | [`Hook::on_post_tool_use`]  | After a tool call returns (result is mutable) |
//!
//! Outcomes come in three flavours, depending on what the hook can do:
//!
//! - [`HookOutcome`]: `Continue` or `Deny(reason)` — observe-and-veto.
//! - [`ToolHookOutcome`]: `Continue`, `Deny(reason)`, or `Stub(payload)` —
//!   pre-tool can inject a synthetic result without invoking the tool.
//! - [`InferenceHookOutcome`]: `Continue`, `Deny(reason)`, or `Stub(message)`
//!   — pre-inference can inject a synthetic assistant turn without calling
//!   the model. Useful for caching and replay.
//!
//! ## Auto-hook ordering (locked, spec §3.8)
//!
//! When `WorkflowExecutor::run` (BRO-999) lands, it will register the four
//! Life-native auto-hooks **before** any user-supplied hook. Auto-hooks fire
//! first to short-circuit Life-policy-violating runs as cheaply as possible.
//! If a user hook denies after an auto-hook approved, the deny still wins.
//!
//! ## Default impls
//!
//! All eight events default to `Ok(_::Continue)`, so a hook implementer can
//! override only the events they care about (the common case). This is a
//! deliberate deviation from spec §3.7, which only defaulted
//! `on_workflow_start`. Rationale: a real-world hook (e.g.,
//! `NousScoreHook`) only cares about one event; forcing eight no-op
//! implementations on every hook adds boilerplate without adding safety —
//! the same `Continue` behaviour ships either way. Documented in the v0.1
//! CHANGELOG.

use crate::SessionId;
use crate::error::Result;
use crate::model::{Message, ModelRequest, ModelResponse, ToolCall, ToolResult};
use async_trait::async_trait;
use std::sync::Arc;

/// Lightweight context passed to every hook event.
///
/// Borrows from `StepCtx` (BRO-998) without owning it. Hook impls should
/// treat this as read-only metadata about the current run.
#[non_exhaustive]
pub struct HookCtx<'a> {
    /// The session this run belongs to.
    pub session_id: SessionId,
    /// Name of the [`crate::Workflow`] currently executing.
    pub workflow_name: &'a str,
    /// The current `tracing` span. Hook impls SHOULD record events on this
    /// span rather than creating disconnected spans.
    pub trace: &'a tracing::Span,
}

impl<'a> HookCtx<'a> {
    /// Construct a hook context.
    pub fn new(session_id: SessionId, workflow_name: &'a str, trace: &'a tracing::Span) -> Self {
        Self {
            session_id,
            workflow_name,
            trace,
        }
    }
}

/// Outcome of a non-mutating hook event.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum HookOutcome {
    /// Proceed with the next hook (and ultimately, the underlying action).
    Continue,
    /// Veto the action with a human-readable reason. Subsequent hooks for
    /// the same event are not invoked.
    Deny(String),
}

/// Outcome of a tool-use hook ([`Hook::on_pre_tool_use`]).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ToolHookOutcome {
    /// Proceed with normal tool dispatch.
    Continue,
    /// Veto tool execution with a reason.
    Deny(String),
    /// Replace the tool's real output with the supplied JSON payload.
    /// The tool runtime is **not** invoked; the synthetic result is fed
    /// back to the model as if the tool had returned it.
    ///
    /// Used by hooks that implement caching, replay, or testing flows.
    Stub(serde_json::Value),
}

/// Outcome of an inference hook ([`Hook::on_pre_inference`]).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum InferenceHookOutcome {
    /// Proceed with the provider call.
    Continue,
    /// Veto the model call with a reason.
    Deny(String),
    /// Skip the provider call entirely; treat the supplied [`Message`] as
    /// the model's response. Used for testing and deterministic replays.
    Stub(Message),
}

/// The hook trait — eight lifecycle events.
///
/// All events default to `Continue` (see module docs for rationale). Override
/// only what you need.
///
/// ## Mutability
///
/// Two events expose `&mut` access to the underlying object:
/// [`Self::on_pre_inference`] (`&mut ModelRequest`) and
/// [`Self::on_pre_tool_use`] (`&mut ToolCall`). A hook can rewrite these
/// in-place (e.g., redact a secret, add a tool, narrow a scope) before
/// returning [`HookOutcome::Continue`]. [`Self::on_post_tool_use`] receives
/// `&mut ToolResult` so post-processing hooks can transform results before
/// they reach the model.
#[async_trait]
pub trait Hook: Send + Sync {
    /// Stable, human-readable name of this hook (used in error messages
    /// and tracing). Convention: kebab-case, e.g. `"praxis-capability"`.
    fn name(&self) -> &str;

    /// Fired once at the start of a workflow run, before any user code.
    async fn on_workflow_start(&self, _ctx: &HookCtx<'_>) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    /// Fired once when a workflow run finishes (success or failure).
    /// `ok` is `true` iff `Workflow::execute` returned `Ok`.
    async fn on_workflow_end(&self, _ctx: &HookCtx<'_>, _ok: bool) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    /// Fired before each [`crate::Workflow`]-internal `Step::run`.
    async fn on_step_start(&self, _ctx: &HookCtx<'_>, _step_name: &str) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    /// Fired after each `Step::run` returns. `ok` indicates success.
    async fn on_step_end(
        &self,
        _ctx: &HookCtx<'_>,
        _step_name: &str,
        _ok: bool,
    ) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    /// Fired before each provider streaming call. The request is mutable
    /// so hooks can inject system prompts, rewrite tool sets, redact PII,
    /// or stub the entire call via [`InferenceHookOutcome::Stub`].
    async fn on_pre_inference(
        &self,
        _ctx: &HookCtx<'_>,
        _req: &mut ModelRequest,
    ) -> Result<InferenceHookOutcome> {
        Ok(InferenceHookOutcome::Continue)
    }

    /// Fired after each provider streaming call returns a complete
    /// [`ModelResponse`]. Read-only — to alter the next turn, mutate the
    /// next [`ModelRequest`] in [`Self::on_pre_inference`].
    async fn on_post_inference(
        &self,
        _ctx: &HookCtx<'_>,
        _resp: &ModelResponse,
    ) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    /// Fired before each tool dispatch. Mutable access lets hooks rewrite
    /// arguments (e.g., narrow a path scope, redact a secret) or stub the
    /// result entirely via [`ToolHookOutcome::Stub`].
    async fn on_pre_tool_use(
        &self,
        _ctx: &HookCtx<'_>,
        _call: &mut ToolCall,
    ) -> Result<ToolHookOutcome> {
        Ok(ToolHookOutcome::Continue)
    }

    /// Fired after each tool dispatch. The result is mutable so
    /// post-processing hooks can transform it (truncation, summarisation,
    /// PII scrubbing) before it's fed back into the model on the next turn.
    async fn on_post_tool_use(
        &self,
        _ctx: &HookCtx<'_>,
        _call: &ToolCall,
        _result: &mut ToolResult,
    ) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }
}

/// Registry of hooks fired in registration order.
///
/// The autonomous loop in `step.rs` (BRO-998) iterates this registry at
/// each lifecycle event, short-circuiting on the first
/// `Deny` / `Stub` outcome.
#[derive(Default, Clone)]
pub struct HookRegistry {
    hooks: Vec<Arc<dyn Hook>>,
}

impl HookRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a hook to the registry. Returns `self` for builder-style use.
    #[must_use]
    pub fn with(mut self, hook: impl Hook + 'static) -> Self {
        self.hooks.push(Arc::new(hook));
        self
    }

    /// Append a pre-boxed hook.
    #[must_use]
    pub fn with_arc(mut self, hook: Arc<dyn Hook>) -> Self {
        self.hooks.push(hook);
        self
    }

    /// Iterate the hooks in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Hook>> {
        self.hooks.iter()
    }

    /// Number of registered hooks.
    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    /// True iff [`Self::len`] is zero.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }
}

impl std::fmt::Debug for HookRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookRegistry")
            .field("hooks", &self.hooks.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ContentBlock, MessageRole, Usage};
    use crate::stream::StopReason;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Marker hook that asserts which methods were called and supports
    /// custom outcomes per event for white-box testing.
    struct RecorderHook {
        name: String,
        seen: Mutex<Vec<String>>,
        deny_pre_tool: AtomicBool,
    }

    impl RecorderHook {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                seen: Mutex::new(Vec::new()),
                deny_pre_tool: AtomicBool::new(false),
            }
        }
        fn observed(&self) -> Vec<String> {
            self.seen.lock().expect("poisoned").clone()
        }
        fn record(&self, event: &str) {
            self.seen.lock().expect("poisoned").push(event.to_string());
        }
    }

    #[async_trait]
    impl Hook for RecorderHook {
        fn name(&self) -> &str {
            &self.name
        }

        async fn on_workflow_start(&self, _ctx: &HookCtx<'_>) -> Result<HookOutcome> {
            self.record("workflow_start");
            Ok(HookOutcome::Continue)
        }

        async fn on_workflow_end(&self, _ctx: &HookCtx<'_>, _ok: bool) -> Result<HookOutcome> {
            self.record("workflow_end");
            Ok(HookOutcome::Continue)
        }

        async fn on_step_start(&self, _ctx: &HookCtx<'_>, _step: &str) -> Result<HookOutcome> {
            self.record("step_start");
            Ok(HookOutcome::Continue)
        }

        async fn on_step_end(
            &self,
            _ctx: &HookCtx<'_>,
            _step: &str,
            _ok: bool,
        ) -> Result<HookOutcome> {
            self.record("step_end");
            Ok(HookOutcome::Continue)
        }

        async fn on_pre_inference(
            &self,
            _ctx: &HookCtx<'_>,
            _req: &mut ModelRequest,
        ) -> Result<InferenceHookOutcome> {
            self.record("pre_inference");
            Ok(InferenceHookOutcome::Continue)
        }

        async fn on_post_inference(
            &self,
            _ctx: &HookCtx<'_>,
            _resp: &ModelResponse,
        ) -> Result<HookOutcome> {
            self.record("post_inference");
            Ok(HookOutcome::Continue)
        }

        async fn on_pre_tool_use(
            &self,
            _ctx: &HookCtx<'_>,
            _call: &mut ToolCall,
        ) -> Result<ToolHookOutcome> {
            self.record("pre_tool_use");
            if self.deny_pre_tool.load(Ordering::Relaxed) {
                Ok(ToolHookOutcome::Deny(format!("{} denied", self.name)))
            } else {
                Ok(ToolHookOutcome::Continue)
            }
        }

        async fn on_post_tool_use(
            &self,
            _ctx: &HookCtx<'_>,
            _call: &ToolCall,
            _result: &mut ToolResult,
        ) -> Result<HookOutcome> {
            self.record("post_tool_use");
            Ok(HookOutcome::Continue)
        }
    }

    fn ctx<'a>(name: &'a str, span: &'a tracing::Span) -> HookCtx<'a> {
        HookCtx::new(
            SessionId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            name,
            span,
        )
    }

    #[test]
    fn empty_registry_is_empty() {
        let r = HookRegistry::new();
        assert_eq!(r.len(), 0);
        assert!(r.is_empty());
        assert_eq!(r.iter().count(), 0);
    }

    #[test]
    fn registry_preserves_insertion_order() {
        let r = HookRegistry::new()
            .with(RecorderHook::new("a"))
            .with(RecorderHook::new("b"))
            .with(RecorderHook::new("c"));
        let names: Vec<&str> = r.iter().map(|h| h.name()).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn default_hook_returns_continue_for_every_event() {
        struct Empty;
        #[async_trait]
        impl Hook for Empty {
            fn name(&self) -> &str {
                "empty"
            }
        }

        let span = tracing::Span::current();
        let hook_ctx = ctx("test-wf", &span);
        let h = Empty;

        assert!(matches!(
            h.on_workflow_start(&hook_ctx).await.unwrap(),
            HookOutcome::Continue
        ));
        assert!(matches!(
            h.on_workflow_end(&hook_ctx, true).await.unwrap(),
            HookOutcome::Continue
        ));
        assert!(matches!(
            h.on_step_start(&hook_ctx, "s").await.unwrap(),
            HookOutcome::Continue
        ));
        assert!(matches!(
            h.on_step_end(&hook_ctx, "s", true).await.unwrap(),
            HookOutcome::Continue
        ));

        let mut req = ModelRequest::new("m", vec![]);
        assert!(matches!(
            h.on_pre_inference(&hook_ctx, &mut req).await.unwrap(),
            InferenceHookOutcome::Continue
        ));

        let resp = ModelResponse {
            content: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        assert!(matches!(
            h.on_post_inference(&hook_ctx, &resp).await.unwrap(),
            HookOutcome::Continue
        ));

        let mut call = ToolCall::new("c1", "fs_read", serde_json::json!({}));
        assert!(matches!(
            h.on_pre_tool_use(&hook_ctx, &mut call).await.unwrap(),
            ToolHookOutcome::Continue
        ));

        let mut result = ToolResult::success("c1", serde_json::json!(null));
        assert!(matches!(
            h.on_post_tool_use(&hook_ctx, &call, &mut result)
                .await
                .unwrap(),
            HookOutcome::Continue
        ));
    }

    #[tokio::test]
    async fn recorder_hook_observes_each_event_when_invoked() {
        let span = tracing::Span::current();
        let hook_ctx = ctx("test-wf", &span);
        let r = RecorderHook::new("rec");

        let _ = r.on_workflow_start(&hook_ctx).await;
        let _ = r.on_step_start(&hook_ctx, "s").await;
        let mut req = ModelRequest::new("m", vec![]);
        let _ = r.on_pre_inference(&hook_ctx, &mut req).await;
        let resp = ModelResponse {
            content: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        let _ = r.on_post_inference(&hook_ctx, &resp).await;
        let mut call = ToolCall::new("c1", "fs_read", serde_json::json!({}));
        let _ = r.on_pre_tool_use(&hook_ctx, &mut call).await;
        let mut result = ToolResult::success("c1", serde_json::json!({"ok": true}));
        let _ = r.on_post_tool_use(&hook_ctx, &call, &mut result).await;
        let _ = r.on_step_end(&hook_ctx, "s", true).await;
        let _ = r.on_workflow_end(&hook_ctx, true).await;

        assert_eq!(
            r.observed(),
            vec![
                "workflow_start",
                "step_start",
                "pre_inference",
                "post_inference",
                "pre_tool_use",
                "post_tool_use",
                "step_end",
                "workflow_end",
            ]
        );
    }

    #[tokio::test]
    async fn pre_tool_can_deny_via_outcome() {
        let span = tracing::Span::current();
        let hook_ctx = ctx("test-wf", &span);
        let r = RecorderHook::new("strict");
        r.deny_pre_tool.store(true, Ordering::Relaxed);

        let mut call = ToolCall::new("c1", "shell", serde_json::json!({"cmd": "rm -rf /"}));
        let outcome = r.on_pre_tool_use(&hook_ctx, &mut call).await.unwrap();
        match outcome {
            ToolHookOutcome::Deny(reason) => assert!(reason.contains("strict denied")),
            _ => panic!("expected Deny"),
        }
    }

    #[tokio::test]
    async fn pre_inference_can_mutate_request() {
        struct Injector;
        #[async_trait]
        impl Hook for Injector {
            fn name(&self) -> &str {
                "injector"
            }
            async fn on_pre_inference(
                &self,
                _ctx: &HookCtx<'_>,
                req: &mut ModelRequest,
            ) -> Result<InferenceHookOutcome> {
                req.system = Some("you are concise".to_string());
                req.max_tokens = Some(64);
                Ok(InferenceHookOutcome::Continue)
            }
        }

        let span = tracing::Span::current();
        let hook_ctx = ctx("test-wf", &span);
        let mut req = ModelRequest::new("claude-sonnet-4", vec![]);
        let _ = Injector.on_pre_inference(&hook_ctx, &mut req).await;
        assert_eq!(req.system.as_deref(), Some("you are concise"));
        assert_eq!(req.max_tokens, Some(64));
    }

    #[tokio::test]
    async fn pre_tool_stub_carries_synthetic_payload() {
        struct Stubber;
        #[async_trait]
        impl Hook for Stubber {
            fn name(&self) -> &str {
                "stubber"
            }
            async fn on_pre_tool_use(
                &self,
                _ctx: &HookCtx<'_>,
                _call: &mut ToolCall,
            ) -> Result<ToolHookOutcome> {
                Ok(ToolHookOutcome::Stub(serde_json::json!({"cached": true})))
            }
        }

        let span = tracing::Span::current();
        let hook_ctx = ctx("test-wf", &span);
        let mut call = ToolCall::new("c1", "fs_read", serde_json::json!({"path": "/x"}));
        match Stubber.on_pre_tool_use(&hook_ctx, &mut call).await.unwrap() {
            ToolHookOutcome::Stub(v) => assert_eq!(v["cached"], true),
            _ => panic!("expected Stub"),
        }
    }

    #[tokio::test]
    async fn pre_inference_stub_carries_synthetic_message() {
        struct Replay;
        #[async_trait]
        impl Hook for Replay {
            fn name(&self) -> &str {
                "replay"
            }
            async fn on_pre_inference(
                &self,
                _ctx: &HookCtx<'_>,
                _req: &mut ModelRequest,
            ) -> Result<InferenceHookOutcome> {
                Ok(InferenceHookOutcome::Stub(Message {
                    role: MessageRole::Assistant,
                    content: vec![ContentBlock::text("from cache")],
                }))
            }
        }

        let span = tracing::Span::current();
        let hook_ctx = ctx("test-wf", &span);
        let mut req = ModelRequest::new("m", vec![]);
        match Replay.on_pre_inference(&hook_ctx, &mut req).await.unwrap() {
            InferenceHookOutcome::Stub(m) => {
                assert_eq!(m.role, MessageRole::Assistant);
                match &m.content[0] {
                    ContentBlock::Text { text } => assert_eq!(text, "from cache"),
                    _ => panic!("expected text"),
                }
            }
            _ => panic!("expected Stub"),
        }
    }

    #[tokio::test]
    async fn registry_hooks_can_be_invoked_sequentially_via_iter() {
        struct Counter {
            count: std::sync::atomic::AtomicUsize,
            name: &'static str,
        }
        #[async_trait]
        impl Hook for Counter {
            fn name(&self) -> &str {
                self.name
            }
            async fn on_workflow_start(&self, _ctx: &HookCtx<'_>) -> Result<HookOutcome> {
                self.count.fetch_add(1, Ordering::Relaxed);
                Ok(HookOutcome::Continue)
            }
        }

        let a = Arc::new(Counter {
            count: 0.into(),
            name: "a",
        });
        let b = Arc::new(Counter {
            count: 0.into(),
            name: "b",
        });
        let r = HookRegistry::new()
            .with_arc(a.clone() as Arc<dyn Hook>)
            .with_arc(b.clone() as Arc<dyn Hook>);

        let span = tracing::Span::current();
        let hook_ctx = ctx("test-wf", &span);
        for hook in r.iter() {
            let _ = hook.on_workflow_start(&hook_ctx).await;
        }

        assert_eq!(a.count.load(Ordering::Relaxed), 1);
        assert_eq!(b.count.load(Ordering::Relaxed), 1);
    }
}
