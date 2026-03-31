//! Claude Code adapter — spawns and manages Claude Code sessions.
//!
//! Launches `claude --print --output-format stream-json --input-format stream-json`
//! as a subprocess with piped stdin/stdout. This is Claude Code's non-interactive
//! mode designed for programmatic use — clean JSONL on stdout, structured input
//! on stdin, no TUI rendering.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use life_relay_core::{
    DaemonMessage, RelayError, RelayResult, SessionInfo, SessionStatus, SessionType, SpawnConfig,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::SessionAdapter;
use super::parser::{self, ClaudeEvent};

/// Handle for a running Claude Code session.
struct SessionHandle {
    input_tx: mpsc::Sender<String>,
}

/// Manages Claude Code sessions via piped subprocess.
#[derive(Debug, Default)]
pub struct ClaudeAdapter {
    sessions: Arc<RwLock<HashMap<Uuid, SessionHandle>>>,
}

impl std::fmt::Debug for SessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionHandle").finish()
    }
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the claude CLI command args for a session.
    pub fn build_command(config: &SpawnConfig) -> Vec<String> {
        let mut args = vec![
            "claude".to_string(),
            "--print".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
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
        let cmd_args = Self::build_command(config);

        info!(
            session_id = %id,
            cmd = ?cmd_args,
            workdir = %config.workdir,
            "spawning claude code session"
        );

        let mut child = Command::new(&cmd_args[0])
            .args(&cmd_args[1..])
            .current_dir(&config.workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| RelayError::SpawnFailed(e.to_string()))?;

        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");

        let info = SessionInfo {
            id,
            session_type: SessionType::ClaudeCode,
            status: SessionStatus::Active,
            name: config.name.clone(),
            workdir: config.workdir.clone(),
            model: config.model.clone(),
            created_at: Utc::now(),
        };

        // Input channel → child stdin (stream-json format)
        let (input_tx, mut input_rx) = mpsc::channel::<String>(64);
        let mut stdin_writer = stdin;
        let input_session_id = id;
        tokio::spawn(async move {
            while let Some(text) = input_rx.recv().await {
                // stream-json input format: send as JSON user message
                let msg = serde_json::json!({
                    "type": "user",
                    "message": {
                        "role": "user",
                        "content": [{"type": "text", "text": text.trim()}]
                    }
                });
                let line = format!("{}\n", serde_json::to_string(&msg).unwrap_or_default());
                debug!(session_id = %input_session_id, "sending input to claude stdin");
                if stdin_writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                let _ = stdin_writer.flush().await;
            }
        });

        // Background: parse stdout JSONL → structured events
        let session_id = id;
        let forward_tx = event_tx.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                debug!(session_id = %session_id, line = %trimmed.get(..100).unwrap_or(trimmed), "claude stdout line");

                match parser::parse_line(trimmed) {
                    ClaudeEvent::AssistantText { text } => {
                        if !text.is_empty() {
                            info!(session_id = %session_id, text_len = text.len(), "assistant message");
                            let _ = forward_tx
                                .send(DaemonMessage::AssistantMessage { session_id, text })
                                .await;
                        }
                    }
                    ClaudeEvent::ToolUse { id: tool_id, name, input } => {
                        debug!(session_id = %session_id, tool = %name, "tool use");
                        let _ = forward_tx
                            .send(DaemonMessage::ToolEvent {
                                session_id,
                                tool_name: name,
                                tool_id,
                                input,
                            })
                            .await;
                    }
                    ClaudeEvent::ApprovalRequest { approval_id, capability, context } => {
                        debug!(session_id = %session_id, capability = %capability, "approval request");
                        let _ = forward_tx
                            .send(DaemonMessage::ApprovalRequest {
                                session_id,
                                approval_id,
                                capability,
                                context,
                            })
                            .await;
                    }
                    ClaudeEvent::Result { cost_usd, duration_ms } => {
                        info!(session_id = %session_id, cost = ?cost_usd, duration = ?duration_ms, "session completed");
                        let _ = forward_tx
                            .send(DaemonMessage::SessionEnded {
                                session_id,
                                reason: "completed".to_string(),
                            })
                            .await;
                    }
                    ClaudeEvent::SystemInit { model, session_id: sid, cwd } => {
                        debug!(session_id = %session_id, model = ?model, remote_sid = ?sid, cwd = ?cwd, "session init");
                    }
                    ClaudeEvent::ToolResult { .. } | ClaudeEvent::Raw(_) => {}
                }
            }

            warn!(session_id = %session_id, "Claude Code stdout closed");
            let _ = forward_tx
                .send(DaemonMessage::SessionEnded {
                    session_id,
                    reason: "process exited".to_string(),
                })
                .await;
        });

        // Background: git workspace status every 30s
        let ws_workdir = config.workdir.clone();
        let ws_tx = event_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
                let status = git_workspace_status(session_id, &ws_workdir).await;
                let _ = ws_tx.send(status).await;
            }
        });

        // Background: wait for child exit
        tokio::spawn(async move {
            let status = child.wait().await;
            info!(session_id = %session_id, status = ?status, "Claude Code process exited");
        });

        let handle = SessionHandle { input_tx };
        self.sessions.write().await.insert(id, handle);
        Ok(info)
    }

    async fn send_input(&self, session_id: &Uuid, data: &str) -> RelayResult<()> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(session_id)
            .ok_or_else(|| RelayError::SessionNotFound(session_id.to_string()))?;
        handle
            .input_tx
            .send(data.to_string())
            .await
            .map_err(|e| RelayError::Adapter(e.to_string()))
    }

    async fn kill(&self, session_id: &Uuid) -> RelayResult<()> {
        let mut sessions = self.sessions.write().await;
        if sessions.remove(session_id).is_some() {
            Ok(())
        } else {
            Err(RelayError::SessionNotFound(session_id.to_string()))
        }
    }

    async fn resize(&self, _session_id: &Uuid, _cols: u16, _rows: u16) -> RelayResult<()> {
        Ok(())
    }
}

