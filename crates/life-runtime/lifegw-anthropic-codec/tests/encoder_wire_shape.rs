//! Cross-module integration tests for the codec.
//!
//! Each test drives an [`Encoder`] through a realistic `pb::AgentEvent`
//! sequence and checks the produced [`AnthropicSseEvent`] list passes
//! [`assert_wire_shape`]. This is the closest analog of free-claude-code's
//! `tests/core/anthropic/test_native_sse_block_policy.py` battery — it
//! tests behaviour at the *encoded-stream* layer rather than at the
//! state-machine layer.

use lifegw_anthropic_codec::contracts::assert_wire_shape;
use lifegw_anthropic_codec::encoder::{AnthropicSseEvent, ContentBlockInit};
use lifegw_anthropic_codec::{
    AnthropicError, AnthropicErrorKind, AnthropicMessagesRequest, Encoder,
    canonicalize_first_user_message, synthesize_sid,
};

use life_runtime_proto::life::v1::{AgentEvent, AgentEventKind, EventRecord};

fn token(seq: u64, text: &str) -> AgentEvent {
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

fn thinking_token(seq: u64, thinking: &str) -> AgentEvent {
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

fn tool_call(seq: u64, payload: serde_json::Value) -> AgentEvent {
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

fn finish(seq: u64, reason: &str) -> AgentEvent {
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

fn drive(events: &[AgentEvent]) -> Vec<AnthropicSseEvent> {
    let mut e = Encoder::new("msg_test", "claude-sonnet-4-20250514");
    let mut out = Vec::new();
    for evt in events {
        out.extend(e.encode(evt).unwrap());
    }
    out
}

#[test]
fn simple_text_stream_round_trips() {
    let out = drive(&[
        token(1, "Hello"),
        token(2, " "),
        token(3, "world"),
        finish(4, "stop"),
    ]);
    assert_wire_shape(&out);
    // All text content collapses to a single block.
    let text_starts: Vec<_> = out
        .iter()
        .filter(|e| {
            matches!(
                e,
                AnthropicSseEvent::ContentBlockStart(p)
                    if matches!(p.content_block, ContentBlockInit::Text { .. })
            )
        })
        .collect();
    assert_eq!(text_starts.len(), 1);
}

#[test]
fn empty_response_emits_only_envelope() {
    let out = drive(&[finish(1, "stop")]);
    assert_wire_shape(&out);
    // No content blocks for an empty assistant turn.
    let has_content_block = out
        .iter()
        .any(|e| matches!(e, AnthropicSseEvent::ContentBlockStart(_)));
    assert!(!has_content_block);
}

#[test]
fn thinking_then_text_emits_two_blocks_in_order() {
    let out = drive(&[
        thinking_token(1, "Let me consider..."),
        thinking_token(2, " yes, I'll respond."),
        token(3, "Hello."),
        finish(4, "stop"),
    ]);
    assert_wire_shape(&out);
    // First content block is thinking, second is text.
    let starts: Vec<_> = out
        .iter()
        .filter_map(|e| match e {
            AnthropicSseEvent::ContentBlockStart(p) => Some(&p.content_block),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 2);
    assert!(matches!(starts[0], ContentBlockInit::Thinking { .. }));
    assert!(matches!(starts[1], ContentBlockInit::Text { .. }));
}

#[test]
fn tool_use_round_trip_with_streamed_input_json() {
    let out = drive(&[
        token(1, "I'll read it."),
        tool_call(
            2,
            serde_json::json!({"id":"toolu_01","name":"read_file","input":{}}),
        ),
        tool_call(
            3,
            serde_json::json!({"id":"toolu_01","name":"read_file","partial_json":"{\"path\":"}),
        ),
        tool_call(
            4,
            serde_json::json!({"id":"toolu_01","name":"read_file","partial_json":" \"foo.txt\"}", "done": true}),
        ),
        finish(5, "tool_use"),
    ]);
    assert_wire_shape(&out);
    // The message_delta reflects tool_use stop_reason.
    let md = out
        .iter()
        .find_map(|e| match e {
            AnthropicSseEvent::MessageDelta(p) => Some(p),
            _ => None,
        })
        .unwrap();
    assert_eq!(md.delta.stop_reason, "tool_use");
}

#[test]
fn multi_tool_simultaneous_in_one_response() {
    let out = drive(&[
        tool_call(
            1,
            serde_json::json!({"id":"toolu_A","name":"a","partial_json":"{}", "done": true}),
        ),
        tool_call(
            2,
            serde_json::json!({"id":"toolu_B","name":"b","partial_json":"{}", "done": true}),
        ),
        finish(3, "tool_use"),
    ]);
    assert_wire_shape(&out);
    // Two distinct tool_use content blocks.
    let tools: Vec<_> = out
        .iter()
        .filter_map(|e| match e {
            AnthropicSseEvent::ContentBlockStart(p) => match &p.content_block {
                ContentBlockInit::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(tools, vec!["toolu_A".to_string(), "toolu_B".to_string()]);
}

#[test]
fn upstream_error_emits_inline_error_then_stops() {
    let out = drive(&[
        token(1, "Hello"),
        AgentEvent {
            record: Some(EventRecord {
                session_id: None,
                sequence: 2,
                at: None,
                kind: "ERROR".into(),
                payload: serde_json::to_vec(&serde_json::json!({
                    "kind": "overloaded_error",
                    "message": "Backend overloaded",
                }))
                .unwrap(),
            }),
            kind: AgentEventKind::Error as i32,
        },
    ]);
    assert_wire_shape(&out);
    // The in-band error event with `overloaded_error` is in the stream.
    let has_err = out.iter().any(|e| {
        matches!(
            e,
            AnthropicSseEvent::Error(p) if p.error.kind == "overloaded_error"
        )
    });
    assert!(has_err);
}

#[test]
fn ping_can_be_emitted_alongside_lifecycle() {
    // Production callers inject heartbeats themselves; the codec only
    // exposes the helper. Verify wire shape is unchanged by pings.
    let mut e = Encoder::new("msg_test", "m");
    let mut out = Vec::new();
    out.extend(e.encode(&token(1, "Hi")).unwrap());
    out.push(Encoder::ping());
    out.push(Encoder::ping());
    out.extend(e.encode(&finish(2, "stop")).unwrap());
    assert_wire_shape(&out);
}

#[test]
fn sse_frame_renders_canonical_anthropic_wire_shape() {
    let out = drive(&[token(1, "hi"), finish(2, "stop")]);
    let wire: String = out.iter().map(|e| e.to_sse_frame()).collect();
    // Verify the canonical "event:\ndata:\n\n" framing.
    assert!(wire.contains("event: message_start\ndata: "));
    assert!(wire.contains("event: content_block_start\ndata: "));
    assert!(wire.contains("event: content_block_delta\ndata: "));
    assert!(wire.contains("event: content_block_stop\ndata: "));
    assert!(wire.contains("event: message_delta\ndata: "));
    assert!(wire.ends_with("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
}

#[test]
fn sid_synthesizer_round_trips_through_full_request_parse() {
    let body = r#"{
        "model": "claude-sonnet-4-20250514",
        "max_tokens": 100,
        "messages": [
            {"role": "user", "content": "read foo.txt"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "ok"},
                {"type": "tool_use", "id": "toolu_01", "name": "read_file", "input": {"path": "foo.txt"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_01", "content": "hello"}
            ]}
        ]
    }"#;
    let req: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
    let sid = synthesize_sid(&req, "did:life:abc").unwrap();
    assert!(sid.starts_with("claude-code:"));
    // Re-parsing the same body produces the same sid.
    let req2: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
    let sid2 = synthesize_sid(&req2, "did:life:abc").unwrap();
    assert_eq!(sid, sid2);
    // Canonicalization works.
    let canon = canonicalize_first_user_message(&req.messages[0].content);
    assert_eq!(canon, "read foo.txt");
}

#[test]
fn anthropic_error_helper_renders_top_level_event() {
    let err = AnthropicError::new(AnthropicErrorKind::BillingError, "Insufficient credits");
    let frame = err.to_sse_frame();
    assert!(frame.starts_with("event: error\n"));
    assert!(frame.contains("\"billing_error\""));
    assert!(frame.contains("Insufficient credits"));
}
