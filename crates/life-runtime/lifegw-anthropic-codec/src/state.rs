//! `EmittedTracker` — replay de-dup state for the encoder.
//!
//! Ports the role of `core/anthropic/emitted_sse_tracker.py`. After a
//! mid-stream disconnect, Claude Code re-issues the same request with
//! the same `messages: [...]` array. lifegw replays the missed
//! portion of the assistant turn from the lago event tail (see Spec J
//! §[Streaming + Reconnect]) — but it MUST NOT re-emit events the
//! client has already seen.
//!
//! The tracker remembers, per Life session, which events have been
//! emitted (identified by the (`(EventKind, sequence)` pair) and lets
//! the encoder skip past already-sent events on resume.

use std::collections::HashSet;

/// Compact identifier for one emitted upstream event.
///
/// We key on `(kind_tag, sequence)` rather than on the full
/// `EventRecord` because the `kind` enum is finite and sequences are
/// monotonic per session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EmittedKey {
    /// `pb::AgentEventKind` integer value.
    pub kind: i32,
    /// Lago-assigned sequence number for this event.
    pub sequence: u64,
}

/// Per-session de-dup tracker for emitted SSE events.
///
/// Cheap to clone (the inner `HashSet` is `Clone`); each tracker
/// belongs to one in-flight HTTP response and lives for that
/// response's lifetime. Resumed streams construct a fresh tracker and
/// seed it from the EventRecords already replayed.
#[derive(Clone, Debug, Default)]
pub struct EmittedTracker {
    seen: HashSet<EmittedKey>,
    /// Block-index allocator. The encoder uses it so the
    /// `content_block_*` event stream uses monotonically-increasing
    /// indices even across mid-stream block-policy synthetic closes.
    next_block_index: u32,
    /// Highest `sequence` we've observed (or replayed past). Useful
    /// for `Agent.StreamSession { from_sequence }` on reconnect.
    highest_seen_seq: u64,
}

impl EmittedTracker {
    /// Create a fresh tracker for a new HTTP response.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return whether this event has been emitted already.
    pub fn already_emitted(&self, key: EmittedKey) -> bool {
        self.seen.contains(&key)
    }

    /// Mark an event as emitted. Idempotent.
    pub fn record(&mut self, key: EmittedKey) {
        if key.sequence > self.highest_seen_seq {
            self.highest_seen_seq = key.sequence;
        }
        self.seen.insert(key);
    }

    /// Allocate the next downstream content_block index.
    pub fn allocate_block_index(&mut self) -> u32 {
        let idx = self.next_block_index;
        self.next_block_index = self
            .next_block_index
            .checked_add(1)
            .expect("more than u32::MAX block indices in one response is impossible");
        idx
    }

    /// Read the next-block-index without advancing.
    pub const fn peek_next_block_index(&self) -> u32 {
        self.next_block_index
    }

    /// Manually advance the block index allocator past `n`. Used when
    /// resuming a stream from a saved snapshot.
    pub fn seed_block_index(&mut self, n: u32) {
        if n > self.next_block_index {
            self.next_block_index = n;
        }
    }

    /// Highest upstream sequence number we've emitted (0 if none).
    pub const fn highest_emitted_sequence(&self) -> u64 {
        self.highest_seen_seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(kind: i32, seq: u64) -> EmittedKey {
        EmittedKey {
            kind,
            sequence: seq,
        }
    }

    #[test]
    fn fresh_tracker_has_no_emissions() {
        let t = EmittedTracker::new();
        assert!(!t.already_emitted(key(1, 0)));
        assert_eq!(t.peek_next_block_index(), 0);
        assert_eq!(t.highest_emitted_sequence(), 0);
    }

    #[test]
    fn record_marks_event_as_emitted() {
        let mut t = EmittedTracker::new();
        t.record(key(1, 5));
        assert!(t.already_emitted(key(1, 5)));
        // Different sequence is distinct.
        assert!(!t.already_emitted(key(1, 6)));
        // Different kind, same sequence is distinct.
        assert!(!t.already_emitted(key(2, 5)));
    }

    #[test]
    fn record_is_idempotent() {
        let mut t = EmittedTracker::new();
        t.record(key(1, 5));
        t.record(key(1, 5));
        assert!(t.already_emitted(key(1, 5)));
    }

    #[test]
    fn highest_seq_advances_monotonically() {
        let mut t = EmittedTracker::new();
        t.record(key(1, 3));
        t.record(key(1, 1));
        t.record(key(1, 7));
        t.record(key(1, 5));
        assert_eq!(t.highest_emitted_sequence(), 7);
    }

    #[test]
    fn block_index_allocator_returns_monotonic_ids() {
        let mut t = EmittedTracker::new();
        assert_eq!(t.allocate_block_index(), 0);
        assert_eq!(t.allocate_block_index(), 1);
        assert_eq!(t.allocate_block_index(), 2);
        assert_eq!(t.peek_next_block_index(), 3);
    }

    #[test]
    fn seed_block_index_jumps_forward_only() {
        let mut t = EmittedTracker::new();
        t.seed_block_index(5);
        assert_eq!(t.allocate_block_index(), 5);
        // Seeding backwards is a no-op so resumed streams never
        // collide with earlier block indices.
        t.seed_block_index(2);
        assert_eq!(t.allocate_block_index(), 6);
    }

    #[test]
    fn replay_dedup_uses_tracker_state() {
        // Simulates the resume flow: a tracker is rehydrated from a
        // checkpoint, then the upstream replays the historical events.
        // Each event is consulted against the tracker; already-seen
        // events get skipped.
        let mut t = EmittedTracker::new();
        for s in 1..=5 {
            t.record(key(1, s));
        }
        let replay = [(1, 1), (1, 2), (1, 3), (1, 4), (1, 5), (1, 6), (1, 7)];
        let to_emit: Vec<_> = replay
            .into_iter()
            .filter(|(k, s)| !t.already_emitted(key(*k, *s)))
            .collect();
        // Only the new tail (seq 6 and 7) should pass through.
        assert_eq!(to_emit, vec![(1, 6), (1, 7)]);
    }
}
