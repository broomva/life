//! Claude Code output parser — extracts structured events from JSONL output.
#![allow(dead_code)]
//!
//! Claude Code with `--output-format stream-json` emits newline-delimited JSON.
//! Each line is one event. Unknown or non-JSON lines fall back to `Raw`.

use serde::Deserialize;
use uuid::Uuid;

/// A structured event extracted from Claude Code JSONL output.
#[derive(Debug, Clone)]
pub enum ClaudeEvent {
    /// System initialization (emitted at session start).
    SystemInit {
        session_id: Option<String>,
        model: Option<String>,
        cwd: Option<String>,
    },
    /// Text from the assistant.
    AssistantText { text: String },
    /// Tool invocation request.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool execution result.
    ToolResult {
        tool_use_id: Option<String>,
        content: String,
        is_error: bool,
    },
    /// Claude Code is requesting permission for a capability.
    ApprovalRequest {
        approval_id: String,
        capability: String,
        context: String,
    },
    /// Session result (cost, duration — emitted at completion).
    Result {
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
    },
    /// Raw terminal output (ANSI sequences, plain text, or unrecognised JSON).
    Raw(String),
}

/// Internal DTO for deserialising the top-level event envelope.
#[derive(Deserialize)]
struct RawEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

/// Parse one line of Claude Code output into a [`ClaudeEvent`].
///
/// Non-JSON lines and unknown event types map to [`ClaudeEvent::Raw`].
pub fn parse_line(line: &str) -> ClaudeEvent {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ClaudeEvent::Raw(String::new());
    }

    let Ok(raw): Result<RawEvent, _> = serde_json::from_str(trimmed) else {
        return ClaudeEvent::Raw(line.to_string());
    };

    match raw.event_type.as_str() {
        "system" => ClaudeEvent::SystemInit {
            session_id: raw
                .rest
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            model: raw
                .rest
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            cwd: raw
                .rest
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        },

        "assistant" => {
            let text = raw
                .rest
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .and_then(|b| b.get("text"))
                        .and_then(|t| t.as_str())
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            ClaudeEvent::AssistantText { text }
        }

        "tool_use" => ClaudeEvent::ToolUse {
            id: raw
                .rest
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            name: raw
                .rest
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            input: raw
                .rest
                .get("input")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        },

        "tool_result" => {
            let content = raw
                .rest
                .get("content")
                .and_then(|c| c.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .and_then(|b| b.get("text"))
                        .and_then(|t| t.as_str())
                        .map(str::to_owned)
                })
                .or_else(|| {
                    raw.rest
                        .get("content")
                        .and_then(|c| c.as_str())
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            ClaudeEvent::ToolResult {
                tool_use_id: raw
                    .rest
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                content,
                is_error: raw
                    .rest
                    .get("is_error")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            }
        }

        "approval_request" => ClaudeEvent::ApprovalRequest {
            approval_id: raw
                .rest
                .get("approval_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            capability: raw
                .rest
                .get("capability")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            context: raw
                .rest
                .get("context")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        },

        "result" => ClaudeEvent::Result {
            cost_usd: raw.rest.get("cost_usd").and_then(serde_json::Value::as_f64),
            duration_ms: raw
                .rest
                .get("duration_ms")
                .and_then(serde_json::Value::as_u64),
        },

        _ => ClaudeEvent::Raw(line.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_system_event() {
        let line = r#"{"type":"system","subtype":"init","cwd":"/home/user","model":"claude-opus-4-5","session_id":"sess-123"}"#;
        let event = parse_line(line);
        let ClaudeEvent::SystemInit {
            model, session_id, ..
        } = event
        else {
            panic!("expected SystemInit");
        };
        assert_eq!(model.as_deref(), Some("claude-opus-4-5"));
        assert_eq!(session_id.as_deref(), Some("sess-123"));
    }

    #[test]
    fn parse_tool_use_event() {
        let line = r#"{"type":"tool_use","id":"tu_123","name":"Bash","input":{"command":"ls"}}"#;
        let ClaudeEvent::ToolUse { name, id, .. } = parse_line(line) else {
            panic!("expected ToolUse");
        };
        assert_eq!(name, "Bash");
        assert_eq!(id, "tu_123");
    }

    #[test]
    fn parse_result_event() {
        let line = r#"{"type":"result","cost_usd":0.0012,"duration_ms":3400}"#;
        let ClaudeEvent::Result {
            cost_usd,
            duration_ms,
        } = parse_line(line)
        else {
            panic!("expected Result");
        };
        assert!(cost_usd.is_some());
        assert_eq!(duration_ms, Some(3400));
    }

    #[test]
    fn parse_ansi_raw_fallback() {
        let line = "\x1b[1;32mHello\x1b[0m";
        assert!(matches!(parse_line(line), ClaudeEvent::Raw(_)));
    }

    #[test]
    fn parse_unknown_json_falls_back_to_raw() {
        let line = r#"{"type":"future_type","data":"x"}"#;
        assert!(matches!(parse_line(line), ClaudeEvent::Raw(_)));
    }

    #[test]
    fn parse_empty_line() {
        assert!(matches!(parse_line(""), ClaudeEvent::Raw(s) if s.is_empty()));
    }
}
