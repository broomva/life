//! Contract-derived span builders for the Life Agent OS.
//!
//! Provides helper functions that create properly-attributed `tracing` spans
//! from aiOS kernel types. The `tracing-opentelemetry` layer bridges these
//! to OTel spans automatically.

use aios_protocol::event::{EventEnvelope, LoopPhase, TokenUsage};
use tracing::Span;

use crate::semconv;

/// Create a root `invoke_agent` span for an agent session.
///
/// This is the top-level span for an agent invocation, containing all
/// loop phases and LLM calls as children.
///
/// Emits `session.id` for LangSmith thread grouping alongside the
/// canonical `life.session_id` attribute.
pub fn agent_span(session_id: &str, agent_name: &str) -> Span {
    tracing::info_span!(
        "invoke_agent",
        { semconv::GEN_AI_OPERATION_NAME } = "invoke_agent",
        { semconv::GEN_AI_AGENT_NAME } = agent_name,
        { semconv::GEN_AI_AGENT_ID } = session_id,
        { semconv::LIFE_SESSION_ID } = session_id,
        // LangSmith thread grouping: session.id maps to Threads tab.
        "session.id" = session_id,
    )
}

/// Create a child span for a loop phase.
pub fn phase_span(phase: LoopPhase) -> Span {
    let phase_str = match phase {
        LoopPhase::Perceive => "perceive",
        LoopPhase::Deliberate => "deliberate",
        LoopPhase::Gate => "gate",
        LoopPhase::Execute => "execute",
        LoopPhase::Commit => "commit",
        LoopPhase::Reflect => "reflect",
        LoopPhase::Sleep => "sleep",
    };

    tracing::info_span!("loop_phase", { semconv::LIFE_LOOP_PHASE } = phase_str,)
}

/// Create a GenAI `chat` client span for an LLM call.
///
/// Follows the OTel GenAI semantic convention for client spans:
/// `chat {gen_ai.request.model}` naming pattern.
///
/// Includes `session.id` for LangSmith thread grouping — traces sharing
/// the same session ID appear together in the Threads tab.
pub fn chat_span(
    model: &str,
    provider: &str,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    session_id: &str,
) -> Span {
    tracing::info_span!(
        "chat",
        { semconv::GEN_AI_OPERATION_NAME } = "chat",
        { semconv::GEN_AI_SYSTEM } = provider,
        { semconv::GEN_AI_REQUEST_MODEL } = model,
        { semconv::GEN_AI_REQUEST_MAX_TOKENS } = max_tokens,
        { semconv::GEN_AI_REQUEST_TEMPERATURE } = temperature,
        // Filled after the response via record_token_usage / record_finish_reason:
        { semconv::GEN_AI_USAGE_INPUT_TOKENS } = tracing::field::Empty,
        { semconv::GEN_AI_USAGE_OUTPUT_TOKENS } = tracing::field::Empty,
        { semconv::GEN_AI_RESPONSE_FINISH_REASONS } = tracing::field::Empty,
        { semconv::GEN_AI_RESPONSE_ID } = tracing::field::Empty,
        // Reliability (filled via record_reliability after provider call):
        { semconv::LIFE_RETRY_COUNT } = tracing::field::Empty,
        { semconv::LIFE_FALLBACK_TRIGGERED } = tracing::field::Empty,
        { semconv::LIFE_CIRCUIT_STATE } = tracing::field::Empty,
        // LangSmith thread grouping: session.id on the GenAI span itself.
        "session.id" = session_id,
    )
}

/// Create a GenAI `execute_tool` span for a tool call.
pub fn tool_span(tool_name: &str, tool_call_id: &str) -> Span {
    tracing::info_span!(
        "execute_tool",
        { semconv::GEN_AI_OPERATION_NAME } = "execute_tool",
        { semconv::GEN_AI_TOOL_NAME } = tool_name,
        { semconv::GEN_AI_TOOL_CALL_ID } = tool_call_id,
        { semconv::LIFE_TOOL_STATUS } = tracing::field::Empty,
    )
}

/// Record token usage on the current span via attributes.
///
/// Sets `gen_ai.usage.input_tokens` and `gen_ai.usage.output_tokens`.
pub fn record_token_usage(span: &Span, usage: &TokenUsage) {
    span.record(semconv::GEN_AI_USAGE_INPUT_TOKENS, usage.prompt_tokens);
    span.record(semconv::GEN_AI_USAGE_OUTPUT_TOKENS, usage.completion_tokens);
}

