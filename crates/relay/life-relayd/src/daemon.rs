//! Main daemon event loop — bridges HTTP polling to local session adapters.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use life_relay_core::{
    DaemonMessage, DirEntry, HistoryMessage, HistoryToolUse, ServerMessage, SessionInfo,
};
use tokio::sync::{RwLock, mpsc};
use tracing::{info, warn};

use crate::adapters::SessionAdapter;
use crate::adapters::claude::ClaudeAdapter;
use crate::config;
use crate::connection;

/// Shared session registry — read by the local HTTP API.
pub type SessionRegistry = Arc<RwLock<HashMap<uuid::Uuid, SessionInfo>>>;

/// Run the relay daemon: register node, start local API + polling loop.
pub async fn run(bind: &str, server_url: &str) -> Result<()> {
    let cfg = config::load_config()?;
    let token = config::read_token(&cfg)?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let base_url = server_url
        .trim_end_matches("/api/relay/connect")
        .trim_end_matches('/');

    let node_id = connection::register_node(
        &http,
        base_url,
        &token,
        &cfg.node_name,
        &hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string()),
    )
    .await?;

    info!(node_id = %node_id, "relay node registered");

    // Shared session registry — written by daemon, read by local API.
    let session_registry: SessionRegistry = Arc::new(RwLock::new(HashMap::new()));

    // Channels bridging the polling loop and command dispatcher.
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<DaemonMessage>(256);
    let (inbound_tx, mut inbound_rx) = mpsc::channel::<ServerMessage>(256);

    // Session adapter — manages live PTY sessions.
    let claude_adapter = Arc::new(ClaudeAdapter::new());
    let adapter: Arc<dyn SessionAdapter> = claude_adapter.clone();

    // Start the local HTTP API.
    let api_state = life_relay_api::AppState {
        sessions: session_registry.clone(),
    };
    let api_router = life_relay_api::build_router(api_state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(addr = %bind, "local API listening");

    let api_handle = tokio::spawn(async move {
        axum::serve(listener, api_router)
            .with_graceful_shutdown(async {
                tokio::signal::ctrl_c().await.ok();
            })
            .await
            .ok();
    });

    // Start the HTTP polling loop.
    let poll_base = base_url.to_string();
    let poll_token = token.clone();
    let poll_node = node_id.clone();
    let poll_handle = tokio::spawn(async move {
        connection::run_polling_loop(
            &http,
            &poll_base,
            &poll_token,
            &poll_node,
            &mut outbound_rx,
            &inbound_tx,
        )
        .await;
    });

    // Dispatch incoming server commands to adapters.
    let outbound = outbound_tx.clone();
    let registry = session_registry.clone();
    let claude_for_dispatch = claude_adapter.clone();
    let cmd_handle = tokio::spawn(async move {
        while let Some(msg) = inbound_rx.recv().await {
            dispatch_command(msg, &adapter, &claude_for_dispatch, &registry, &outbound).await;
        }
    });

    tokio::signal::ctrl_c().await?;
    info!("shutdown signal received");

    api_handle.abort();
    poll_handle.abort();
    cmd_handle.abort();

    info!("life-relayd stopped");
    Ok(())
}

