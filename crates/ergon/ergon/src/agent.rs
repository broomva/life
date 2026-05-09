//! Agent primitive — first-class typed-I/O bounded computation with
//! structured output.
//!
//! ## Where this fits
//!
//! `Workflow` is the deterministic outer body. `Step` is any unit of
//! work composed inside that body. `Agent` is **a Step that performs
//! the agent loop** — a model call (possibly multi-turn with tool
//! dispatch) whose final act produces a typed answer matching a
//! pre-declared schema.
//!
//! The Agent primitive's job is small and precise:
//!
//! 1. Enforce a typed I/O contract at the boundary (input matches an
//!    `input_schema`, output matches an `output_schema`).
//! 2. Drive the autonomous loop ([`crate::StepCtx::run_inference_streaming`])
//!    with a synthetic `record_answer` tool whose schema matches the
//!    declared output type.
//! 3. Capture the model's `record_answer` tool call, validate its
//!    args against the schema, deserialize into the typed output.
//! 4. Retry on schema violation with a corrective message; surface a
//!    typed [`AgentError`] when retries are exhausted.
//!
//! Everything else — identity, capabilities, budgets, observability,
//! trust, communication — is delegated to the substrates that already
//! provide those primitives (anima, autonomic, vigil, lago, …) via
//! the existing hook + sink + port architecture.
//!
//! ## Two paths into one engine
//!
//! - **Static-typed**: implement [`crate::TypedAgent`] in Rust with
//!   compile-time `Input`/`Output` types. The framework auto-derives
//!   an [`AgentSpec`] from your impl.
//! - **Dynamic / data-driven**: construct an [`AgentSpec`] at runtime
//!   (deserialized from JSON, returned by another agent, loaded from
//!   lago, etc.) and call [`AgentSpec::run`] directly.
//!
//! Both lower to the same interpreter ([`run_spec`]). The framework's
//! correctness guarantees (schema validation, retry semantics, hook
//! firing, lifecycle events) are uniform across both paths.
//!
//! ## What this primitive does NOT do (and why that's a feature)
//!
//! Deliberately out of scope at v0.1:
//!
//! - **Recursion / spawn_agent**: agents calling agents from within
//!   their loop. The primitive is shaped to absorb this without
//!   breaking changes (see module-level note on [`AgentSpec.extensions`]),
//!   but not shipped here. The first workflow that genuinely needs
//!   in-loop recursive spawning will land that as a narrow follow-up.
//! - **Async invocation / mailboxes**: agents-as-services. Future
//!   `AgentInvoker` trait will wrap `Agent::run` with mailbox
//!   semantics; the primitive's JSON-typed boundary makes that
//!   trivial when the time comes.
//! - **Discovery / registry**: name → agent lookup. Workflows compose
//!   agents explicitly today; future `AgentRegistry` adds string-keyed
//!   lookup once a workflow needs it.
//! - **Long-lived / daemon agents**: reactive agents that don't
//!   return a typed result. That's a SIBLING primitive (not a
//!   subclass) — different shape, different lifecycle.
//!
//! Every deferred concern composes with the existing primitive
//! without requiring it to change.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::{ErgonError, Result};
use crate::model::{ContentBlock, ToolCall, ToolDefinition, ToolResult};
use crate::role::Role;
use crate::runtime::ToolRegistry;
use crate::step::{InferenceRequest, StepCtx};

// ─── AgentSpec ──────────────────────────────────────────────────────────

