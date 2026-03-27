//! Claude Code adapter — spawns and manages Claude Code sessions via PTY.
//!
//! Launches `claude` CLI in a pseudo-terminal, captures output continuously,
//! and injects keystrokes for input and permission approvals.

use life_relay_core::{SessionInfo, SessionStatus, SessionType, SpawnConfig};
use tracing::info;

/// Claude Code session adapter (placeholder for PTY implementation).
pub struct ClaudeAdapter;

impl ClaudeAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Build the claude CLI command for a session.
    pub fn build_command(config: &SpawnConfig) -> Vec<String> {
        let mut args = vec![
            "claude".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "--channels".to_string(),
            "plugin:discord@claude-plugins-official".to_string(),
            "--name".to_string(),
            config.name.clone(),
        ];

        if let Some(ref sid) = config.session_id {
            args.push("--session-id".to_string());
            args.push(sid.to_string());
        }

        if let Some(ref model) = config.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        args
    }
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

// Note: Full SessionAdapter trait implementation requires `portable-pty`
// and `async-trait` crates. The PTY spawn/read/write loop will be added
// in a follow-up commit once the workspace compiles cleanly.
//
// The pattern follows Mission Control's agent service:
// 1. portable_pty::native_pty_system().openpty(size)
// 2. child = pair.slave.spawn_command(cmd)
// 3. reader = pair.master.try_clone_reader()
// 4. writer = pair.master.take_writer()
// 5. tokio::spawn read loop → tx.send(output)
// 6. writer.write_all(input) for keystroke injection

/// Placeholder: create session info for a spawned Claude Code session.
pub fn mock_session_info(config: &SpawnConfig) -> SessionInfo {
    let id = config.session_id.unwrap_or_else(uuid::Uuid::new_v4);
    info!(session_id = %id, name = %config.name, "spawning claude code session");
    SessionInfo {
        id,
        session_type: SessionType::ClaudeCode,
        status: SessionStatus::Active,
        name: config.name.clone(),
        workdir: config.workdir.clone(),
        model: config.model.clone(),
        created_at: chrono::Utc::now(),
    }
}
