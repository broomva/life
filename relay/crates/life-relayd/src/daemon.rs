//! Main daemon event loop — bridges HTTP polling to local session adapters.

use anyhow::Result;
use life_relay_core::{DaemonMessage, ServerMessage};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::adapters::claude;
use crate::config;
use crate::connection;

/// Run the relay daemon: register node, start local API + polling loop.
pub async fn run(bind: &str, server_url: &str) -> Result<()> {
    let cfg = config::load_config()?;

    // Load stored auth token
    let token = config::read_token(&cfg)?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // Derive server base URL (strip /api/relay/connect path if present)
    let base_url = server_url
        .trim_end_matches("/api/relay/connect")
        .trim_end_matches('/');

    // Register this node
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

    // Channels for communication
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

    // Start the HTTP polling loop
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

    // Process incoming server commands
    let outbound = outbound_tx.clone();
    let cmd_handle = tokio::spawn(async move {
        while let Some(msg) = inbound_rx.recv().await {
            match msg {
                ServerMessage::Ping => {
                    let _ = outbound.send(DaemonMessage::Pong).await;
                }
                ServerMessage::ListSessions => {
                    let _ = outbound
                        .send(DaemonMessage::SessionList { sessions: vec![] })
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
                ServerMessage::Input { session_id, data } => {
                    warn!(session_id = %session_id, len = data.len(), "input not yet routed to PTY");
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
    poll_handle.abort();
    cmd_handle.abort();

    info!("life-relayd stopped");
    Ok(())
}
