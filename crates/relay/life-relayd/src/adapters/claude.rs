//! Claude Code adapter — spawns and manages Claude Code sessions.
//!
//! Uses `claude --print --output-format stream-json --verbose` in non-interactive
//! mode. Each user message spawns a new Claude Code process; conversation context
//! is preserved via `--continue` which resumes the most recent session in the
//! working directory.
//!
//! This gives clean JSONL output without ANSI contamination, and multi-turn
//! conversation support through Claude Code's built-in session resumption.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use life_relay_core::{
    DaemonMessage, RelayError, RelayResult, SessionInfo, SessionStatus, SessionType, SpawnConfig,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::SessionAdapter;
use super::parser::{self, ClaudeEvent};

/// State for a relay session.
/// Each user message spawns a new Claude Code process with `--continue`.
struct SessionState {
    workdir: String,
    model: Option<String>,
    /// Number of messages processed (0 = first message, uses no --continue)
    message_count: u64,
    /// Channel to send user messages for processing
    input_tx: mpsc::Sender<String>,
    /// Claude Code's internal session ID (captured from SystemInit event).
    claude_session_id: Option<String>,
}

/// Manages Claude Code sessions.
#[derive(Debug, Default)]
pub struct ClaudeAdapter {
    sessions: Arc<RwLock<HashMap<Uuid, SessionState>>>,
}

impl std::fmt::Debug for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionState")
            .field("workdir", &self.workdir)
            .field("message_count", &self.message_count)
            .finish()
    }
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store the Claude Code session ID for a relay session.
    pub async fn set_claude_session_id(&self, session_id: &Uuid, claude_sid: String) {
        if let Some(state) = self.sessions.write().await.get_mut(session_id) {
            state.claude_session_id = Some(claude_sid);
        }
    }

    /// Get the workdir and Claude session ID for a relay session.
    pub async fn get_session_info(&self, session_id: &Uuid) -> Option<(String, Option<String>)> {
        self.sessions.read().await.get(session_id).map(|s| {
            (s.workdir.clone(), s.claude_session_id.clone())
        })
    }

    /// Build the claude CLI command for a session.
    pub fn build_command(config: &SpawnConfig) -> Vec<String> {
        let mut args = vec![
            "claude".to_string(),
            "--print".to_string(),
            "--output-format".to_string(),
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

/// Run a single Claude Code invocation for one user message.
/// Returns when Claude Code finishes processing.
async fn run_claude_turn(
    session_id: Uuid,
    workdir: &str,
    model: &Option<String>,
    message: &str,
    is_continuation: bool,
    event_tx: &mpsc::Sender<DaemonMessage>,
) {
    let mut args = vec![
        "claude".to_string(),
        "--print".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
    ];

    if is_continuation {
        args.push("--continue".to_string());
    }

    if let Some(m) = model {
        args.push("--model".to_string());
        args.push(m.clone());
    }

    // The message is the positional argument
    args.push(message.to_string());

    debug!(session_id = %session_id, args = ?args, "running claude code turn");

    let child = Command::new(&args[0])
        .args(&args[1..])
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            warn!(session_id = %session_id, error = %e, "failed to spawn claude");
            return;
        }
    };

    let stdout = child.stdout.take().expect("stdout piped");
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        debug!(session_id = %session_id, line = %trimmed.get(..120).unwrap_or(trimmed), "claude stdout");

        match parser::parse_line(trimmed) {
            ClaudeEvent::AssistantText { text } => {
                if !text.is_empty() {
                    info!(session_id = %session_id, text_len = text.len(), "assistant message");
                    let _ = event_tx
                        .send(DaemonMessage::AssistantMessage { session_id, text })
                        .await;
                }
            }
            ClaudeEvent::ToolUse { id: tool_id, name, input } => {
                debug!(session_id = %session_id, tool = %name, "tool use");
                let _ = event_tx
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
                let _ = event_tx
                    .send(DaemonMessage::ApprovalRequest {
                        session_id,
                        approval_id,
                        capability,
                        context,
                    })
                    .await;
            }
            ClaudeEvent::ToolResult { tool_use_id, content, is_error } => {
                debug!(session_id = %session_id, tool_use_id = ?tool_use_id, "tool result");
                let _ = event_tx
                    .send(DaemonMessage::ToolResult {
                        session_id,
                        tool_use_id: tool_use_id.unwrap_or_default(),
                        content,
                        is_error,
                    })
                    .await;
            }
            ClaudeEvent::StreamContentDelta { index, text } => {
                let _ = event_tx
                    .send(DaemonMessage::ContentDelta {
                        session_id,
                        index,
                        text,
                    })
                    .await;
            }
            ClaudeEvent::StreamContentStart { index, block_type } => {
                let _ = event_tx
                    .send(DaemonMessage::ContentBlockStart {
                        session_id,
                        index,
                        block_type,
                    })
                    .await;
            }
            ClaudeEvent::StreamContentStop { index } => {
                let _ = event_tx
                    .send(DaemonMessage::ContentBlockStop {
                        session_id,
                        index,
                    })
                    .await;
            }
            ClaudeEvent::StreamToolInputDelta { .. } => {
                // Skip — we handle tool_use events at the aggregate level already
            }
            ClaudeEvent::Result { cost_usd, duration_ms } => {
                info!(session_id = %session_id, cost = ?cost_usd, duration = ?duration_ms, "turn completed");
                let _ = event_tx
                    .send(DaemonMessage::TurnResult {
                        session_id,
                        cost_usd,
                        duration_ms,
                        num_turns: None,
                    })
                    .await;
            }
            ClaudeEvent::SystemInit { session_id: sid, .. } => {
                if let Some(ref claude_sid) = sid {
                    let _ = event_tx
                        .send(DaemonMessage::SessionMapping {
                            session_id,
                            claude_session_id: claude_sid.clone(),
                        })
                        .await;
                }
            }
            ClaudeEvent::Raw(_) => {}
        }
    }

    let _ = child.wait().await;
}

