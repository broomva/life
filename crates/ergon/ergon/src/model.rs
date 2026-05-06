//! Model-interaction wire types.
//!
//! Ergon owns its own canonical types for model requests, responses,
//! messages, content blocks, tools, and usage accounting. These types are
//! deliberately **independent of any specific provider crate** — the
//! autonomous loop in `step.rs` (BRO-998) translates between these and
//! provider-specific shapes (`arcan_provider`, `praxis_core::Tool`, etc.).
//!
//! ## Why ergon owns these shapes
//!
//! Hooks ([`crate::hook::Hook`]) need stable signatures: `&mut ModelRequest`,
//! `&ModelResponse`, `&mut ToolCall`, `&mut ToolResult`. If those types
//! came from `arcan_provider` or `praxis_core`, every change in those
//! crates would ripple through every hook implementation. Ergon's contract
//! is to the *workflow author*, not to the runtime — so the wire types must
//! belong to ergon.
//!
//! ## Block-structured content
//!
//! Unlike a flat `ChatMessage { role, content: String }`, ergon's
//! [`Message`] holds `Vec<ContentBlock>`. This mirrors the underlying
//! reality of every modern provider (Anthropic, OpenAI, Bedrock): a single
//! assistant turn can interleave text, reasoning, tool_use, and citations.
//! Flattening to a string loses the structure the hooks need to inspect.
//!
//! ## Stability contract
//!
//! All public types are `#[non_exhaustive]`. Fields are public so workflow
//! authors can construct values directly; new fields land in any minor
//! version without breaking existing constructors that use struct-update
//! syntax.

use crate::stream::StopReason;
use serde::{Deserialize, Serialize};

/// The role a [`Message`] carries in a conversation history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// System / instruction role. Typically materialised from a [`crate::Role`]
    /// overlay rendered into a single string. Some providers represent this
    /// as a top-level `system` field rather than a message — translation is
    /// the runtime's responsibility, not ergon's.
    System,
    /// Human user input.
    User,
    /// Model output. May contain [`ContentBlock::Text`],
    /// [`ContentBlock::Reasoning`], and [`ContentBlock::ToolUse`] blocks.
    Assistant,
    /// Tool execution result fed back into the conversation. Carried as
    /// [`ContentBlock::ToolResult`] inside the message content.
    Tool,
}

/// A typed content block within a [`Message`].
///
/// Mirrors the multi-block content model used by every modern model API.
/// Variants are append-only after v1.0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text — the most common variant.
    Text { text: String },

    /// Extended-thinking / reasoning block. `signed` indicates whether the
    /// block carries a provider-issued signature (Anthropic) — preserved
    /// through replay to allow downstream verification.
    Reasoning {
        id: String,
        text: String,
        #[serde(default)]
        signed: bool,
    },

    /// Model-issued tool invocation.
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },

    /// Result of a tool invocation, fed back into history.
    ToolResult {
        call_id: String,
        #[serde(default)]
        output: serde_json::Value,
        #[serde(default)]
        is_error: bool,
    },
}

impl ContentBlock {
    /// Construct a plain-text block with the given content.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// True iff this block is a [`Self::ToolUse`] variant.
    pub fn is_tool_use(&self) -> bool {
        matches!(self, Self::ToolUse { .. })
    }
}

/// A single chat message — role plus structured content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Message {
    /// Conversation role.
    pub role: MessageRole,
    /// Ordered, typed content blocks.
    pub content: Vec<ContentBlock>,
}

impl Message {
    /// Construct a message with the given role and content blocks.
    pub fn new(role: MessageRole, content: Vec<ContentBlock>) -> Self {
        Self { role, content }
    }

    /// Helper: a user message with a single text block.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// Helper: an assistant message with a single text block.
    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// Iterate over the message's [`ContentBlock::ToolUse`] blocks.
    pub fn tool_uses(&self) -> impl Iterator<Item = &ContentBlock> {
        self.content.iter().filter(|b| b.is_tool_use())
    }
}

/// A tool definition exposed to the model.
///
/// Ergon does not itself execute tools — execution is delegated to
/// [`praxis_core::ToolRegistry`] (wired in `step.rs`). This type exists so
/// hooks and workflow authors can inspect / mutate the schema the model
/// will see on a given turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolDefinition {
    /// Stable identifier the model uses when emitting `tool_use` blocks.
    pub name: String,
    /// Human-readable description (becomes part of the prompt).
    pub description: String,
    /// JSON Schema for the tool's input. Provider-agnostic shape.
    pub input_schema: serde_json::Value,
}

