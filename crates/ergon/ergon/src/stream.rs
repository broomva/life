//! Canonical stream-event taxonomy and sink trait.
//!
//! [`StreamEvent`] is ergon's wire-shape contract for what flows out of the
//! autonomous loop. Every observable signal from a model + tool run lands as
//! one of these variants. The taxonomy is **append-only after v1.0** —
//! never removed, never reordered semantically.
//!
//! [`StreamSink`] is the consumer side: anything that wants to observe a
//! workflow run (durable replay, OTel traces, end-user SSE) implements it.
//!
//! ## Default sinks landing in this module
//!
//! - [`BufferSink`] — accumulates every event in memory; for tests.
//! - [`FanoutSink`] — broadcasts to N child sinks.
//!
//! ## Default sinks **deferred** to follow-up PRs (substrate integration)
//!
//! - `LagoSink` — durable replay via `lago_journal::Journal`.
//! - `VigilSink` — OTel spans via `life_vigil::Tracer`.
//! - `LifegwSink` — bounded mpsc towards lifegw upstream stream.
//!
//! Those three pull in Life-substrate crate dependencies that this PR
//! intentionally keeps out of ergon's foundational layer.

use crate::SessionId;
use crate::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Reason a stream terminated. Mirrors the upstream-provider taxonomy
/// (Anthropic/OpenAI/Bedrock all converge on these four high-level cases).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Model finished its turn voluntarily.
    EndTurn,
    /// Provider hit the configured max-tokens / max-output cap.
    MaxTokens,
    /// Model emitted a `tool_use` block; outer loop should dispatch and
    /// re-enter `run_inference_streaming`.
    ToolUse,
    /// Provider matched a stop sequence supplied in the request.
    StopSequence,
    /// Refusal / safety termination (vendor-classified).
    Refusal,
    /// Provider error or unexpected termination.
    Error,
}

/// Canonical event taxonomy emitted from the autonomous loop.
///
/// **Stability contract**: variants are append-only after v1.0. New variants
/// may land in any minor version; existing variants are never removed and
/// never have their semantics altered. Consumers MUST handle the
/// [`Self::VendorEvent`] variant to gracefully accept future or
/// vendor-specific events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Session bookkeeping — emitted once at the start of every workflow run.
    SessionStart {
        session_id: SessionId,
        model: String,
        provider: String,
    },

    /// Begin of a top-level text block.
    TextStart { id: String },
    /// Streamed text token / chunk.
    TextDelta { id: String, delta: String },
    /// End of the text block whose `id` matches.
    TextEnd { id: String },

    /// Begin of an extended-thinking / reasoning block.
    ReasoningStart { id: String, signed: bool },
    /// Streamed reasoning chunk.
    ReasoningDelta { id: String, delta: String },
    /// End of the reasoning block.
    ReasoningEnd { id: String, redacted: bool },

    /// Model has decided to invoke a tool.
    ToolUseStart { id: String, name: String },
    /// Streamed JSON-encoded args for an in-progress tool invocation.
    ToolUseInputDelta { id: String, partial_args: String },
    /// End of a tool invocation. `denied` is true when a hook rejected the
    /// call (capability gate, approval flow). `error` carries the message
    /// when the underlying tool failed.
    ToolUseEnd {
        id: String,
        ok: bool,
        denied: bool,
        error: Option<String>,
    },

    /// Begin of a structured-output block (e.g. JSON-schema constrained
    /// generation).
    StructuredStart { id: String, schema_name: String },
    /// Streamed partial JSON for a structured-output block.
    StructuredDelta { id: String, partial_json: String },
    /// End of a structured-output block.
    StructuredEnd { id: String },

    /// A citation reference into a previously streamed source.
    Citation {
        id: String,
        source_id: String,
        span: (usize, usize),
    },
    /// A source reference (URL / title) attached to the response.
    Source {
        id: String,
        url: Option<String>,
        title: Option<String>,
    },

    /// Token / cost accounting for the upstream call.
    Usage {
        input: u32,
        output: u32,
        cached_input: Option<u32>,
        reasoning: Option<u32>,
    },

    /// Stream terminated.
    Done { stop_reason: StopReason },

    /// Stream surfaced a recoverable error (the loop may retry).
    Error { message: String },

    /// Vendor-specific or future event ergon doesn't yet understand. Sinks
    /// MUST accept and forward this variant unchanged.
    VendorEvent {
        vendor: String,
        kind: String,
        payload: serde_json::Value,
    },
}

