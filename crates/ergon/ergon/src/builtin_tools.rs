//! Builtin tools synthesized into every workflow's tool registry.
//!
//! These are the tools that the framework provides directly, rather
//! than registering through praxis. They implement [`crate::ToolRegistry`]
//! and are chained into the tool registry the autonomous loop sees.
//!
//! Currently shipped:
//!
//! - [`SpawnAgentTool`] — `spawn_agent(name, input)`. Resolves an
//!   agent by name from an [`crate::AgentRegistry`], runs it as a
//!   sub-agent, returns the typed JSON output as the tool result.
//!   Goes through the full hook lifecycle (capability gate, budget,
//!   score, attest) on the sub-agent's invocation. Recursion safety
//!   via [`crate::RecursionContext`].
//!
//! Reserved for follow-ups:
//!
//! - `improve_agent(spec, feedback)` — for the eventual self-
//!   improvement loop. See architecture spec §7.3.
//! - `lago_query(filter)` — for nous-promoter and similar
//!   meta-agents. Lives in `nous-tools` (separate crate).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::agent_registry::AgentRegistry;
use crate::error::{ErgonError, Result};
use crate::model::{ToolCall, ToolDefinition, ToolResult};
use crate::recursion::RecursionError;
use crate::runtime::ToolRegistry;

/// Stable name of the spawn-agent tool. Agents call this exactly to
/// invoke a registered sub-agent.
pub const SPAWN_AGENT_TOOL: &str = "spawn_agent";

/// Builtin tool that lets agents invoke other agents by name from
/// within their autonomous loop.
///
/// Constructed by the host runtime ([`arcan-ergon`]'s
/// `run_workflow_as_tick`) and chained into the agent's tool registry
/// alongside the user-supplied tools and the synthetic
/// `record_answer` tool.
///
/// ## Tool input shape
///
/// ```json
/// {
///   "name": "<registered agent name>",
///   "input": <agent's typed input as JSON>
/// }
/// ```
///
/// ## Tool output shape (success)
///
/// ```json
/// {
///   "ok": true,
///   "agent": "<resolved name>",
///   "output": <agent's typed output as JSON>
/// }
/// ```
///
/// ## Tool output shape (model-visible error)
///
/// On any failure (unknown agent, recursion-context refusal, sub-
/// agent error), returns `ToolResult::model_error` so the parent
/// agent sees the failure in-band and can adapt:
///
/// ```json
/// {
///   "error": "<error category>",
///   "detail": "<human-readable message>",
///   "agent": "<requested name>"
/// }
/// ```
///
/// ## Why this is a tool, not a method
///
/// Modeling spawn as a tool means it goes through the standard
/// dispatch path:
///
/// - `Hook::on_pre_tool_use` fires (capability gate, budget gate
///   inspect the call before it executes)
/// - The tool result is appended to the autonomous loop's history
///   as a normal `ContentBlock::ToolResult`
/// - `Hook::on_post_tool_use` fires
/// - Lago events record the spawn as part of the tick's journal
///
/// All this happens uniformly with every other tool — no special
/// dispatch path, no special tracing, no special governance. The
/// only thing the framework does specially is **inject** this tool
/// into every workflow's registry.
pub struct SpawnAgentTool {
    registry: Arc<dyn AgentRegistry>,
}

impl SpawnAgentTool {
    /// Construct from an [`AgentRegistry`].
    pub fn new(registry: Arc<dyn AgentRegistry>) -> Self {
        Self { registry }
    }

    /// The tool's input JSON Schema.
    fn input_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The registered name of the sub-agent to invoke."
                },
                "input": {
                    "description": "The sub-agent's typed input. Must match the sub-agent's declared input_schema.",
                }
            },
            "required": ["name", "input"],
            "additionalProperties": false,
        })
    }

    /// Read-only access to the underlying registry. Used by
    /// [`crate::run_spec`]'s chained dispatch path.
    pub fn registry(&self) -> &Arc<dyn AgentRegistry> {
        &self.registry
    }
}

#[async_trait]
impl ToolRegistry for SpawnAgentTool {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition::new(
            SPAWN_AGENT_TOOL,
            "Invoke a sub-agent by name. The sub-agent runs with its own \
             isolated message history, hook lifecycle, and recursion frame. \
             Returns the sub-agent's typed JSON output as the `output` field. \
             On any failure (unknown agent, recursion limit, sub-agent error), \
             returns a model-visible error you can reason about and adapt to.",
            Self::input_schema(),
        )]
    }

    /// This `invoke` is **deliberately not used** in production.
    /// Production dispatch goes through [`crate::run_spec`]'s chained
    /// registry, which intercepts spawn_agent calls and runs the
    /// sub-agent against the live `StepCtx`. This impl exists to
    /// satisfy the `ToolRegistry` trait so `SpawnAgentTool` can be
    /// chained as a definitions-only source.
    async fn invoke(&self, call: ToolCall) -> Result<ToolResult> {
        Err(ErgonError::internal(format!(
            "SpawnAgentTool::invoke called directly for `{}` — production \
             dispatch must go through run_spec's chained registry which \
             carries the live StepCtx. This is a framework bug; please \
             report.",
            call.name
        )))
    }
}

