//! `pb::AgentEvent` → Anthropic Messages SSE translation.
//!
//! Ports the role of `core/anthropic/sse.py::SSEBuilder` from
//! free-claude-code. The encoder owns the response-life state machine
//! and turns each [`life_runtime_proto::life::v1::AgentEvent`] into
//! zero or more [`AnthropicSseEvent`]s ready to write to the HTTP
//! response body.
//!
//! ## What the encoder is
//!
//! * **State machine over `pb::AgentEvent`.** Composes
//!   [`crate::BlockPolicyState`] (downstream block-index allocation),
//!   [`crate::EmittedTracker`] (replay de-dup), [`crate::ThinkingState`]
//!   (thinking lifecycle), and per-id [`crate::ToolUseState`]
//!   (tool_use blocks).
//! * **Pure.** Owns no I/O, opens no sockets, does no networking. Each
//!   `encode(...)` call is a deterministic function of `(self, event)`.
//! * **Frame producer.** Each output [`AnthropicSseEvent`] knows its
//!   own SSE-wire string via [`AnthropicSseEvent::to_sse_frame`].
//!
//! ## What the encoder is not
//!
//! * It does NOT speak the upstream Anthropic-SSE wire. That's
//!   `arcan-proxy::AnthropicArcan`'s job. By the time we receive a
//!   `pb::AgentEvent`, the upstream Anthropic SSE has already been
//!   parsed into a normalized form.
//! * It does NOT verify auth, mint capability tokens, or invoke
//!   lifed — that's `services/anthropic_messages.rs` in lifegw
//!   (J-Sub-B).
//!
//! ## Variant coverage caveat
//!
//! `life.v1.AgentEventKind` currently exposes: `Token`,
//! `ToolCallPending`, `ToolResult`, `ApprovalRequired`, `Finish`,
//! `Error`, `Hibernate`. There is *no* dedicated `Thinking` variant
//! and *no* `ToolCallEmit` variant in the proto today. Per Spec J
//! §[Sub-phase decomposition] risk table, that's tracked as a
//! precursor for J-Sub-D (tool-use bridge). This encoder consumes the
//! variants that *do* exist and treats `ToolCallPending` as the
//! tool_use-start signal; the missing variants graduate to first-class
//! handling once the proto bump lands.

use serde::{Deserialize, Serialize};

use crate::block_policy::{BlockKind, BlockPolicyState};
use crate::errors::{AnthropicError, AnthropicErrorKind, CodecError};
use crate::state::{EmittedKey, EmittedTracker};
use crate::thinking::ThinkingState;
use crate::tools::ToolUseState;

use life_runtime_proto::life::v1 as pb;
use std::collections::HashMap;

// ─── Wire shapes ────────────────────────────────────────────────────

/// One Anthropic Messages SSE event ready to write to the HTTP body.
///
/// Every variant maps to one logical SSE frame:
///
/// ```text
/// event: <type>\n
/// data: <json>\n
/// \n
/// ```
///
/// Build the wire bytes via [`AnthropicSseEvent::to_sse_frame`].
#[derive(Clone, Debug, PartialEq)]
pub enum AnthropicSseEvent {
    /// `event: message_start` — the start-of-response marker.
    MessageStart(MessageStartPayload),
    /// `event: content_block_start` — opens a content block.
    ContentBlockStart(ContentBlockStartPayload),
    /// `event: content_block_delta` — streams content into a block.
    ContentBlockDelta(ContentBlockDeltaPayload),
    /// `event: content_block_stop` — closes a content block.
    ContentBlockStop(ContentBlockStopPayload),
    /// `event: message_delta` — final usage + stop_reason.
    MessageDelta(MessageDeltaPayload),
    /// `event: message_stop` — end-of-response marker.
    MessageStop,
    /// `event: ping` — keepalive heartbeat for HTTP-idle proxies.
    Ping,
    /// `event: error` — in-stream error (HTTP 200 stream stays open).
    Error(AnthropicError),
}

impl AnthropicSseEvent {
    /// Format this event as a full SSE frame ending in `\n\n`.
    pub fn to_sse_frame(&self) -> String {
        match self {
            Self::MessageStart(p) => sse_frame("message_start", p),
            Self::ContentBlockStart(p) => sse_frame("content_block_start", p),
            Self::ContentBlockDelta(p) => sse_frame("content_block_delta", p),
            Self::ContentBlockStop(p) => sse_frame("content_block_stop", p),
            Self::MessageDelta(p) => sse_frame("message_delta", p),
            Self::MessageStop => {
                // Anthropic's protocol still requires the `data:` line
                // with a payload, even for the otherwise-bodyless
                // `message_stop` event.
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_string()
            }
            Self::Ping => "event: ping\ndata: {\"type\":\"ping\"}\n\n".to_string(),
            Self::Error(e) => e.to_sse_frame(),
        }
    }
}

fn sse_frame<T: Serialize>(event_name: &str, payload: &T) -> String {
    // Serialization is infallible for our payloads — all-`String`/`u32`
    // structs. Falling back to `{}` keeps the stream healthy on the
    // theoretical impossible path.
    let body = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    format!("event: {event_name}\ndata: {body}\n\n")
}

/// `message_start` payload — mirrors Anthropic's published shape.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MessageStartPayload {
    /// Always `"message_start"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Inner message envelope.
    pub message: MessageEnvelope,
}

/// The `message` sub-object inside `message_start`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MessageEnvelope {
    /// Anthropic-format message id (`msg_01...`). Synthesized at edge.
    pub id: String,
    /// Always the literal string `"message"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Always `"assistant"`.
    pub role: String,
    /// Always `[]` at message_start — content streams as separate events.
    pub content: Vec<serde_json::Value>,
    /// Model identifier echo (matches the inbound request).
    pub model: String,
    /// `null` at start; populated in `message_delta`.
    pub stop_reason: Option<String>,
    /// `null` at start.
    pub stop_sequence: Option<String>,
    /// Token usage. `output_tokens` is `1` at start (Anthropic
    /// convention — there's always at least one beat of usage even
    /// for empty responses).
    pub usage: Usage,
}

