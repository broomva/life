//! `TypedAgent` — static-typed convenience over [`crate::Agent`].
//!
//! Implementing [`TypedAgent`] in Rust is the **default path** for
//! known-shape agents. The framework auto-derives an [`AgentSpec`]
//! from your impl (using `schemars` for input/output schema
//! synthesis) and auto-impls [`Agent`] over it. The impl's `run`
//! body delegates to the same [`crate::agent::run_spec`] interpreter
//! the dynamic [`AgentSpec`] path uses.
//!
//! ## Why two paths into one engine
//!
//! - `TypedAgent` (this module): static-typed, compile-time-checked,
//!   serde-erased `Input`/`Output`. Use this for agents that ship as
//!   part of your binary.
//! - [`AgentSpec`] (data): runtime-constructed, JSON-typed. Use this
//!   for dynamic patterns — agents that generate other agents,
//!   agents loaded from a registry, agents flowing through a wire
//!   transport.
//!
//! Both lower to the same engine. The framework's correctness
//! guarantees (schema validation, retries, hook firing, lifecycle
//! events) are uniform across both.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use schemars::schema_for;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::agent::{Agent, AgentSpec, run_spec};
use crate::error::{ErgonError, Result};
use crate::step::StepCtx;

/// Default for [`TypedAgent::max_turns`]. Matches the autonomous
/// loop's general-purpose default — high enough for genuine agents,
/// low enough to fail fast on runaway behavior.
pub const DEFAULT_TYPED_AGENT_MAX_TURNS: u32 = 16;

/// Default for [`TypedAgent::max_retries`]. One initial attempt plus
/// up to three corrective retries on schema-violation feedback.
pub const DEFAULT_TYPED_AGENT_MAX_RETRIES: u8 = 3;

/// Static-typed convenience trait for known-shape agents.
///
/// Implementers supply the `Input` / `Output` types (with `Serialize`,
/// `Deserialize`, and `JsonSchema` bounds) and a small set of config
/// methods describing the agent's behavior. The framework auto-derives
/// an [`AgentSpec`] from your impl and auto-impls [`Agent`] over it.
///
/// ## Example
///
/// ```ignore
/// use ergon::{TypedAgent, StepCtx};
/// use std::borrow::Cow;
/// use schemars::JsonSchema;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Serialize, JsonSchema)] struct ScoreInput { text: String }
/// #[derive(Deserialize, JsonSchema)] struct ScoreOutput { score: u8 }
///
/// struct Scorer;
/// impl TypedAgent for Scorer {
///     type Input = ScoreInput;
///     type Output = ScoreOutput;
///     fn name(&self) -> &str { "demo.scorer" }
///     fn instructions(&self) -> Cow<'_, str> {
///         Cow::Borrowed("Score the text 1-10. Higher is better.")
///     }
///     fn model(&self) -> &str { "claude-haiku-4-5" }
///     fn max_turns(&self) -> u32 { 1 }  // single-shot judge
/// }
/// ```
pub trait TypedAgent: Send + Sync + 'static {
    /// Typed input. Must be JSON-serializable AND have a JSON Schema
    /// (so the framework can declare the agent's input contract).
    type Input: Serialize + JsonSchema + Send;

    /// Typed output. Must be JSON-deserializable AND have a JSON
    /// Schema (so the framework can synthesize the `record_answer`
    /// tool's input schema and validate the model's response).
    type Output: DeserializeOwned + JsonSchema + Send;

    /// Stable agent name. Convention: dotted-path, e.g.
    /// `"bookkeeping.score-extract"`.
    fn name(&self) -> &str;

    /// Behavioral contract — becomes the agent-scope [`crate::Role`]
    /// overlay. Conventionally written as a system prompt.
    fn instructions(&self) -> Cow<'_, str>;

    /// Provider-specific model identifier, e.g.
    /// `"claude-sonnet-4-5-20250929"`.
    fn model(&self) -> &str;

    /// Maximum autonomous loop turns. Default
    /// [`DEFAULT_TYPED_AGENT_MAX_TURNS`]. Override low (1-2) for
    /// single-shot judges, high (32+) for genuine agents that plan,
    /// use tools, observe results, and iterate.
    fn max_turns(&self) -> u32 {
        DEFAULT_TYPED_AGENT_MAX_TURNS
    }

    /// Maximum retries on output schema validation failure. Default
    /// [`DEFAULT_TYPED_AGENT_MAX_RETRIES`].
    fn max_retries(&self) -> u8 {
        DEFAULT_TYPED_AGENT_MAX_RETRIES
    }

    /// Optional whitelist of tool names the agent may invoke. `None`
    /// (default) means inherit the workflow's full registry. The
    /// framework always synthesizes the agent's own `record_answer`
    /// tool; this filters everything else.
    fn allowed_tools(&self) -> Option<&[String]> {
        None
    }
}

/// Bridge `TypedAgent` impls into `Agent` impls.
///
/// Auto-derives an [`AgentSpec`] from the impl's typed I/O and
/// config, then runs through the shared [`run_spec`] interpreter.
#[async_trait]
impl<T: TypedAgent> Agent for T {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            name: self.name().to_owned(),
            instructions: self.instructions().to_string(),
            model: self.model().to_owned(),
            max_turns: self.max_turns(),
            max_retries: self.max_retries(),
            allowed_tools: self.allowed_tools().map(|s| s.to_vec()),
            input_schema: typed_schema::<T::Input>(),
            output_schema: typed_schema::<T::Output>(),
            extensions: HashMap::new(),
        }
    }

    async fn run(&self, ctx: &mut StepCtx<'_>, input: Value) -> Result<Value> {
        run_spec(&self.spec(), ctx, input).await
    }
}

