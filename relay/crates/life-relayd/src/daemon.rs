//! Main daemon event loop — bridges WebSocket commands to local session adapters.

use anyhow::Result;
use life_relay_core::{DaemonMessage, ServerMessage};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::adapters::claude;
use crate::connection;

/// Run the relay daemon: start local API + WebSocket connection loop.
pub async fn run(bind: &str, server_url: &str) -> Result<()> {
    // Channels for WebSocket communication
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<DaemonMessage>(256);
    let (inbound_tx, mut inbound_rx) = mpsc::channel::<ServerMessage>(256);

    // Start the local HTTP API
    let api_router = life_relay_api::build_router();
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

    // Start the WebSocket connection loop
    let ws_url = server_url.to_string();
    let ws_handle = tokio::spawn(async move {
        connection::run_connection(&ws_url, &mut outbound_rx, &inbound_tx).await;
    });

    // Process incoming server messages
    let outbound = outbound_tx.clone();
    let cmd_handle = tokio::spawn(async move {
        while let Some(msg) = inbound_rx.recv().await {
            match msg {
                ServerMessage::Ping => {
                    let _ = outbound.send(DaemonMessage::Pong).await;
                }
                ServerMessage::ListSessions => {
                    let _ = outbound
                        .send(DaemonMessage::SessionList {
                            sessions: vec![],
                        })
                        .await;
                }
                ServerMessage::Spawn {
                    session_type: _,
                    config,
                } => {
                    let session = claude::mock_session_info(&config);
                    let _ = outbound
                        .send(DaemonMessage::SessionCreated { session })
                        .await;
                }
                ServerMessage::Input {
                    session_id,
                    data,
                } => {
                    warn!(session_id = %session_id, len = data.len(), "input not yet implemented");
                }
                ServerMessage::Kill { session_id } => {
                    info!(session_id = %session_id, "kill requested");
                    let _ = outbound
                        .send(DaemonMessage::SessionEnded {
                            session_id,
                            reason: "killed by user".to_string(),
                        })
                        .await;
                }
                ServerMessage::Approve { .. } | ServerMessage::Resize { .. } => {
                    warn!("not yet implemented");
                }
            }
        }
    });

    // Wait for shutdown
    tokio::signal::ctrl_c().await?;
    info!("shutdown signal received");

    api_handle.abort();
    ws_handle.abort();
    cmd_handle.abort();

    info!("life-relayd stopped");
    Ok(())
}