/// Consumer of a [`StreamEvent`] flow.
///
/// Implementations receive every event from the autonomous loop in
/// emission order. Sinks may persist (Lago), trace (Vigil), forward to a
/// client (Lifegw), or fan out to multiple downstream sinks.
///
/// ## Backpressure
///
/// Sinks that buffer (mpsc-style) propagate backpressure by awaiting on
/// their internal channel. The autonomous loop awaits each `emit` call
/// in turn, so a slow sink throttles the upstream provider naturally.
#[async_trait]
pub trait StreamSink: Send + Sync {
    /// Forward a single stream event. Returns an error if the sink can no
    /// longer accept events (e.g. the consumer has disconnected).
    async fn emit(&self, event: StreamEvent) -> Result<()>;
}

/// In-memory sink that captures every event in emission order. Intended
/// for unit tests and integration assertions.
#[derive(Debug, Default)]
pub struct BufferSink {
    events: Mutex<Vec<StreamEvent>>,
}

impl BufferSink {
    /// Create a new empty buffer sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot the current event buffer, leaving the sink usable.
    pub async fn snapshot(&self) -> Vec<StreamEvent> {
        self.events.lock().await.clone()
    }

    /// Drain and return all events captured so far.
    pub async fn drain(&self) -> Vec<StreamEvent> {
        std::mem::take(&mut *self.events.lock().await)
    }

    /// Number of events captured so far.
    pub async fn len(&self) -> usize {
        self.events.lock().await.len()
    }

    /// True iff [`Self::len`] is zero.
    pub async fn is_empty(&self) -> bool {
        self.events.lock().await.is_empty()
    }
}

#[async_trait]
impl StreamSink for BufferSink {
    async fn emit(&self, event: StreamEvent) -> Result<()> {
        self.events.lock().await.push(event);
        Ok(())
    }
}

/// Broadcast a stream to N child sinks. Each `emit` call awaits every
/// child sequentially in registration order. If any child returns an
/// error, [`FanoutSink::emit`] short-circuits and returns that error;
/// remaining children for the current event are skipped (they will see
/// subsequent events normally).
///
/// This sequential semantics is intentional: it preserves backpressure
/// (a slow durable sink throttles the loop) and yields deterministic
/// failure ordering.
pub struct FanoutSink {
    sinks: Vec<Arc<dyn StreamSink>>,
}

impl FanoutSink {
    /// Build a fanout from an explicit list of sinks.
    pub fn new(sinks: Vec<Arc<dyn StreamSink>>) -> Self {
        Self { sinks }
    }

    /// Number of registered child sinks.
    pub fn len(&self) -> usize {
        self.sinks.len()
    }

    /// True iff there are no registered child sinks.
    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

impl std::fmt::Debug for FanoutSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FanoutSink")
            .field("sinks", &self.sinks.len())
            .finish()
    }
}

#[async_trait]
impl StreamSink for FanoutSink {
    async fn emit(&self, event: StreamEvent) -> Result<()> {
        for sink in &self.sinks {
            sink.emit(event.clone()).await?;
        }
        Ok(())
    }
}

/// A sink that always fails — used to assert error-propagation semantics
/// from [`FanoutSink`] in tests.
#[doc(hidden)]
#[cfg(test)]
struct FailingSink {
    msg: String,
}

