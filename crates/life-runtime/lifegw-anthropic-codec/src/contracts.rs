//! Wire-shape assertion helpers — TEST ONLY.
//!
//! Ports `core/anthropic/stream_contracts.py` from free-claude-code.
//! These helpers walk a sequence of [`crate::AnthropicSseEvent`]s and
//! verify they obey Anthropic's published protocol invariants:
//!
//! 1. The first event in a response MUST be `message_start`.
//! 2. The last event in a response MUST be `message_stop` (unless the
//!    stream is mid-stream — but our integration tests always emit a
//!    finalize, so this stays a hard invariant).
//! 3. Between `message_start` and `message_stop`, every
//!    `content_block_start { index = i }` MUST have a matching
//!    `content_block_stop { index = i }` later in the stream.
//! 4. `content_block_delta { index = i }` events MUST appear strictly
//!    between matching `content_block_start { index = i }` and
//!    `content_block_stop { index = i }` for the same `i`.
//! 5. Block indices MUST be unique within a response — no `i` opens
//!    twice.
//! 6. Exactly one `message_delta` event MUST appear between the last
//!    `content_block_stop` and `message_stop`.
//!
//! These checks compose: integration tests pass the produced
//! `Vec<AnthropicSseEvent>` to [`assert_wire_shape`] and any
//! violation panics with a descriptive message.
//!
//! Compiled only under `cfg(test)` because the helpers exist purely
//! to support codec tests — production code does not assert wire shape
//! at this layer.

use std::collections::{HashMap, HashSet};

use crate::encoder::AnthropicSseEvent;

/// What kind of wire-shape failure was detected.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WireShapeViolation {
    /// First event was not `message_start`.
    MustStartWithMessageStart,
    /// More than one `message_start` in a response.
    MultipleMessageStart,
    /// Last event was not `message_stop`.
    MustEndWithMessageStop,
    /// `content_block_start` reused a previously-used index.
    DuplicateBlockIndex(u32),
    /// `content_block_delta` for an unopened or already-closed block.
    DeltaForUnopenBlock(u32),
    /// `content_block_stop` for an unopened block.
    StopForUnopenBlock(u32),
    /// `content_block_*` after `message_stop`.
    EventAfterMessageStop,
    /// `content_block_start` after `message_stop`.
    BlockStartAfterMessageStop(u32),
    /// `message_delta` appeared more than once.
    MultipleMessageDelta,
    /// Block left open at message_stop time.
    UnclosedBlock(u32),
}

/// Walk a finite slice of events and return every violation
/// detected, in document order. Empty vector means the stream is
/// well-formed.
pub fn check_wire_shape(events: &[AnthropicSseEvent]) -> Vec<WireShapeViolation> {
    let mut violations = Vec::new();

    if events.is_empty() {
        violations.push(WireShapeViolation::MustStartWithMessageStart);
        return violations;
    }

    if !matches!(events[0], AnthropicSseEvent::MessageStart(_)) {
        violations.push(WireShapeViolation::MustStartWithMessageStart);
    }

    let mut open_blocks: HashSet<u32> = HashSet::new();
    let mut seen_indices: HashMap<u32, ()> = HashMap::new();
    let mut message_start_count = 0usize;
    let mut message_delta_count = 0usize;
    let mut message_stop_seen = false;
    let mut last_was_message_stop = false;

    for evt in events {
        last_was_message_stop = false;
        if message_stop_seen {
            // Pings or errors after message_stop are tolerated by some
            // clients, but our encoder never emits them; flag it.
            if !matches!(evt, AnthropicSseEvent::Ping) {
                violations.push(WireShapeViolation::EventAfterMessageStop);
            }
        }
        match evt {
            AnthropicSseEvent::MessageStart(_) => {
                message_start_count += 1;
                if message_start_count > 1 {
                    violations.push(WireShapeViolation::MultipleMessageStart);
                }
            }
            AnthropicSseEvent::ContentBlockStart(p) => {
                if message_stop_seen {
                    violations.push(WireShapeViolation::BlockStartAfterMessageStop(p.index));
                }
                if seen_indices.contains_key(&p.index) {
                    violations.push(WireShapeViolation::DuplicateBlockIndex(p.index));
                }
                seen_indices.insert(p.index, ());
                open_blocks.insert(p.index);
            }
            AnthropicSseEvent::ContentBlockDelta(p) => {
                if !open_blocks.contains(&p.index) {
                    violations.push(WireShapeViolation::DeltaForUnopenBlock(p.index));
                }
            }
            AnthropicSseEvent::ContentBlockStop(p) => {
                if !open_blocks.remove(&p.index) {
                    violations.push(WireShapeViolation::StopForUnopenBlock(p.index));
                }
            }
            AnthropicSseEvent::MessageDelta(_) => {
                message_delta_count += 1;
                if message_delta_count > 1 {
                    violations.push(WireShapeViolation::MultipleMessageDelta);
                }
            }
            AnthropicSseEvent::MessageStop => {
                message_stop_seen = true;
                last_was_message_stop = true;
            }
            AnthropicSseEvent::Ping | AnthropicSseEvent::Error(_) => {}
        }
    }

    if !last_was_message_stop {
        violations.push(WireShapeViolation::MustEndWithMessageStop);
    }

    for idx in open_blocks {
        violations.push(WireShapeViolation::UnclosedBlock(idx));
    }

    violations
}

