//! `VigilSink` — emits each [`StreamEvent`] as a structured `tracing`
//! event on the current span.
//!
//! When the host has initialised vigil's OTel subscriber (via
//! `life_vigil::init_telemetry`), these tracing events flow into vigil's
//! span pipeline and out to OTLP. When no subscriber is configured, the
//! events go to whatever fallback the host has set up (typically a
//! human-readable formatter for development).
//!
//! ## No vigil dep
//!
//! Despite the name, `VigilSink` does **not** import `life-vigil` —
//! it uses `tracing` directly. Vigil's role is to *configure* the
//! tracing subscriber; it doesn't sit on the emit path. This means
//! `VigilSink` works correctly in any deployment with any tracing
//! subscriber, and can be tested without spinning up vigil.
//!
//! The "Vigil" name is intentional: the sink emits events with the
//! field names and conventions vigil's pipeline expects. A future
//! ergon consumer with a different observability stack would write a
//! different sink and call it something else.
//!
//! ## Failure semantics
//!
//! `VigilSink::emit` is **infallible** (always returns `Ok(())`).
//! Tracing failures are not detectable from the emitter side, and
//! observability events should never block the autonomous loop.
//!
//! ## Field naming
//!
//! Each [`StreamEvent`] variant emits a tracing event with the variant
//! name as the "kind" field, plus all variant fields as additional
//! tracing fields. Consumers can filter / route by `kind` and read
//! the rich detail from the other fields.

use async_trait::async_trait;
use ergon::{Result, StreamEvent, StreamSink};

/// Tracing target for ergon stream events. Use this in subscriber
/// filters to route ergon events distinctly from other tracing output.
///
/// **Note**: this is a `pub const` (not configurable per-instance)
/// because `tracing` macros require `target:` to be a string literal
/// at the macro callsite. To filter under a different target, wrap a
/// `VigilSink` in your own `StreamSink` impl that re-emits with your
/// preferred target.
pub const ERGON_STREAM_TARGET: &str = "ergon::stream";

/// A [`StreamSink`] that forwards every [`StreamEvent`] as a structured
/// `tracing::info!` event on the current span, target
/// [`ERGON_STREAM_TARGET`].
#[derive(Debug, Default)]
pub struct VigilSink {
    _private: (),
}