/// Anthropic usage counters.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Usage {
    /// Prompt tokens.
    pub input_tokens: u64,
    /// Completion tokens.
    pub output_tokens: u64,
    /// Cached prompt tokens that hit the cache (Anthropic prompt caching).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    /// Cached prompt tokens served from cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
}

/// `content_block_start` payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContentBlockStartPayload {
    /// Always `"content_block_start"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Downstream content-block index.
    pub index: u32,
    /// The block's initial content shape.
    pub content_block: ContentBlockInit,
}

/// `content_block` sub-object inside `content_block_start`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockInit {
    /// Text block, starts empty.
    Text {
        /// Always `""`.
        text: String,
    },
    /// Thinking block, starts empty.
    Thinking {
        /// Always `""`.
        thinking: String,
    },
    /// Tool-use block — id + name known up-front, input streams as deltas.
    ToolUse {
        /// `toolu_...` id.
        id: String,
        /// Tool name.
        name: String,
        /// Initial input object — always `{}` at start; full JSON
        /// arrives via `input_json_delta` events.
        input: serde_json::Value,
    },
}

/// `content_block_delta` payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContentBlockDeltaPayload {
    /// Always `"content_block_delta"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Block index this delta belongs to.
    pub index: u32,
    /// The delta payload — type-tagged.
    pub delta: BlockDelta,
}

/// Type-tagged content-block delta.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockDelta {
    /// Streamed text fragment.
    TextDelta {
        /// Text fragment to append.
        text: String,
    },
    /// Streamed thinking fragment.
    ThinkingDelta {
        /// Thinking fragment to append.
        thinking: String,
    },
    /// Streamed thinking signature.
    SignatureDelta {
        /// Signature fragment.
        signature: String,
    },
    /// Streamed tool-input-JSON fragment.
    InputJsonDelta {
        /// JSON fragment to append.
        partial_json: String,
    },
}

/// `content_block_stop` payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContentBlockStopPayload {
    /// Always `"content_block_stop"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Block index being closed.
    pub index: u32,
}

/// `message_delta` payload — final usage + stop_reason.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MessageDeltaPayload {
    /// Always `"message_delta"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Stop-reason + stop-sequence updates.
    pub delta: MessageDeltaInner,
    /// Final token-usage counters.
    pub usage: Usage,
}

/// Inner `delta: {}` block of `message_delta`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MessageDeltaInner {
    /// e.g. `"end_turn"`, `"max_tokens"`, `"tool_use"`, `"stop_sequence"`.
    pub stop_reason: String,
    /// Stop sequence that fired, if any.
    pub stop_sequence: Option<String>,
}

// ─── Encoder ────────────────────────────────────────────────────────

/// In-memory state of an in-flight Anthropic-SSE encoder.
///
/// One instance per HTTP response. Combines all the sub-state into a
/// single owner so the encoder reasoning stays local.
#[derive(Clone, Debug)]
pub struct EncoderState {
    /// Anthropic-format `id` for this message (e.g. `msg_01abc`).
    pub message_id: String,
    /// Model identifier echo (matches the inbound request).
    pub model: String,
    /// Whether `message_start` has been emitted.
    pub started: bool,
    /// Whether `message_stop` has been emitted (terminal).
    pub finished: bool,
    /// Aggregated token usage so far.
    pub usage: Usage,
    /// In-flight content-block bookkeeping.
    pub blocks: BlockPolicyState,
    /// Per-response replay de-dup tracker.
    pub tracker: EmittedTracker,
    /// Thinking-block state.
    pub thinking: ThinkingState,
    /// Open tool_use blocks, keyed by Anthropic tool_use id.
    pub tool_states: HashMap<String, ToolUseState>,
    /// Stop reason chosen on `message_delta` emission. `None` until
    /// finalization runs.
    pub stop_reason: Option<String>,
}

impl EncoderState {
    /// Build fresh encoder state for a new response.
    pub fn new(message_id: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            message_id: message_id.into(),
            model: model.into(),
            started: false,
            finished: false,
            usage: Usage::default(),
            blocks: BlockPolicyState::new(),
            tracker: EmittedTracker::new(),
            thinking: ThinkingState::default(),
            tool_states: HashMap::new(),
            stop_reason: None,
        }
    }
}

/// The encoder.
///
/// Construct via [`Encoder::new`] (initial state) or
/// [`Encoder::resume`] (seed from an existing `EncoderState` — used
/// when replaying a partially-emitted response after a client
/// reconnect).
#[derive(Clone, Debug)]
pub struct Encoder {
    state: EncoderState,
}

