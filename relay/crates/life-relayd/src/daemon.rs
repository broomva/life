//! Main daemon event loop — bridges HTTP polling to local session adapters.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use life_relay_core::{DaemonMessage, DirEntry, ServerMessage, SessionInfo};
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
    let adapter: Arc<dyn SessionAdapter> = Arc::new(ClaudeAdapter::new());

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
    let cmd_handle = tokio::spawn(async move {
        while let Some(msg) = inbound_rx.recv().await {
            dispatch_command(msg, &adapter, &registry, &outbound).await;
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
        let is_dir = entry.file_type().await.map(|ft| ft.is_dir()).unwrap_or(false);
        entries.push(DirEntry { name, is_dir });
    }

    // Sort: directories first, then alphabetical
    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name))
    });

    Ok((canonical_str, entries))
}
