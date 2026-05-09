//! Step trait + StepCtx + InferenceRequest + the autonomous loop.
//!
//! `step.rs` is **the deterministic / autonomous boundary**. Code above
//! [`StepCtx::run_inference_streaming`] is deterministic (the workflow
//! body running in plain async Rust). Code inside that function is the
//! autonomous loop — multiple model+tool turns, each driven by
//! provider events and hook outcomes.
//!
//! ## Relationship to other modules
//!
//! - [`Step`] consumes [`StepCtx`] by `&mut`; sub-steps run inside a
//!   workflow body via `ctx.step(&MyStep, input).await`.
//! - [`StepCtx::run_inference_streaming`] drives the autonomous loop —
//!   firing pre/post-inference and pre/post-tool-use hooks (declared in
//!   [`crate::hook`]) at every seam.
//! - The provider, tool registry, and runtime handle come from
//!   [`crate::runtime`] — implementer-facing traits, **not** substrate
//!   types.
//! - Every observable event flows out through the
//!   [`crate::stream::StreamSink`] held in [`StepCtx::sink`].
//!
//! ## What lives here vs. nowhere
//!
//! - [`InferenceRequest`] — the workflow-author-facing knob (model, role,
//!   max_turns, etc). Distinct from [`crate::ModelRequest`] which is the
//!   per-turn provider-facing payload assembled by the loop.
//! - [`Step`] — the unit-of-work trait a workflow body composes.
//! - [`StepCtx`] — the per-tick orchestration arena that arcan
//!   (BRO-1001) builds and hands to `Workflow::execute`.
//!
//! ## What does **not** live here
//!
//! - `Workflow` and `WorkflowExecutor` (BRO-999, separate PR) — the outer
//!   driver that constructs the auto-hook registry and calls
//!   `Workflow::execute`.
//! - `LagoSink`, `VigilSink`, `LifegwSink` — substrate-coupled sink impls,
//!   deferred to the same PR that adds the substrate dependencies.
//! - The four auto-hooks (BRO-1000) — each holds its own substrate handle
//!   and is registered by the executor before user hooks.

use crate::SessionId;
use crate::error::{ErgonError, Result};
use crate::hook::{HookCtx, HookOutcome, HookRegistry, InferenceHookOutcome, ToolHookOutcome};
use crate::model::{
    ContentBlock, Message, MessageRole, ModelRequest, ModelResponse, ToolCall, ToolResult,
};
use crate::role::Role;
use crate::runtime::{Provider, RuntimeHandle, ToolRegistry};
use crate::stream::{StopReason, StreamSink};
use async_trait::async_trait;
use std::sync::Arc;

/// Default `max_turns` for an [`InferenceRequest`]. Matches spec §3.6.
pub const DEFAULT_INFERENCE_MAX_TURNS: u32 = 16;

/// Workflow-author-facing knobs for a single autonomous step.
///
/// Distinct from [`crate::ModelRequest`] (which is the per-turn
/// provider-facing payload). One [`InferenceRequest`] drives multiple
/// turns of the autonomous loop, up to `max_turns`.
///
/// The `role` field, when present, takes precedence as a call-scope
/// overlay — see [`crate::Role::merge`] for precedence semantics.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InferenceRequest {
    /// Provider-specific model identifier (e.g. `"claude-sonnet-4"`).
    pub model: String,
    /// Optional call-scope role overlay (highest precedence).
    pub role: Option<Role>,
    /// Provider max-output cap.
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Stop sequences passed through to the provider.
    pub stop: Vec<String>,
    /// Maximum autonomous turns this request can drive before the loop
    /// gives up with [`ErgonError::MaxTurns`]. Defaults to
    /// [`DEFAULT_INFERENCE_MAX_TURNS`].
    pub max_turns: u32,
}

impl InferenceRequest {
    /// Construct a request for the given model with default knobs.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            role: None,
            max_tokens: None,
            temperature: None,
            stop: Vec::new(),
            max_turns: DEFAULT_INFERENCE_MAX_TURNS,
        }
    }

    /// Attach a call-scope [`Role`] overlay.
    #[must_use]
    pub fn with_role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    /// Set the provider's max-output cap.
    #[must_use]
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }

    /// Set the sampling temperature.
    #[must_use]
    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    /// Set the autonomous-loop turn budget.
    #[must_use]
    pub fn with_max_turns(mut self, n: u32) -> Self {
        self.max_turns = n;
        self
    }

    /// Set provider stop sequences.
    #[must_use]
    pub fn with_stop(mut self, stop: Vec<String>) -> Self {
        self.stop = stop;
        self
    }
}