impl ToolDefinition {
    /// Construct a new definition with required fields.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

/// A tool invocation extracted from a [`ContentBlock::ToolUse`] block.
///
/// Ergon's [`crate::hook::Hook::on_pre_tool_use`] receives this as
/// `&mut ToolCall` so capability gates can deny or stub the call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolCall {
    /// Unique identifier within a turn (matches the `id` of the originating
    /// [`ContentBlock::ToolUse`]).
    pub id: String,
    /// Name of the [`ToolDefinition`] being invoked.
    pub name: String,
    /// Input arguments parsed as JSON. Schema validation happens at the
    /// runtime layer (praxis), not in ergon.
    #[serde(default)]
    pub input: serde_json::Value,
}

impl ToolCall {
    /// Construct a new tool call.
    pub fn new(id: impl Into<String>, name: impl Into<String>, input: serde_json::Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            input,
        }
    }
}

/// The result of a tool invocation, fed back to the model on the next turn.
///
/// `is_error` is signal for the model — it does **not** mean the tool
/// runtime failed catastrophically. A tool can return `is_error = true` to
/// tell the model "you used me wrong, try again." Hard runtime failures
/// surface as [`crate::ErgonError::Tool`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolResult {
    /// `id` of the originating [`ToolCall`].
    pub call_id: String,
    /// Output payload, fed back to the model as a
    /// [`ContentBlock::ToolResult`].
    #[serde(default)]
    pub output: serde_json::Value,
    /// Whether this result represents a recoverable error the model should
    /// reason about (MCP `isError`-style flag).
    #[serde(default)]
    pub is_error: bool,
}

impl ToolResult {
    /// Construct a successful tool result.
    pub fn success(call_id: impl Into<String>, output: serde_json::Value) -> Self {
        Self {
            call_id: call_id.into(),
            output,
            is_error: false,
        }
    }

    /// Construct a model-visible error result.
    pub fn model_error(call_id: impl Into<String>, output: serde_json::Value) -> Self {
        Self {
            call_id: call_id.into(),
            output,
            is_error: true,
        }
    }
}

/// Per-turn request to the model provider.
///
/// Built by `step.rs` from the workflow author's `InferenceRequest` (which
/// is a higher-level configuration plus the workflow's accumulated history).
/// Hooks see this as `&mut ModelRequest` via
/// [`crate::hook::Hook::on_pre_inference`] and may rewrite any field —
/// including injecting / stripping tools, changing the system prompt, or
/// adjusting limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelRequest {
    /// Provider-specific model identifier (e.g. `"claude-sonnet-4"`).
    pub model: String,
    /// Conversation history for this turn.
    pub messages: Vec<Message>,
    /// Tools advertised to the model on this turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    /// Rendered system prompt (see [`crate::Role::render`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Provider max-output cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Stop sequences supplied to the provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
}

impl ModelRequest {
    /// Construct a request with model and history; sensible defaults for
    /// the rest.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: Vec::new(),
            system: None,
            max_tokens: None,
            temperature: None,
            stop: Vec::new(),
        }
    }
}

/// Token / cost accounting for a single provider call.
///
/// Mirrors the [`crate::stream::StreamEvent::Usage`] payload, kept as a
/// distinct struct so [`ModelResponse`] can carry it without depending on
/// the stream module. The fields are intentionally identical so future
/// pipelines can convert between them lossless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
}

/// Per-turn response from the model provider.
///
/// Hooks see this as `&ModelResponse` via
/// [`crate::hook::Hook::on_post_inference`] (read-only — the response
/// itself is not mutated; subsequent turns build a fresh
/// [`ModelRequest`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelResponse {
    /// Content blocks the model produced this turn.
    pub content: Vec<ContentBlock>,
    /// Why the stream terminated.
    pub stop_reason: StopReason,
    /// Token / cost accounting.
    #[serde(default)]
    pub usage: Usage,
}

impl ModelResponse {
    /// Construct a response with the given content blocks and stop
    /// reason. Defaults [`Usage`] to zeros — populate it via
    /// [`Self::with_usage`] if needed.
    pub fn new(content: Vec<ContentBlock>, stop_reason: StopReason) -> Self {
        Self {
            content,
            stop_reason,
            usage: Usage::default(),
        }
    }

