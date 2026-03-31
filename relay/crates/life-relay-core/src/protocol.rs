//! Wire protocol types for HTTP polling communication between relayd and server.
//!
//! All messages serialize as JSON with camelCase field names (matching TypeScript).
//! Variant tags use `snake_case` (e.g. `"session_created"`, `"tool_event"`).
//!
//! - `ServerMessage`: commands from the web UI → daemon (via `/api/relay/poll`)
//! - `DaemonMessage`: events from local sessions → server (via `/api/relay/events`)

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::session::{SessionInfo, SessionType, SpawnConfig};

// ── History Types ──────────────────────────────────────────────────────

/// A single tool invocation within a history message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryToolUse {
    /// Tool name (e.g. "Bash", "Edit", "Read").
    pub name: String,
    /// First 100 characters of the serialised input.
    pub input_preview: String,
}

/// A single message extracted from a Claude Code session JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessage {
    /// "user" or "assistant".
    pub role: String,
    /// Extracted text content.
    pub text: String,
    /// Tool calls within this message (assistant only).
    pub tools: Vec<HistoryToolUse>,
    /// ISO-8601 timestamp if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

// ── Server → Daemon ─────────────────────────────────────────────────────

/// Commands sent from broomva.tech to the relay daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Spawn a new agent session.
    #[serde(rename_all = "camelCase")]
    Spawn {
        session_type: SessionType,
        config: SpawnConfig,
    },
    /// Send input (text or keystrokes) to a session.
    #[serde(rename_all = "camelCase")]
    Input { session_id: Uuid, data: String },
    /// Resize the PTY for a session.
    #[serde(rename_all = "camelCase")]
    Resize {
        session_id: Uuid,
        cols: u16,
        rows: u16,
    },
    /// Resolve a capability approval request.
    #[serde(rename_all = "camelCase")]
    Approve {
        session_id: Uuid,
        approval_id: String,
        approved: bool,
    },
    /// Kill a session.
    #[serde(rename_all = "camelCase")]
    Kill { session_id: Uuid },
    /// Request the current session list.
    ListSessions,
    /// List directory contents on the daemon machine.
    #[serde(rename_all = "camelCase")]
    ListDir {
        path: String,
        request_id: String,
    },
    /// Load conversation history from Claude Code session files.
    #[serde(rename_all = "camelCase")]
    LoadHistory {
        session_id: Uuid,
        request_id: String,
    },
    /// Keepalive ping.
    Ping,
}

// ── Daemon → Server ─────────────────────────────────────────────────────

/// Events sent from the relay daemon to broomva.tech.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMessage {
    /// Raw output data from a session (terminal bytes).
    #[serde(rename_all = "camelCase")]
    Output {
        session_id: Uuid,
        data: String,
        seq: u64,
    },
    /// An assistant text message extracted from stream-json output.
    #[serde(rename_all = "camelCase")]
    AssistantMessage { session_id: Uuid, text: String },
    /// A tool was invoked by the agent (e.g. Edit, Bash, Write, Read).
    #[serde(rename_all = "camelCase")]
    ToolEvent {
        session_id: Uuid,
        tool_name: String,
        tool_id: String,
        input: Value,
    },
    /// A new session was created.
    SessionCreated { session: SessionInfo },
    /// A session has ended.
    #[serde(rename_all = "camelCase")]
    SessionEnded { session_id: Uuid, reason: String },
    /// A session needs capability approval from the user.
    #[serde(rename_all = "camelCase")]
    ApprovalRequest {
        session_id: Uuid,
        approval_id: String,
        capability: String,
        context: String,
    },
    /// Response to `ListSessions`.
    SessionList { sessions: Vec<SessionInfo> },
    /// Node identification sent on connect.
    NodeInfo {
        name: String,
        hostname: String,
        capabilities: Vec<String>,
    },
    /// Streaming: incremental text delta within a content block.
    #[serde(rename_all = "camelCase")]
    ContentDelta {
        session_id: Uuid,
        index: u32,
        text: String,
    },
    /// Streaming: a content block has started.
    #[serde(rename_all = "camelCase")]
    ContentBlockStart {
        session_id: Uuid,
        index: u32,
        block_type: String,
    },
    /// Streaming: a content block has finished.
    #[serde(rename_all = "camelCase")]
    ContentBlockStop {
        session_id: Uuid,
        index: u32,
    },
    /// Tool execution result forwarded from Claude Code.
    #[serde(rename_all = "camelCase")]
    ToolResult {
        session_id: Uuid,
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Summary emitted at the end of a Claude Code turn.
    #[serde(rename_all = "camelCase")]
    TurnResult {
        session_id: Uuid,
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
        num_turns: Option<u64>,
    },
    /// Workspace git status emitted periodically by relayd.
    #[serde(rename_all = "camelCase")]
    WorkspaceStatus {
        session_id: Uuid,
        /// Current branch name, or `None` if not a git repo.
        branch: Option<String>,
        /// Number of modified (unstaged) files.
        modified: u32,
        /// Number of staged files.
        staged: u32,
        /// Short hash + subject of the last commit, or `None` if no commits.
        last_commit: Option<String>,
    },
    /// Response to `ListDir` — directory listing from the daemon machine.
    #[serde(rename_all = "camelCase")]
    DirListing {
        request_id: String,
        path: String,
        entries: Vec<DirEntry>,
    },
    /// Maps a relay session to a Claude Code session ID.
    #[serde(rename_all = "camelCase")]
    SessionMapping {
        session_id: Uuid,
        claude_session_id: String,
    },
    /// Response to `LoadHistory` — conversation history from JSONL files.
    #[serde(rename_all = "camelCase")]
    HistoryMessages {
        session_id: Uuid,
        request_id: String,
        messages: Vec<HistoryMessage>,
    },
    /// Keepalive pong.
    Pong,
    /// Error message.
    Error { code: String, message: String },
}