/// A unit of work composed inside a workflow body.
///
/// Steps are the deterministic-side building blocks: each `Step::run`
/// receives `&mut StepCtx` and the typed `Input`, and returns a typed
/// `Output`. Steps can compose other steps (`ctx.step(...)`) and drive
/// the autonomous loop (`ctx.run_inference_streaming(...)`).
///
/// Step lifecycle hooks ([`crate::Hook::on_step_start`],
/// [`crate::Hook::on_step_end`]) fire automatically when a step runs
/// via [`StepCtx::step`].
#[async_trait]
pub trait Step: Send + Sync {
    /// Typed input.
    type Input: Send;
    /// Typed output.
    type Output: Send;

    /// Stable, human-readable name used in lifecycle hooks and tracing.
    /// Convention: kebab-case, e.g. `"load-extract"`, `"score-verdict"`.
    fn name(&self) -> &str;

    /// Run this step against the given context.
    async fn run(&self, ctx: &mut StepCtx<'_>, input: Self::Input) -> Result<Self::Output>;
}

/// Per-tick orchestration arena handed to a workflow body.
///
/// `StepCtx` carries the runtime handles a workflow needs to drive an
/// autonomous step. It is built by the workflow executor (BRO-999) at the
/// start of each `Workflow::execute` call and dropped when the call
/// returns.
///
/// ## Mutable history
///
/// The autonomous loop accumulates a [`Message`] history across calls to
/// [`Self::run_inference_streaming`]. The history is private (the loop
/// owns it) but workflow authors can seed it via [`Self::push_message`]
/// before the first inference call.
pub struct StepCtx<'a> {
    /// Identifier of the session this run belongs to.
    pub session_id: SessionId,

    /// Name of the [`crate::Workflow`] currently executing.
    pub workflow_name: &'a str,

    /// The streaming model provider.
    pub provider: Arc<dyn Provider>,

    /// The tool registry exposed to the model.
    pub tools: Arc<dyn ToolRegistry>,

    /// Lifecycle hooks fired across the autonomous loop.
    pub hooks: Arc<HookRegistry>,

    /// Where every observable event flows.
    pub sink: Arc<dyn StreamSink>,

    /// Narrow back-channel into the host runtime.
    pub runtime: Arc<dyn RuntimeHandle>,

    /// The current `tracing` span. Hook impls and provider impls SHOULD
    /// record events on this span.
    pub trace: tracing::Span,

    /// Conversation history accumulated across `run_inference_streaming`
    /// calls. Workflow authors can push initial messages via
    /// [`Self::push_message`].
    history: Vec<Message>,
}

impl<'a> StepCtx<'a> {
    /// Construct a new orchestration arena.
    ///
    /// In production this is called by `WorkflowExecutor::run` (BRO-999);
    /// tests can construct one directly.
    #[allow(clippy::too_many_arguments)] // 7 fields is intentional — see field docs.
    pub fn new(
        session_id: SessionId,
        workflow_name: &'a str,
        provider: Arc<dyn Provider>,
        tools: Arc<dyn ToolRegistry>,
        hooks: Arc<HookRegistry>,
        sink: Arc<dyn StreamSink>,
        runtime: Arc<dyn RuntimeHandle>,
        trace: tracing::Span,
    ) -> Self {
        Self {
            session_id,
            workflow_name,
            provider,
            tools,
            hooks,
            sink,
            runtime,
            trace,
            history: Vec::new(),
        }
    }

    /// Append a message to the autonomous-loop history.
    ///
    /// Workflow authors call this to seed the conversation (e.g. with the
    /// initial user input) before invoking
    /// [`Self::run_inference_streaming`].
    pub fn push_message(&mut self, message: Message) {
        self.history.push(message);
    }

    /// Read-only view of the current history.
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// Atomically replace the conversation scope (message history + tool
    /// registry) and return the previous values.
    ///
    /// Used by the [`crate::agent::run_spec`] interpreter to give each
    /// agent invocation its own isolated conversation while sharing the
    /// rest of the [`StepCtx`] (provider, hooks, sink, runtime, trace).
    /// Most workflow authors should never call this directly; prefer
    /// [`Self::step`] (with an `Agent` or any other `Step` impl), which
    /// handles scope management for you.
    pub fn swap_scope(
        &mut self,
        new_history: Vec<Message>,
        new_tools: Arc<dyn ToolRegistry>,
    ) -> (Vec<Message>, Arc<dyn ToolRegistry>) {
        let prev_history = std::mem::replace(&mut self.history, new_history);
        let prev_tools = std::mem::replace(&mut self.tools, new_tools);
        (prev_history, prev_tools)
    }

