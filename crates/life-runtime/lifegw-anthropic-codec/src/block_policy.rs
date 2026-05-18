//! Block-policy state machine.
//!
//! Ports `core/anthropic/native_sse_block_policy.py` adapted for our
//! direction of travel: we encode lifed `pb::AgentEvent` (a flat token
//! stream) **into** Anthropic Messages SSE (which framed each content
//! run as `content_block_start → content_block_delta* →
//! content_block_stop`). The reference Python module re-maps inbound
//! Anthropic SSE indices to outbound ones; we instead allocate
//! downstream indices from scratch.
//!
//! What the policy owns:
//!
//! 1. Which downstream `content_block` index is currently open for
//!    which logical kind (text vs thinking vs tool_use).
//! 2. The transition rules: when upstream switches from text to
//!    thinking, the text block MUST close before thinking opens. The
//!    Anthropic protocol forbids overlapping content blocks within a
//!    message.
//! 3. Tool_use blocks are distinguishable per-instance (each has a
//!    unique `id`), so we track them by `id` rather than collapsing
//!    "the tool_use block" the way we do for text.
//!
//! What the policy does NOT own:
//!
//! * SSE event emission (that's [`crate::encoder`]).
//! * Anthropic Messages request validation (that's [`crate::request`]).
//! * Mid-stream reconnect de-dup (that's [`crate::state`]).

use std::collections::HashMap;

use crate::state::EmittedTracker;

/// Logical kind of an Anthropic content block, in the granularity our
/// upstream `pb::AgentEvent` stream can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// Plain assistant text.
    Text,
    /// Extended-thinking trace.
    Thinking,
}

/// Snapshot of which (if any) block is currently open in the
/// downstream SSE stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OpenBlock {
    #[default]
    None,
    /// A text-or-thinking content block is open at this downstream index.
    Singular { kind: BlockKind, index: u32 },
}

/// Outcome of [`BlockPolicyState::enter_block`]: the caller must emit
/// the events the policy decided to fire.
///
/// Each variant maps to a concrete sequence of SSE events the encoder
/// must produce — but the policy itself never produces strings; it
/// only commands. Keeping wire-emission outside this module keeps the
/// state machine purely-functional and trivially testable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockTransition {
    /// If non-zero — there was a singular block previously open at
    /// this `(kind, index)` and the caller MUST emit
    /// `content_block_stop { index }` before opening the new block.
    pub close_previous: Option<u32>,
    /// The downstream index allocated for the newly-opening block.
    /// The caller MUST emit `content_block_start { index }` for this.
    pub open_index: u32,
    /// The kind of block now open.
    pub kind: BlockKind,
    /// Whether the policy actually opened a new block (false if the
    /// caller's request was a no-op because the same singular block
    /// is already open).
    pub opened_new: bool,
}

/// Outcome of [`BlockPolicyState::enter_tool_use`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolBlockTransition {
    /// If `Some`, a singular block was open and must be closed before
    /// the tool_use block opens.
    pub close_singular: Option<u32>,
    /// The downstream index allocated for this tool_use.
    pub open_index: u32,
    /// Whether the tool_use was newly opened (false if the caller is
    /// adding partial-json to an already-open tool_use).
    pub opened_new: bool,
}

/// The policy itself.
///
/// One instance per HTTP response. Composes with [`EmittedTracker`]
/// for downstream index allocation so the policy never reuses an
/// index across the response.
#[derive(Clone, Debug, Default)]
pub struct BlockPolicyState {
    singular: OpenBlock,
    tool_by_id: HashMap<String, u32>,
}

impl BlockPolicyState {
    /// Fresh policy state at the start of a response.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a text or thinking block is currently open.
    pub fn has_singular_open(&self) -> bool {
        !matches!(self.singular, OpenBlock::None)
    }

    /// Borrow the in-flight singular block, if any.
    pub fn current_singular(&self) -> Option<(BlockKind, u32)> {
        match self.singular {
            OpenBlock::None => None,
            OpenBlock::Singular { kind, index } => Some((kind, index)),
        }
    }

    /// Whether a tool_use block keyed by `id` is currently open.
    pub fn has_tool_use(&self, id: &str) -> bool {
        self.tool_by_id.contains_key(id)
    }

    /// Borrow the downstream index assigned to tool_use `id` if open.
    pub fn tool_use_index(&self, id: &str) -> Option<u32> {
        self.tool_by_id.get(id).copied()
    }