/// Emit a `gen_ai.usage` span event with token counts.
///
/// This uses the span event mechanism (which reliably propagates through
/// `tracing-opentelemetry` → LangSmith) rather than `span.record()` on
/// Empty fields, which may not be exported by some OTel bridges.
///
/// Must be called within an entered span context (the chat span).
pub fn record_usage_event(input_tokens: u32, output_tokens: u32, model: &str, finish_reason: &str) {
    tracing::event!(
        name: "gen_ai.usage",
        tracing::Level::INFO,
        "gen_ai.usage.input_tokens" = input_tokens,
        "gen_ai.usage.output_tokens" = output_tokens,
        "gen_ai.usage.total_tokens" = input_tokens + output_tokens,
        "gen_ai.response.model" = model,
        "gen_ai.response.finish_reasons" = finish_reason,
    );
}

/// Record the finish reason on the current span.
pub fn record_finish_reason(span: &Span, reason: &str) {
    span.record(semconv::GEN_AI_RESPONSE_FINISH_REASONS, reason);
}

/// Record the response ID on the current span.
pub fn record_response_id(span: &Span, response_id: &str) {
    span.record(semconv::GEN_AI_RESPONSE_ID, response_id);
}

/// Record tool execution status on a tool span.
pub fn record_tool_status(span: &Span, status: &str) {
    span.record(semconv::LIFE_TOOL_STATUS, status);
}

/// Emit a `gen_ai.content.prompt` span event recording the input messages.
///
/// Follows the OTel GenAI semantic conventions for content events.
/// Must be called within an entered span context (the chat span).
/// Only call when content capture is enabled (`VIGIL_CAPTURE_CONTENT=true`).
pub fn record_prompt_content(content: &str) {
    tracing::event!(
        name: "gen_ai.content.prompt",
        tracing::Level::INFO,
        "gen_ai.prompt" = content,
    );
}

/// Emit a `gen_ai.content.completion` span event recording the output content.
///
/// Follows the OTel GenAI semantic conventions for content events.
/// Must be called within an entered span context (the chat span).
/// Only call when content capture is enabled (`VIGIL_CAPTURE_CONTENT=true`).
pub fn record_completion_content(content: &str) {
    tracing::event!(
        name: "gen_ai.content.completion",
        tracing::Level::INFO,
        "gen_ai.completion" = content,
    );
}

/// Record reliability attributes on a chat span.
///
/// Emits retry count, fallback status, and circuit breaker state.
/// Call after a provider call completes (successful or not).
pub fn record_reliability(
    span: &Span,
    retry_count: u32,
    fallback_triggered: bool,
    circuit_state: &str,
) {
    span.record(semconv::LIFE_RETRY_COUNT, retry_count);
    span.record(semconv::LIFE_FALLBACK_TRIGGERED, fallback_triggered);
    span.record(semconv::LIFE_CIRCUIT_STATE, circuit_state);
}

/// Emit a `gen_ai.evaluation.result` span event with eval attributes.
///
/// Records the event on the current span. This follows the OTel GenAI
/// semantic conventions v1.39.0 for evaluation result events.
///
/// # Arguments
///
/// * `evaluator` — Name of the evaluator (e.g. `"token_efficiency"`)
/// * `score` — Normalized quality score in `[0.0, 1.0]`
/// * `label` — Categorical label (`"good"`, `"warning"`, `"critical"`)
/// * `layer` — Evaluation layer (`"reasoning"`, `"action"`, `"execution"`, `"safety"`, `"cost"`)
/// * `timing` — Evaluation timing (`"inline"`, `"async"`)
pub fn eval_event(evaluator: &str, score: f64, label: &str, layer: &str, timing: &str) {
    tracing::event!(
        name: "gen_ai.evaluation.result",
        tracing::Level::INFO,
        "life.eval.evaluator" = evaluator,
        "life.eval.score" = score,
        "life.eval.label" = label,
        "life.eval.layer" = layer,
        "life.eval.timing" = timing,
    );
}

/// Write the current trace context (trace_id, span_id) into an EventEnvelope.
///
/// This enables dual-write: events carry OTel correlation IDs so that
/// persisted events can be linked back to their traces.
pub fn write_trace_context(envelope: &mut EventEnvelope) {
    use opentelemetry::trace::TraceContextExt;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    let current_span = Span::current();
    let otel_context = current_span.context();
    let span_ref = otel_context.span();
    let span_context = span_ref.span_context();

    if span_context.is_valid() {
        envelope.trace_id = Some(span_context.trace_id().to_string());
        envelope.span_id = Some(span_context.span_id().to_string());
    }
}