impl VigilSink {
    /// Construct a sink emitting on [`ERGON_STREAM_TARGET`].
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl StreamSink for VigilSink {
    async fn emit(&self, event: StreamEvent) -> Result<()> {
        // Match on the event variant so subscribers can filter on `kind`
        // and read structured fields. We use `tracing::info!` (not
        // `debug!`) because stream events are operationally interesting
        // — they're the wire-shape of the agent's behaviour.
        match &event {
            StreamEvent::SessionStart {
                session_id,
                model,
                provider,
            } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "session_start",
                session_id = %session_id,
                model = %model,
                provider = %provider,
            ),
            StreamEvent::TextStart { id } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "text_start",
                id = %id,
            ),
            StreamEvent::TextDelta { id, delta } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "text_delta",
                id = %id,
                delta_len = delta.len(),
            ),
            StreamEvent::TextEnd { id } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "text_end",
                id = %id,
            ),
            StreamEvent::ReasoningStart { id, signed } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "reasoning_start",
                id = %id,
                signed = *signed,
            ),
            StreamEvent::ReasoningDelta { id, delta } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "reasoning_delta",
                id = %id,
                delta_len = delta.len(),
            ),
            StreamEvent::ReasoningEnd { id, redacted } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "reasoning_end",
                id = %id,
                redacted = *redacted,
            ),
            StreamEvent::ToolUseStart { id, name } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "tool_use_start",
                id = %id,
                name = %name,
            ),
            StreamEvent::ToolUseInputDelta { id, partial_args } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "tool_use_input_delta",
                id = %id,
                partial_args_len = partial_args.len(),
            ),
            StreamEvent::ToolUseEnd {
                id,
                ok,
                denied,
                error,
            } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "tool_use_end",
                id = %id,
                ok = *ok,
                denied = *denied,
                error = error.as_deref().unwrap_or(""),
            ),
            StreamEvent::StructuredStart { id, schema_name } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "structured_start",
                id = %id,
                schema_name = %schema_name,
            ),
            StreamEvent::StructuredDelta { id, partial_json } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "structured_delta",
                id = %id,
                partial_json_len = partial_json.len(),
            ),
            StreamEvent::StructuredEnd { id } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "structured_end",
                id = %id,
            ),
            StreamEvent::Citation {
                id,
                source_id,
                span,
            } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "citation",
                id = %id,
                source_id = %source_id,
                span_start = span.0,
                span_end = span.1,
            ),
            StreamEvent::Source { id, url, title } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "source",
                id = %id,
                url = url.as_deref().unwrap_or(""),
                title = title.as_deref().unwrap_or(""),
            ),
            StreamEvent::Usage {
                input,
                output,
                cached_input,
                reasoning,
            } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "usage",
                input_tokens = *input,
                output_tokens = *output,
                cached_input_tokens = cached_input.unwrap_or(0),
                reasoning_tokens = reasoning.unwrap_or(0),
            ),
            StreamEvent::Done { stop_reason } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "done",
                stop_reason = ?stop_reason,
            ),
            StreamEvent::Error { message } => tracing::warn!(
                target: ERGON_STREAM_TARGET,
                kind = "error",
                message = %message,
            ),
            StreamEvent::VendorEvent {
                vendor,
                kind,
                payload,
            } => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "vendor_event",
                vendor = %vendor,
                vendor_kind = %kind,
                payload_len = payload.to_string().len(),
            ),
            // Forward-compatible: future StreamEvent variants are not a
            // breaking change. We log a generic event noting the
            // unknown variant so observability still sees something.
            _ => tracing::info!(
                target: ERGON_STREAM_TARGET,
                kind = "unknown_variant",
                "ergon-life-sinks: VigilSink saw an unknown StreamEvent variant",
            ),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ergon::{StopReason, Usage};

    #[tokio::test]
    async fn emit_is_always_ok() {
        let sink = VigilSink::new();
        sink.emit(StreamEvent::Done {
            stop_reason: StopReason::EndTurn,
        })
        .await
        .expect("infallible");
        sink.emit(StreamEvent::Error {
            message: "oops".into(),
        })
        .await
        .expect("infallible");
        sink.emit(StreamEvent::Usage {
            input: 100,
            output: 200,
            cached_input: Some(50),
            reasoning: None,
        })
        .await
        .expect("infallible");
    }

    #[tokio::test]
    async fn emits_for_every_basic_variant() {
        let sink = VigilSink::new();
        let events = vec![
            StreamEvent::SessionStart {
                session_id: ergon::SessionId::from_string("s"),
                model: "m".into(),
                provider: "p".into(),
            },
            StreamEvent::TextStart { id: "t".into() },
            StreamEvent::TextDelta {
                id: "t".into(),
                delta: "hi".into(),
            },
            StreamEvent::TextEnd { id: "t".into() },
            StreamEvent::ReasoningStart {
                id: "r".into(),
                signed: false,
            },
            StreamEvent::ReasoningDelta {
                id: "r".into(),
                delta: "thinking...".into(),
            },
            StreamEvent::ReasoningEnd {
                id: "r".into(),
                redacted: true,
            },
            StreamEvent::ToolUseStart {
                id: "tu".into(),
                name: "fs_read".into(),
            },
            StreamEvent::ToolUseInputDelta {
                id: "tu".into(),
                partial_args: "{}".into(),
            },
            StreamEvent::ToolUseEnd {
                id: "tu".into(),
                ok: true,
                denied: false,
                error: None,
            },
            StreamEvent::StructuredStart {
                id: "st".into(),
                schema_name: "Verdict".into(),
            },
            StreamEvent::StructuredDelta {
                id: "st".into(),
                partial_json: "{}".into(),
            },
            StreamEvent::StructuredEnd { id: "st".into() },
            StreamEvent::Citation {
                id: "c1".into(),
                source_id: "s1".into(),
                span: (0, 10),
            },
            StreamEvent::Source {
                id: "s1".into(),
                url: Some("https://example.com".into()),
                title: Some("Example".into()),
            },
            StreamEvent::Usage {
                input: 100,
                output: 200,
                cached_input: None,
                reasoning: Some(50),
            },
            StreamEvent::Done {
                stop_reason: StopReason::EndTurn,
            },
            StreamEvent::VendorEvent {
                vendor: "anthropic".into(),
                kind: "message_start".into(),
                payload: serde_json::json!({"id": "m_1"}),
            },
        ];
        for event in events {
            sink.emit(event).await.expect("infallible");
        }
    }

    #[test]
    fn target_constant_is_stable() {
        assert_eq!(ERGON_STREAM_TARGET, "ergon::stream");
    }

    // Compile-time check: Usage struct can be referenced (it lives in
    // ergon::model::Usage and is needed by our Usage variant emission).
    #[allow(dead_code)]
    fn _usage_compiles() {
        let _ = Usage::default();
    }
}