    /// Request to open a singular text/thinking block.
    ///
    /// * If the *same* kind is already open, returns
    ///   [`BlockTransition::opened_new`] = false and the existing
    ///   index; the caller emits no events.
    /// * If a *different* singular block is open, returns a
    ///   `close_previous` directive — the caller MUST emit
    ///   `content_block_stop` for the old index before opening the
    ///   new one.
    /// * If no block is open, simply allocates a new index.
    pub fn enter_block(
        &mut self,
        kind: BlockKind,
        tracker: &mut EmittedTracker,
    ) -> BlockTransition {
        match self.singular {
            OpenBlock::Singular {
                kind: existing,
                index,
            } if existing == kind => BlockTransition {
                close_previous: None,
                open_index: index,
                kind,
                opened_new: false,
            },
            OpenBlock::Singular {
                kind: _existing,
                index,
            } => {
                // Close prior, open new.
                let new_idx = tracker.allocate_block_index();
                self.singular = OpenBlock::Singular {
                    kind,
                    index: new_idx,
                };
                BlockTransition {
                    close_previous: Some(index),
                    open_index: new_idx,
                    kind,
                    opened_new: true,
                }
            }
            OpenBlock::None => {
                let new_idx = tracker.allocate_block_index();
                self.singular = OpenBlock::Singular {
                    kind,
                    index: new_idx,
                };
                BlockTransition {
                    close_previous: None,
                    open_index: new_idx,
                    kind,
                    opened_new: true,
                }
            }
        }
    }

    /// Close whatever singular block is currently open, returning its
    /// index (the caller emits `content_block_stop` for it). No-op if
    /// nothing is open.
    pub fn close_singular(&mut self) -> Option<u32> {
        let idx = match self.singular {
            OpenBlock::None => None,
            OpenBlock::Singular { index, .. } => Some(index),
        };
        self.singular = OpenBlock::None;
        idx
    }

    /// Open a tool_use block keyed by `id`. If `id` is already open,
    /// returns the existing index with `opened_new=false`. Otherwise
    /// allocates a new downstream index and — if a singular block was
    /// open — instructs the caller to close it first.
    pub fn enter_tool_use(
        &mut self,
        id: &str,
        tracker: &mut EmittedTracker,
    ) -> ToolBlockTransition {
        if let Some(&idx) = self.tool_by_id.get(id) {
            return ToolBlockTransition {
                close_singular: None,
                open_index: idx,
                opened_new: false,
            };
        }
        let close_singular = self.close_singular();
        let idx = tracker.allocate_block_index();
        self.tool_by_id.insert(id.to_string(), idx);
        ToolBlockTransition {
            close_singular,
            open_index: idx,
            opened_new: true,
        }
    }

    /// Close a tool_use block (drops it from the tracking map) and
    /// returns its downstream index. Returns `None` if `id` was not
    /// open.
    pub fn close_tool_use(&mut self, id: &str) -> Option<u32> {
        self.tool_by_id.remove(id)
    }