/// A single entry in a directory listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    /// File or directory name (not full path).
    pub name: String,
    /// Whether this entry is a directory.
    pub is_dir: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_message_roundtrip() {
        let msg = ServerMessage::Input {
            session_id: Uuid::new_v4(),
            data: "hello".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ServerMessage::Input { .. }));
    }

    #[test]
    fn server_message_fields_are_camel_case() {
        let msg = ServerMessage::Input {
            session_id: Uuid::new_v4(),
            data: "hi".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("sessionId"),
            "expected camelCase sessionId: {json}"
        );
        assert!(
            !json.contains("session_id"),
            "unexpected snake_case: {json}"
        );
    }

    #[test]
    fn daemon_message_roundtrip() {
        let msg = DaemonMessage::Output {
            session_id: Uuid::new_v4(),
            data: "test output".to_string(),
            seq: 42,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: DaemonMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, DaemonMessage::Output { seq: 42, .. }));
    }

    #[test]
    fn daemon_message_fields_are_camel_case() {
        let msg = DaemonMessage::Output {
            session_id: Uuid::new_v4(),
            data: "x".to_string(),
            seq: 1,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("sessionId"), "expected camelCase: {json}");
    }

    #[test]
    fn assistant_message_roundtrip() {
        let id = Uuid::new_v4();
        let msg = DaemonMessage::AssistantMessage {
            session_id: id,
            text: "Hello from Claude".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("assistant_message"));
        assert!(json.contains("sessionId"));
        assert!(json.contains("Hello from Claude"));
        let parsed: DaemonMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, DaemonMessage::AssistantMessage { .. }));
    }

    #[test]
    fn tool_event_roundtrip() {
        let id = Uuid::new_v4();
        let msg = DaemonMessage::ToolEvent {
            session_id: id,
            tool_name: "Bash".to_string(),
            tool_id: "tu_abc".to_string(),
            input: serde_json::json!({ "command": "ls -la" }),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("tool_event"));
        assert!(json.contains("toolName"));
        assert!(json.contains("Bash"));
        let parsed: DaemonMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, DaemonMessage::ToolEvent { .. }));
    }

    #[test]
    fn workspace_status_roundtrip() {
        let id = Uuid::new_v4();
        let msg = DaemonMessage::WorkspaceStatus {
            session_id: id,
            branch: Some("main".to_string()),
            modified: 3,
            staged: 1,
            last_commit: Some("a1b2c3d Add relay session replay buffer".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("workspace_status"), "variant tag: {json}");
        assert!(json.contains("sessionId"), "camelCase field: {json}");
        assert!(json.contains("lastCommit"), "camelCase field: {json}");
        let parsed: DaemonMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            parsed,
            DaemonMessage::WorkspaceStatus { modified: 3, .. }
        ));
    }

    #[test]
    fn node_info_serialization() {
        let msg = DaemonMessage::NodeInfo {
            name: "macbook-pro".to_string(),
            hostname: "MacBook-Pro.local".to_string(),
            capabilities: vec!["claude-code".to_string(), "arcan".to_string()],
        };
        let json = serde_json::to_string_pretty(&msg).unwrap();
        assert!(json.contains("node_info"));
        assert!(json.contains("claude-code"));
    }
}
