//! Tool-use block construction helpers.
//!
//! Ports `core/anthropic/sse.py::ToolCallState` to a Rust struct. The
//! encoder uses [`ToolUseState`] to remember the block index, the
//! Anthropic tool_use id, the tool name, and the streamed JSON input
//! fragments while emitting `input_json_delta` chunks.

use serde::Serialize;

/// In-flight state for one streaming tool_use content block.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct ToolUseState {
    /// Downstream content-block index allocated for this tool_use.
    pub block_index: u32,
    /// Anthropic tool_use id (`toolu_01abc`).
    pub tool_id: String,
    /// Tool name.
    pub name: String,
    /// Streamed JSON-input fragments in arrival order.
    pub partial_jsons: Vec<String>,
    /// Whether the `content_block_start` for this tool has been emitted.
    pub started: bool,
}

impl ToolUseState {
    /// Build a new tool_use state stub.
    pub fn new(block_index: u32, tool_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            block_index,
            tool_id: tool_id.into(),
            name: name.into(),
            partial_jsons: Vec::new(),
            started: false,
        }
    }

    /// Append a JSON-input fragment that the upstream produced.
    pub fn append_partial(&mut self, partial: impl Into<String>) {
        self.partial_jsons.push(partial.into());
    }

    /// Concatenated input JSON text.
    pub fn input_json(&self) -> String {
        self.partial_jsons.join("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_unstarted_state() {
        let s = ToolUseState::new(1, "toolu_01", "read_file");
        assert_eq!(s.block_index, 1);
        assert_eq!(s.tool_id, "toolu_01");
        assert_eq!(s.name, "read_file");
        assert!(!s.started);
        assert!(s.partial_jsons.is_empty());
    }

    #[test]
    fn partial_json_chunks_concatenate_in_order() {
        let mut s = ToolUseState::new(0, "id", "name");
        s.append_partial("{\"path\":");
        s.append_partial(" \"foo.txt\"}");
        assert_eq!(s.input_json(), "{\"path\": \"foo.txt\"}");
    }

    #[test]
    fn tool_use_state_serializes_for_debug_paths() {
        let mut s = ToolUseState::new(3, "tid", "tn");
        s.append_partial("x");
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["block_index"], 3);
        assert_eq!(v["tool_id"], "tid");
        assert_eq!(v["name"], "tn");
    }
}