/// Helper for `TypedAgent` impls: generate the JSON Schema for a type
/// in the LLM-friendly form the interpreter expects.
///
/// The schema is post-processed to strip schemars metadata that
/// confuses LLMs — `$schema`, `title` for primitives, `definitions`
/// when there's only one definition. Inlines refs where possible.
pub fn typed_schema<T: JsonSchema>() -> Value {
    let raw = schema_for!(T);
    let mut value = serde_json::to_value(&raw).unwrap_or(Value::Null);
    sanitize_schema_for_llm(&mut value);
    value
}

/// Strip / inline schemars-specific decoration that's noise to the
/// model. Modifies in-place.
fn sanitize_schema_for_llm(value: &mut Value) {
    if let Value::Object(map) = value {
        // Drop the top-level `$schema` URL — purely informational.
        map.remove("$schema");

        // If schemars emitted a `definitions` block with exactly one
        // entry AND the top-level is a `$ref` to it, inline the
        // definition. This is the common case for a single derived
        // type and produces a flatter, more LLM-friendly schema.
        if let (Some(Value::Object(defs)), Some(Value::String(reference))) =
            (map.get("definitions").cloned(), map.get("$ref").cloned())
            && let Some(name) = reference.strip_prefix("#/definitions/")
            && defs.len() == 1
            && let Some(def) = defs.get(name).cloned()
        {
            map.remove("definitions");
            map.remove("$ref");
            if let Value::Object(def_map) = def {
                for (k, v) in def_map {
                    map.insert(k, v);
                }
            }
        }

        // Recurse into nested objects/arrays. Avoid borrowing issues
        // by collecting keys first.
        let keys: Vec<String> = map.keys().cloned().collect();
        for k in keys {
            if let Some(child) = map.get_mut(&k) {
                sanitize_schema_for_llm(child);
            }
        }
    } else if let Value::Array(arr) = value {
        for item in arr.iter_mut() {
            sanitize_schema_for_llm(item);
        }
    }
}

// ─── Convenient bridge: any Agent runs as a Step via the AgentStep wrapper ─

/// Wrap any [`Agent`] as a [`crate::Step`] so workflow bodies can
/// compose agents through the standard `ctx.step(...)` API.
///
/// Why a wrapper rather than a blanket `impl<A: Agent> Step for A`?
/// Trait coherence — a blanket impl would collide with future blanket
/// `Step` impls and prevent users from writing their own
/// non-LLM-Step types. The wrapper is explicit and intentional.
///
/// The wrapped agent's `spec().name` is cached at construction so
/// `Step::name()` produces per-agent telemetry rather than a
/// collapsed `"agent"` literal — important for tracing, hook
/// filtering, and lago provenance.
pub struct AgentStep<A: Agent + 'static> {
    inner: Arc<A>,
    cached_name: String,
}

impl<A: Agent + 'static> AgentStep<A> {
    /// Construct from an Arc-wrapped agent. The agent's
    /// `spec().name` is read once and cached for `Step::name()`.
    pub fn new(agent: Arc<A>) -> Self {
        let cached_name = agent.spec().name;
        Self {
            inner: agent,
            cached_name,
        }
    }

    /// Convenience: construct from a boxed agent.
    pub fn from_value(agent: A) -> Self {
        Self::new(Arc::new(agent))
    }

    /// Read-only access to the wrapped agent.
    pub fn agent(&self) -> &Arc<A> {
        &self.inner
    }
}

#[async_trait]
impl<A: Agent + 'static> crate::Step for AgentStep<A> {
    type Input = Value;
    type Output = Value;

    fn name(&self) -> &str {
        &self.cached_name
    }

    async fn run(&self, ctx: &mut StepCtx<'_>, input: Self::Input) -> Result<Self::Output> {
        self.inner.run(ctx, input).await
    }
}

/// Convenience: convert any `TypedAgent` into a typed `Step` whose
/// `Input`/`Output` are the agent's typed I/O. Hides the JSON
/// boundary inside the wrapper so workflow bodies stay statically
/// typed end-to-end.
pub struct TypedAgentStep<T: TypedAgent> {
    inner: Arc<T>,
}

impl<T: TypedAgent> TypedAgentStep<T> {
    pub fn new(agent: Arc<T>) -> Self {
        Self { inner: agent }
    }
    pub fn from_value(agent: T) -> Self {
        Self::new(Arc::new(agent))
    }
    pub fn agent(&self) -> &Arc<T> {
        &self.inner
    }
}

#[async_trait]
impl<T: TypedAgent> crate::Step for TypedAgentStep<T> {
    type Input = T::Input;
    type Output = T::Output;

    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn run(&self, ctx: &mut StepCtx<'_>, input: Self::Input) -> Result<Self::Output> {
        let input_value = serde_json::to_value(&input).map_err(ErgonError::Codec)?;
        let output_value = self.inner.run(ctx, input_value).await?;
        serde_json::from_value(output_value).map_err(ErgonError::Codec)
    }
}