impl Encoder {
    /// Build a fresh encoder for `(message_id, model)`.
    pub fn new(message_id: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            state: EncoderState::new(message_id, model),
        }
    }

    /// Resume from a saved snapshot. Used on Claude Code reconnect:
    /// the previous EncoderState is read back from the lago tail (or
    /// reconstructed from the per-session journal) so the resumed
    /// response continues with the same block indices.
    pub const fn resume(state: EncoderState) -> Self {
        Self { state }
    }

    /// Borrow the underlying state. Useful for snapshotting before
    /// replying to a reconnect.
    pub const fn state(&self) -> &EncoderState {
        &self.state
    }

    /// Set the input-tokens count on the `usage` block. Called by the
    /// handler once the upstream `input_tokens` is known (often the
    /// first thing the upstream tells us). Has no effect once
    /// `message_start` has been emitted.
    pub fn set_input_tokens(&mut self, n: u64) {
        if !self.state.started {
            self.state.usage.input_tokens = n;
        }
    }

    /// Emit `message_start` if it hasn't been already. Returns the
    /// event (or `None` if already emitted).
    pub fn emit_message_start(&mut self) -> Option<AnthropicSseEvent> {
        if self.state.started {
            return None;
        }
        self.state.started = true;
        let payload = MessageStartPayload {
            kind: "message_start".to_string(),
            message: MessageEnvelope {
                id: self.state.message_id.clone(),
                kind: "message".to_string(),
                role: "assistant".to_string(),
                content: Vec::new(),
                model: self.state.model.clone(),
                stop_reason: None,
                stop_sequence: None,
                usage: Usage {
                    input_tokens: self.state.usage.input_tokens,
                    // Anthropic emits `output_tokens: 1` even at
                    // start — at least one "beat" of usage so
                    // clients don't have to special-case empty.
                    output_tokens: 1,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
            },
        };
        Some(AnthropicSseEvent::MessageStart(payload))
    }

    /// Translate one upstream `pb::AgentEvent` into a list of
    /// Anthropic SSE events. Returns an empty list when the upstream
    /// event was suppressed (de-dup hit, redacted thinking, etc.).
    pub fn encode(&mut self, evt: &pb::AgentEvent) -> Result<Vec<AnthropicSseEvent>, CodecError> {
        if self.state.finished {
            // Already emitted message_stop — anything after is dropped.
            return Ok(Vec::new());
        }

        // De-dup against replay.
        let seq = evt.record.as_ref().map_or(0, |r| r.sequence);
        let key = EmittedKey {
            kind: evt.kind,
            sequence: seq,
        };
        if self.state.tracker.already_emitted(key) {
            return Ok(Vec::new());
        }

        let mut out: Vec<AnthropicSseEvent> = Vec::new();
        if let Some(start) = self.emit_message_start() {
            out.push(start);
        }

        use pb::AgentEventKind as K;
        match K::try_from(evt.kind).unwrap_or(K::Unspecified) {
            K::Unspecified => {
                // Drop — neither side should be generating these.
            }
            K::Token => {
                let payload_obj: serde_json::Value = decode_payload(evt);
                // Most upstreams emit `{"text":"..."}`. Some emit
                // `{"thinking":"..."}` as a side-channel signal in the
                // absence of a dedicated proto variant (see crate-level
                // caveat).
                if let Some(text) = payload_obj
                    .get("text")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    self.encode_text_token(text, &mut out);
                } else if let Some(thinking) = payload_obj
                    .get("thinking")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    self.encode_thinking_token(thinking, &mut out);
                } else if let Some(sig) = payload_obj
                    .get("signature")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    self.encode_thinking_signature(sig, &mut out);
                }
                // Token usage increments per char/4 approximation in
                // the absence of an upstream count — only when the
                // upstream sends explicit usage do we trust it. We
                // do NOT estimate here; Spec J L10-D7 keeps tokens
                // a Vigil/Haima surface, not a codec surface.
            }
            K::ToolCallPending => {
                // Treated as tool_use-start. The proto today doesn't
                // distinguish "I'm about to call a tool, here's the
                // id" (ToolCallEmit) from "I'm waiting on the user to
                // execute the tool" (ToolCallPending). Until that
                // splits in J-Sub-D's proto bump, we treat them as a
                // single signal: open the tool_use block now.
                let payload_obj: serde_json::Value = decode_payload(evt);
                self.encode_tool_call_pending(&payload_obj, &mut out)?;
            }
            K::ToolResult => {
                // Tool_result mid-stream means the upstream is
                // re-injecting after a Spec E §6.5 ToolAwait close.
                // Anthropic protocol places tool_result on the *next*
                // request, not in the response, so we drop these
                // events at the encoder. The handler (J-Sub-D) carries
                // the lifecycle.
            }
            K::ApprovalRequired => {
                // Approval gates aren't visible in Anthropic's wire
                // protocol — Claude Code has no UX for them. Drop;
                // approval flow rides on a separate Life route.
            }
            K::Finish => {
                // Finish closes the response cleanly.
                let payload_obj: serde_json::Value = decode_payload(evt);
                let stop_reason = payload_obj
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(map_stop_reason)
                    .unwrap_or_else(|| "end_turn".to_string());
                self.finalize(stop_reason, &mut out);
            }
            K::Error => {
                // Mid-stream upstream error → emit in-stream
                // `event: error` and close.
                let payload_obj: serde_json::Value = decode_payload(evt);
                let message = payload_obj
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("upstream error")
                    .to_string();
                let kind = payload_obj
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .map(classify_error_kind)
                    .unwrap_or(AnthropicErrorKind::ApiError);
                self.emit_error(kind, message, &mut out);
                // Treat as terminal — close all blocks + message_stop.
                self.finalize("end_turn".to_string(), &mut out);
            }
            K::Hibernate => {
                // Hibernate is a Life-internal lifecycle signal — not
                // visible to Anthropic clients. The handler closes
                // the HTTP stream cleanly.
                self.finalize("end_turn".to_string(), &mut out);
            }
        }

        // Mark this upstream event as emitted only after we've
        // produced output for it. Replay-resume re-reads the
        // EncoderState first and re-injects the prior events; without
        // recording here, replay would re-emit the same events
        // forever.
        self.state.tracker.record(key);
        Ok(out)
    }

    /// Emit a ping heartbeat. Caller decides the cadence.
    pub fn ping() -> AnthropicSseEvent {
        AnthropicSseEvent::Ping
    }

    /// Emit a top-level Anthropic `event: error` event without
    /// finalizing the stream. Returns the event so the caller can
    /// schedule write + close.
    pub fn emit_top_level_error(
        kind: AnthropicErrorKind,
        message: impl Into<String>,
    ) -> AnthropicSseEvent {
        AnthropicSseEvent::Error(AnthropicError::new(kind, message))
    }

    /// Forcibly finalize the response (closes any open blocks + emits
    /// `message_delta` with `end_turn` + `message_stop`). Idempotent.
    pub fn force_finalize(&mut self) -> Vec<AnthropicSseEvent> {
        let mut out = Vec::new();
        if !self.state.finished {
            self.finalize("end_turn".to_string(), &mut out);
        }
        out
    }

    // ─── private helpers ───

    fn encode_text_token(&mut self, text: &str, out: &mut Vec<AnthropicSseEvent>) {
        let transition = self
            .state
            .blocks
            .enter_block(BlockKind::Text, &mut self.state.tracker);
        if let Some(prev) = transition.close_previous {
            // Closing of a different singular kind (e.g. thinking)
            // also clears the thinking state machine.
            if self.state.thinking.is_open() {
                self.state.thinking.close();
            }
            out.push(AnthropicSseEvent::ContentBlockStop(
                ContentBlockStopPayload {
                    kind: "content_block_stop".to_string(),
                    index: prev,
                },
            ));
        }
        if transition.opened_new {
            out.push(AnthropicSseEvent::ContentBlockStart(
                ContentBlockStartPayload {
                    kind: "content_block_start".to_string(),
                    index: transition.open_index,
                    content_block: ContentBlockInit::Text {
                        text: String::new(),
                    },
                },
            ));
        }
        out.push(AnthropicSseEvent::ContentBlockDelta(
            ContentBlockDeltaPayload {
                kind: "content_block_delta".to_string(),
                index: transition.open_index,
                delta: BlockDelta::TextDelta {
                    text: text.to_string(),
                },
            },
        ));
    }

    fn encode_thinking_token(&mut self, thinking: &str, out: &mut Vec<AnthropicSseEvent>) {
        let transition = self
            .state
            .blocks
            .enter_block(BlockKind::Thinking, &mut self.state.tracker);
        if let Some(prev) = transition.close_previous {
            out.push(AnthropicSseEvent::ContentBlockStop(
                ContentBlockStopPayload {
                    kind: "content_block_stop".to_string(),
                    index: prev,
                },
            ));
        }
        if transition.opened_new {
            self.state.thinking.open(transition.open_index);
            out.push(AnthropicSseEvent::ContentBlockStart(
                ContentBlockStartPayload {
                    kind: "content_block_start".to_string(),
                    index: transition.open_index,
                    content_block: ContentBlockInit::Thinking {
                        thinking: String::new(),
                    },
                },
            ));
        }
        out.push(AnthropicSseEvent::ContentBlockDelta(
            ContentBlockDeltaPayload {
                kind: "content_block_delta".to_string(),
                index: transition.open_index,
                delta: BlockDelta::ThinkingDelta {
                    thinking: thinking.to_string(),
                },
            },
        ));
    }

    fn encode_thinking_signature(&mut self, sig: &str, out: &mut Vec<AnthropicSseEvent>) {
        if !self.state.thinking.is_open() {
            // Signatures without an open thinking block are
            // structurally invalid — drop silently. Upstream is
            // misbehaving; we'd rather not break the stream.
            return;
        }
        out.push(AnthropicSseEvent::ContentBlockDelta(
            ContentBlockDeltaPayload {
                kind: "content_block_delta".to_string(),
                index: self.state.thinking.block_index(),
                delta: BlockDelta::SignatureDelta {
                    signature: sig.to_string(),
                },
            },
        ));
    }

    fn encode_tool_call_pending(
        &mut self,
        payload: &serde_json::Value,
        out: &mut Vec<AnthropicSseEvent>,
    ) -> Result<(), CodecError> {
        // Required fields: id, name. Optional: partial_json,
        // complete (a fully-known input object).
        let id = payload
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CodecError::Upstream("tool_call_pending missing id".into()))?;
        let name = payload
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CodecError::Upstream("tool_call_pending missing name".into()))?;

        let transition = self
            .state
            .blocks
            .enter_tool_use(id, &mut self.state.tracker);
        if let Some(prev) = transition.close_singular {
            // Singular block stayed open from a prior text/thinking
            // run — close it before the tool_use opens.
            if self.state.thinking.is_open() {
                self.state.thinking.close();
            }
            out.push(AnthropicSseEvent::ContentBlockStop(
                ContentBlockStopPayload {
                    kind: "content_block_stop".to_string(),
                    index: prev,
                },
            ));
        }
        let state = self
            .state
            .tool_states
            .entry(id.to_string())
            .or_insert_with(|| ToolUseState::new(transition.open_index, id, name));
        if transition.opened_new {
            state.started = true;
            let initial_input = payload
                .get("input")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            out.push(AnthropicSseEvent::ContentBlockStart(
                ContentBlockStartPayload {
                    kind: "content_block_start".to_string(),
                    index: transition.open_index,
                    content_block: ContentBlockInit::ToolUse {
                        id: id.to_string(),
                        name: name.to_string(),
                        input: initial_input,
                    },
                },
            ));
        }
        if let Some(partial) = payload.get("partial_json").and_then(|v| v.as_str()) {
            state.append_partial(partial);
            out.push(AnthropicSseEvent::ContentBlockDelta(
                ContentBlockDeltaPayload {
                    kind: "content_block_delta".to_string(),
                    index: transition.open_index,
                    delta: BlockDelta::InputJsonDelta {
                        partial_json: partial.to_string(),
                    },
                },
            ));
        }
        if payload.get("done").and_then(|v| v.as_bool()) == Some(true) {
            // Upstream signals "this tool_use is complete" — close it.
            out.push(AnthropicSseEvent::ContentBlockStop(
                ContentBlockStopPayload {
                    kind: "content_block_stop".to_string(),
                    index: transition.open_index,
                },
            ));
            self.state.blocks.close_tool_use(id);
        }
        Ok(())
    }

    fn emit_error(
        &mut self,
        kind: AnthropicErrorKind,
        message: String,
        out: &mut Vec<AnthropicSseEvent>,
    ) {
        out.push(AnthropicSseEvent::Error(AnthropicError::new(kind, message)));
    }

    fn finalize(&mut self, stop_reason: String, out: &mut Vec<AnthropicSseEvent>) {
        if self.state.finished {
            return;
        }
        // Close every still-open block.
        let to_close = self.state.blocks.close_all();
        for idx in to_close {
            if self.state.thinking.is_open() && self.state.thinking.block_index() == idx {
                self.state.thinking.close();
            }
            out.push(AnthropicSseEvent::ContentBlockStop(
                ContentBlockStopPayload {
                    kind: "content_block_stop".to_string(),
                    index: idx,
                },
            ));
        }
        // message_delta — final usage + stop reason.
        out.push(AnthropicSseEvent::MessageDelta(MessageDeltaPayload {
            kind: "message_delta".to_string(),
            delta: MessageDeltaInner {
                stop_reason: stop_reason.clone(),
                stop_sequence: None,
            },
            usage: self.state.usage.clone(),
        }));
        // message_stop — terminal.
        out.push(AnthropicSseEvent::MessageStop);
        self.state.stop_reason = Some(stop_reason);
        self.state.finished = true;
    }
}

