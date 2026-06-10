//! Inbound Anthropic Messages request shape + validator.
//!
//! Mirrors `core/anthropic/native_messages_request.py` from the
//! free-claude-code reference: defines a typed Rust shape for the
//! `POST /v1/messages` body and validates the `anthropic-version`
//! header (Spec J L10-D5).
//!
//! ## Strictness policy
//!
//! Drift detection is layered:
//!
//! * **Header-level (strict)** — [`AnthropicVersion::parse`] rejects
//!   unknown `anthropic-version` values with
//!   [`CodecError::UnsupportedAnthropicVersion`]. Per Spec J L10-D5
//!   the header is the canonical "what API version are you speaking"
//!   pin and drift there must be loud.
//! * **Body-level (forward-compatible)** — `AnthropicMessagesRequest`
//!   does NOT use `deny_unknown_fields`. free-claude-code's reference
//!   impl accepts an open list (`mcp_servers`, `context_management`,
//!   `output_config`, `extra_body`, ...) and Anthropic itself adds
//!   optional fields over time. Rejecting at the body envelope would
//!   break working clients.
//! * **Variant-level (strict)** — `ContentBlock` is a tagged enum, so
//!   unknown `type:` discriminants still fail loudly (an unknown
//!   content_block type means the encoder cannot translate it to a
//!   `pb::AgentEvent`, which is a structural error).
//!
//! This module is a pure decoder. It does not call any substrate, does
//! not perform authentication, and does not synthesize any state — it
//! only turns bytes into a typed [`AnthropicMessagesRequest`] (or a
//! typed [`CodecError`]).
//!
//! [`CodecError`]: crate::errors::CodecError

use serde::{Deserialize, Serialize};

use crate::errors::CodecError;

/// Conversation role. Anthropic Messages only uses `user` and
/// `assistant`; system prompts ride on a separate `system` field
/// rather than a third role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Role {
    /// End-user turn.
    User,
    /// Assistant turn.
    Assistant,
}

/// One content block inside a [`Message`].
///
/// Claude Code allows two shapes for `messages[].content`:
///
/// * `"hello"` — a plain string (treated as a single text block); or
/// * `[{type:"text", text:"..."}, {type:"tool_use", ...}, ...]`.
///
/// The codec accepts both; see [`MessageContent`].
///
/// Per Anthropic Messages, every content block (and `SystemBlock`)
/// accepts an optional `cache_control: {"type":"ephemeral"}` marker
/// for prompt caching. The codec doesn't act on it, but it must
/// round-trip without error so requests using Anthropic's prompt
/// caching (the recommended default for long system prompts + tool
/// definitions) parse cleanly.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContentBlock {
    /// Plain text content.
    Text {
        /// UTF-8 text body.
        text: String,
        /// Optional `cache_control` hint (e.g. `{type:"ephemeral"}`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<serde_json::Value>,
    },
    /// Tool invocation block (assistant turn).
    ToolUse {
        /// Tool-use id chosen by the model (e.g. `toolu_01abc`).
        id: String,
        /// Tool name.
        name: String,
        /// Tool input — free-form JSON.
        input: serde_json::Value,
        /// Optional `cache_control` hint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<serde_json::Value>,
    },
    /// Tool result block (user turn following an assistant tool_use).
    ToolResult {
        /// Tool-use id this is a response to.
        tool_use_id: String,
        /// Optional textual result content. May be a string or an
        /// array of content blocks; the JSON value keeps both shapes
        /// valid without forcing a schema on the input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<serde_json::Value>,
        /// Whether the tool call failed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        /// Optional `cache_control` hint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<serde_json::Value>,
    },
    /// Thinking block — extended thinking traces.
    Thinking {
        /// Thinking content text.
        thinking: String,
        /// Optional signed thinking signature.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        /// Optional `cache_control` hint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<serde_json::Value>,
    },
    /// Redacted thinking block — opaque encrypted bytes.
    RedactedThinking {
        /// Opaque base64 payload.
        data: String,
    },
}