    /// Run a sub-step. Fires `on_step_start` / `on_step_end` hooks
    /// automatically.
    pub async fn step<S>(&mut self, s: &S, input: S::Input) -> Result<S::Output>
    where
        S: Step + ?Sized,
    {
        let hook_ctx = HookCtx::new(self.session_id.clone(), self.workflow_name, &self.trace);

        // Fire on_step_start. First Deny short-circuits.
        for hook in self.hooks.iter() {
            match hook.on_step_start(&hook_ctx, s.name()).await? {
                HookOutcome::Continue => {}
                HookOutcome::Deny(reason) => {
                    return Err(ErgonError::Hook(format!(
                        "step {} denied by {}: {reason}",
                        s.name(),
                        hook.name()
                    )));
                }
            }
        }

        // Borrow checker: hooks are Arc, so cloning lets us release the
        // borrow before calling run().
        let hooks = Arc::clone(&self.hooks);
        let result = s.run(self, input).await;
        let ok = result.is_ok();

        // Fire on_step_end (best-effort — even on error). Construct a
        // fresh HookCtx because trace span borrow is now released.
        let hook_ctx = HookCtx::new(self.session_id.clone(), self.workflow_name, &self.trace);
        for hook in hooks.iter() {
            let _ = hook.on_step_end(&hook_ctx, s.name(), ok).await;
        }

        result
    }

    /// Drive the autonomous loop until the model decides to stop.
    ///
    /// **Loop body** (per spec §5):
    ///
    /// ```text
    /// for turn in 0..req.max_turns {
    ///     1. Build per-turn ModelRequest from req + history + tool defs
    ///     2. Fire on_pre_inference hooks:
    ///         - Continue → proceed
    ///         - Deny     → return ErgonError::Hook
    ///         - Stub(m)  → use m as response, treat as no-tool-use, break
    ///     3. provider.stream(req, sink) → ModelResponse
    ///     4. Fire on_post_inference hooks (Deny → ErgonError::Hook)
    ///     5. Append assistant message to history
    ///     6. If response has tool_use blocks:
    ///         For each call:
    ///             a. Fire on_pre_tool_use:
    ///                  - Continue → tools.invoke(call)
    ///                  - Deny     → synthetic ToolResult { is_error: true, ... }
    ///                  - Stub(v)  → ToolResult { output: v, is_error: false }
    ///             b. Fire on_post_tool_use (mutates result)
    ///         Append tool messages to history; continue to next turn
    ///        Else: break (this is the final turn)
    /// }
    /// If turn budget exhausted: return ErgonError::MaxTurns
    /// ```
    ///
    /// Returns the final [`ModelResponse`] (the response from the last
    /// turn, which has no tool uses).
    pub async fn run_inference_streaming(
        &mut self,
        req: &InferenceRequest,
    ) -> Result<ModelResponse> {
        let tool_defs = self.tools.definitions();
        let system_prompt = req.role.as_ref().and_then(Role::render);

        for _turn in 0..req.max_turns {
            // 1. Assemble per-turn ModelRequest.
            let mut model_req = ModelRequest::new(req.model.clone(), self.history.clone());
            model_req.tools = tool_defs.clone();
            model_req.system = system_prompt.clone();
            model_req.max_tokens = req.max_tokens;
            model_req.temperature = req.temperature;
            model_req.stop = req.stop.clone();

            // 2. Fire on_pre_inference hooks.
            let hook_ctx = HookCtx::new(self.session_id.clone(), self.workflow_name, &self.trace);
            let mut stub_response: Option<Message> = None;
            for hook in self.hooks.iter() {
                match hook.on_pre_inference(&hook_ctx, &mut model_req).await? {
                    InferenceHookOutcome::Continue => {}
                    InferenceHookOutcome::Deny(reason) => {
                        return Err(ErgonError::Hook(format!(
                            "inference denied by {}: {reason}",
                            hook.name()
                        )));
                    }
                    InferenceHookOutcome::Stub(message) => {
                        stub_response = Some(message);
                        break;
                    }
                }
            }

            // 3. Either use the stubbed response or call the provider.
            let response = if let Some(stub_msg) = stub_response {
                // The stub IS the model's response. Materialize it as a
                // ModelResponse with EndTurn stop_reason and no tool calls.
                ModelResponse {
                    content: stub_msg.content.clone(),
                    stop_reason: StopReason::EndTurn,
                    usage: crate::model::Usage::default(),
                }
            } else {
                self.provider
                    .stream(model_req, Arc::clone(&self.sink))
                    .await?
            };

            // 4. Fire on_post_inference hooks.
            let hook_ctx = HookCtx::new(self.session_id.clone(), self.workflow_name, &self.trace);
            for hook in self.hooks.iter() {
                match hook.on_post_inference(&hook_ctx, &response).await? {
                    HookOutcome::Continue => {}
                    HookOutcome::Deny(reason) => {
                        return Err(ErgonError::Hook(format!(
                            "post-inference denied by {}: {reason}",
                            hook.name()
                        )));
                    }
                }
            }

            // 5. Append the assistant message to history.
            self.history.push(Message {
                role: MessageRole::Assistant,
                content: response.content.clone(),
            });

            // 6. Tool dispatch or terminate.
            let mut tool_calls = response.extract_tool_calls();
            if tool_calls.is_empty() {
                return Ok(response);
            }

            // Sequential tool dispatch (parallel is a v0.2 concern).
            let mut tool_messages_content: Vec<ContentBlock> = Vec::with_capacity(tool_calls.len());
            for call in tool_calls.drain(..) {
                let result = self.dispatch_tool(call).await?;
                tool_messages_content.push(ContentBlock::ToolResult {
                    call_id: result.call_id.clone(),
                    output: result.output.clone(),
                    is_error: result.is_error,
                });
            }
            self.history.push(Message {
                role: MessageRole::Tool,
                content: tool_messages_content,
            });
            // Loop continues to next turn.
        }

        Err(ErgonError::MaxTurns(req.max_turns))
    }