// ─── helpers ────────────────────────────────────────────────────────

fn decode_payload(evt: &pb::AgentEvent) -> serde_json::Value {
    let bytes = evt
        .record
        .as_ref()
        .map_or(&[][..], |r| r.payload.as_slice());
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice::<serde_json::Value>(bytes).unwrap_or(serde_json::Value::Null)
}

/// Map upstream stop-reason strings into Anthropic's vocabulary.
fn map_stop_reason(raw: &str) -> String {
    match raw {
        "stop" | "end_turn" => "end_turn",
        "length" | "max_tokens" => "max_tokens",
        "tool_use" | "tool_calls" => "tool_use",
        "stop_sequence" => "stop_sequence",
        "content_filter" => "end_turn",
        other => other,
    }
    .to_string()
}

fn classify_error_kind(raw: &str) -> AnthropicErrorKind {
    match raw {
        "invalid_request_error" => AnthropicErrorKind::InvalidRequestError,
        "authentication_error" => AnthropicErrorKind::AuthenticationError,
        "permission_error" => AnthropicErrorKind::PermissionError,
        "not_found_error" => AnthropicErrorKind::NotFoundError,
        "rate_limit_error" => AnthropicErrorKind::RateLimitError,
        "overloaded_error" => AnthropicErrorKind::OverloadedError,
        "billing_error" => AnthropicErrorKind::BillingError,
        _ => AnthropicErrorKind::ApiError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use life_runtime_proto::life::v1::{AgentEvent, AgentEventKind, EventRecord};

    fn token_event(seq: u64, text: &str) -> AgentEvent {
        AgentEvent {
            record: Some(EventRecord {
                session_id: None,
                sequence: seq,
                at: None,
                kind: "TOKEN".into(),
                payload: serde_json::to_vec(&serde_json::json!({"text": text})).unwrap(),
            }),
            kind: AgentEventKind::Token as i32,
        }
    }

    fn thinking_event(seq: u64, thinking: &str) -> AgentEvent {
        AgentEvent {
            record: Some(EventRecord {
                session_id: None,
                sequence: seq,
                at: None,
                kind: "TOKEN".into(),
                payload: serde_json::to_vec(&serde_json::json!({"thinking": thinking})).unwrap(),
            }),
            kind: AgentEventKind::Token as i32,
        }
    }

    fn finish_event(seq: u64, reason: &str) -> AgentEvent {
        AgentEvent {
            record: Some(EventRecord {
                session_id: None,
                sequence: seq,
                at: None,
                kind: "FINISH".into(),
                payload: serde_json::to_vec(&serde_json::json!({"reason": reason})).unwrap(),
            }),
            kind: AgentEventKind::Finish as i32,
        }
    }

    fn tool_call_event(seq: u64, payload: serde_json::Value) -> AgentEvent {
        AgentEvent {
            record: Some(EventRecord {
                session_id: None,
                sequence: seq,
                at: None,
                kind: "TOOL_CALL_PENDING".into(),
                payload: serde_json::to_vec(&payload).unwrap(),
            }),
            kind: AgentEventKind::ToolCallPending as i32,
        }
    }

    fn error_event(seq: u64, kind_str: &str, message: &str) -> AgentEvent {
        AgentEvent {
            record: Some(EventRecord {
                session_id: None,
                sequence: seq,
                at: None,
                kind: "ERROR".into(),
                payload: serde_json::to_vec(&serde_json::json!({
                    "kind": kind_str,
                    "message": message,
                }))
                .unwrap(),
            }),
            kind: AgentEventKind::Error as i32,
        }
    }

    #[test]
    fn empty_stream_with_finish_emits_canonical_frames() {
        let mut e = Encoder::new("msg_test", "claude-sonnet-4-20250514");
        let mut out = Vec::new();
        out.extend(e.encode(&token_event(1, "")).unwrap()); // empty token → only message_start
        out.extend(e.encode(&finish_event(2, "stop")).unwrap());
        // Expected frame sequence: message_start, message_delta,
        // message_stop. No content blocks because the only token was
        // empty.
        let names: Vec<&str> = out
            .iter()
            .map(|e| match e {
                AnthropicSseEvent::MessageStart(_) => "message_start",
                AnthropicSseEvent::ContentBlockStart(_) => "content_block_start",
                AnthropicSseEvent::ContentBlockDelta(_) => "content_block_delta",
                AnthropicSseEvent::ContentBlockStop(_) => "content_block_stop",
                AnthropicSseEvent::MessageDelta(_) => "message_delta",
                AnthropicSseEvent::MessageStop => "message_stop",
                AnthropicSseEvent::Ping => "ping",
                AnthropicSseEvent::Error(_) => "error",
            })
            .collect();
        assert_eq!(
            names,
            vec!["message_start", "message_delta", "message_stop"]
        );
    }

    #[test]
    fn text_token_stream_emits_full_lifecycle() {
        let mut e = Encoder::new("msg_test", "m");
        let mut out = Vec::new();
        out.extend(e.encode(&token_event(1, "Hello")).unwrap());
        out.extend(e.encode(&token_event(2, " world")).unwrap());
        out.extend(e.encode(&finish_event(3, "stop")).unwrap());

        // Expected: message_start, content_block_start(text, 0),
        // content_block_delta(text_delta:"Hello"),
        // content_block_delta(text_delta:" world"),
        // content_block_stop(0), message_delta(end_turn), message_stop.
        assert!(matches!(out[0], AnthropicSseEvent::MessageStart(_)));
        match &out[1] {
            AnthropicSseEvent::ContentBlockStart(p) => {
                assert_eq!(p.index, 0);
                assert!(matches!(p.content_block, ContentBlockInit::Text { .. }));
            }
            o => panic!("expected content_block_start, got {o:?}"),
        }
        match &out[2] {
            AnthropicSseEvent::ContentBlockDelta(p) => {
                assert_eq!(p.index, 0);
                match &p.delta {
                    BlockDelta::TextDelta { text } => assert_eq!(text, "Hello"),
                    o => panic!("expected text_delta, got {o:?}"),
                }
            }
            o => panic!("expected content_block_delta, got {o:?}"),
        }
        // The final 3 frames are predictable.
        match &out[out.len() - 3] {
            AnthropicSseEvent::ContentBlockStop(p) => assert_eq!(p.index, 0),
            o => panic!("expected content_block_stop, got {o:?}"),
        }
        match &out[out.len() - 2] {
            AnthropicSseEvent::MessageDelta(p) => {
                assert_eq!(p.delta.stop_reason, "end_turn");
            }
            o => panic!("expected message_delta, got {o:?}"),
        }
        assert!(matches!(out[out.len() - 1], AnthropicSseEvent::MessageStop));
    }

    #[test]
    fn thinking_token_opens_thinking_block_then_switches_to_text() {
        let mut e = Encoder::new("msg_test", "m");
        let mut frames = Vec::new();
        frames.extend(e.encode(&thinking_event(1, "...")).unwrap());
        frames.extend(e.encode(&token_event(2, "answer")).unwrap());
        frames.extend(e.encode(&finish_event(3, "stop")).unwrap());

        // Look for: a thinking block_start, a stop on idx=0 right
        // before a text content_block_start at idx=1.
        let kinds: Vec<_> = frames
            .iter()
            .map(|f| match f {
                AnthropicSseEvent::MessageStart(_) => "ms",
                AnthropicSseEvent::ContentBlockStart(p) => match &p.content_block {
                    ContentBlockInit::Thinking { .. } => "cbs_thinking",
                    ContentBlockInit::Text { .. } => "cbs_text",
                    ContentBlockInit::ToolUse { .. } => "cbs_tool",
                },
                AnthropicSseEvent::ContentBlockDelta(p) => match p.delta {
                    BlockDelta::ThinkingDelta { .. } => "cbd_think",
                    BlockDelta::TextDelta { .. } => "cbd_text",
                    BlockDelta::SignatureDelta { .. } => "cbd_sig",
                    BlockDelta::InputJsonDelta { .. } => "cbd_json",
                },
                AnthropicSseEvent::ContentBlockStop(_) => "cb_stop",
                AnthropicSseEvent::MessageDelta(_) => "md",
                AnthropicSseEvent::MessageStop => "mst",
                _ => "?",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "ms",
                "cbs_thinking",
                "cbd_think",
                "cb_stop", // close thinking before text opens
                "cbs_text",
                "cbd_text",
                "cb_stop", // close text on finalize
                "md",
                "mst"
            ]
        );
    }

    #[test]
    fn tool_call_emits_tool_use_block_with_input_json_delta() {
        let mut e = Encoder::new("msg_test", "m");
        let mut out = Vec::new();
        out.extend(e.encode(&token_event(1, "I'll read it.")).unwrap());
        out.extend(
            e.encode(&tool_call_event(
                2,
                serde_json::json!({"id":"toolu_01","name":"read_file","partial_json":"{\"path\":"}),
            ))
            .unwrap(),
        );
        out.extend(
            e.encode(&tool_call_event(
                3,
                serde_json::json!({"id":"toolu_01","name":"read_file","partial_json":" \"foo.txt\"}", "done": true}),
            ))
            .unwrap(),
        );
        out.extend(e.encode(&finish_event(4, "tool_use")).unwrap());

        // Look for the message_delta with stop_reason = "tool_use".
        let md = out
            .iter()
            .find_map(|e| match e {
                AnthropicSseEvent::MessageDelta(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert_eq!(md.delta.stop_reason, "tool_use");

        // Tool_use block at index 1 (text was at 0).
        let tool_start = out
            .iter()
            .find_map(|e| match e {
                AnthropicSseEvent::ContentBlockStart(p) => match &p.content_block {
                    ContentBlockInit::ToolUse { id, name, .. } => {
                        Some((p.index, id.clone(), name.clone()))
                    }
                    _ => None,
                },
                _ => None,
            })
            .unwrap();
        assert_eq!(tool_start.0, 1);
        assert_eq!(tool_start.1, "toolu_01");
        assert_eq!(tool_start.2, "read_file");

        // Two input_json_delta frames carrying the JSON fragments.
        let json_fragments: Vec<_> = out
            .iter()
            .filter_map(|e| match e {
                AnthropicSseEvent::ContentBlockDelta(p) => match &p.delta {
                    BlockDelta::InputJsonDelta { partial_json } => Some(partial_json.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(json_fragments.len(), 2);
        assert_eq!(json_fragments[0], "{\"path\":");
        assert_eq!(json_fragments[1], " \"foo.txt\"}");
    }

    #[test]
    fn error_event_emits_inline_error_event_and_closes_stream() {
        let mut e = Encoder::new("msg_test", "m");
        let mut out = Vec::new();
        out.extend(e.encode(&token_event(1, "Hi")).unwrap());
        out.extend(
            e.encode(&error_event(2, "overloaded_error", "backend overloaded"))
                .unwrap(),
        );
        // The encoder should have finalized internally.
        let has_error = out.iter().any(|e| matches!(e, AnthropicSseEvent::Error(_)));
        let has_stop = out
            .iter()
            .any(|e| matches!(e, AnthropicSseEvent::MessageStop));
        assert!(has_error);
        assert!(has_stop);

        // Subsequent events are dropped (encoder is finished).
        let later = e.encode(&token_event(3, "should drop")).unwrap();
        assert!(later.is_empty());
    }

    #[test]
    fn message_delta_emits_with_canonical_max_tokens_reason() {
        let mut e = Encoder::new("msg_test", "m");
        let mut out = Vec::new();
        out.extend(e.encode(&token_event(1, "abc")).unwrap());
        out.extend(e.encode(&finish_event(2, "length")).unwrap());
        let md = out
            .iter()
            .find_map(|e| match e {
                AnthropicSseEvent::MessageDelta(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert_eq!(md.delta.stop_reason, "max_tokens");
    }

    #[test]
    fn replay_dedup_drops_already_emitted_event_on_resume() {
        // Encoder snapshot saved at seq=2; resume re-reads the same
        // event — should produce empty output for the duplicate.
        let mut e = Encoder::new("msg_test", "m");
        let _ = e.encode(&token_event(1, "Hello")).unwrap();
        let _ = e.encode(&token_event(2, " world")).unwrap();
        // Replay seq=2 — should produce no new events.
        let replay = e.encode(&token_event(2, " world")).unwrap();
        assert!(
            replay.is_empty(),
            "replay of already-emitted event must produce no output, got {replay:?}"
        );
    }

    #[test]
    fn set_input_tokens_propagates_to_message_start() {
        let mut e = Encoder::new("msg_test", "m");
        e.set_input_tokens(123);
        let start = e.emit_message_start().unwrap();
        if let AnthropicSseEvent::MessageStart(p) = start {
            assert_eq!(p.message.usage.input_tokens, 123);
            assert_eq!(p.message.usage.output_tokens, 1);
        } else {
            panic!("expected MessageStart");
        }
    }

    #[test]
    fn message_start_only_emits_once_per_encoder() {
        let mut e = Encoder::new("msg_test", "m");
        assert!(e.emit_message_start().is_some());
        assert!(e.emit_message_start().is_none());
    }

    #[test]
    fn sse_frame_renders_with_data_line_and_blank_terminator() {
        let evt = AnthropicSseEvent::MessageStop;
        let frame = evt.to_sse_frame();
        assert!(frame.starts_with("event: message_stop\n"));
        assert!(frame.contains("\ndata: "));
        assert!(frame.ends_with("\n\n"));
    }

    #[test]
    fn ping_helper_renders_minimal_ping_frame() {
        let p = Encoder::ping();
        let frame = p.to_sse_frame();
        assert!(frame.starts_with("event: ping\n"));
        assert!(frame.ends_with("\n\n"));
    }

    #[test]
    fn top_level_error_helper_emits_typed_error_event() {
        let e = Encoder::emit_top_level_error(
            AnthropicErrorKind::AuthenticationError,
            "missing bearer",
        );
        match e {
            AnthropicSseEvent::Error(payload) => {
                assert_eq!(payload.error.kind, "authentication_error");
                assert_eq!(payload.error.message, "missing bearer");
            }
            o => panic!("expected error event, got {o:?}"),
        }
    }

    #[test]
    fn force_finalize_emits_close_then_is_idempotent() {
        let mut e = Encoder::new("msg_test", "m");
        let _ = e.encode(&token_event(1, "hi")).unwrap();
        let first = e.force_finalize();
        assert!(
            first
                .iter()
                .any(|x| matches!(x, AnthropicSseEvent::MessageStop))
        );
        // Second call is a no-op.
        let second = e.force_finalize();
        assert!(second.is_empty());
    }

    #[test]
    fn map_stop_reason_handles_canonical_aliases() {
        assert_eq!(map_stop_reason("stop"), "end_turn");
        assert_eq!(map_stop_reason("length"), "max_tokens");
        assert_eq!(map_stop_reason("tool_calls"), "tool_use");
        assert_eq!(map_stop_reason("tool_use"), "tool_use");
        assert_eq!(map_stop_reason("stop_sequence"), "stop_sequence");
        assert_eq!(map_stop_reason("content_filter"), "end_turn");
        // Unknown passes through.
        assert_eq!(map_stop_reason("unknown_reason"), "unknown_reason");
    }

    #[test]
    fn classify_error_kind_maps_published_codes() {
        assert!(matches!(
            classify_error_kind("rate_limit_error"),
            AnthropicErrorKind::RateLimitError
        ));
        assert!(matches!(
            classify_error_kind("overloaded_error"),
            AnthropicErrorKind::OverloadedError
        ));
        assert!(matches!(
            classify_error_kind("authentication_error"),
            AnthropicErrorKind::AuthenticationError
        ));
        // Unknown defaults to api_error.
        assert!(matches!(
            classify_error_kind("weird_new_anthropic_code"),
            AnthropicErrorKind::ApiError
        ));
    }

    #[test]
    fn resume_carries_block_index_state() {
        // Build encoder, advance, snapshot, resume — the resumed
        // encoder must allocate the next block index past where the
        // original left off. We assert the property via the observable
        // SSE wire shape: the `content_block_start` emitted on the
        // resumed encoder must carry a higher `index` than any block
        // emitted before the snapshot.
        let mut e = Encoder::new("msg_test", "m");
        let pre = e.encode(&token_event(1, "first")).unwrap();
        let max_pre_index = pre
            .iter()
            .filter_map(|ev| match ev {
                AnthropicSseEvent::ContentBlockStart(p) => Some(p.index),
                _ => None,
            })
            .max()
            .expect("first encode must emit at least one content_block_start");
        let _ = e.encode(&finish_event(2, "stop")).unwrap();
        // Manually clear `finished` so resume can continue (in real
        // life replay only resumes mid-stream sessions, but the
        // mechanism is the same).
        let mut state = e.state().clone();
        state.finished = false;
        let _ = state.blocks.close_all();
        // Snapshot expectation: no second message_start on resume.
        assert!(
            state.started,
            "snapshot must remember that message_start was already emitted"
        );
        let mut resumed = Encoder::resume(state);
        let post = resumed.encode(&token_event(3, "second")).unwrap();
        // (a) Resume must NOT re-emit message_start.
        assert!(
            !post
                .iter()
                .any(|ev| matches!(ev, AnthropicSseEvent::MessageStart(_))),
            "resume must not re-emit message_start"
        );
        // (b) The new content_block_start index must be strictly
        //     greater than any index used pre-snapshot.
        let post_start_index = post
            .iter()
            .find_map(|ev| match ev {
                AnthropicSseEvent::ContentBlockStart(p) => Some(p.index),
                _ => None,
            })
            .expect("resumed encode must emit a content_block_start for the new token");
        assert!(
            post_start_index > max_pre_index,
            "resumed block index must be > pre-snapshot max ({post_start_index} <= {max_pre_index})"
        );
    }

    #[test]
    fn ping_is_separate_from_in_band_message_lifecycle() {
        // Sending pings does not consume tracker state and does not
        // count as `started`.
        let mut e = Encoder::new("msg_test", "m");
        let p = Encoder::ping();
        assert!(matches!(p, AnthropicSseEvent::Ping));
        // Message_start is still unsent.
        assert!(e.emit_message_start().is_some());
    }

    /// J-Sub-D (BRO-1143): a tool_use event missing the required `id`
    /// must produce a typed `CodecError::Upstream` rather than panic or
    /// silently corrupting the stream.
    #[test]
    fn tool_call_pending_without_id_returns_upstream_error() {
        let mut e = Encoder::new("msg_test", "m");
        let bad = AgentEvent {
            record: Some(EventRecord {
                session_id: None,
                sequence: 1,
                at: None,
                kind: "TOOL_CALL_PENDING".into(),
                payload: serde_json::to_vec(&serde_json::json!({"name": "f"})).unwrap(),
            }),
            kind: AgentEventKind::ToolCallPending as i32,
        };
        let err = e.encode(&bad).unwrap_err();
        assert!(
            matches!(err, crate::errors::CodecError::Upstream(ref m) if m.contains("id")),
            "expected CodecError::Upstream mentioning id, got {err:?}"
        );
    }

    /// J-Sub-D (BRO-1143): a tool_use event missing the required `name`
    /// likewise surfaces a typed `CodecError::Upstream`.
    #[test]
    fn tool_call_pending_without_name_returns_upstream_error() {
        let mut e = Encoder::new("msg_test", "m");
        let bad = AgentEvent {
            record: Some(EventRecord {
                session_id: None,
                sequence: 1,
                at: None,
                kind: "TOOL_CALL_PENDING".into(),
                payload: serde_json::to_vec(&serde_json::json!({"id": "toolu_01"})).unwrap(),
            }),
            kind: AgentEventKind::ToolCallPending as i32,
        };
        let err = e.encode(&bad).unwrap_err();
        assert!(
            matches!(err, crate::errors::CodecError::Upstream(ref m) if m.contains("name")),
            "expected CodecError::Upstream mentioning name, got {err:?}"
        );
    }

    /// J-Sub-D (BRO-1143): the `done: true` signal on a tool_call event
    /// closes the tool_use block at the correct index; subsequent
    /// tool_call events for a new id allocate a fresh index.
    #[test]
    fn tool_call_pending_done_closes_block_at_correct_index() {
        let mut e = Encoder::new("msg_test", "m");
        let mut out = Vec::new();
        out.extend(
            e.encode(&tool_call_event(
                1,
                serde_json::json!({"id":"toolu_A","name":"a","partial_json":"{}","done":true}),
            ))
            .unwrap(),
        );
        // First tool_use opened + closed at index 0.
        let opens: Vec<_> = out
            .iter()
            .filter_map(|ev| match ev {
                AnthropicSseEvent::ContentBlockStart(p) => match &p.content_block {
                    ContentBlockInit::ToolUse { id, .. } => Some((p.index, id.clone())),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(opens, vec![(0, "toolu_A".to_string())]);
        let closes: Vec<_> = out
            .iter()
            .filter_map(|ev| match ev {
                AnthropicSseEvent::ContentBlockStop(p) => Some(p.index),
                _ => None,
            })
            .collect();
        assert_eq!(closes, vec![0]);

        // Now open a second tool_use — it gets a fresh index (1).
        out.clear();
        out.extend(
            e.encode(&tool_call_event(
                2,
                serde_json::json!({"id":"toolu_B","name":"b","input":{}}),
            ))
            .unwrap(),
        );
        let second_idx = out
            .iter()
            .find_map(|ev| match ev {
                AnthropicSseEvent::ContentBlockStart(p) => Some(p.index),
                _ => None,
            })
            .unwrap();
        assert_eq!(second_idx, 1, "second tool_use must get a fresh index");
    }
}