/// The shape of `messages[].content` — string OR array of blocks.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum MessageContent {
    /// Single-string shorthand. Equivalent to `[ContentBlock::Text{text}]`.
    Text(String),
    /// Explicit array of content blocks.
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Borrow the message body as a slice of `ContentBlock`s, lazily
    /// wrapping the `Text` shorthand into a single-block array.
    pub fn as_blocks(&self) -> Vec<ContentBlock> {
        match self {
            Self::Text(s) => vec![ContentBlock::Text {
                text: s.clone(),
                cache_control: None,
            }],
            Self::Blocks(b) => b.clone(),
        }
    }

    /// Extract the plain text concatenation. tool_use / tool_result /
    /// thinking blocks contribute the empty string — they are not
    /// "user text" for the purposes of sid synthesis (see Spec J
    /// §[Anima binding]).
    pub fn plain_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => {
                let mut out = String::new();
                for b in blocks {
                    if let ContentBlock::Text { text, .. } = b {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(text);
                    }
                }
                out
            }
        }
    }
}

/// One message in the `messages: [...]` array.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Message {
    /// Conversation role.
    pub role: Role,
    /// Content payload — string or array.
    pub content: MessageContent,
}

/// `system` field shape — Anthropic accepts either a single string or
/// an array of `{type:"text", text:"..."}` blocks.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum SystemPrompt {
    /// Single-string shorthand.
    Text(String),
    /// Array of text blocks (Claude Code SDK shape).
    Blocks(Vec<SystemBlock>),
}

/// Single block inside an array-shaped system prompt.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SystemBlock {
    /// Always `"text"` for now; future Anthropic releases may add new
    /// block types here.
    #[serde(rename = "type")]
    pub kind: String,
    /// Text body.
    pub text: String,
    /// Optional `cache_control: {type:"ephemeral"}` marker for prompt
    /// caching. We don't reject it but we don't act on it at the codec
    /// level either.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<serde_json::Value>,
}

/// `tools[*]` entry — a function tool the model may emit.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    /// Tool name.
    pub name: String,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema describing the tool input.
    pub input_schema: serde_json::Value,
    /// Optional `cache_control` hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<serde_json::Value>,
}

/// `tool_choice` field — controls whether/which tool the model may
/// invoke.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub enum ToolChoice {
    /// Model picks freely (default).
    Auto,
    /// Model MUST call any tool.
    Any,
    /// Model MUST call a specific tool.
    Tool {
        /// Tool name to force.
        name: String,
    },
    /// Model must NOT call any tool.
    None,
}

/// `thinking` field — extended thinking knob.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub enum ThinkingConfig {
    /// Thinking enabled with a token budget.
    Enabled {
        /// Maximum thinking tokens.
        budget_tokens: u32,
    },
    /// Thinking explicitly disabled.
    Disabled,
}

/// Inbound `POST /v1/messages` body.
///
/// The body is **forward-compatible**: unknown top-level fields are
/// silently ignored by serde's default. Spec J L10-D5's strictness
/// applies to the `anthropic-version` *header* (see
/// [`AnthropicVersion::parse`]), not to the body envelope.
///
/// Rationale: free-claude-code's reference impl
/// (`core/anthropic/native_messages_request.py`) accepts an open list
/// of fields including `mcp_servers`, `context_management`,
/// `output_config`, and `extra_body`, and Anthropic itself adds new
/// optional fields (e.g. `cache_control`-aware variants) without
/// notice. Body-level `deny_unknown_fields` would reject the very
/// clients this codec exists to serve. Drift detection lives at the
/// header (`AnthropicVersion`) and per-`ContentBlock` `type` level.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AnthropicMessagesRequest {
    /// Model identifier — Anthropic-named (`claude-sonnet-4-...`) or
    /// life-routed (`life/<backend>/<model>`).
    pub model: String,
    /// Conversation history.
    pub messages: Vec<Message>,
    /// Optional system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemPrompt>,
    /// Maximum tokens the model may produce in this response.
    pub max_tokens: u32,
    /// Optional stop sequences.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    /// Whether the response should stream (Claude Code always sets `true`).
    #[serde(default)]
    pub stream: bool,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Top-p nucleus sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Top-k sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Free-form metadata (Anthropic accepts `{user_id: ...}` here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Tools available to the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    /// Tool-call policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Extended thinking knob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
}