#[async_trait]
impl SessionAdapter for ClaudeAdapter {
    async fn spawn(
        &self,
        config: &SpawnConfig,
        event_tx: mpsc::Sender<DaemonMessage>,
    ) -> RelayResult<SessionInfo> {
        let id = config.session_id.unwrap_or_else(Uuid::new_v4);

        info!(
            session_id = %id,
            workdir = %config.workdir,
            "creating claude code session"
        );

        let info = SessionInfo {
            id,
            session_type: SessionType::ClaudeCode,
            status: SessionStatus::Active,
            name: config.name.clone(),
            workdir: config.workdir.clone(),
            model: config.model.clone(),
            created_at: Utc::now(),
        };

        // Message processing loop — each message spawns a Claude Code invocation
        let (input_tx, mut input_rx) = mpsc::channel::<String>(32);
        let workdir = config.workdir.clone();
        let model = config.model.clone();
        let session_id = id;
        let forward_tx = event_tx.clone();
        tokio::spawn(async move {
            let mut turn_count: u64 = 0;

            while let Some(message) = input_rx.recv().await {
                let trimmed = message.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let is_continuation = turn_count > 0;
                turn_count += 1;

                info!(session_id = %session_id, turn = turn_count, continuation = is_continuation, "processing user message");

                run_claude_turn(
                    session_id,
                    &workdir,
                    &model,
                    trimmed,
                    is_continuation,
                    &forward_tx,
                )
                .await;
            }

            // Input channel closed — session ended
            warn!(session_id = %session_id, "session input channel closed");
            let _ = forward_tx
                .send(DaemonMessage::SessionEnded {
                    session_id,
                    reason: "session closed".to_string(),
                })
                .await;
        });

        // Background: git workspace status every 30s
        let ws_workdir = config.workdir.clone();
        let ws_tx = event_tx;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
                let status = git_workspace_status(session_id, &ws_workdir).await;
                let _ = ws_tx.send(status).await;
            }
        });

        let state = SessionState {
            workdir: config.workdir.clone(),
            model: config.model.clone(),
            message_count: 0,
            input_tx,
            claude_session_id: None,
        };
        self.sessions.write().await.insert(id, state);
        Ok(info)
    }

    async fn send_input(&self, session_id: &Uuid, data: &str) -> RelayResult<()> {
        let mut sessions = self.sessions.write().await;
        let state = sessions
            .get_mut(session_id)
            .ok_or_else(|| RelayError::SessionNotFound(session_id.to_string()))?;
        state.message_count += 1;
        state
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
            } else { None }
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
            } else { None }
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
        assert!(cmd.contains(&"--verbose".to_string()));
    }
}