    /// Dispatch a single tool call through the registered hooks.
    async fn dispatch_tool(&self, mut call: ToolCall) -> Result<ToolResult> {
        let hook_ctx = HookCtx::new(self.session_id.clone(), self.workflow_name, &self.trace);

        // Fire on_pre_tool_use. Hooks may rewrite call args, deny, or stub.
        for hook in self.hooks.iter() {
            match hook.on_pre_tool_use(&hook_ctx, &mut call).await? {
                ToolHookOutcome::Continue => {}
                ToolHookOutcome::Deny(reason) => {
                    // Synthesize a model-visible error result. This lets
                    // the model see the denial reason and adjust on the
                    // next turn rather than aborting the workflow.
                    let denied = ToolResult::model_error(
                        call.id.clone(),
                        serde_json::json!({
                            "denied_by_hook": hook.name(),
                            "reason": reason,
                        }),
                    );
                    return self.run_post_tool_hooks(&call, denied).await;
                }
                ToolHookOutcome::Stub(output) => {
                    let stubbed = ToolResult::success(call.id.clone(), output);
                    return self.run_post_tool_hooks(&call, stubbed).await;
                }
            }
        }

        // Fall through: invoke the registry.
        let mut result = self.tools.invoke(call.clone()).await?;
        // Fire on_post_tool_use (may mutate the result).
        let hook_ctx = HookCtx::new(self.session_id.clone(), self.workflow_name, &self.trace);
        for hook in self.hooks.iter() {
            let _ = hook.on_post_tool_use(&hook_ctx, &call, &mut result).await;
        }
        Ok(result)
    }