    /// Builder: attach token / cost accounting.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = usage;
        self
    }

    /// Iterate over the response's tool-use blocks.
    pub fn tool_uses(&self) -> impl Iterator<Item = &ContentBlock> {
        self.content.iter().filter(|b| b.is_tool_use())
    }

    /// Convenience: extract every [`ContentBlock::ToolUse`] as a [`ToolCall`].
    pub fn extract_tool_calls(&self) -> Vec<ToolCall> {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => Some(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// Concatenate all [`ContentBlock::Text`] blocks into a single string.
    /// Useful for workflows that want a quick text-only view.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for block in &self.content {
            if let ContentBlock::Text { text } = block {
                out.push_str(text);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn message_helpers_produce_single_text_block() {
        let m = Message::user_text("hi");
        assert_eq!(m.role, MessageRole::User);
        assert_eq!(m.content.len(), 1);
        match &m.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hi"),
            _ => panic!("expected text block"),
        }
    }

    #[test]
    fn content_block_round_trips_through_json() {
        let block = ContentBlock::ToolUse {
            id: "call_42".into(),
            name: "fs_read".into(),
            input: json!({"path": "/tmp/x"}),
        };
        let raw = serde_json::to_string(&block).expect("ser");
        assert!(raw.contains("\"type\":\"tool_use\""));
        let back: ContentBlock = serde_json::from_str(&raw).expect("de");
        assert_eq!(back, block);
    }

    #[test]
    fn tool_result_distinguishes_success_from_model_error() {
        let ok = ToolResult::success("c1", json!({"x": 1}));
        let err = ToolResult::model_error("c2", json!({"reason": "bad arg"}));
        assert!(!ok.is_error);
        assert!(err.is_error);
    }

    #[test]
    fn model_request_defaults_are_minimal_and_serialize_cleanly() {
        let req = ModelRequest::new("claude-sonnet-4", vec![Message::user_text("hi")]);
        let raw = serde_json::to_string(&req).expect("ser");
        // Optional fields should be elided when None / empty.
        assert!(!raw.contains("\"max_tokens\""));
        assert!(!raw.contains("\"system\""));
        assert!(!raw.contains("\"stop\""));
        assert!(!raw.contains("\"tools\""));
    }

    #[test]
    fn extract_tool_calls_pulls_from_response_content() {
        let resp = ModelResponse {
            content: vec![
                ContentBlock::text("thinking..."),
                ContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "fs_read".into(),
                    input: json!({"path": "/x"}),
                },
                ContentBlock::ToolUse {
                    id: "tu_2".into(),
                    name: "grep".into(),
                    input: json!({"q": "todo"}),
                },
            ],
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        };
        let calls = resp.extract_tool_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "fs_read");
        assert_eq!(calls[1].id, "tu_2");
    }

    #[test]
    fn response_text_concatenates_text_blocks_only() {
        let resp = ModelResponse {
            content: vec![
                ContentBlock::text("hello "),
                ContentBlock::ToolUse {
                    id: "x".into(),
                    name: "y".into(),
                    input: json!({}),
                },
                ContentBlock::text("world"),
            ],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        assert_eq!(resp.text(), "hello world");
    }

    #[test]
    fn usage_round_trips_with_optional_fields() {
        let u = Usage {
            input_tokens: 100,
            output_tokens: 200,
            cached_input_tokens: Some(50),
            reasoning_tokens: None,
        };
        let raw = serde_json::to_string(&u).expect("ser");
        // None field should be elided
        assert!(!raw.contains("reasoning_tokens"));
        assert!(raw.contains("cached_input_tokens"));
        let back: Usage = serde_json::from_str(&raw).expect("de");
        assert_eq!(back, u);
    }

    #[test]
    fn message_role_serializes_snake_case() {
        let raw = serde_json::to_string(&MessageRole::Assistant).expect("ser");
        assert_eq!(raw, "\"assistant\"");
    }

    #[test]
    fn content_block_text_helper_is_terse() {
        let b = ContentBlock::text("hi");
        assert!(matches!(b, ContentBlock::Text { .. }));
        assert!(!b.is_tool_use());
    }
}