/// First-class agent value.
///
/// `AgentSpec` is **data**, not just a trait. It can be:
///
/// - Constructed in Rust at compile time
/// - Deserialized from JSON / a config file / a network message
/// - Returned as the typed `Output` of another agent (the "factory"
///   pattern that enables agent-emits-agent composition)
/// - Embedded in a [`crate::stream::StreamEvent`] or lago event for
///   full replay of an agent run
/// - Passed through a transport (spaces channel, mailbox, RPC) without
///   the framework caring
///
/// Marked `#[non_exhaustive]` so we can land additive fields (recursion
/// configs, identity constraints, scheduling hints, remote refs)
/// without a breaking change. Forward-compat values that haven't been
/// promoted to first-class fields yet land in [`Self::extensions`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct AgentSpec {
    /// Stable agent name, e.g. `"bookkeeping.score-extract"`.
    /// Convention: dotted-path, kebab-case segments.
    pub name: String,

    /// Behavioral contract — becomes the agent-scope [`Role`] overlay.
    /// Conventionally written as a system prompt: "You are X. Your
    /// task is Y. You must finish by calling `record_answer` with …"
    pub instructions: String,

    /// Provider-specific model identifier.
    pub model: String,

    /// Maximum autonomous loop turns. Treat as the agent's compute
    /// budget — high for genuine multi-turn agents (research,
    /// planning), low for single-shot judges.
    pub max_turns: u32,

    /// Maximum *corrective retries* on output schema validation
    /// failure (in addition to the initial attempt). A value of `3`
    /// means up to `1 + 3 = 4` total attempts at producing a
    /// schema-conformant answer before [`AgentError::SchemaViolation`]
    /// is surfaced.
    pub max_retries: u8,

    /// Optional whitelist of tool names the agent may invoke. `None`
    /// means inherit the workflow's full registry. The framework
    /// always synthesizes the agent's own `record_answer` tool;
    /// `allowed_tools` filters everything ELSE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,

    /// JSON Schema for the input the agent accepts. Used at the
    /// boundary to validate inbound JSON.
    pub input_schema: Value,

    /// JSON Schema for the typed output. The framework synthesizes a
    /// `record_answer(answer: <output_schema>)` tool from this and
    /// validates the model's tool-use args against it.
    pub output_schema: Value,

    /// Forward-compat slot. New patterns land here as agreed-upon
    /// keys (e.g. `"max_depth": 8`, `"backend_hint": "mlx"`,
    /// `"remote": {…}`) before being promoted to first-class fields.
    /// Empty for v0.1 statically-constructed specs.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extensions: HashMap<String, Value>,
}

impl AgentSpec {
    /// Construct a new spec with the supplied required fields plus
    /// sensible defaults for the rest.
    ///
    /// Defaults:
    /// - `max_turns`: 16
    /// - `max_retries`: 3
    /// - `allowed_tools`: `None` (inherits the workflow's full registry)
    /// - `extensions`: empty
    ///
    /// Builder methods (`with_max_turns`, `with_max_retries`,
    /// `with_allowed_tools`, `with_extension`) refine the result.
    pub fn new(
        name: impl Into<String>,
        model: impl Into<String>,
        instructions: impl Into<String>,
        input_schema: Value,
        output_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            instructions: instructions.into(),
            model: model.into(),
            max_turns: 16,
            max_retries: 3,
            allowed_tools: None,
            input_schema,
            output_schema,
            extensions: HashMap::new(),
        }
    }

    /// Builder: set the autonomous loop turn budget.
    #[must_use]
    pub fn with_max_turns(mut self, n: u32) -> Self {
        self.max_turns = n;
        self
    }

    /// Builder: set the schema-violation retry budget.
    #[must_use]
    pub fn with_max_retries(mut self, n: u8) -> Self {
        self.max_retries = n;
        self
    }

    /// Builder: restrict the agent's tool access to a specific list.
    #[must_use]
    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = Some(tools);
        self
    }

    /// Builder: add a forward-compat extension entry.
    #[must_use]
    pub fn with_extension(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extensions.insert(key.into(), value);
        self
    }

    /// Run this spec against a [`StepCtx`].
    ///
    /// Functionally equivalent to constructing a [`crate::TypedAgent`]
    /// with matching types and calling its `run`. Both lower to the
    /// same engine ([`run_spec`]).
    pub async fn run(&self, ctx: &mut StepCtx<'_>, input: Value) -> Result<Value> {
        run_spec(self, ctx, input).await
    }

    /// Validate that this spec is well-formed enough for the
    /// interpreter to accept. Cheap structural checks only — no I/O.
    /// Called automatically at the start of [`Self::run`].
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(ErgonError::internal("AgentSpec.name is empty"));
        }
        if self.model.is_empty() {
            return Err(ErgonError::internal(format!(
                "AgentSpec `{}`: model is empty",
                self.name
            )));
        }
        if self.max_turns == 0 {
            return Err(ErgonError::internal(format!(
                "AgentSpec `{}`: max_turns must be >= 1",
                self.name
            )));
        }
        if !self.input_schema.is_object() {
            return Err(ErgonError::internal(format!(
                "AgentSpec `{}`: input_schema must be a JSON object",
                self.name
            )));
        }
        if !self.output_schema.is_object() {
            return Err(ErgonError::internal(format!(
                "AgentSpec `{}`: output_schema must be a JSON object",
                self.name
            )));
        }
        Ok(())
    }
}