/// Run git commands in `workdir` and assemble a [`DaemonMessage::WorkspaceStatus`].
async fn git_workspace_status(session_id: Uuid, workdir: &str) -> DaemonMessage {
    let branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(workdir)
        .output()
        .await
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != "HEAD")
            } else {
                None
            }
        });

    let (modified, staged) = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(workdir)
        .output()
        .await
        .map(|o| {
            if !o.status.success() { return (0u32, 0u32); }
            let text = String::from_utf8_lossy(&o.stdout);
            let (mut m, mut s) = (0u32, 0u32);
            for line in text.lines() {
                if line.len() < 2 { continue; }
                let sc = line.chars().next().unwrap_or(' ');
                let uc = line.chars().nth(1).unwrap_or(' ');
                if sc != ' ' && sc != '?' { s += 1; }
                if uc != ' ' && uc != '?' { m += 1; }
            }
            (m, s)
        })
        .unwrap_or((0, 0));

    let last_commit = Command::new("git")
        .args(["log", "-1", "--format=%h %s"])
        .current_dir(workdir)
        .output()
        .await
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
            } else {
                None
            }
        });

    DaemonMessage::WorkspaceStatus { session_id, branch, modified, staged, last_commit }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_command_includes_print_and_stream_json() {
        let config = SpawnConfig {
            name: "test".to_string(),
            workdir: "/tmp".to_string(),
            model: None,
            session_id: None,
        };
        let cmd = ClaudeAdapter::build_command(&config);
        assert!(cmd.contains(&"--print".to_string()));
        assert!(cmd.contains(&"stream-json".to_string()));
        assert!(cmd.contains(&"--input-format".to_string()));
        assert!(cmd.contains(&"--verbose".to_string()));
    }
}