#[cfg(test)]
#[async_trait]
impl StreamSink for FailingSink {
    async fn emit(&self, _event: StreamEvent) -> Result<()> {
        Err(crate::ErgonError::Internal(self.msg.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_text_delta(id: &str, delta: &str) -> StreamEvent {
        StreamEvent::TextDelta {
            id: id.to_string(),
            delta: delta.to_string(),
        }
    }

    #[tokio::test]
    async fn buffer_sink_captures_events_in_order() {
        let sink = BufferSink::new();
        sink.emit(sample_text_delta("t1", "hello "))
            .await
            .expect("emit ok");
        sink.emit(sample_text_delta("t1", "world"))
            .await
            .expect("emit ok");
        assert_eq!(sink.len().await, 2);
        let events = sink.snapshot().await;
        match (&events[0], &events[1]) {
            (StreamEvent::TextDelta { delta: a, .. }, StreamEvent::TextDelta { delta: b, .. }) => {
                assert_eq!(a, "hello ");
                assert_eq!(b, "world");
            }
            _ => panic!("unexpected event shape"),
        }
    }

    #[tokio::test]
    async fn buffer_sink_drain_clears_state() {
        let sink = BufferSink::new();
        sink.emit(sample_text_delta("t1", "x"))
            .await
            .expect("emit ok");
        let drained = sink.drain().await;
        assert_eq!(drained.len(), 1);
        assert!(sink.is_empty().await);
    }

    #[tokio::test]
    async fn fanout_emits_to_all_children_in_order() {
        let a = Arc::new(BufferSink::new());
        let b = Arc::new(BufferSink::new());
        let fanout = FanoutSink::new(vec![a.clone(), b.clone()]);
        assert_eq!(fanout.len(), 2);
        assert!(!fanout.is_empty());

        fanout
            .emit(sample_text_delta("t1", "alpha"))
            .await
            .expect("emit ok");
        fanout
            .emit(sample_text_delta("t1", "beta"))
            .await
            .expect("emit ok");

        assert_eq!(a.len().await, 2);
        assert_eq!(b.len().await, 2);
    }

    #[tokio::test]
    async fn fanout_short_circuits_on_first_error() {
        let good = Arc::new(BufferSink::new());
        let failing: Arc<dyn StreamSink> = Arc::new(FailingSink {
            msg: "boom".to_string(),
        });
        let fanout = FanoutSink::new(vec![good.clone(), failing]);

        let err = fanout
            .emit(sample_text_delta("t1", "x"))
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("boom"));
        // good sink received the event before the failing sink errored
        assert_eq!(good.len().await, 1);
    }

    #[tokio::test]
    async fn empty_fanout_is_a_no_op() {
        let fanout = FanoutSink::new(vec![]);
        assert!(fanout.is_empty());
        fanout
            .emit(sample_text_delta("t1", "x"))
            .await
            .expect("empty fanout emits ok");
    }

    #[test]
    fn stream_event_round_trips_through_json() {
        // Snapshot the canonical JSON shape for a representative variant
        // — this catches accidental rename / serde-rule changes.
        let evt = StreamEvent::TextDelta {
            id: "t1".into(),
            delta: "hi".into(),
        };
        let json = serde_json::to_string(&evt).expect("serializable");
        assert!(json.contains("\"event\":\"text_delta\""));
        let back: StreamEvent = serde_json::from_str(&json).expect("deserializable");
        match back {
            StreamEvent::TextDelta { id, delta } => {
                assert_eq!(id, "t1");
                assert_eq!(delta, "hi");
            }
            _ => panic!("variant mismatch"),
        }
    }

    #[test]
    fn done_serializes_with_stop_reason() {
        let evt = StreamEvent::Done {
            stop_reason: StopReason::EndTurn,
        };
        let json = serde_json::to_string(&evt).expect("serializable");
        assert!(json.contains("\"event\":\"done\""));
        assert!(json.contains("end_turn"));
    }

    #[test]
    fn vendor_event_round_trips_with_payload() {
        let evt = StreamEvent::VendorEvent {
            vendor: "anthropic".into(),
            kind: "message_start".into(),
            payload: serde_json::json!({ "id": "m_1" }),
        };
        let json = serde_json::to_string(&evt).expect("serializable");
        let back: StreamEvent = serde_json::from_str(&json).expect("deserializable");
        match back {
            StreamEvent::VendorEvent {
                vendor,
                kind,
                payload,
            } => {
                assert_eq!(vendor, "anthropic");
                assert_eq!(kind, "message_start");
                assert_eq!(payload["id"], "m_1");
            }
            _ => panic!("variant mismatch"),
        }
    }

    #[test]
    fn stop_reason_variants_are_distinguishable() {
        let reasons = [
            StopReason::EndTurn,
            StopReason::MaxTokens,
            StopReason::ToolUse,
            StopReason::StopSequence,
            StopReason::Refusal,
            StopReason::Error,
        ];
        for r in reasons {
            let json = serde_json::to_string(&r).expect("serializable");
            let back: StopReason = serde_json::from_str(&json).expect("deserializable");
            assert_eq!(r, back);
        }
    }
}
