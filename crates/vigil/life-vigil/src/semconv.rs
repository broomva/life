//! Semantic convention constants for GenAI and Life Agent OS observability.
//!
//! These follow the [OTel GenAI semantic conventions](https://opentelemetry.io/docs/specs/semconv/gen-ai/)
//! and extend them with Life-specific attributes under the `life.` namespace.

// ─── GenAI Semantic Conventions ──────────────────────────────────────────────

/// The name of the operation being performed (e.g. "chat", "text_completion").
pub const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";

/// The AI system / provider (e.g. "anthropic", "openai").
pub const GEN_AI_SYSTEM: &str = "gen_ai.system";

/// The model requested (e.g. "claude-sonnet-4-20250514").
pub const GEN_AI_REQUEST_MODEL: &str = "gen_ai.request.model";

/// Maximum number of tokens the model should generate.
pub const GEN_AI_REQUEST_MAX_TOKENS: &str = "gen_ai.request.max_tokens";

/// Sampling temperature.
pub const GEN_AI_REQUEST_TEMPERATURE: &str = "gen_ai.request.temperature";

/// Top-p (nucleus) sampling parameter.
pub const GEN_AI_REQUEST_TOP_P: &str = "gen_ai.request.top_p";

/// Number of input (prompt) tokens consumed.
pub const GEN_AI_USAGE_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";

/// Number of output (completion) tokens generated.
pub const GEN_AI_USAGE_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";

/// Reasons the model stopped generating (e.g. "stop", "tool_use", "max_tokens").
pub const GEN_AI_RESPONSE_FINISH_REASONS: &str = "gen_ai.response.finish_reasons";

/// Provider-assigned response ID.
pub const GEN_AI_RESPONSE_ID: &str = "gen_ai.response.id";

/// The model actually used (may differ from requested model).
pub const GEN_AI_RESPONSE_MODEL: &str = "gen_ai.response.model";

/// Tool name being called or executed.
pub const GEN_AI_TOOL_NAME: &str = "gen_ai.tool.name";

/// Tool call ID assigned by the model.
pub const GEN_AI_TOOL_CALL_ID: &str = "gen_ai.tool.call.id";

/// Agent name.
pub const GEN_AI_AGENT_NAME: &str = "gen_ai.agent.name";

/// Agent ID.
pub const GEN_AI_AGENT_ID: &str = "gen_ai.agent.id";

// ─── GenAI Content Event Names ──────────────────────────────────────────────

/// Span event name for recording prompt/input content on a GenAI span.
pub const GEN_AI_CONTENT_PROMPT: &str = "gen_ai.content.prompt";

/// Span event name for recording completion/output content on a GenAI span.
pub const GEN_AI_CONTENT_COMPLETION: &str = "gen_ai.content.completion";

// ─── Server Attributes ──────────────────────────────────────────────────────

/// Server hostname or IP.
pub const SERVER_ADDRESS: &str = "server.address";

/// Server port number.
pub const SERVER_PORT: &str = "server.port";

// ─── Life Agent OS Attributes (life.* namespace) ─────────────────────────────

/// Agent OS session ID.
pub const LIFE_SESSION_ID: &str = "life.session_id";

/// Agent OS run ID (within a session).
pub const LIFE_RUN_ID: &str = "life.run_id";

/// Agent OS branch ID.
pub const LIFE_BRANCH_ID: &str = "life.branch_id";

/// Current loop phase (perceive, deliberate, gate, execute, commit, reflect, sleep).
pub const LIFE_LOOP_PHASE: &str = "life.loop_phase";

/// Current operating mode (explore, execute, verify, recover, ask_human, sleep).
pub const LIFE_OPERATING_MODE: &str = "life.operating_mode";

/// Event sequence number within a session.
pub const LIFE_EVENT_SEQ: &str = "life.event_seq";

/// Event kind discriminant (e.g. "tool_call_requested", "assistant_text_delta").
pub const LIFE_EVENT_KIND: &str = "life.event_kind";

/// Remaining token budget.
pub const LIFE_BUDGET_TOKENS: &str = "life.budget.tokens_remaining";

/// Remaining cost budget (USD).
pub const LIFE_BUDGET_COST: &str = "life.budget.cost_remaining_usd";

/// Remaining time budget (ms).
pub const LIFE_BUDGET_TIME: &str = "life.budget.time_remaining_ms";

/// Remaining tool calls budget.
pub const LIFE_BUDGET_TOOL_CALLS: &str = "life.budget.tool_calls_remaining";

/// Agent state vector: progress (0.0..1.0).
pub const LIFE_STATE_PROGRESS: &str = "life.state.progress";

/// Agent state vector: uncertainty (0.0..1.0).
pub const LIFE_STATE_UNCERTAINTY: &str = "life.state.uncertainty";

/// Agent state vector: risk level.
pub const LIFE_STATE_RISK_LEVEL: &str = "life.state.risk_level";