    /// Helper: run on_post_tool_use against a result (used by Deny/Stub
    /// branches that bypassed the registry).
    async fn run_post_tool_hooks(
        &self,
        call: &ToolCall,
        mut result: ToolResult,
    ) -> Result<ToolResult> {
        let hook_ctx = HookCtx::new(self.session_id.clone(), self.workflow_name, &self.trace);
        for hook in self.hooks.iter() {
            let _ = hook.on_post_tool_use(&hook_ctx, call, &mut result).await;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::Hook;
    use crate::model::{ContentBlock, ToolDefinition, Usage};
    use crate::stream::{BufferSink, StreamEvent};
    use aios_protocol::mode::OperatingMode;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── Mock implementations ─────────────────────────────────────────

    struct MockProvider {
        name: &'static str,
        // Each entry: (events_to_emit, response_to_return)
        plan: Mutex<std::collections::VecDeque<(Vec<StreamEvent>, ModelResponse)>>,
        call_count: AtomicUsize,
    }

    impl MockProvider {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                plan: Mutex::new(std::collections::VecDeque::new()),
                call_count: AtomicUsize::new(0),
            }
        }

        fn enqueue(&self, events: Vec<StreamEvent>, response: ModelResponse) -> &Self {
            self.plan
                .lock()
                .expect("lock")
                .push_back((events, response));
            self
        }

        fn calls(&self) -> usize {
            self.call_count.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn name(&self) -> &str {
            self.name
        }
        async fn stream(
            &self,
            _req: ModelRequest,
            sink: Arc<dyn StreamSink>,
        ) -> Result<ModelResponse> {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            let (events, response) = self
                .plan
                .lock()
                .expect("lock")
                .pop_front()
                .expect("MockProvider plan exhausted");
            for evt in events {
                sink.emit(evt).await?;
            }
            Ok(response)
        }
    }

    type ToolHandler = Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value> + Send + Sync>;

    struct MockToolRegistry {
        defs: Vec<ToolDefinition>,
        handlers: HashMap<String, ToolHandler>,
        invocation_log: Mutex<Vec<ToolCall>>,
    }

    impl MockToolRegistry {
        fn new() -> Self {
            Self {
                defs: Vec::new(),
                handlers: HashMap::new(),
                invocation_log: Mutex::new(Vec::new()),
            }
        }

        fn register(
            &mut self,
            name: &str,
            handler: impl Fn(serde_json::Value) -> Result<serde_json::Value> + Send + Sync + 'static,
        ) -> &mut Self {
            self.defs.push(ToolDefinition::new(
                name,
                format!("{name} tool"),
                serde_json::json!({"type": "object"}),
            ));
            self.handlers.insert(name.to_string(), Box::new(handler));
            self
        }

        fn invocations(&self) -> Vec<ToolCall> {
            self.invocation_log.lock().expect("lock").clone()
        }
    }

    #[async_trait]
    impl ToolRegistry for MockToolRegistry {
        fn definitions(&self) -> Vec<ToolDefinition> {
            self.defs.clone()
        }
        async fn invoke(&self, call: ToolCall) -> Result<ToolResult> {
            self.invocation_log.lock().expect("lock").push(call.clone());
            let handler = self
                .handlers
                .get(&call.name)
                .ok_or_else(|| ErgonError::Tool(format!("no handler for {}", call.name)))?;
            let output = handler(call.input.clone())?;
            Ok(ToolResult::success(call.id, output))
        }
    }

    struct StaticRuntime(OperatingMode);
    impl RuntimeHandle for StaticRuntime {
        fn operating_mode(&self) -> OperatingMode {
            self.0
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn session() -> SessionId {
        SessionId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV")
    }

    fn build_ctx<'a>(
        provider: Arc<dyn Provider>,
        tools: Arc<dyn ToolRegistry>,
        hooks: HookRegistry,
        sink: Arc<dyn StreamSink>,
    ) -> StepCtx<'a> {
        StepCtx::new(
            session(),
            "test-wf",
            provider,
            tools,
            Arc::new(hooks),
            sink,
            Arc::new(StaticRuntime(OperatingMode::Execute)),
            tracing::Span::current(),
        )
    }

    fn text_response(text: &str) -> ModelResponse {
        ModelResponse {
            content: vec![ContentBlock::text(text)],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }
    }

    fn tool_use_response(id: &str, name: &str, input: serde_json::Value) -> ModelResponse {
        ModelResponse {
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input,
            }],
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        }
    }

    // ── InferenceRequest tests ───────────────────────────────────────

    #[test]
    fn inference_request_defaults_max_turns_to_constant() {
        let req = InferenceRequest::new("m");
        assert_eq!(req.max_turns, DEFAULT_INFERENCE_MAX_TURNS);
        assert!(req.role.is_none());
        assert!(req.stop.is_empty());
    }

    #[test]
    fn inference_request_builders_chain() {
        let req = InferenceRequest::new("claude-sonnet-4")
            .with_max_tokens(1024)
            .with_temperature(0.5)
            .with_max_turns(8)
            .with_stop(vec!["</done>".to_string()]);
        assert_eq!(req.model, "claude-sonnet-4");
        assert_eq!(req.max_tokens, Some(1024));
        assert_eq!(req.temperature, Some(0.5));
        assert_eq!(req.max_turns, 8);
        assert_eq!(req.stop, vec!["</done>"]);
    }

    // ── Loop body tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn single_turn_no_tool_returns_response() {
        let provider = Arc::new(MockProvider::new("mock"));
        provider.enqueue(
            vec![StreamEvent::TextDelta {
                id: "t1".into(),
                delta: "hello".into(),
            }],
            text_response("hello"),
        );
        let tools = Arc::new(MockToolRegistry::new());
        let sink: Arc<dyn StreamSink> = Arc::new(BufferSink::new());
        let mut ctx = build_ctx(
            provider.clone(),
            tools,
            HookRegistry::new(),
            Arc::clone(&sink),
        );
        ctx.push_message(Message::user_text("hi"));

        let req = InferenceRequest::new("m").with_max_turns(2);
        let resp = ctx.run_inference_streaming(&req).await.expect("ok");
        assert_eq!(resp.text(), "hello");
        assert_eq!(provider.calls(), 1);
        // History: user + assistant
        assert_eq!(ctx.history().len(), 2);
    }

    #[tokio::test]
    async fn tool_use_drives_a_second_turn() {
        let provider = Arc::new(MockProvider::new("mock"));
        provider.enqueue(
            vec![],
            tool_use_response("tu1", "echo", serde_json::json!({"x": 1})),
        );
        provider.enqueue(vec![], text_response("done"));

        let mut registry = MockToolRegistry::new();
        registry.register("echo", |v| Ok(serde_json::json!({"echoed": v})));
        let tools: Arc<dyn ToolRegistry> = Arc::new(registry);
        let sink: Arc<dyn StreamSink> = Arc::new(BufferSink::new());
        let mut ctx = build_ctx(
            provider.clone(),
            Arc::clone(&tools),
            HookRegistry::new(),
            sink,
        );
        ctx.push_message(Message::user_text("call echo"));

        let req = InferenceRequest::new("m").with_max_turns(4);
        let resp = ctx.run_inference_streaming(&req).await.expect("ok");
        assert_eq!(resp.text(), "done");
        assert_eq!(provider.calls(), 2);
        // History: user, assistant(tool_use), tool, assistant(text)
        assert_eq!(ctx.history().len(), 4);
        assert_eq!(ctx.history()[2].role, MessageRole::Tool);
    }

    #[tokio::test]
    async fn max_turns_exhaustion_returns_max_turns_error() {
        let provider = Arc::new(MockProvider::new("mock"));
        // Always returns tool_use → loop never breaks.
        for _ in 0..10 {
            provider.enqueue(
                vec![],
                tool_use_response("tu", "echo", serde_json::json!({})),
            );
        }
        let mut registry = MockToolRegistry::new();
        registry.register("echo", |_| Ok(serde_json::json!(null)));
        let tools: Arc<dyn ToolRegistry> = Arc::new(registry);
        let sink: Arc<dyn StreamSink> = Arc::new(BufferSink::new());
        let mut ctx = build_ctx(provider, tools, HookRegistry::new(), sink);
        ctx.push_message(Message::user_text("loop"));

        let req = InferenceRequest::new("m").with_max_turns(3);
        let err = ctx
            .run_inference_streaming(&req)
            .await
            .expect_err("should err");
        match err {
            ErgonError::MaxTurns(n) => assert_eq!(n, 3),
            other => panic!("expected MaxTurns, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pre_inference_hook_can_deny() {
        struct Denier;
        #[async_trait]
        impl Hook for Denier {
            fn name(&self) -> &str {
                "denier"
            }
            async fn on_pre_inference(
                &self,
                _ctx: &HookCtx<'_>,
                _req: &mut ModelRequest,
            ) -> Result<InferenceHookOutcome> {
                Ok(InferenceHookOutcome::Deny("nope".into()))
            }
        }

        let provider = Arc::new(MockProvider::new("mock"));
        let tools: Arc<dyn ToolRegistry> = Arc::new(MockToolRegistry::new());
        let sink: Arc<dyn StreamSink> = Arc::new(BufferSink::new());
        let hooks = HookRegistry::new().with(Denier);
        let mut ctx = build_ctx(provider.clone(), tools, hooks, sink);
        ctx.push_message(Message::user_text("test"));

        let req = InferenceRequest::new("m").with_max_turns(2);
        let err = ctx.run_inference_streaming(&req).await.expect_err("denied");
        assert!(matches!(err, ErgonError::Hook(_)));
        assert_eq!(provider.calls(), 0);
    }

    #[tokio::test]
    async fn pre_inference_hook_can_stub_response() {
        struct Stubber;
        #[async_trait]
        impl Hook for Stubber {
            fn name(&self) -> &str {
                "stubber"
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

        let provider = Arc::new(MockProvider::new("mock"));
        let tools: Arc<dyn ToolRegistry> = Arc::new(MockToolRegistry::new());
        let sink: Arc<dyn StreamSink> = Arc::new(BufferSink::new());
        let hooks = HookRegistry::new().with(Stubber);
        let mut ctx = build_ctx(provider.clone(), tools, hooks, sink);
        ctx.push_message(Message::user_text("test"));

        let req = InferenceRequest::new("m").with_max_turns(2);
        let resp = ctx.run_inference_streaming(&req).await.expect("ok");
        assert_eq!(resp.text(), "from cache");
        assert_eq!(provider.calls(), 0); // provider was bypassed
    }

    #[tokio::test]
    async fn pre_tool_hook_can_deny_and_synthesize_error_result() {
        struct ToolGate;
        #[async_trait]
        impl Hook for ToolGate {
            fn name(&self) -> &str {
                "tool-gate"
            }
            async fn on_pre_tool_use(
                &self,
                _ctx: &HookCtx<'_>,
                _call: &mut ToolCall,
            ) -> Result<ToolHookOutcome> {
                Ok(ToolHookOutcome::Deny("forbidden".into()))
            }
        }

        let provider = Arc::new(MockProvider::new("mock"));
        provider.enqueue(
            vec![],
            tool_use_response("tu1", "echo", serde_json::json!({})),
        );
        provider.enqueue(vec![], text_response("ok"));

        let mut registry = MockToolRegistry::new();
        registry.register("echo", |_| Ok(serde_json::json!("real")));
        let registry = Arc::new(registry);
        let tools: Arc<dyn ToolRegistry> = registry.clone();
        let sink: Arc<dyn StreamSink> = Arc::new(BufferSink::new());
        let hooks = HookRegistry::new().with(ToolGate);
        let mut ctx = build_ctx(provider.clone(), tools, hooks, sink);
        ctx.push_message(Message::user_text("test"));

        let req = InferenceRequest::new("m").with_max_turns(3);
        let _ = ctx.run_inference_streaming(&req).await.expect("ok");
        // Registry was NEVER invoked (denied at the hook).
        assert_eq!(registry.invocations().len(), 0);
        // History contains a tool message with is_error=true.
        let tool_msg = ctx
            .history()
            .iter()
            .find(|m| m.role == MessageRole::Tool)
            .expect("tool msg");
        match &tool_msg.content[0] {
            ContentBlock::ToolResult { is_error, .. } => assert!(*is_error),
            _ => panic!("expected tool_result"),
        }
    }

    #[tokio::test]
    async fn pre_tool_hook_can_stub_result() {
        struct ToolStub;
        #[async_trait]
        impl Hook for ToolStub {
            fn name(&self) -> &str {
                "tool-stub"
            }
            async fn on_pre_tool_use(
                &self,
                _ctx: &HookCtx<'_>,
                _call: &mut ToolCall,
            ) -> Result<ToolHookOutcome> {
                Ok(ToolHookOutcome::Stub(serde_json::json!({"cached": true})))
            }
        }

        let provider = Arc::new(MockProvider::new("mock"));
        provider.enqueue(
            vec![],
            tool_use_response("tu1", "echo", serde_json::json!({})),
        );
        provider.enqueue(vec![], text_response("ok"));

        let mut registry = MockToolRegistry::new();
        registry.register("echo", |_| Ok(serde_json::json!("real")));
        let registry = Arc::new(registry);
        let tools: Arc<dyn ToolRegistry> = registry.clone();
        let sink: Arc<dyn StreamSink> = Arc::new(BufferSink::new());
        let hooks = HookRegistry::new().with(ToolStub);
        let mut ctx = build_ctx(provider.clone(), tools, hooks, sink);
        ctx.push_message(Message::user_text("test"));

        let req = InferenceRequest::new("m").with_max_turns(3);
        let _ = ctx.run_inference_streaming(&req).await.expect("ok");
        // Registry was NEVER invoked.
        assert_eq!(registry.invocations().len(), 0);
        // Tool message carries the stub.
        let tool_msg = ctx
            .history()
            .iter()
            .find(|m| m.role == MessageRole::Tool)
            .expect("tool msg");
        match &tool_msg.content[0] {
            ContentBlock::ToolResult {
                is_error, output, ..
            } => {
                assert!(!is_error);
                assert_eq!(output["cached"], true);
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[tokio::test]
    async fn step_dispatch_fires_step_hooks() {
        struct Recorder {
            seen: Mutex<Vec<String>>,
        }
        #[async_trait]
        impl Hook for Recorder {
            fn name(&self) -> &str {
                "rec"
            }
            async fn on_step_start(
                &self,
                _ctx: &HookCtx<'_>,
                step_name: &str,
            ) -> Result<HookOutcome> {
                self.seen
                    .lock()
                    .expect("lock")
                    .push(format!("start:{step_name}"));
                Ok(HookOutcome::Continue)
            }
            async fn on_step_end(
                &self,
                _ctx: &HookCtx<'_>,
                step_name: &str,
                ok: bool,
            ) -> Result<HookOutcome> {
                self.seen
                    .lock()
                    .expect("lock")
                    .push(format!("end:{step_name}:{ok}"));
                Ok(HookOutcome::Continue)
            }
        }

        struct Echo;
        #[async_trait]
        impl Step for Echo {
            type Input = String;
            type Output = String;
            fn name(&self) -> &str {
                "echo"
            }
            async fn run(&self, _ctx: &mut StepCtx<'_>, input: String) -> Result<String> {
                Ok(input.to_uppercase())
            }
        }

        let provider = Arc::new(MockProvider::new("mock"));
        let tools: Arc<dyn ToolRegistry> = Arc::new(MockToolRegistry::new());
        let sink: Arc<dyn StreamSink> = Arc::new(BufferSink::new());
        let recorder = Arc::new(Recorder {
            seen: Mutex::new(Vec::new()),
        });
        let hooks = HookRegistry::new().with_arc(recorder.clone() as Arc<dyn Hook>);
        let mut ctx = build_ctx(provider, tools, hooks, sink);

        let out = ctx.step(&Echo, "hi".to_string()).await.expect("ok");
        assert_eq!(out, "HI");
        assert_eq!(
            *recorder.seen.lock().expect("lock"),
            vec!["start:echo".to_string(), "end:echo:true".to_string()]
        );
    }

    #[tokio::test]
    async fn provider_error_propagates() {
        struct ErrorProvider;
        #[async_trait]
        impl Provider for ErrorProvider {
            fn name(&self) -> &str {
                "error"
            }
            async fn stream(
                &self,
                _req: ModelRequest,
                _sink: Arc<dyn StreamSink>,
            ) -> Result<ModelResponse> {
                Err(ErgonError::Provider("boom".into()))
            }
        }

        let provider: Arc<dyn Provider> = Arc::new(ErrorProvider);
        let tools: Arc<dyn ToolRegistry> = Arc::new(MockToolRegistry::new());
        let sink: Arc<dyn StreamSink> = Arc::new(BufferSink::new());
        let mut ctx = build_ctx(provider, tools, HookRegistry::new(), sink);
        ctx.push_message(Message::user_text("x"));

        let req = InferenceRequest::new("m");
        let err = ctx
            .run_inference_streaming(&req)
            .await
            .expect_err("should err");
        assert!(matches!(err, ErgonError::Provider(_)));
    }

    #[tokio::test]
    async fn role_renders_into_system_prompt_on_request() {
        struct AssertSystem;
        #[async_trait]
        impl Hook for AssertSystem {
            fn name(&self) -> &str {
                "assert-system"
            }
            async fn on_pre_inference(
                &self,
                _ctx: &HookCtx<'_>,
                req: &mut ModelRequest,
            ) -> Result<InferenceHookOutcome> {
                assert_eq!(req.system.as_deref(), Some("You are concise.\n\nNo fluff."));
                Ok(InferenceHookOutcome::Continue)
            }
        }

        let provider = Arc::new(MockProvider::new("mock"));
        provider.enqueue(vec![], text_response("ok"));
        let tools: Arc<dyn ToolRegistry> = Arc::new(MockToolRegistry::new());
        let sink: Arc<dyn StreamSink> = Arc::new(BufferSink::new());
        let hooks = HookRegistry::new().with(AssertSystem);
        let mut ctx = build_ctx(provider, tools, hooks, sink);
        ctx.push_message(Message::user_text("hi"));

        let role = Role::agent("You are concise.")
            .with_instruction("No fluff.")
            .with_scope(crate::role::RoleScope::Call);
        let req = InferenceRequest::new("m").with_role(role);
        let _ = ctx.run_inference_streaming(&req).await.expect("ok");
    }

    #[tokio::test]
    async fn tool_definitions_propagate_into_request() {
        struct AssertTools;
        #[async_trait]
        impl Hook for AssertTools {
            fn name(&self) -> &str {
                "assert-tools"
            }
            async fn on_pre_inference(
                &self,
                _ctx: &HookCtx<'_>,
                req: &mut ModelRequest,
            ) -> Result<InferenceHookOutcome> {
                assert!(req.tools.iter().any(|t| t.name == "echo"));
                Ok(InferenceHookOutcome::Continue)
            }
        }

        let provider = Arc::new(MockProvider::new("mock"));
        provider.enqueue(vec![], text_response("ok"));
        let mut registry = MockToolRegistry::new();
        registry.register("echo", |_| Ok(serde_json::json!(null)));
        let tools: Arc<dyn ToolRegistry> = Arc::new(registry);
        let sink: Arc<dyn StreamSink> = Arc::new(BufferSink::new());
        let hooks = HookRegistry::new().with(AssertTools);
        let mut ctx = build_ctx(provider, tools, hooks, sink);
        ctx.push_message(Message::user_text("hi"));

        let req = InferenceRequest::new("m");
        let _ = ctx.run_inference_streaming(&req).await.expect("ok");
    }

    #[tokio::test]
    async fn stream_events_reach_sink() {
        let provider = Arc::new(MockProvider::new("mock"));
        provider.enqueue(
            vec![
                StreamEvent::TextStart { id: "t1".into() },
                StreamEvent::TextDelta {
                    id: "t1".into(),
                    delta: "hello ".into(),
                },
                StreamEvent::TextDelta {
                    id: "t1".into(),
                    delta: "world".into(),
                },
                StreamEvent::TextEnd { id: "t1".into() },
            ],
            text_response("hello world"),
        );
        let tools: Arc<dyn ToolRegistry> = Arc::new(MockToolRegistry::new());
        let buffer = Arc::new(BufferSink::new());
        let sink: Arc<dyn StreamSink> = buffer.clone();
        let mut ctx = build_ctx(provider, tools, HookRegistry::new(), sink);
        ctx.push_message(Message::user_text("hi"));

        let req = InferenceRequest::new("m").with_max_turns(1);
        let _ = ctx.run_inference_streaming(&req).await.expect("ok");
        let events = buffer.snapshot().await;
        assert_eq!(events.len(), 4);
    }
}