    /// Close every still-open block — singular and every tool_use —
    /// and return their indices in (no-particular-but-stable) order.
    /// The caller emits one `content_block_stop` per index.
    pub fn close_all(&mut self) -> Vec<u32> {
        let mut out = Vec::new();
        if let Some(i) = self.close_singular() {
            out.push(i);
        }
        // Order by downstream index so the output is deterministic.
        let mut tools: Vec<u32> = self.tool_by_id.values().copied().collect();
        tools.sort_unstable();
        out.extend(tools);
        self.tool_by_id.clear();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_state_has_nothing_open() {
        let s = BlockPolicyState::new();
        assert!(!s.has_singular_open());
        assert!(s.current_singular().is_none());
    }

    #[test]
    fn enter_first_text_block_allocates_index_zero() {
        let mut s = BlockPolicyState::new();
        let mut tracker = EmittedTracker::new();
        let t = s.enter_block(BlockKind::Text, &mut tracker);
        assert_eq!(t.open_index, 0);
        assert_eq!(t.kind, BlockKind::Text);
        assert!(t.opened_new);
        assert!(t.close_previous.is_none());
        assert!(s.has_singular_open());
    }

    #[test]
    fn enter_same_kind_twice_is_a_noop() {
        let mut s = BlockPolicyState::new();
        let mut tracker = EmittedTracker::new();
        s.enter_block(BlockKind::Text, &mut tracker);
        let t2 = s.enter_block(BlockKind::Text, &mut tracker);
        assert!(!t2.opened_new);
        assert_eq!(t2.open_index, 0); // same block index as before
        assert!(t2.close_previous.is_none());
        // tracker did not advance.
        assert_eq!(tracker.peek_next_block_index(), 1);
    }

    #[test]
    fn text_to_thinking_closes_previous_and_opens_new() {
        let mut s = BlockPolicyState::new();
        let mut tracker = EmittedTracker::new();
        s.enter_block(BlockKind::Text, &mut tracker);
        let t = s.enter_block(BlockKind::Thinking, &mut tracker);
        assert_eq!(t.close_previous, Some(0));
        assert_eq!(t.open_index, 1);
        assert_eq!(t.kind, BlockKind::Thinking);
        assert!(t.opened_new);
        assert_eq!(s.current_singular(), Some((BlockKind::Thinking, 1)));
    }

    #[test]
    fn close_singular_returns_open_index_and_clears_state() {
        let mut s = BlockPolicyState::new();
        let mut tracker = EmittedTracker::new();
        s.enter_block(BlockKind::Text, &mut tracker);
        let closed = s.close_singular();
        assert_eq!(closed, Some(0));
        assert!(!s.has_singular_open());
        // Idempotent.
        assert!(s.close_singular().is_none());
    }

    #[test]
    fn entering_tool_use_closes_singular_block_first() {
        let mut s = BlockPolicyState::new();
        let mut tracker = EmittedTracker::new();
        s.enter_block(BlockKind::Text, &mut tracker);
        let t = s.enter_tool_use("toolu_01", &mut tracker);
        assert_eq!(t.close_singular, Some(0));
        assert_eq!(t.open_index, 1);
        assert!(t.opened_new);
        assert!(s.has_tool_use("toolu_01"));
        assert!(!s.has_singular_open());
    }

    #[test]
    fn entering_tool_use_twice_with_same_id_is_a_noop() {
        let mut s = BlockPolicyState::new();
        let mut tracker = EmittedTracker::new();
        s.enter_tool_use("toolu_01", &mut tracker);
        let t = s.enter_tool_use("toolu_01", &mut tracker);
        assert!(!t.opened_new);
        assert_eq!(t.open_index, 0);
        assert!(t.close_singular.is_none());
    }

    #[test]
    fn multiple_tool_uses_allocate_distinct_indices() {
        let mut s = BlockPolicyState::new();
        let mut tracker = EmittedTracker::new();
        let a = s.enter_tool_use("toolu_A", &mut tracker);
        let b = s.enter_tool_use("toolu_B", &mut tracker);
        assert_ne!(a.open_index, b.open_index);
        assert_eq!(s.tool_use_index("toolu_A"), Some(a.open_index));
        assert_eq!(s.tool_use_index("toolu_B"), Some(b.open_index));
    }

    #[test]
    fn close_tool_use_returns_index_then_forgets() {
        let mut s = BlockPolicyState::new();
        let mut tracker = EmittedTracker::new();
        s.enter_tool_use("toolu_01", &mut tracker);
        assert_eq!(s.close_tool_use("toolu_01"), Some(0));
        assert!(!s.has_tool_use("toolu_01"));
        assert!(s.close_tool_use("toolu_01").is_none());
    }

    #[test]
    fn close_all_returns_every_open_block_index() {
        let mut s = BlockPolicyState::new();
        let mut tracker = EmittedTracker::new();
        s.enter_block(BlockKind::Text, &mut tracker); // idx 0
        s.close_singular();
        s.enter_tool_use("toolu_A", &mut tracker); // idx 1
        s.enter_tool_use("toolu_B", &mut tracker); // idx 2
        s.enter_block(BlockKind::Text, &mut tracker); // idx 3
        let closed = s.close_all();
        // singular at 3, tools 1 and 2 — sorted result is 3,1,2 (since
        // singular comes first per the impl).
        assert_eq!(closed.len(), 3);
        assert!(closed.contains(&3));
        assert!(closed.contains(&1));
        assert!(closed.contains(&2));
        assert!(!s.has_singular_open());
        assert!(!s.has_tool_use("toolu_A"));
    }

    #[test]
    fn block_indices_never_collide_across_transitions() {
        // The hard invariant: every NEWLY-ALLOCATED block index is
        // unique across the response, even with many alternations.
        // (Close-directives can reference earlier indices, but they
        // never re-allocate — that's a separate property guaranteed
        // by `EmittedTracker::allocate_block_index`.)
        let mut s = BlockPolicyState::new();
        let mut tracker = EmittedTracker::new();
        let mut seen = std::collections::HashSet::new();

        let alloc = |s: &mut BlockPolicyState, tracker: &mut EmittedTracker| {
            let kinds = [BlockKind::Text, BlockKind::Thinking];
            let mut indices = Vec::new();
            for k in kinds {
                let t = s.enter_block(k, tracker);
                if t.opened_new {
                    indices.push(t.open_index);
                }
            }
            for i in 0..5 {
                let id = format!("toolu_{i}");
                let t = s.enter_tool_use(&id, tracker);
                if t.opened_new {
                    indices.push(t.open_index);
                }
            }
            indices
        };

        let first_batch = alloc(&mut s, &mut tracker);
        for i in &first_batch {
            assert!(seen.insert(*i), "duplicate block index {i}");
        }
        s.close_all();
        let second_batch = alloc(&mut s, &mut tracker);
        for i in &second_batch {
            assert!(seen.insert(*i), "duplicate block index {i}");
        }
    }
}