/// Agent state vector: consecutive error count.
pub const LIFE_STATE_ERROR_STREAK: &str = "life.state.error_streak";

/// Agent state vector: context pressure (0.0..1.0).
pub const LIFE_STATE_CONTEXT_PRESSURE: &str = "life.state.context_pressure";

/// Tool execution status (ok, error, timeout, cancelled).
pub const LIFE_TOOL_STATUS: &str = "life.tool.status";

/// Tool execution output (when content capture is enabled).
pub const LIFE_TOOL_OUTPUT: &str = "life.tool.output";

// ─── Vigil LLM Cost Envelope Attributes ─────────────────────────────────────

/// Stable request identifier for one LLM provider call.
pub const VIGIL_LLM_REQUEST_ID: &str = "vigil.llm.request_id";

/// Tenant identifier used for multi-tenant cost attribution.
pub const VIGIL_LLM_TENANT_ID: &str = "vigil.llm.tenant_id";

/// Caller identifier used for actor-level attribution.
pub const VIGIL_LLM_CALLER_ID: &str = "vigil.llm.caller_id";

/// Task identifier used for task-level cost attribution.
pub const VIGIL_LLM_TASK_ID: &str = "vigil.llm.task_id";

/// Provider requested before routing.
pub const VIGIL_LLM_PROVIDER_REQUESTED: &str = "vigil.llm.provider_requested";

/// Provider selected after routing.
pub const VIGIL_LLM_PROVIDER_SELECTED: &str = "vigil.llm.provider_selected";

/// Model tier classification used for policy and budget analysis.
pub const VIGIL_LLM_MODEL_TIER: &str = "vigil.llm.model_tier";

/// Routing decision or policy that selected the model/provider.
pub const VIGIL_LLM_ROUTING_DECISION: &str = "vigil.llm.routing_decision";

/// Estimated pre-call input tokens.
pub const VIGIL_LLM_TOKENS_IN: &str = "vigil.llm.tokens_in";

/// Estimated pre-call output tokens.
pub const VIGIL_LLM_TOKENS_OUT: &str = "vigil.llm.tokens_out";

/// Estimated or actual total call cost in USD.
pub const VIGIL_LLM_ESTIMATED_COST_USD: &str = "vigil.llm.estimated_cost_usd";

/// Provenance for the cost figure.
pub const VIGIL_LLM_COST_SOURCE: &str = "vigil.llm.cost_source";

/// Provider call latency in milliseconds.
pub const VIGIL_LLM_LATENCY_MS: &str = "vigil.llm.latency_ms";

/// Time to first streamed token in milliseconds, when available.
pub const VIGIL_LLM_TTFT_MS: &str = "vigil.llm.time_to_first_token_ms";

/// Provider policy decision applied at the request boundary.
pub const VIGIL_LLM_POLICY_DECISION: &str = "vigil.llm.policy_decision";

/// Policy mode active for the request.
pub const VIGIL_LLM_POLICY_MODE: &str = "vigil.llm.policy_mode";

/// Whether PII was detected before the provider call.
pub const VIGIL_LLM_PII_DETECTED: &str = "vigil.llm.pii_detected";

/// Whether provider-bound content was redacted.
pub const VIGIL_LLM_REDACTION_APPLIED: &str = "vigil.llm.redaction_applied";

// ─── Reliability Attributes ─────────────────────────────────────────────────

/// Number of retries before the request succeeded (0 = first attempt).
pub const LIFE_RETRY_COUNT: &str = "life.reliability.retry_count";

/// Whether a fallback provider was used for this call.
pub const LIFE_FALLBACK_TRIGGERED: &str = "life.reliability.fallback_triggered";

/// Circuit breaker state at time of call (closed, open, half_open).
pub const LIFE_CIRCUIT_STATE: &str = "life.reliability.circuit_state";

// ─── Autonomic Attributes ────────────────────────────────────────────────────

/// Autonomic economic mode (sovereign, conserving, hustle, hibernate).
pub const AUTONOMIC_ECONOMIC_MODE: &str = "autonomic.economic_mode";

/// Autonomic operational health (0.0..1.0).
pub const AUTONOMIC_OPERATIONAL_HEALTH: &str = "autonomic.operational_health";

/// Autonomic cognitive health (0.0..1.0).
pub const AUTONOMIC_COGNITIVE_HEALTH: &str = "autonomic.cognitive_health";

/// Autonomic economic health (0.0..1.0).
pub const AUTONOMIC_ECONOMIC_HEALTH: &str = "autonomic.economic_health";

// ─── Nous Evaluation Attributes ──────────────────────────────────────────────

/// Evaluation result event name (follows GenAI semconv v1.39.0).
pub const GEN_AI_EVAL_RESULT: &str = "gen_ai.evaluation.result";

/// Name of the evaluator that produced the score.
pub const LIFE_EVAL_EVALUATOR: &str = "life.eval.evaluator";