impl AnthropicMessagesRequest {
    /// Find the first message with role=user. Returns `None` when the
    /// request carries no user turn at all (which the sid synthesizer
    /// rejects as [`CodecError::NoUserMessage`]).
    pub fn first_user_message(&self) -> Option<&Message> {
        self.messages.iter().find(|m| m.role == Role::User)
    }

    /// Validate the request after deserialization.
    ///
    /// Catches semantic errors `serde` cannot — e.g. empty `messages`,
    /// `max_tokens == 0`, an empty `model`.
    pub fn validate(&self) -> Result<(), CodecError> {
        if self.model.trim().is_empty() {
            return Err(CodecError::InvalidRequest("model must not be empty".into()));
        }
        if self.messages.is_empty() {
            return Err(CodecError::InvalidRequest(
                "messages must contain at least one entry".into(),
            ));
        }
        if self.max_tokens == 0 {
            return Err(CodecError::InvalidRequest("max_tokens must be > 0".into()));
        }
        if self.first_user_message().is_none() {
            return Err(CodecError::NoUserMessage);
        }
        Ok(())
    }
}

/// Supported value of the `anthropic-version` HTTP header.
///
/// Per Spec J L10-D5, lifegw rejects unknown values via HTTP 400 so
/// upstream protocol drift surfaces immediately. As of Claude Code
/// v0.x (May 2026), the only published stable value is
/// `2023-06-01`. The `2023-01-01` beta value is also accepted because
/// some launcher binaries still pin to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnthropicVersion {
    /// `2023-06-01` — Claude Code current pin.
    V20230601,
    /// `2023-01-01` — legacy beta pin still used by some forks.
    V20230101,
}

impl AnthropicVersion {
    /// Parse the raw header value. Returns
    /// [`CodecError::UnsupportedAnthropicVersion`] for unknown values.
    pub fn parse(raw: &str) -> Result<Self, CodecError> {
        match raw.trim() {
            "2023-06-01" => Ok(Self::V20230601),
            "2023-01-01" => Ok(Self::V20230101),
            other => Err(CodecError::UnsupportedAnthropicVersion(other.to_string())),
        }
    }