/// Parsed `spawn_agent` arguments.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SpawnArgs {
    pub name: String,
    pub input: Value,
}

/// Translate a `RecursionError` into the model-visible tool error
/// payload. Stable shape so agents can pattern-match on the
/// `error` field.
pub fn spawn_error_payload(name: &str, err: &RecursionError) -> Value {
    let category = match err {
        RecursionError::DepthExceeded { .. } => "depth_exceeded",
        RecursionError::CycleDetected { .. } => "cycle_detected",
        RecursionError::InvocationLimitExceeded { .. } => "invocation_limit",
        RecursionError::TokenBudgetExhausted { .. } => "token_budget_exhausted",
        RecursionError::WallClockBudgetExhausted { .. } => "wall_clock_exhausted",
    };
    serde_json::json!({
        "error": category,
        "detail": err.to_string(),
        "agent": name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentSpec;
    use crate::agent_registry::InMemoryAgentRegistry;

    fn empty_agent_registry() -> Arc<dyn AgentRegistry> {
        Arc::new(InMemoryAgentRegistry::new()) as Arc<dyn AgentRegistry>
    }

    fn one_agent_registry() -> Arc<dyn AgentRegistry> {
        let reg = InMemoryAgentRegistry::new();
        reg.insert_spec(AgentSpec::new(
            "summarizer",
            "claude-haiku-4-5",
            "You summarize input text into one sentence.",
            serde_json::json!({"type": "object"}),
            serde_json::json!({"type": "object", "properties": {"summary": {"type": "string"}}}),
        ));
        Arc::new(reg) as Arc<dyn AgentRegistry>
    }

    #[test]
    fn spawn_tool_definitions_include_spawn_agent() {
        let tool = SpawnAgentTool::new(empty_agent_registry());
        let defs = tool.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, SPAWN_AGENT_TOOL);
        // Schema includes name + input as required fields.
        let schema = &defs[0].input_schema;
        let required = schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("required array");
        let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required_names.contains(&"name"));
        assert!(required_names.contains(&"input"));
    }

    #[tokio::test]
    async fn spawn_tool_direct_invoke_returns_internal_error() {
        // Direct invocation (not through run_spec) must error — the
        // production dispatch path lives elsewhere.
        let tool = SpawnAgentTool::new(empty_agent_registry());
        let call = ToolCall::new(
            "tu-1",
            SPAWN_AGENT_TOOL,
            serde_json::json!({"name": "x", "input": {}}),
        );
        let err = tool.invoke(call).await.expect_err("must error");
        assert!(matches!(err, ErgonError::Internal(_)));
    }

    #[tokio::test]
    async fn spawn_args_parses_typed_input() {
        let raw = serde_json::json!({"name": "summarizer", "input": {"text": "hi"}});
        let args: SpawnArgs = serde_json::from_value(raw).unwrap();
        assert_eq!(args.name, "summarizer");
        assert_eq!(args.input["text"], "hi");
    }

    #[test]
    fn spawn_error_payload_categorizes_recursion_errors() {
        let depth_err = RecursionError::DepthExceeded {
            depth: 8,
            max_depth: 8,
            attempted: "x".into(),
        };
        let payload = spawn_error_payload("x", &depth_err);
        assert_eq!(payload["error"], "depth_exceeded");
        assert_eq!(payload["agent"], "x");
        assert!(payload["detail"].as_str().unwrap().contains("depth=8"));

        let cycle_err = RecursionError::CycleDetected {
            cycle_target: "y".into(),
            stack: vec!["a".into(), "y".into()],
        };
        let payload = spawn_error_payload("y", &cycle_err);
        assert_eq!(payload["error"], "cycle_detected");

        let inv_err = RecursionError::InvocationLimitExceeded {
            total: 256,
            max: 256,
            attempted: "z".into(),
        };
        assert_eq!(
            spawn_error_payload("z", &inv_err)["error"],
            "invocation_limit"
        );

        let tok_err = RecursionError::TokenBudgetExhausted {
            attempted: "w".into(),
        };
        assert_eq!(
            spawn_error_payload("w", &tok_err)["error"],
            "token_budget_exhausted"
        );
    }

    #[tokio::test]
    async fn registry_round_trip() {
        let tool = SpawnAgentTool::new(one_agent_registry());
        assert!(tool.registry().get("summarizer").await.is_some());
        assert!(tool.registry().get("missing").await.is_none());
    }
}