/// Route a single [`ServerMessage`] to the appropriate adapter action.
async fn dispatch_command(
    msg: ServerMessage,
    adapter: &Arc<dyn SessionAdapter>,
    claude_adapter: &Arc<ClaudeAdapter>,
    registry: &SessionRegistry,
    outbound: &mpsc::Sender<DaemonMessage>,
) {
    match msg {
        ServerMessage::Ping => {
            let _ = outbound.send(DaemonMessage::Pong).await;
        }

        ServerMessage::ListSessions => {
            let sessions: Vec<SessionInfo> = registry.read().await.values().cloned().collect();
            let _ = outbound.send(DaemonMessage::SessionList { sessions }).await;
        }

        ServerMessage::Spawn {
            session_type: _,
            config,
        } => {
            // Clone outbound so the adapter can emit events from its background task.
            match adapter.spawn(&config, outbound.clone()).await {
                Ok(session) => {
                    registry.write().await.insert(session.id, session.clone());
                    let _ = outbound
                        .send(DaemonMessage::SessionCreated { session })
                        .await;
                }
                Err(e) => {
                    warn!(error = %e, "failed to spawn session");
                    let _ = outbound
                        .send(DaemonMessage::Error {
                            code: "spawn_failed".to_string(),
                            message: e.to_string(),
                        })
                        .await;
                }
            }
        }

        ServerMessage::Input { session_id, data } => {
            if let Err(e) = adapter.send_input(&session_id, &data).await {
                warn!(session_id = %session_id, error = %e, "send_input failed");
            }
        }

        ServerMessage::Resize {
            session_id,
            cols,
            rows,
        } => {
            if let Err(e) = adapter.resize(&session_id, cols, rows).await {
                warn!(session_id = %session_id, error = %e, "resize failed");
            }
        }

        ServerMessage::Kill { session_id } => match adapter.kill(&session_id).await {
            Ok(()) => {
                registry.write().await.remove(&session_id);
                let _ = outbound
                    .send(DaemonMessage::SessionEnded {
                        session_id,
                        reason: "killed by user".to_string(),
                    })
                    .await;
            }
            Err(e) => {
                warn!(session_id = %session_id, error = %e, "kill failed");
            }
        },

        ServerMessage::Approve {
            session_id,
            approval_id,
            approved,
        } => {
            // Send 'y' or 'n' to the session stdin so Claude Code gets the answer.
            let answer = if approved { "y\n" } else { "n\n" };
            if let Err(e) = adapter.send_input(&session_id, answer).await {
                warn!(
                    session_id = %session_id,
                    approval_id = %approval_id,
                    error = %e,
                    "approval routing failed"
                );
            }
        }

        ServerMessage::LoadHistory {
            session_id,
            request_id,
        } => {
            let outbound = outbound.clone();
            let claude = claude_adapter.clone();
            let registry = registry.clone();
            tokio::spawn(async move {
                // Look up session info from the adapter (workdir + claude session id)
                let session_info = claude.get_session_info(&session_id).await;

                // Fall back to the registry if the adapter doesn't have it
                let (workdir, claude_sid) = match session_info {
                    Some((w, c)) => (w, c),
                    None => {
                        // Try the registry for workdir
                        let reg = registry.read().await;
                        match reg.get(&session_id) {
                            Some(info) => (info.workdir.clone(), None),
                            None => {
                                warn!(session_id = %session_id, "load_history: session not found");
                                let _ = outbound
                                    .send(DaemonMessage::Error {
                                        code: "session_not_found".to_string(),
                                        message: "Session not found for history loading"
                                            .to_string(),
                                    })
                                    .await;
                                return;
                            }
                        }
                    }
                };

                let messages = load_session_history(&workdir, claude_sid.as_deref()).await;
                let _ = outbound
                    .send(DaemonMessage::HistoryMessages {
                        session_id,
                        request_id,
                        messages,
                    })
                    .await;
            });
        }

        ServerMessage::ListDir { path, request_id } => {
            let outbound = outbound.clone();
            // Spawn a blocking task so the file I/O doesn't stall the command loop.
            tokio::spawn(async move {
                let result = list_directory(&path).await;
                match result {
                    Ok((resolved_path, entries)) => {
                        let _ = outbound
                            .send(DaemonMessage::DirListing {
                                request_id,
                                path: resolved_path,
                                entries,
                            })
                            .await;
                    }
                    Err(e) => {
                        warn!(path = %path, error = %e, "list_dir failed");
                        let _ = outbound
                            .send(DaemonMessage::Error {
                                code: "list_dir_failed".to_string(),
                                message: e.to_string(),
                            })
                            .await;
                    }
                }
            });
        }
    }
}

/// List directory contents on the local filesystem.
///
/// Resolves `~` to the user's home directory. Returns the canonical
/// path and a vector of entries (name + is_dir).
async fn list_directory(path: &str) -> Result<(String, Vec<DirEntry>)> {
    use std::path::PathBuf;

    let resolved: PathBuf = if path == "~" || path.starts_with("~/") {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        if path == "~" {
            home
        } else {
            home.join(&path[2..])
        }
    } else {
        PathBuf::from(path)
    };

    let canonical = tokio::fs::canonicalize(&resolved).await?;
    let canonical_str = canonical.to_string_lossy().to_string();

    let mut read_dir = tokio::fs::read_dir(&canonical).await?;
    let mut entries = Vec::new();

    while let Some(entry) = read_dir.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden files/dirs for cleaner browsing (user can type path directly)
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry
            .file_type()
            .await
            .map(|ft| ft.is_dir())
            .unwrap_or(false);
        entries.push(DirEntry { name, is_dir });
    }

    // Sort: directories first, then alphabetical
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));

    Ok((canonical_str, entries))
}

/// Encode a workdir path to the Claude Code project directory name.
///
/// Claude Code stores sessions at `~/.claude/projects/{encoded}/{session}.jsonl`
/// where `encoded` replaces `/` with `-` and prepends `-`.
/// E.g., `/Users/broomva/broomva` becomes `-Users-broomva-broomva`.
fn encode_workdir(workdir: &str) -> String {
    workdir.replace('/', "-")
}

