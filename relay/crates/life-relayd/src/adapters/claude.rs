//! Claude Code adapter — spawns and manages Claude Code sessions via PTY.
//!
//! Launches `claude --output-format stream-json` in a pseudo-terminal,
//! streams structured JSONL output to the daemon event channel, and
//! injects keystrokes for input. Approval requests are surfaced as
//! [`DaemonMessage::ApprovalRequest`] so the web UI can render buttons.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use life_relay_core::{
    DaemonMessage, RelayError, RelayResult, SessionInfo, SessionStatus, SessionType, SpawnConfig,
};
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::SessionAdapter;
use super::parser::{self, ClaudeEvent};
use super::pty::PtyHandle;

/// Manages Claude Code sessions via PTY.
///
/// Session handles are stored in `sessions`. Output is forwarded to the
/// daemon's outbound channel as [`DaemonMessage`] events.
#[derive(Debug, Default)]
pub struct ClaudeAdapter {
    sessions: Arc<RwLock<HashMap<Uuid, PtyHandle>>>,
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the claude CLI command for a session.
    pub fn build_command(config: &SpawnConfig) -> Vec<String> {
        let mut args = vec![
            "claude".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
        ];

        if let Some(ref model) = config.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        args
    }
}

#[async_trait]
impl SessionAdapter for ClaudeAdapter {
    async fn spawn(
        &self,
        config: &SpawnConfig,
        event_tx: mpsc::Sender<DaemonMessage>,
    ) -> RelayResult<SessionInfo> {
        let id = config.session_id.unwrap_or_else(Uuid::new_v4);
        let cmd = Self::build_command(config);

        info!(
            session_id = %id,
            cmd = ?cmd,
            workdir = %config.workdir,
            "spawning claude code session"
        );

        let mut handle = PtyHandle::spawn(id, &cmd, &config.workdir)?;
        let mut output_rx = handle.take_output_rx();

        let info = SessionInfo {
            id,
            session_type: SessionType::ClaudeCode,
            status: SessionStatus::Active,
            name: config.name.clone(),
            workdir: config.workdir.clone(),
            model: config.model.clone(),
            created_at: Utc::now(),
        };

        // Background task: forward output and parsed events to daemon.
        let session_id = id;
        let forward_tx = event_tx.clone();
        tokio::spawn(async move {
            let mut seq: u64 = 0;
            let mut line_buf = String::new();

            while let Some(chunk) = output_rx.recv().await {
                // Forward raw chunk for terminal display.
                seq += 1;
                let _ = forward_tx
                    .send(DaemonMessage::Output {
                        session_id,
                        data: chunk.clone(),
                        seq,
                    })
                    .await;

                // Parse complete lines for structured events.
                line_buf.push_str(&chunk);
                while let Some(pos) = line_buf.find('\n') {
                    let line = line_buf[..pos].to_string();
                    line_buf = line_buf[pos + 1..].to_string();
                    handle_parsed_event(session_id, &line, &forward_tx).await;
                }
            }

            // PTY closed — session ended.
            warn!(session_id = %session_id, "PTY output stream closed");
            let _ = forward_tx
                .send(DaemonMessage::SessionEnded {
                    session_id,
                    reason: "pty closed".to_string(),
                })
                .await;
        });

        self.sessions.write().await.insert(id, handle);
        Ok(info)
    }

    async fn send_input(&self, session_id: &Uuid, data: &str) -> RelayResult<()> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(session_id)
            .ok_or_else(|| RelayError::SessionNotFound(session_id.to_string()))?;
        handle.send_input(data).await
    }

    async fn kill(&self, session_id: &Uuid) -> RelayResult<()> {
        let mut sessions = self.sessions.write().await;
        let handle = sessions
            .get(session_id)
            .ok_or_else(|| RelayError::SessionNotFound(session_id.to_string()))?;
        handle.kill()?;
        sessions.remove(session_id);
        Ok(())
    }

    async fn resize(&self, session_id: &Uuid, cols: u16, rows: u16) -> RelayResult<()> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(session_id)
            .ok_or_else(|| RelayError::SessionNotFound(session_id.to_string()))?;
        handle.resize(cols, rows)
    }
}

/// Dispatch a parsed [`ClaudeEvent`] to the daemon channel.
async fn handle_parsed_event(
    session_id: Uuid,
    line: &str,
    forward_tx: &mpsc::Sender<DaemonMessage>,
) {
    match parser::parse_line(line) {
        ClaudeEvent::ApprovalRequest {
            approval_id,
            capability,
            context,
        } => {
            debug!(
                session_id = %session_id,
                capability = %capability,
                "approval request detected"
            );
            let _ = forward_tx
                .send(DaemonMessage::ApprovalRequest {
                    session_id,
                    approval_id,
                    capability,
                    context,
                })
                .await;
        }
        ClaudeEvent::Result {
            cost_usd,
            duration_ms,
        } => {
            info!(
                session_id = %session_id,
                cost_usd = ?cost_usd,
                duration_ms = ?duration_ms,
                "claude session completed"
            );
            let _ = forward_tx
                .send(DaemonMessage::SessionEnded {
                    session_id,
                    reason: "completed".to_string(),
                })
                .await;
        }
        ClaudeEvent::SystemInit {
            model,
            session_id: sid,
            cwd,
        } => {
            debug!(
                session_id = %session_id,
                model = ?model,
                remote_sid = ?sid,
                cwd = ?cwd,
                "claude session initialized"
            );
        }
        ClaudeEvent::AssistantText { text } => {
            if !text.is_empty() {
                let _ = forward_tx
                    .send(DaemonMessage::AssistantMessage { session_id, text })
                    .await;
            }
        }
        ClaudeEvent::ToolUse { id, name, input } => {
            debug!(session_id = %session_id, tool = %name, "tool use");
            let _ = forward_tx
                .send(DaemonMessage::ToolEvent {
                    session_id,
                    tool_name: name,
                    tool_id: id,
                    input,
                })
                .await;
        }
        ClaudeEvent::ToolResult { .. } | ClaudeEvent::Raw(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_command_includes_stream_json() {
        let config = SpawnConfig {
            name: "test".to_string(),
            workdir: "/tmp".to_string(),
            model: None,
            session_id: None,
        };
        let cmd = ClaudeAdapter::build_command(&config);
        assert!(cmd.contains(&"stream-json".to_string()));
        assert!(cmd.contains(&"--output-format".to_string()));
    }

    #[test]
    fn build_command_includes_model_when_set() {
        let config = SpawnConfig {
            name: "test".to_string(),
            workdir: "/tmp".to_string(),
            model: Some("claude-opus-4-5".to_string()),
            session_id: None,
        };
        let cmd = ClaudeAdapter::build_command(&config);
        assert!(cmd.contains(&"--model".to_string()));
        assert!(cmd.contains(&"claude-opus-4-5".to_string()));
    }
}