/// Extract trace context from a persisted EventEnvelope.
///
/// Returns `(trace_id, span_id)` if both are present.
pub fn extract_trace_context(envelope: &EventEnvelope) -> Option<(String, String)> {
    match (&envelope.trace_id, &envelope.span_id) {
        (Some(trace_id), Some(span_id)) if !trace_id.is_empty() && !span_id.is_empty() => {
            Some((trace_id.clone(), span_id.clone()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::event::*;
    use std::collections::HashMap;

    /// Install a minimal subscriber so spans are not disabled during tests.
    /// Uses `try_init` to tolerate tests running in any order.
    fn ensure_subscriber() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    }

    fn test_envelope() -> EventEnvelope {
        EventEnvelope {
            event_id: "evt-1".into(),
            session_id: "sess-1".into(),
            agent_id: "agent-1".into(),
            branch_id: "main".into(),
            run_id: None,
            seq: 0,
            timestamp: 0,
            actor: EventActor {
                actor_type: ActorType::System,
                component: Some("test".to_string()),
            },
            schema: EventSchema {
                name: "aios-protocol".to_string(),
                version: "0.1.0".to_string(),
            },
            parent_id: None,
            trace_id: None,
            span_id: None,
            digest: None,
            kind: EventKind::SessionCreated {
                name: "test-session".to_string(),
                config: serde_json::Value::Object(serde_json::Map::new()),
            },
            metadata: HashMap::new(),
            schema_version: 1,
        }
    }

    #[test]
    fn agent_span_has_correct_name() {
        ensure_subscriber();
        let span = agent_span("sess-123", "test-agent");
        assert!(!span.is_disabled());
    }

    #[test]
    fn phase_span_all_variants() {
        ensure_subscriber();
        let phases = [
            LoopPhase::Perceive,
            LoopPhase::Deliberate,
            LoopPhase::Gate,
            LoopPhase::Execute,
            LoopPhase::Commit,
            LoopPhase::Reflect,
            LoopPhase::Sleep,
        ];
        for phase in phases {
            let span = phase_span(phase);
            assert!(!span.is_disabled());
        }
    }

    #[test]
    fn chat_span_creates_valid_span() {
        ensure_subscriber();
        let span = chat_span(
            "claude-sonnet-4-20250514",
            "anthropic",
            Some(4096),
            Some(0.7),
            "sess-chat-1",
        );
        assert!(!span.is_disabled());
    }

    #[test]
    fn tool_span_creates_valid_span() {
        ensure_subscriber();
        let span = tool_span("read_file", "call-abc123");
        assert!(!span.is_disabled());
    }

    #[test]
    fn extract_trace_context_returns_none_for_empty() {
        let envelope = test_envelope();
        assert!(extract_trace_context(&envelope).is_none());
    }

    #[test]
    fn extract_trace_context_returns_values() {
        let mut envelope = test_envelope();
        envelope.trace_id = Some("abc123".to_string());
        envelope.span_id = Some("def456".to_string());
        let ctx = extract_trace_context(&envelope);
        assert!(ctx.is_some());
        let (tid, sid) = ctx.unwrap();
        assert_eq!(tid, "abc123");
        assert_eq!(sid, "def456");
    }

    #[test]
    fn extract_trace_context_rejects_empty_strings() {
        let mut envelope = test_envelope();
        envelope.trace_id = Some(String::new());
        envelope.span_id = Some("def456".to_string());
        assert!(extract_trace_context(&envelope).is_none());
    }

    #[test]
    fn record_token_usage_does_not_panic() {
        ensure_subscriber();
        let span = chat_span("test-model", "test", None, None, "sess-usage");
        let usage = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };
        record_token_usage(&span, &usage);
    }

    #[test]
    fn record_finish_reason_does_not_panic() {
        ensure_subscriber();
        let span = chat_span("test-model", "test", None, None, "sess-finish");
        record_finish_reason(&span, "stop");
    }

    #[test]
    fn record_prompt_content_does_not_panic() {
        ensure_subscriber();
        let span = chat_span("test-model", "test", None, None, "sess-prompt");
        let _guard = span.enter();
        record_prompt_content("Hello, how are you?");
    }

    #[test]
    fn record_completion_content_does_not_panic() {
        ensure_subscriber();
        let span = chat_span("test-model", "test", None, None, "sess-completion");
        let _guard = span.enter();
        record_completion_content("I'm doing well, thanks!");
    }

    #[test]
    fn record_reliability_does_not_panic() {
        ensure_subscriber();
        let span = chat_span("test-model", "test", None, None, "sess-reliability");
        record_reliability(&span, 2, true, "half_open");
    }

    #[test]
    fn eval_event_does_not_panic() {
        ensure_subscriber();
        // Emit eval event within an active span context
        let span = agent_span("sess-eval", "test-agent");
        let _guard = span.enter();
        eval_event("token_efficiency", 0.85, "good", "execution", "inline");
    }

    #[test]
    fn eval_event_all_labels() {
        ensure_subscriber();
        let span = agent_span("sess-labels", "test-agent");
        let _guard = span.enter();
        eval_event("safety_compliance", 0.95, "good", "safety", "inline");
        eval_event("budget_adherence", 0.65, "warning", "cost", "inline");
        eval_event("tool_correctness", 0.3, "critical", "action", "async");
    }
}