// ─── Agent trait ────────────────────────────────────────────────────────

/// Anything runnable that conforms to the agent-loop discipline.
///
/// Implemented for:
///
/// - [`AgentSpec`] directly (dynamic / data-driven path)
/// - Any `T: TypedAgent` via the auto-impl in `typed_agent.rs`
///
/// Both lower to [`run_spec`]. Custom `Agent` impls are unusual —
/// prefer `TypedAgent` for static cases or `AgentSpec` for dynamic
/// cases. Implementing `Agent` directly is reserved for advanced
/// patterns like recording wrappers, remote-dispatch shims, etc.
#[async_trait]
pub trait Agent: Send + Sync {
    /// The (possibly-derived) spec for this agent. Cheap to call —
    /// implementations should memoize if the spec is expensive to
    /// construct.
    fn spec(&self) -> AgentSpec;

    /// Run with JSON-typed boundaries. Default impl runs the spec
    /// through the shared interpreter; override only if you have a
    /// genuine reason to bypass it (e.g. remote dispatch).
    async fn run(&self, ctx: &mut StepCtx<'_>, input: Value) -> Result<Value> {
        run_spec(&self.spec(), ctx, input).await
    }
}

#[async_trait]
impl Agent for AgentSpec {
    fn spec(&self) -> AgentSpec {
        self.clone()
    }
    async fn run(&self, ctx: &mut StepCtx<'_>, input: Value) -> Result<Value> {
        run_spec(self, ctx, input).await
    }
}

// ─── AgentError (carried through ErgonError::Workflow / ::Internal) ─────