/// Normalized quality score (0.0..1.0).
pub const LIFE_EVAL_SCORE: &str = "life.eval.score";

/// Categorical label (good, warning, critical).
pub const LIFE_EVAL_LABEL: &str = "life.eval.label";

/// Evaluation layer (reasoning, action, execution, safety, cost).
pub const LIFE_EVAL_LAYER: &str = "life.eval.layer";

/// Evaluation timing (inline, async).
pub const LIFE_EVAL_TIMING: &str = "life.eval.timing";

// ─── Lago Attributes ─────────────────────────────────────────────────────────

/// Lago journal stream ID.
pub const LAGO_STREAM_ID: &str = "lago.stream_id";

/// Lago blob content hash (SHA-256).
pub const LAGO_BLOB_HASH: &str = "lago.blob_hash";

/// Lago filesystem manifest branch.
pub const LAGO_FS_BRANCH: &str = "lago.fs_branch";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genai_constants_match_otel_spec() {
        // Verify naming convention: all GenAI attrs start with "gen_ai."
        assert!(GEN_AI_OPERATION_NAME.starts_with("gen_ai."));
        assert!(GEN_AI_SYSTEM.starts_with("gen_ai."));
        assert!(GEN_AI_REQUEST_MODEL.starts_with("gen_ai."));
        assert!(GEN_AI_REQUEST_MAX_TOKENS.starts_with("gen_ai."));
        assert!(GEN_AI_REQUEST_TEMPERATURE.starts_with("gen_ai."));
        assert!(GEN_AI_USAGE_INPUT_TOKENS.starts_with("gen_ai."));
        assert!(GEN_AI_USAGE_OUTPUT_TOKENS.starts_with("gen_ai."));
        assert!(GEN_AI_RESPONSE_FINISH_REASONS.starts_with("gen_ai."));
        assert!(GEN_AI_RESPONSE_ID.starts_with("gen_ai."));
        assert!(GEN_AI_TOOL_NAME.starts_with("gen_ai."));
        assert!(GEN_AI_TOOL_CALL_ID.starts_with("gen_ai."));
        assert!(GEN_AI_AGENT_NAME.starts_with("gen_ai."));
        assert!(GEN_AI_AGENT_ID.starts_with("gen_ai."));
    }

    #[test]
    fn life_constants_use_correct_prefix() {
        assert!(LIFE_SESSION_ID.starts_with("life."));
        assert!(LIFE_RUN_ID.starts_with("life."));
        assert!(LIFE_LOOP_PHASE.starts_with("life."));
        assert!(LIFE_OPERATING_MODE.starts_with("life."));
        assert!(LIFE_BUDGET_TOKENS.starts_with("life."));
        assert!(LIFE_STATE_PROGRESS.starts_with("life."));
    }

    #[test]
    fn vigil_llm_constants_use_correct_prefix() {
        assert!(VIGIL_LLM_REQUEST_ID.starts_with("vigil.llm."));
        assert!(VIGIL_LLM_PROVIDER_REQUESTED.starts_with("vigil.llm."));
        assert!(VIGIL_LLM_ESTIMATED_COST_USD.starts_with("vigil.llm."));
        assert!(VIGIL_LLM_POLICY_DECISION.starts_with("vigil.llm."));
    }

    #[test]
    fn autonomic_constants_use_correct_prefix() {
        assert!(AUTONOMIC_ECONOMIC_MODE.starts_with("autonomic."));
        assert!(AUTONOMIC_OPERATIONAL_HEALTH.starts_with("autonomic."));
    }

    #[test]
    fn lago_constants_use_correct_prefix() {
        assert!(LAGO_STREAM_ID.starts_with("lago."));
        assert!(LAGO_BLOB_HASH.starts_with("lago."));
    }

    #[test]
    fn genai_exact_values() {
        assert_eq!(GEN_AI_OPERATION_NAME, "gen_ai.operation.name");
        assert_eq!(GEN_AI_SYSTEM, "gen_ai.system");
        assert_eq!(GEN_AI_REQUEST_MODEL, "gen_ai.request.model");
        assert_eq!(GEN_AI_USAGE_INPUT_TOKENS, "gen_ai.usage.input_tokens");
        assert_eq!(GEN_AI_USAGE_OUTPUT_TOKENS, "gen_ai.usage.output_tokens");
        assert_eq!(
            GEN_AI_RESPONSE_FINISH_REASONS,
            "gen_ai.response.finish_reasons"
        );
        assert_eq!(GEN_AI_RESPONSE_ID, "gen_ai.response.id");
        assert_eq!(GEN_AI_TOOL_NAME, "gen_ai.tool.name");
        assert_eq!(GEN_AI_TOOL_CALL_ID, "gen_ai.tool.call.id");
    }

    #[test]
    fn server_attributes_match_otel() {
        assert_eq!(SERVER_ADDRESS, "server.address");
        assert_eq!(SERVER_PORT, "server.port");
    }
}