/// Load conversation history from Claude Code's session JSONL files.
///
/// If a `claude_session_id` is provided, looks for the matching file.
/// Otherwise, picks the most recently modified `.jsonl` in the project directory.
async fn load_session_history(
    workdir: &str,
    claude_session_id: Option<&str>,
) -> Vec<HistoryMessage> {
    use std::path::PathBuf;

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let encoded = encode_workdir(workdir);
    let project_dir = home.join(".claude").join("projects").join(&encoded);

    if !project_dir.exists() {
        info!(workdir = %workdir, encoded = %encoded, "no claude project dir found");
        return Vec::new();
    }

    // Find the target JSONL file
    let jsonl_path = if let Some(sid) = claude_session_id {
        let path = project_dir.join(format!("{sid}.jsonl"));
        if path.exists() {
            Some(path)
        } else {
            // Fall back to most recent file
            find_most_recent_jsonl(&project_dir).await
        }
    } else {
        find_most_recent_jsonl(&project_dir).await
    };

    let Some(path) = jsonl_path else {
        info!(dir = %project_dir.display(), "no JSONL files found in project dir");
        return Vec::new();
    };

    info!(path = %path.display(), "loading session history");

    match tokio::fs::read_to_string(&path).await {
        Ok(content) => parse_jsonl_history(&content),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to read JSONL file");
            Vec::new()
        }
    }
}

/// Find the most recently modified `.jsonl` file in a directory.
async fn find_most_recent_jsonl(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut read_dir = tokio::fs::read_dir(dir).await.ok()?;
    let mut best: Option<(std::path::PathBuf, std::time::SystemTime)> = None;

    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Ok(meta) = entry.metadata().await {
            if let Ok(modified) = meta.modified() {
                if best.as_ref().map_or(true, |(_, t)| modified > *t) {
                    best = Some((path, modified));
                }
            }
        }
    }

    best.map(|(p, _)| p)
}

/// Parse JSONL content into a list of history messages.
///
/// Extracts `user` and `assistant` messages, skipping system events,
/// thinking blocks, and queue operations.
fn parse_jsonl_history(content: &str) -> Vec<HistoryMessage> {
    let mut messages = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(obj) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        let Some(msg_type) = obj.get("type").and_then(|t| t.as_str()) else {
            continue;
        };

        let timestamp = obj
            .get("timestamp")
            .and_then(|t| t.as_str())
            .map(str::to_owned);

        match msg_type {
            "user" => {
                // Extract text from message.content array
                let text = obj
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();

                if !text.is_empty() {
                    messages.push(HistoryMessage {
                        role: "user".to_string(),
                        text,
                        tools: Vec::new(),
                        timestamp,
                    });
                }
            }
            "assistant" => {
                let content_arr = obj
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array());

                let Some(blocks) = content_arr else {
                    continue;
                };

                let mut text_parts = Vec::new();
                let mut tools = Vec::new();

                for block in blocks {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                if !t.is_empty() {
                                    text_parts.push(t.to_string());
                                }
                            }
                        }
                        "tool_use" => {
                            let name = block
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let input_preview = block
                                .get("input")
                                .map(|i| {
                                    let s = serde_json::to_string(i).unwrap_or_default();
                                    if s.len() > 100 {
                                        format!("{}...", &s[..100])
                                    } else {
                                        s
                                    }
                                })
                                .unwrap_or_default();
                            tools.push(HistoryToolUse {
                                name,
                                input_preview,
                            });
                        }
                        // Skip "thinking" and other block types
                        _ => {}
                    }
                }

                let text = text_parts.join("\n");
                if !text.is_empty() || !tools.is_empty() {
                    messages.push(HistoryMessage {
                        role: "assistant".to_string(),
                        text,
                        tools,
                        timestamp,
                    });
                }
            }
            // Skip system, queue-operation, result, tool_result, etc.
            _ => {}
        }
    }

    messages
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn encode_workdir_replaces_slashes() {
        assert_eq!(
            encode_workdir("/Users/broomva/broomva"),
            "-Users-broomva-broomva"
        );
        assert_eq!(encode_workdir("/tmp"), "-tmp");
    }

    #[test]
    fn parse_user_message() {
        let jsonl =
            r#"{"type":"user","message":{"content":[{"type":"text","text":"hello world"}]}}"#;
        let messages = parse_jsonl_history(jsonl);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].text, "hello world");
        assert!(messages[0].tools.is_empty());
    }

    #[test]
    fn parse_assistant_message_with_tools() {
        let jsonl = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"I'll check that"},{"type":"tool_use","name":"Bash","input":{"command":"ls -la"}}]}}"#;
        let messages = parse_jsonl_history(jsonl);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "assistant");
        assert_eq!(messages[0].text, "I'll check that");
        assert_eq!(messages[0].tools.len(), 1);
        assert_eq!(messages[0].tools[0].name, "Bash");
    }

    #[test]
    fn skips_system_and_result_events() {
        let jsonl = "
{\"type\":\"system\",\"session_id\":\"abc\",\"model\":\"claude-opus-4-5\"}
{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}
{\"type\":\"result\",\"cost_usd\":0.01}
";
        let messages = parse_jsonl_history(jsonl);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
    }

    #[test]
    fn skips_thinking_blocks() {
        let jsonl = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Here's the answer"}]}}"#;
        let messages = parse_jsonl_history(jsonl);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "Here's the answer");
    }
}