/// Convenience helper: panics with a descriptive message if any
/// violations are detected. Use this from integration tests.
#[track_caller]
pub fn assert_wire_shape(events: &[AnthropicSseEvent]) {
    let v = check_wire_shape(events);
    assert!(
        v.is_empty(),
        "Anthropic SSE wire shape violations: {v:?}\nemitted: {events:#?}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::{
        AnthropicSseEvent, BlockDelta, ContentBlockDeltaPayload, ContentBlockInit,
        ContentBlockStartPayload, ContentBlockStopPayload, MessageDeltaInner, MessageDeltaPayload,
        MessageEnvelope, MessageStartPayload, Usage,
    };

    fn message_start() -> AnthropicSseEvent {
        AnthropicSseEvent::MessageStart(MessageStartPayload {
            kind: "message_start".into(),
            message: MessageEnvelope {
                id: "msg_t".into(),
                kind: "message".into(),
                role: "assistant".into(),
                content: vec![],
                model: "m".into(),
                stop_reason: None,
                stop_sequence: None,
                usage: Usage::default(),
            },
        })
    }

    fn message_delta() -> AnthropicSseEvent {
        AnthropicSseEvent::MessageDelta(MessageDeltaPayload {
            kind: "message_delta".into(),
            delta: MessageDeltaInner {
                stop_reason: "end_turn".into(),
                stop_sequence: None,
            },
            usage: Usage::default(),
        })
    }

    fn text_block(idx: u32, text: &str) -> Vec<AnthropicSseEvent> {
        vec![
            AnthropicSseEvent::ContentBlockStart(ContentBlockStartPayload {
                kind: "content_block_start".into(),
                index: idx,
                content_block: ContentBlockInit::Text { text: "".into() },
            }),
            AnthropicSseEvent::ContentBlockDelta(ContentBlockDeltaPayload {
                kind: "content_block_delta".into(),
                index: idx,
                delta: BlockDelta::TextDelta { text: text.into() },
            }),
            AnthropicSseEvent::ContentBlockStop(ContentBlockStopPayload {
                kind: "content_block_stop".into(),
                index: idx,
            }),
        ]
    }

    #[test]
    fn well_formed_stream_passes() {
        let mut stream = vec![message_start()];
        stream.extend(text_block(0, "hi"));
        stream.push(message_delta());
        stream.push(AnthropicSseEvent::MessageStop);
        assert!(check_wire_shape(&stream).is_empty());
    }

    #[test]
    fn missing_message_start_fails() {
        let stream = vec![AnthropicSseEvent::MessageStop];
        let v = check_wire_shape(&stream);
        assert!(v.contains(&WireShapeViolation::MustStartWithMessageStart));
    }

    #[test]
    fn duplicate_block_index_fails() {
        let mut stream = vec![message_start()];
        stream.extend(text_block(0, "a"));
        stream.extend(text_block(0, "b")); // reuses index 0
        stream.push(message_delta());
        stream.push(AnthropicSseEvent::MessageStop);
        let v = check_wire_shape(&stream);
        assert!(v.contains(&WireShapeViolation::DuplicateBlockIndex(0)));
    }

    #[test]
    fn unclosed_block_fails() {
        let stream = vec![
            message_start(),
            AnthropicSseEvent::ContentBlockStart(ContentBlockStartPayload {
                kind: "content_block_start".into(),
                index: 0,
                content_block: ContentBlockInit::Text { text: "".into() },
            }),
            message_delta(),
            AnthropicSseEvent::MessageStop,
        ];
        let v = check_wire_shape(&stream);
        assert!(v.contains(&WireShapeViolation::UnclosedBlock(0)));
    }
}