    /// Round-trip wire string.
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::V20230601 => "2023-06-01",
            Self::V20230101 => "2023-01-01",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_body(model: &str) -> &'static str {
        // Static so we keep test-data near tests.
        let _ = model;
        r#"{
            "model": "claude-sonnet-4-20250514",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 100
        }"#
    }

    #[test]
    fn minimal_request_parses() {
        let body = minimal_body("claude-sonnet-4-20250514");
        let req: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        req.validate().unwrap();
        assert_eq!(req.model, "claude-sonnet-4-20250514");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.max_tokens, 100);
    }

    #[test]
    fn accepts_unknown_body_fields_silently() {
        // The body envelope is forward-compatible (see struct docs).
        // free-claude-code's reference impl accepts `mcp_servers`,
        // `context_management`, `output_config`, `extra_body`, and
        // future Anthropic-side additions. Rejecting unknown top-level
        // fields here would break the very clients this codec serves.
        let body = r#"{
            "model": "claude-sonnet-4-20250514",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 10,
            "mcp_servers": [],
            "context_management": {"edits": []},
            "output_config": {},
            "extra_body": {"future_field": 42},
            "totally_new_field": 42
        }"#;
        let req: AnthropicMessagesRequest =
            serde_json::from_str(body).expect("body envelope must accept unknown fields silently");
        req.validate().unwrap();
        assert_eq!(req.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn unknown_content_block_types_still_fail_loudly() {
        // Drift detection moves to per-`ContentBlock` `type` — an
        // unknown content_block kind is a hard parse error since the
        // codec's encoder branches on `pb::AgentEvent` ↔ block-type
        // mappings and a new type means structural translation work.
        let body = r#"{
            "model": "m",
            "max_tokens": 10,
            "messages": [{"role":"user","content":[
                {"type":"brand_new_block_type","whatever":1}
            ]}]
        }"#;
        let err = serde_json::from_str::<AnthropicMessagesRequest>(body)
            .expect_err("ContentBlock unknown variants must reject");
        assert!(
            err.to_string().contains("brand_new_block_type") || err.to_string().contains("variant"),
            "error should reference the unknown content_block variant: {err}"
        );
    }

    #[test]
    fn content_blocks_round_trip_with_cache_control() {
        // Anthropic prompt caching attaches `cache_control` markers to
        // any content block (and to system blocks + tools). The codec
        // must accept the marker on text / tool_use / tool_result /
        // thinking blocks without rejecting.
        let body = r#"{
            "model": "m",
            "max_tokens": 1,
            "messages": [
                {"role":"user","content":[
                    {"type":"text","text":"hi","cache_control":{"type":"ephemeral"}}
                ]},
                {"role":"assistant","content":[
                    {"type":"tool_use","id":"toolu_01","name":"f","input":{},"cache_control":{"type":"ephemeral"}}
                ]},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"toolu_01","content":"ok","cache_control":{"type":"ephemeral"}}
                ]},
                {"role":"assistant","content":[
                    {"type":"thinking","thinking":"reasoning","cache_control":{"type":"ephemeral"}}
                ]}
            ]
        }"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let blocks0 = req.messages[0].content.as_blocks();
        let ContentBlock::Text {
            cache_control: cc0, ..
        } = &blocks0[0]
        else {
            panic!("expected Text");
        };
        assert!(cc0.is_some(), "Text cache_control must round-trip");

        let blocks1 = req.messages[1].content.as_blocks();
        let ContentBlock::ToolUse {
            cache_control: cc1, ..
        } = &blocks1[0]
        else {
            panic!("expected ToolUse");
        };
        assert!(cc1.is_some(), "ToolUse cache_control must round-trip");

        let blocks2 = req.messages[2].content.as_blocks();
        let ContentBlock::ToolResult {
            cache_control: cc2, ..
        } = &blocks2[0]
        else {
            panic!("expected ToolResult");
        };
        assert!(cc2.is_some(), "ToolResult cache_control must round-trip");

        let blocks3 = req.messages[3].content.as_blocks();
        let ContentBlock::Thinking {
            cache_control: cc3, ..
        } = &blocks3[0]
        else {
            panic!("expected Thinking");
        };
        assert!(cc3.is_some(), "Thinking cache_control must round-trip");
    }

    #[test]
    fn message_content_supports_string_and_array_shapes() {
        let s = r#"{"role":"user","content":"hello"}"#;
        let m: Message = serde_json::from_str(s).unwrap();
        assert!(matches!(m.content, MessageContent::Text(_)));

        let a = r#"{"role":"user","content":[{"type":"text","text":"hello"}]}"#;
        let m: Message = serde_json::from_str(a).unwrap();
        assert!(matches!(m.content, MessageContent::Blocks(_)));
    }

    #[test]
    fn tool_use_blocks_parse_with_input_object() {
        let body = r#"{
            "model": "claude-sonnet-4-20250514",
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": "read foo"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "ok"},
                    {"type": "tool_use", "id": "toolu_01", "name": "read_file", "input": {"path": "foo.txt"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_01", "content": "hello world"}
                ]}
            ]
        }"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        req.validate().unwrap();
        assert_eq!(req.messages.len(), 3);
        // Second message has the tool_use block.
        let blocks = req.messages[1].content.as_blocks();
        assert!(matches!(blocks[1], ContentBlock::ToolUse { .. }));
    }

    #[test]
    fn validate_rejects_empty_messages() {
        let body = r#"{"model":"claude-sonnet-4-20250514","messages":[],"max_tokens":1}"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let err = req.validate().unwrap_err();
        assert!(matches!(err, CodecError::InvalidRequest(_)));
    }

    #[test]
    fn validate_rejects_zero_max_tokens() {
        let body = r#"{"model":"m","messages":[{"role":"user","content":"x"}],"max_tokens":0}"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let err = req.validate().unwrap_err();
        assert!(matches!(err, CodecError::InvalidRequest(_)));
    }

    #[test]
    fn validate_rejects_empty_model_string() {
        let body = r#"{"model":"   ","messages":[{"role":"user","content":"x"}],"max_tokens":1}"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let err = req.validate().unwrap_err();
        assert!(matches!(err, CodecError::InvalidRequest(_)));
    }

    #[test]
    fn validate_rejects_request_without_user_turn() {
        let body = r#"{
            "model": "m",
            "max_tokens": 1,
            "messages": [{"role":"assistant","content":"a"}]
        }"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let err = req.validate().unwrap_err();
        assert!(matches!(err, CodecError::NoUserMessage));
    }

    #[test]
    fn anthropic_version_accepts_canonical_value() {
        assert_eq!(
            AnthropicVersion::parse("2023-06-01").unwrap(),
            AnthropicVersion::V20230601
        );
        assert_eq!(AnthropicVersion::V20230601.as_wire_str(), "2023-06-01");
    }

    #[test]
    fn anthropic_version_accepts_legacy_value() {
        assert_eq!(
            AnthropicVersion::parse("2023-01-01").unwrap(),
            AnthropicVersion::V20230101
        );
    }

    #[test]
    fn anthropic_version_rejects_unknown() {
        let e = AnthropicVersion::parse("2099-12-31").unwrap_err();
        assert!(matches!(e, CodecError::UnsupportedAnthropicVersion(v) if v == "2099-12-31"));
    }

    #[test]
    fn anthropic_version_trims_whitespace_before_match() {
        assert!(AnthropicVersion::parse("  2023-06-01  ").is_ok());
    }

    #[test]
    fn thinking_config_round_trips() {
        let body = r#"{
            "model": "m",
            "max_tokens": 1,
            "messages": [{"role":"user","content":"x"}],
            "thinking": {"type":"enabled","budget_tokens":1024}
        }"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        assert!(matches!(
            req.thinking,
            Some(ThinkingConfig::Enabled {
                budget_tokens: 1024
            })
        ));
    }

    #[test]
    fn system_prompt_supports_both_shapes() {
        let s = r#"{"model":"m","max_tokens":1,"messages":[{"role":"user","content":"x"}],"system":"You are helpful."}"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(s).unwrap();
        assert!(matches!(req.system, Some(SystemPrompt::Text(_))));

        let a = r#"{"model":"m","max_tokens":1,"messages":[{"role":"user","content":"x"}],"system":[{"type":"text","text":"hi"}]}"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(a).unwrap();
        assert!(matches!(req.system, Some(SystemPrompt::Blocks(_))));
    }

    #[test]
    fn plain_text_concatenates_text_blocks_only() {
        let blocks = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "a".into(),
                cache_control: None,
            },
            ContentBlock::ToolUse {
                id: "id".into(),
                name: "t".into(),
                input: serde_json::json!({}),
                cache_control: None,
            },
            ContentBlock::Text {
                text: "b".into(),
                cache_control: None,
            },
        ]);
        assert_eq!(blocks.plain_text(), "a\nb");
    }

    #[test]
    fn first_user_message_skips_assistant_turns() {
        let body = r#"{
            "model": "m",
            "max_tokens": 1,
            "messages": [
                {"role":"assistant","content":"prior"},
                {"role":"user","content":"my real first user msg"}
            ]
        }"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let m = req.first_user_message().unwrap();
        assert_eq!(m.content.plain_text(), "my real first user msg");
    }
}
