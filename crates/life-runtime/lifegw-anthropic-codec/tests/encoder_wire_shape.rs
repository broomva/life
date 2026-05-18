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

// ─── J-Sub-D (BRO-1143) tool-use bridge integration tests ───────────
//
// Six tests exercise the full SSE wire-shape contract for the tool-use
// round-trip: single-tool, two-round resume, multi-tool simultaneous,
// streamed partial-JSON input, tool_use after a thinking block, and
// error mid-tool_use. Together they assert the encoder emits the
// `content_block_start → input_json_delta+ → content_block_stop`
// sequence per Spec J §[Tool use] and finalizes with
// `message_delta {stop_reason: "tool_use"}` + `message_stop`.

/// J-Sub-D: one tool_use round produces the canonical
/// content_block_start (with tool_use init) → input_json_delta
/// (the streamed JSON) → content_block_stop sequence, terminated by
/// message_delta {stop_reason: "tool_use"} + message_stop.
#[test]
fn tool_use_single_round_emits_correct_sse() {
    let out = drive(&[
        token(1, "I'll read it."),
        tool_call(
            2,
            serde_json::json!({"id":"toolu_01","name":"read_file","input":{}}),
        ),
        tool_call(
            3,
            serde_json::json!({"id":"toolu_01","name":"read_file","partial_json":"{\"path\": \"foo.txt\"}", "done": true}),
        ),
        finish(4, "tool_use"),
    ]);
    assert_wire_shape(&out);

    // 1) tool_use ContentBlockStart carries id + name.
    let tool_start = out
        .iter()
        .find_map(|e| match e {
            AnthropicSseEvent::ContentBlockStart(p) => match &p.content_block {
                ContentBlockInit::ToolUse { id, name, input } => {
                    Some((p.index, id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            },
            _ => None,
        })
        .expect("tool_use content_block_start must be emitted");
    assert_eq!(tool_start.1, "toolu_01");
    assert_eq!(tool_start.2, "read_file");
    // input is `{}` at start — the JSON streams via deltas.
    assert_eq!(tool_start.3, serde_json::json!({}));

    // 2) The accumulated partial_json fragments concatenate to valid JSON.
    let fragments: Vec<String> = out
        .iter()
        .filter_map(|e| match e {
            AnthropicSseEvent::ContentBlockDelta(p) => match &p.delta {
                lifegw_anthropic_codec::encoder::BlockDelta::InputJsonDelta { partial_json } => {
                    Some(partial_json.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    let concatenated = fragments.concat();
    let parsed: serde_json::Value =
        serde_json::from_str(&concatenated).expect("concatenated partial_json must be valid JSON");
    assert_eq!(parsed, serde_json::json!({"path": "foo.txt"}));

    // 3) message_delta carries stop_reason = "tool_use".
    let md = out
        .iter()
        .find_map(|e| match e {
            AnthropicSseEvent::MessageDelta(p) => Some(p),
            _ => None,
        })
        .expect("message_delta is emitted");
    assert_eq!(md.delta.stop_reason, "tool_use");
}

/// J-Sub-D: two-rounds. The first request emits tool_use; the second
/// request (carrying tool_result in messages[-1]) re-derives the same
/// sid and continues the assistant turn against the resumed session.
///
/// The codec layer doesn't own the sid resolution or the resume — that
/// lives in the lifegw handler (J-Sub-G E2E). What the codec MUST
/// guarantee is that the same `(did, canon)` pair synthesizes the same
/// sid across both requests so the substrate sees one continuous
/// session.
#[test]
fn tool_use_two_rounds_resumes_session() {
    use lifegw_anthropic_codec::canonicalize_first_user_message;

    // Round 1: user("read foo.txt") → assistant emits tool_use.
    let round_1_body = r#"{
        "model": "claude-sonnet-4-20250514",
        "max_tokens": 100,
        "messages": [{"role": "user", "content": "read foo.txt"}]
    }"#;
    let round_1: AnthropicMessagesRequest = serde_json::from_str(round_1_body).unwrap();
    let sid_1 = synthesize_sid(&round_1, "did:life:abc").unwrap();
    let canon_1 = canonicalize_first_user_message(&round_1.messages[0].content);

    // Round 2: same user + assistant tool_use + user tool_result.
    let round_2_body = r#"{
        "model": "claude-sonnet-4-20250514",
        "max_tokens": 100,
        "messages": [
            {"role": "user", "content": "read foo.txt"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "I'll read it."},
                {"type": "tool_use", "id": "toolu_01", "name": "read_file", "input": {"path": "foo.txt"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_01", "content": "hello world"}
            ]}
        ]
    }"#;
    let round_2: AnthropicMessagesRequest = serde_json::from_str(round_2_body).unwrap();
    let sid_2 = synthesize_sid(&round_2, "did:life:abc").unwrap();
    let canon_2 = canonicalize_first_user_message(&round_2.messages[0].content);

    // Sid stability: same DID + canon first user message → same sid.
    assert_eq!(canon_1, canon_2, "canonical first user message must match");
    assert_eq!(sid_1, sid_2, "sid must be stable across the round-trip");

    // Round 2's encoder reads back the same encoder state via lago tail
    // (production path); here we simulate by running fresh encoders for
    // each round and checking each independently produces wire-valid
    // SSE.
    let out_1 = drive(&[
        token(1, "I'll read it."),
        tool_call(
            2,
            serde_json::json!({"id":"toolu_01","name":"read_file","partial_json":"{\"path\":\"foo.txt\"}", "done": true}),
        ),
        finish(3, "tool_use"),
    ]);
    assert_wire_shape(&out_1);
    let out_2 = drive(&[
        token(1, "The file says: hello world"),
        finish(2, "end_turn"),
    ]);
    assert_wire_shape(&out_2);

    // Round 1 finalizes with stop_reason=tool_use; round 2 with end_turn.
    let r1_reason = out_1
        .iter()
        .find_map(|e| match e {
            AnthropicSseEvent::MessageDelta(p) => Some(p.delta.stop_reason.clone()),
            _ => None,
        })
        .unwrap();
    let r2_reason = out_2
        .iter()
        .find_map(|e| match e {
            AnthropicSseEvent::MessageDelta(p) => Some(p.delta.stop_reason.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(r1_reason, "tool_use");
    assert_eq!(r2_reason, "end_turn");
}

/// J-Sub-D: two parallel tool_use blocks open in the same response.
/// Anthropic's protocol allows this; the codec must allocate distinct
/// content-block indices and emit input_json_delta for each that
/// reference the correct id.
#[test]
fn tool_use_multi_tool_simultaneous() {
    let out = drive(&[
        tool_call(1, serde_json::json!({"id":"toolu_A","name":"a","input":{}})),
        tool_call(2, serde_json::json!({"id":"toolu_B","name":"b","input":{}})),
        tool_call(
            3,
            serde_json::json!({"id":"toolu_A","name":"a","partial_json":"{\"x\":1}", "done": true}),
        ),
        tool_call(
            4,
            serde_json::json!({"id":"toolu_B","name":"b","partial_json":"{\"y\":2}", "done": true}),
        ),
        finish(5, "tool_use"),
    ]);
    assert_wire_shape(&out);

    // Two distinct ContentBlockStart frames for tool_use, with distinct
    // indices.
    let tool_starts: Vec<_> = out
        .iter()
        .filter_map(|e| match e {
            AnthropicSseEvent::ContentBlockStart(p) => match &p.content_block {
                ContentBlockInit::ToolUse { id, .. } => Some((p.index, id.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(tool_starts.len(), 2);
    assert_ne!(
        tool_starts[0].0, tool_starts[1].0,
        "two tool_use blocks must have distinct indices, got {tool_starts:?}"
    );
    let ids: Vec<_> = tool_starts.iter().map(|(_, id)| id.clone()).collect();
    assert!(ids.contains(&"toolu_A".to_string()));
    assert!(ids.contains(&"toolu_B".to_string()));
}

/// J-Sub-D: input JSON arrives across many small partial_json chunks;
/// the concatenation must produce valid JSON and each chunk emits a
/// distinct input_json_delta with the correct (matching) block index.
#[test]
fn tool_use_with_partial_json() {
    let chunks = ["{\"path\":", " \"foo", ".txt\",", " \"mode\":", " \"r\"}"];
    let mut events = vec![tool_call(
        1,
        serde_json::json!({"id":"toolu_01","name":"read_file","input":{}}),
    )];
    for (i, c) in chunks.iter().enumerate() {
        let seq = (i + 2) as u64;
        let done = i == chunks.len() - 1;
        events.push(tool_call(
            seq,
            serde_json::json!({"id":"toolu_01","name":"read_file","partial_json": c, "done": done}),
        ));
    }
    events.push(finish((events.len() + 1) as u64, "tool_use"));

    let out = drive(&events);
    assert_wire_shape(&out);

    // Each chunk produces an input_json_delta.
    let fragments: Vec<String> = out
        .iter()
        .filter_map(|e| match e {
            AnthropicSseEvent::ContentBlockDelta(p) => match &p.delta {
                lifegw_anthropic_codec::encoder::BlockDelta::InputJsonDelta { partial_json } => {
                    Some(partial_json.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        fragments.len(),
        chunks.len(),
        "one input_json_delta per chunk; got {fragments:?}"
    );
    let concatenated = fragments.concat();
    let parsed: serde_json::Value =
        serde_json::from_str(&concatenated).expect("concatenated JSON must be valid");
    assert_eq!(parsed, serde_json::json!({"path": "foo.txt", "mode": "r"}));
}

/// J-Sub-D: a thinking block precedes a tool_use block. The encoder
/// must close the thinking block (content_block_stop) before opening
/// the tool_use block. The Anthropic protocol forbids overlapping
/// blocks within a message.
#[test]
fn tool_use_after_thinking() {
    let out = drive(&[
        thinking_token(1, "Let me consider..."),
        thinking_token(2, " I should read the file."),
        tool_call(
            3,
            serde_json::json!({"id":"toolu_01","name":"read_file","partial_json":"{\"path\":\"foo.txt\"}", "done": true}),
        ),
        finish(4, "tool_use"),
    ]);
    assert_wire_shape(&out);

    // The thinking block opens first (index 0), then closes BEFORE
    // tool_use opens (index 1).
    let starts: Vec<(u32, &ContentBlockInit)> = out
        .iter()
        .filter_map(|e| match e {
            AnthropicSseEvent::ContentBlockStart(p) => Some((p.index, &p.content_block)),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 2);
    assert!(matches!(starts[0].1, ContentBlockInit::Thinking { .. }));
    assert!(matches!(starts[1].1, ContentBlockInit::ToolUse { .. }));
    assert_ne!(starts[0].0, starts[1].0);

    // Find positions: thinking stop must come before tool_use start.
    let thinking_index = starts[0].0;
    let tool_index = starts[1].0;
    let thinking_stop_pos = out
        .iter()
        .position(|e| {
            matches!(
                e,
                AnthropicSseEvent::ContentBlockStop(p) if p.index == thinking_index
            )
        })
        .expect("thinking block must be closed");
    let tool_start_pos = out
        .iter()
        .position(|e| {
            matches!(
                e,
                AnthropicSseEvent::ContentBlockStart(p) if p.index == tool_index
            )
        })
        .expect("tool_use must be opened");
    assert!(
        thinking_stop_pos < tool_start_pos,
        "thinking block must close before tool_use opens (got thinking_stop={thinking_stop_pos}, tool_start={tool_start_pos})"
    );

    // message_delta carries stop_reason=tool_use.
    let md = out
        .iter()
        .find_map(|e| match e {
            AnthropicSseEvent::MessageDelta(p) => Some(p),
            _ => None,
        })
        .unwrap();
    assert_eq!(md.delta.stop_reason, "tool_use");
}

/// J-Sub-D: upstream errors mid-tool_use must close the stream cleanly
/// — emit the inline error event AND finalize (close blocks + emit
/// message_delta + message_stop). No panic; no malformed wire.
#[test]
fn tool_use_with_error() {
    // Open a tool_use block, then hit an upstream Error mid-stream.
    let out = drive(&[
        tool_call(
            1,
            serde_json::json!({"id":"toolu_01","name":"read_file","input":{}}),
        ),
        tool_call(
            2,
            serde_json::json!({"id":"toolu_01","name":"read_file","partial_json":"{\"pa"}),
        ),
        AgentEvent {
            record: Some(EventRecord {
                session_id: None,
                sequence: 3,
                at: None,
                kind: "ERROR".into(),
                payload: serde_json::to_vec(&serde_json::json!({
                    "kind": "overloaded_error",
                    "message": "upstream overloaded mid-tool_use",
                }))
                .unwrap(),
            }),
            kind: AgentEventKind::Error as i32,
        },
    ]);
    assert_wire_shape(&out);

    // Inline error event is emitted.
    let has_error = out
        .iter()
        .any(|e| matches!(e, AnthropicSseEvent::Error(p) if p.error.kind == "overloaded_error"));
    assert!(has_error, "inline error event must be present");

    // Stream closes cleanly: message_stop is the last event.
    assert!(
        matches!(out.last(), Some(AnthropicSseEvent::MessageStop)),
        "stream must close cleanly with message_stop"
    );

    // The mid-stream tool_use block must be closed before message_stop —
    // the wire-shape contract enforces this; assert_wire_shape above
    // would have failed if a tool_use block were left dangling. Verify
    // explicitly:
    let tool_use_index = out
        .iter()
        .find_map(|e| match e {
            AnthropicSseEvent::ContentBlockStart(p) => match &p.content_block {
                ContentBlockInit::ToolUse { .. } => Some(p.index),
                _ => None,
            },
            _ => None,
        })
        .expect("tool_use must have opened");
    let stop_count = out
        .iter()
        .filter(|e| {
            matches!(
                e,
                AnthropicSseEvent::ContentBlockStop(p) if p.index == tool_use_index
            )
        })
        .count();
    assert_eq!(
        stop_count, 1,
        "tool_use block must be closed exactly once (got {stop_count} stops for index {tool_use_index})"
    );
}
