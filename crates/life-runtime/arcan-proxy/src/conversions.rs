//! From/Into conversions between aios-protocol Rust types and the wire
//! types the proxy round-trips. Hand-written until arcan-proto ships.

use aios_proto::aios::v1 as aios_v1;
use arcan_substrate_proto::arcan::v1 as arcan_pb;
use life_runtime_proto::life::v1 as life_pb;

/// Stateful translator mapping substrate-plane `arcan.v1.AgentEvent`s
/// onto public-plane `life.v1.AgentEvent`s for one dispatch stream.
///
/// Mirrors the record idiom of the HTTP-backed backends
/// (`vercel_ai_gateway.rs` / `anthropic.rs`): every emitted event
/// carries an `EventRecord` with the session id, a stream-local
/// monotonic `sequence`, a wall-clock timestamp, an UPPERCASE kind
/// tag, and a structured JSON payload — the shape lifegw's
/// `event_to_outbound_frame` forwards to the browser, which reads
/// `payload.text` / `payload.call_id` / etc.
///
/// Phase 2 (harness arc): the tool lifecycle passes through —
/// `TOOL_CALL_PENDING` / `TOOL_RESULT` payloads are decoded from the
/// substrate's `payload_json` and inlined as the record payload. This
/// also closes the Phase-1 gap where TOKEN text was dropped at this
/// boundary (`record` was always `None`).
pub struct SubstrateEventTranslator {
    session_id: aios_v1::SessionId,
    sequence: u64,
}

impl SubstrateEventTranslator {
    pub fn new(sid: impl Into<String>) -> Self {
        Self {
            session_id: aios_v1::SessionId { value: sid.into() },
            sequence: 0,
        }
    }

    /// Translate one substrate event. Frames carrying the kernel's
    /// durable per-session sequence (`evt.sequence > 0`) use it
    /// verbatim — keeping real monotonic cursors intact for the WS
    /// resume contract; synthesized frames (sequence 0, e.g. the
    /// fallback FINISH) continue monotonically from the last seen
    /// value.
    pub fn translate(&mut self, evt: arcan_pb::AgentEvent) -> life_pb::AgentEvent {
        use arcan_pb::AgentEventKind as Sub;
        use life_pb::AgentEventKind as Pub;
        self.sequence = if evt.sequence > 0 {
            evt.sequence
        } else {
            self.sequence + 1
        };
        let (kind, kind_tag, payload) = match Sub::try_from(evt.kind).unwrap_or(Sub::Unspecified) {
            Sub::Token => (Pub::Token, "TOKEN", serde_json::json!({ "text": evt.text })),
            Sub::ToolCallPending => (
                Pub::ToolCallPending,
                "TOOL_CALL_PENDING",
                decode_payload(&evt.payload_json),
            ),
            Sub::ToolResult => (
                Pub::ToolResult,
                "TOOL_RESULT",
                decode_payload(&evt.payload_json),
            ),
            Sub::Finish => (Pub::Finish, "FINISH", serde_json::json!({})),
            Sub::Error => (
                Pub::Error,
                "ERROR",
                serde_json::json!({ "error": evt.error }),
            ),
            // Unknown / future substrate kinds: preserve BOTH the text
            // and any structured payload under the UNSPECIFIED tag
            // rather than guessing a semantic for them.
            Sub::Unspecified => (
                Pub::Unspecified,
                "UNSPECIFIED",
                serde_json::json!({
                    "text": evt.text,
                    "payload": decode_payload(&evt.payload_json),
                }),
            ),
        };
        life_pb::AgentEvent {
            record: Some(life_pb::EventRecord {
                session_id: Some(self.session_id.clone()),
                sequence: self.sequence,
                at: now_timestamp(),
                kind: kind_tag.to_string(),
                payload: serde_json::to_vec(&payload).unwrap_or_default(),
            }),
            kind: kind as i32,
        }
    }
}

/// Decode a substrate `payload_json` into a JSON object. Malformed or
/// non-object payloads are wrapped so the record payload is always an
/// object — the browser contract per lifegw's
/// `event_to_outbound_frame` (it inlines objects and wraps everything
/// else).
fn decode_payload(payload_json: &str) -> serde_json::Value {
    if payload_json.is_empty() {
        return serde_json::json!({});
    }
    match serde_json::from_str::<serde_json::Value>(payload_json) {
        Ok(v) if v.is_object() => v,
        Ok(other) => serde_json::json!({ "value": other }),
        Err(_) => serde_json::json!({ "raw": payload_json }),
    }
}

