//! Thinking-block lifecycle helper.
//!
//! Ports the open/delta/close discipline of `core/anthropic/thinking.py`
//! at the SSE-event layer. The free-claude-code reference also embeds
//! a streaming `<think>...</think>` tag parser; that parser belongs at
//! the upstream-SSE-decode layer (J-Sub-D's territory), not the
//! encode-AgentEvent-to-Anthropic-SSE layer this crate owns. What we
//! own here: the small state machine that says "is a thinking block
//! currently open, and at what block index".

/// State of an in-flight thinking content block, if any.
///
/// The encoder doesn't store the thinking-block signature here — when
/// upstream emits a `signature_delta`, the encoder routes it straight
/// to the SSE stream as a `BlockDelta::SignatureDelta`. The state we
/// need to carry across events is only "is a thinking block open, and
/// at what downstream index"; signatures stream through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThinkingState {
    open: bool,
    block_index: u32,
}

impl ThinkingState {
    /// Mark a thinking block as opened at `block_index`. Idempotent —
    /// re-opening at the same index is a no-op so retries are safe.
    pub fn open(&mut self, block_index: u32) {
        self.open = true;
        self.block_index = block_index;
    }

    /// Mark the block as closed.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Whether a thinking block is currently open.
    pub const fn is_open(self) -> bool {
        self.open
    }

    /// The block index of the in-flight thinking block. Only valid
    /// when [`Self::is_open`] returns `true`; otherwise the value is
    /// unspecified.
    pub const fn block_index(self) -> u32 {
        self.block_index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_closed() {
        let s = ThinkingState::default();
        assert!(!s.is_open());
    }

    #[test]
    fn open_then_close_round_trips() {
        let mut s = ThinkingState::default();
        s.open(3);
        assert!(s.is_open());
        assert_eq!(s.block_index(), 3);
        s.close();
        assert!(!s.is_open());
    }

    #[test]
    fn open_is_idempotent_at_same_index() {
        let mut s = ThinkingState::default();
        s.open(2);
        s.open(2);
        assert!(s.is_open());
        assert_eq!(s.block_index(), 2);
    }
}