/// Failures the interpreter can surface. Returned wrapped in
/// [`ErgonError::Workflow`] for boundary callers; tests can downcast
/// via [`AgentError::from_err`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AgentError {
    /// The model never emitted a `record_answer` tool call within the
    /// turn budget.
    AnswerNotEmitted { agent: String, max_turns: u32 },
    /// The model's `record_answer` args failed JSON-schema validation
    /// across all retry attempts.
    SchemaViolation {
        agent: String,
        attempts: u8,
        last_error: String,
    },
    /// Provider returned a refusal stop reason on the final attempt.
    Refusal { agent: String, message: String },
    /// Spec validation failed before the loop even started.
    InvalidSpec { agent: String, reason: String },
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AnswerNotEmitted { agent, max_turns } => {
                write!(
                    f,
                    "agent `{agent}` never emitted record_answer within max_turns={max_turns}"
                )
            }
            Self::SchemaViolation {
                agent,
                attempts,
                last_error,
            } => {
                write!(
                    f,
                    "agent `{agent}`: record_answer args failed schema validation across \
                     {attempts} attempt(s); last error: {last_error}"
                )
            }
            Self::Refusal { agent, message } => {
                write!(f, "agent `{agent}` refused: {message}")
            }
            Self::InvalidSpec { agent, reason } => {
                write!(f, "agent `{agent}` spec invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for AgentError {}

impl From<AgentError> for ErgonError {
    fn from(value: AgentError) -> Self {
        ErgonError::Workflow(value.to_string())
    }
}

// ─── The interpreter ────────────────────────────────────────────────────

/// Name of the synthetic tool the interpreter injects to capture the
/// agent's typed output. Stable string — agents must call this exactly.
pub const RECORD_ANSWER_TOOL: &str = "record_answer";

/// Run an [`AgentSpec`] against a [`StepCtx`].
///
/// This is the single execution engine for both static [`crate::TypedAgent`]
/// and dynamic [`AgentSpec`] paths.
///
/// ## What it does
///
/// 1. Validate the spec.
/// 2. Build a chained [`ToolRegistry`] that wraps the workflow's own
///    registry and intercepts `record_answer` (filtering by
///    `allowed_tools` if set).
/// 3. Open a sub-context (isolated message history; same provider,
///    hooks, sink, runtime).
/// 4. Push the input as a `User` message.
/// 5. Build an [`InferenceRequest`] with `model`, `max_turns`, and a
///    [`Role`] overlay carrying the agent's instructions PLUS a
///    standard suffix instructing the model to finish via `record_answer`.
/// 6. Call [`crate::StepCtx::run_inference_streaming`].
/// 7. After it returns, read the captured answer from the side
///    channel. If absent → `AnswerNotEmitted`. If present but
///    schema-invalid → either retry (with corrective message) up to
///    `max_retries` or surface `SchemaViolation`.
/// 8. Return the validated answer.
///
/// The retry loop uses a fresh `run_inference_streaming` call each
/// time, with the corrective message appended to history. This gives
/// hooks a chance to fire on each retry attempt.
pub async fn run_spec(spec: &AgentSpec, ctx: &mut StepCtx<'_>, input: Value) -> Result<Value> {
    spec.validate().map_err(|e| AgentError::InvalidSpec {
        agent: spec.name.clone(),
        reason: e.to_string(),
    })?;

    // Build the answer-capture sink + chained tool registry.
    let answer_slot: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let recorder = Arc::new(AnswerRecorderTools::new(
        spec.output_schema.clone(),
        Arc::clone(&answer_slot),
    ));
    let chained: Arc<dyn ToolRegistry> = Arc::new(ChainedToolRegistry::new(
        Arc::clone(&ctx.tools),
        recorder,
        spec.allowed_tools.clone(),
    ));

    // Open the sub-context with the chained registry. The Drop impl
    // restores the parent's history + tools when this guard goes out
    // of scope.
    let sub = SubCtx::open(ctx, chained);

    // Seed history with the input as a user message.
    sub.ctx
        .push_message(crate::model::Message::user_text(canonical_input_message(
            spec, &input,
        )));

    // Compose the agent's role overlay. The agent-scope role carries
    // the spec's instructions plus the framework's standard suffix
    // (the record_answer contract). Workflow-scope and call-scope
    // roles, if present, layer on top via the standard precedence rule.
    let role = Role::agent("").with_instruction(compose_instructions(spec));
    let request = InferenceRequest::new(spec.model.clone())
        .with_role(role)
        .with_max_turns(spec.max_turns);

    // Drive the loop with retries on schema violation.
    let mut attempts: u8 = 0;

    loop {
        attempts = attempts.saturating_add(1);

        // Run the autonomous loop. The answer may be captured into
        // the side channel even if the loop subsequently hits
        // MaxTurns waiting for an EndTurn confirmation that never
        // comes — so we check the slot regardless of whether
        // `run_inference_streaming` returned Ok or Err(MaxTurns).
        let response_result = sub.ctx.run_inference_streaming(&request).await;

        // Read the captured answer (if any).
        let captured = {
            let mut guard = answer_slot.lock().await;
            guard.take()
        };

        if let Some(answer) = captured {
            // The answer was delivered. Validate against schema.
            match validate_against_schema(&answer, &spec.output_schema) {
                Ok(()) => return Ok(answer),
                Err(err) => {
                    // `max_retries` is *corrective retries on top of
                    // the initial attempt*, so we exhaust at
                    // `attempts > max_retries` (e.g. max_retries=3
                    // permits attempts 1, 2, 3, 4).
                    if attempts > spec.max_retries.max(1) {
                        return Err(AgentError::SchemaViolation {
                            agent: spec.name.clone(),
                            attempts,
                            last_error: err,
                        }
                        .into());
                    }
                    // Append a corrective user message for the next
                    // attempt and continue the retry loop.
                    sub.ctx
                        .push_message(crate::model::Message::user_text(format!(
                            "Your previous `{RECORD_ANSWER_TOOL}` call's arguments \
                         failed schema validation: {err}. Please call \
                         `{RECORD_ANSWER_TOOL}` again with a corrected payload."
                        )));
                    continue;
                }
            }
        }

        // No answer captured. The terminal state is what determines
        // which typed error we surface.
        return match response_result {
            Ok(response) if matches!(response.stop_reason, crate::stream::StopReason::Refusal) => {
                let text = response
                    .content
                    .iter()
                    .find_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                Err(AgentError::Refusal {
                    agent: spec.name.clone(),
                    message: text,
                }
                .into())
            }
            Ok(_) => Err(AgentError::AnswerNotEmitted {
                agent: spec.name.clone(),
                max_turns: spec.max_turns,
            }
            .into()),
            Err(ErgonError::MaxTurns(_)) => Err(AgentError::AnswerNotEmitted {
                agent: spec.name.clone(),
                max_turns: spec.max_turns,
            }
            .into()),
            Err(other) => Err(other),
        };
    }
}

// ─── Sub-context (isolated history; shared everything else) ─────────────

/// A scoped [`StepCtx`] whose message history is reset for the
/// duration of an agent run, then restored on drop. The parent's
/// history and tools registry are swapped back atomically.
///
/// Why isolated? When a workflow body runs multiple agents *in
/// sequence* against the same `StepCtx`, the sub-scope ensures the
/// next agent doesn't inherit the previous agent's conversation. The
/// `Agent::run` signature takes `&mut StepCtx`, so this isolation is
/// for **serial** reuse of one ctx — concurrent invocations against
/// the same ctx are precluded by the borrow checker.
///
/// For genuine parallel fan-out (e.g. running N agents over a
/// `Vec<Item>` via `try_join_all`), the workflow author should clone
/// the underlying `Provider`/`HookRegistry`/`StreamSink`/`RuntimeHandle`
/// (all `Arc`-based already) and construct N independent
/// `StepCtx` instances — one per branch. This mirrors how
/// `arcan-ergon`'s runner builds a fresh ctx per workflow tick.
struct SubCtx<'a, 'p> {
    ctx: &'a mut StepCtx<'p>,
    saved: Option<(Vec<crate::model::Message>, Arc<dyn ToolRegistry>)>,
}

impl<'a, 'p> SubCtx<'a, 'p> {
    fn open(parent: &'a mut StepCtx<'p>, tools: Arc<dyn ToolRegistry>) -> Self {
        let saved = parent.swap_scope(Vec::new(), tools);
        Self {
            ctx: parent,
            saved: Some(saved),
        }
    }
}

impl<'a, 'p> Drop for SubCtx<'a, 'p> {
    fn drop(&mut self) {
        if let Some((history, tools)) = self.saved.take() {
            self.ctx.swap_scope(history, tools);
        }
    }
}

// ─── Tool registry chaining ─────────────────────────────────────────────

/// Wraps a user-supplied [`ToolRegistry`] with an
/// [`AnswerRecorderTools`] so the synthetic `record_answer` tool is
/// available to the agent. Optionally filters the user's tools by an
/// allow-list (the `allowed_tools` field on [`AgentSpec`]).
struct ChainedToolRegistry {
    user: Arc<dyn ToolRegistry>,
    recorder: Arc<AnswerRecorderTools>,
    allow: Option<Vec<String>>,
}

impl ChainedToolRegistry {
    fn new(
        user: Arc<dyn ToolRegistry>,
        recorder: Arc<AnswerRecorderTools>,
        allow: Option<Vec<String>>,
    ) -> Self {
        Self {
            user,
            recorder,
            allow,
        }
    }

    fn user_tool_allowed(&self, name: &str) -> bool {
        match &self.allow {
            None => true,
            Some(allow) => allow.iter().any(|n| n == name),
        }
    }
}

#[async_trait]
impl ToolRegistry for ChainedToolRegistry {
    fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs = self.recorder.definitions();
        for d in self.user.definitions() {
            if self.user_tool_allowed(&d.name) {
                defs.push(d);
            }
        }
        defs
    }

    async fn invoke(&self, call: ToolCall) -> Result<ToolResult> {
        if call.name == RECORD_ANSWER_TOOL {
            return self.recorder.invoke(call).await;
        }
        if !self.user_tool_allowed(&call.name) {
            return Ok(ToolResult::model_error(
                call.id,
                serde_json::json!({
                    "error": format!(
                        "tool `{}` is not in the agent's allowed_tools list",
                        call.name,
                    )
                }),
            ));
        }
        self.user.invoke(call).await
    }
}

/// One-tool registry exposing only `record_answer`. Captures the
/// model's tool-use args into a side channel for the interpreter to
/// read after the loop terminates.
struct AnswerRecorderTools {
    schema: Value,
    slot: Arc<Mutex<Option<Value>>>,
}

impl AnswerRecorderTools {
    fn new(schema: Value, slot: Arc<Mutex<Option<Value>>>) -> Self {
        Self { schema, slot }
    }
}

#[async_trait]
impl ToolRegistry for AnswerRecorderTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        // Wrap output_schema as `{"type": "object", "properties":
        // {"answer": <schema>}, "required": ["answer"]}` so the model
        // emits `record_answer({"answer": …})`.
        let wrapped = serde_json::json!({
            "type": "object",
            "properties": { "answer": self.schema.clone() },
            "required": ["answer"],
            "additionalProperties": false,
        });
        vec![ToolDefinition::new(
            RECORD_ANSWER_TOOL,
            "Record your final answer. Call this exactly once on your final turn \
             to deliver the typed result. The framework reads the `answer` \
             argument as the agent's output.",
            wrapped,
        )]
    }

    async fn invoke(&self, call: ToolCall) -> Result<ToolResult> {
        let answer = call.input.get("answer").cloned().unwrap_or(Value::Null);
        *self.slot.lock().await = Some(answer);
        // Return synthetic success so the autonomous loop continues
        // and the model gets a chance to terminate cleanly.
        Ok(ToolResult::success(
            call.id,
            serde_json::json!({"ok": true, "recorded": true}),
        ))
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────

fn canonical_input_message(spec: &AgentSpec, input: &Value) -> String {
    // Render the input as a JSON code block so the model can parse it
    // unambiguously. Conventional shape; future agents could override.
    format!(
        "Input for `{}`:\n```json\n{}\n```",
        spec.name,
        serde_json::to_string_pretty(input).unwrap_or_else(|_| "<invalid JSON>".to_owned()),
    )
}

fn compose_instructions(spec: &AgentSpec) -> String {
    // The instructions block PLUS a standard suffix telling the model
    // how to deliver its answer. The suffix is identical across all
    // agents — it's the framework's contract with the model.
    format!(
        "{instructions}\n\n\
         === Output Contract ===\n\
         You must finish by calling the `{RECORD_ANSWER_TOOL}` tool exactly \
         once with `{{\"answer\": <typed answer matching the declared output \
         schema>}}`. Do not respond with text-only on your final turn — the \
         framework reads your answer from the `{RECORD_ANSWER_TOOL}` tool \
         arguments. After calling `{RECORD_ANSWER_TOOL}`, you may stop.",
        instructions = spec.instructions,
    )
}

/// Lightweight schema validation. We don't pull a full JSON-schema
/// validator dep at v0.1 — we check structural fit (object vs array,
/// required keys present, scalar types match). Strict validation is
/// left to consumers that want it; the typed-output `serde::Deserialize`
/// at the static-typed boundary is the real check there.
fn validate_against_schema(value: &Value, schema: &Value) -> std::result::Result<(), String> {
    let kind = schema.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "object" => {
            if !value.is_object() {
                return Err(format!("expected object, got {}", json_kind(value)));
            }
            if let (Some(Value::Array(required)), Some(obj)) =
                (schema.get("required"), value.as_object())
            {
                for r in required {
                    if let Some(key) = r.as_str()
                        && !obj.contains_key(key)
                    {
                        return Err(format!("missing required key `{key}`"));
                    }
                }
            }
        }
        "array" if !value.is_array() => {
            return Err(format!("expected array, got {}", json_kind(value)));
        }
        "string" if !value.is_string() => {
            return Err(format!("expected string, got {}", json_kind(value)));
        }
        "number" | "integer" if !value.is_number() => {
            return Err(format!("expected number, got {}", json_kind(value)));
        }
        "boolean" if !value.is_boolean() => {
            return Err(format!("expected boolean, got {}", json_kind(value)));
        }
        "null" if !value.is_null() => {
            return Err(format!("expected null, got {}", json_kind(value)));
        }
        // Matched type AND value passes that type check, OR no type
        // field / unknown type — both pass through. Permissive at v0.1.
        _ => {}
    }
    Ok(())
}

fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