/// Wall-clock timestamp for proxy-built `EventRecord`s. Shared by the
/// substrate translator and the HTTP-backed backends.
pub(crate) fn now_timestamp() -> Option<prost_types::Timestamp> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(prost_types::Timestamp {
        seconds: now.as_secs() as i64,
        nanos: now.subsec_nanos() as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub_event(
        kind: arcan_pb::AgentEventKind,
        text: &str,
        error: &str,
        payload: &str,
    ) -> arcan_pb::AgentEvent {
        arcan_pb::AgentEvent {
            kind: kind as i32,
            text: text.to_string(),
            error: error.to_string(),
            payload_json: payload.to_string(),
            sequence: 0,
        }
    }

    fn record_payload(evt: &life_pb::AgentEvent) -> serde_json::Value {
        let record = evt.record.as_ref().expect("record present");
        serde_json::from_slice(&record.payload).expect("payload is JSON")
    }

    #[test]
    fn token_text_flows_into_record_payload() {
        let mut tr = SubstrateEventTranslator::new("sid-1");
        let out = tr.translate(sub_event(arcan_pb::AgentEventKind::Token, "hello", "", ""));
        assert_eq!(out.kind(), life_pb::AgentEventKind::Token);
        let record = out.record.as_ref().expect("record present");
        assert_eq!(record.kind, "TOKEN");
        assert_eq!(
            record.session_id.as_ref().map(|s| s.value.as_str()),
            Some("sid-1")
        );
        assert_eq!(record_payload(&out)["text"], "hello");
    }

    #[test]
    fn tool_call_pending_payload_passes_through() {
        let mut tr = SubstrateEventTranslator::new("sid-1");
        let payload = r#"{"call_id":"c1","tool_name":"fs.read","arguments":{"path":"/x"}}"#;
        let out = tr.translate(sub_event(
            arcan_pb::AgentEventKind::ToolCallPending,
            "",
            "",
            payload,
        ));
        assert_eq!(out.kind(), life_pb::AgentEventKind::ToolCallPending);
        let record = out.record.as_ref().expect("record present");
        assert_eq!(record.kind, "TOOL_CALL_PENDING");
        let p = record_payload(&out);
        assert_eq!(p["call_id"], "c1");
        assert_eq!(p["tool_name"], "fs.read");
        assert_eq!(p["arguments"]["path"], "/x");
    }

    #[test]
    fn tool_result_payload_passes_through() {
        let mut tr = SubstrateEventTranslator::new("sid-1");
        let payload =
            r#"{"call_id":"c1","tool_name":"fs.read","result":{"ok":true},"status":"ok"}"#;
        let out = tr.translate(sub_event(
            arcan_pb::AgentEventKind::ToolResult,
            "",
            "",
            payload,
        ));
        assert_eq!(out.kind(), life_pb::AgentEventKind::ToolResult);
        let p = record_payload(&out);
        assert_eq!(p["status"], "ok");
        assert_eq!(p["result"]["ok"], true);
    }

    #[test]
    fn malformed_tool_payload_is_wrapped_not_dropped() {
        let mut tr = SubstrateEventTranslator::new("sid-1");
        let out = tr.translate(sub_event(
            arcan_pb::AgentEventKind::ToolResult,
            "",
            "",
            "not json",
        ));
        assert_eq!(record_payload(&out)["raw"], "not json");
    }

    #[test]
    fn finish_and_error_map_to_terminal_kinds() {
        let mut tr = SubstrateEventTranslator::new("sid-1");
        let finish = tr.translate(sub_event(arcan_pb::AgentEventKind::Finish, "", "", ""));
        assert_eq!(finish.kind(), life_pb::AgentEventKind::Finish);
        assert_eq!(finish.record.as_ref().unwrap().kind, "FINISH");
        let error = tr.translate(sub_event(
            arcan_pb::AgentEventKind::Error,
            "",
            "tick failed",
            "",
        ));
        assert_eq!(error.kind(), life_pb::AgentEventKind::Error);
        assert_eq!(record_payload(&error)["error"], "tick failed");
    }

    #[test]
    fn sequence_is_monotonic_per_stream() {
        let mut tr = SubstrateEventTranslator::new("sid-1");
        let a = tr.translate(sub_event(arcan_pb::AgentEventKind::Token, "a", "", ""));
        let b = tr.translate(sub_event(arcan_pb::AgentEventKind::Token, "b", "", ""));
        assert_eq!(a.record.unwrap().sequence, 1);
        assert_eq!(b.record.unwrap().sequence, 2);
    }

    #[test]
    fn kernel_sequence_wins_and_synthesized_frames_continue_from_it() {
        let mut tr = SubstrateEventTranslator::new("sid-1");
        let mut with_seq = sub_event(arcan_pb::AgentEventKind::Token, "a", "", "");
        with_seq.sequence = 41;
        let a = tr.translate(with_seq);
        assert_eq!(
            a.record.unwrap().sequence,
            41,
            "kernel-assigned sequence passes through verbatim"
        );
        // A synthesized frame (sequence 0) continues monotonically.
        let b = tr.translate(sub_event(arcan_pb::AgentEventKind::Finish, "", "", ""));
        assert_eq!(b.record.unwrap().sequence, 42);
    }

    #[test]
    fn unknown_kind_retains_text_and_payload() {
        let mut tr = SubstrateEventTranslator::new("sid-1");
        let mut evt = sub_event(arcan_pb::AgentEventKind::Unspecified, "t", "", r#"{"k":1}"#);
        evt.kind = 99; // future kind unknown to this build
        let out = tr.translate(evt);
        assert_eq!(out.kind(), life_pb::AgentEventKind::Unspecified);
        let p = record_payload(&out);
        assert_eq!(p["text"], "t");
        assert_eq!(p["payload"]["k"], 1);
    }
}
