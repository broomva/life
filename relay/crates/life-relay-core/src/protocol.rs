//! Wire protocol types for WebSocket communication between relayd and server.
//!
//! All messages serialize as JSON. The WebSocket carries these bidirectionally:
//! - `ServerMessage`: commands from the web UI → daemon
//! - `DaemonMessage`: events from local sessions → server → browser

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::session::{SessionInfo, SessionType, SpawnConfig};

// ── Server → Daemon ─────────────────────────────────────────────────────

/// Commands sent from broomva.tech to the relay daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Spawn a new agent session.
    Spawn {
        session_type: SessionType,
        config: SpawnConfig,
    },
    /// Send input (text or keystrokes) to a session.
    Input {
        session_id: Uuid,
        data: String,
    },
    /// Resize the PTY for a session.
    Resize {
        session_id: Uuid,
        cols: u16,
        rows: u16,
    },
    /// Resolve a capability approval request.
    Approve {
        session_id: Uuid,
        approval_id: String,
        approved: bool,
    },
    /// Kill a session.
    Kill {
        session_id: Uuid,
    },
    /// Request the current session list.
    ListSessions,
    /// Keepalive ping.
    Ping,
}

// ── Daemon → Server ─────────────────────────────────────────────────────

/// Events sent from the relay daemon to broomva.tech.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMessage {
    /// Output data from a session (terminal bytes or structured events).
    Output {
        session_id: Uuid,
        data: String,
        seq: u64,
    },
    /// A new session was created.
    SessionCreated {
        session: SessionInfo,
    },
    /// A session has ended.
    SessionEnded {
        session_id: Uuid,
        reason: String,
    },
    /// A session needs capability approval from the user.
    ApprovalRequest {
        session_id: Uuid,
        approval_id: String,
        capability: String,
        context: String,
    },
    /// Response to `ListSessions`.
    SessionList {
        sessions: Vec<SessionInfo>,
    },
    /// Node identification sent on connect.
    NodeInfo {
        name: String,
        hostname: String,
        capabilities: Vec<String>,
    },
    /// Keepalive pong.
    Pong,
    /// Error message.
    Error {
        code: String,
        message: String,
    },
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
