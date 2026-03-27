//! WebSocket client to broomva.tech relay edge.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use life_relay_core::{DaemonMessage, ServerMessage};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

/// Maximum reconnect backoff.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Run the WebSocket connection loop with auto-reconnect.
pub async fn run_connection(
    server_url: &str,
    outbound_rx: &mut mpsc::Receiver<DaemonMessage>,
    inbound_tx: &mpsc::Sender<ServerMessage>,
) {
    let mut backoff = Duration::from_secs(1);

    loop {
        info!(url = %server_url, "connecting to relay server");

        match connect_async(server_url).await {
            Ok((ws_stream, _)) => {
                info!("connected to relay server");
                backoff = Duration::from_secs(1); // reset on success

                let (mut write, mut read) = ws_stream.split();

                // Send node info on connect
                let hostname = hostname::get()
                    .map(|h| h.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "unknown".to_string());

                let node_info = DaemonMessage::NodeInfo {
                    name: hostname.clone(),
                    hostname,
                    capabilities: vec![
                        "claude-code".to_string(),
                        "codex".to_string(),
                        "arcan".to_string(),
                    ],
                };

                if let Ok(json) = serde_json::to_string(&node_info) {
                    let _ = write.send(Message::Text(json.into())).await;
                }

                // Message loop
                loop {
                    tokio::select! {
                        // Receive from server
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    match serde_json::from_str::<ServerMessage>(&text) {
                                        Ok(server_msg) => {
                                            if inbound_tx.send(server_msg).await.is_err() {
                                                error!("daemon receiver dropped");
                                                return;
                                            }
                                        }
                                        Err(e) => warn!(error = %e, "invalid server message"),
                                    }
                                }
                                Some(Ok(Message::Ping(data))) => {
                                    let _ = write.send(Message::Pong(data)).await;
                                }
                                Some(Ok(Message::Close(_))) | None => {
                                    info!("server closed connection");
                                    break;
                                }
                                Some(Err(e)) => {
                                    warn!(error = %e, "websocket error");
                                    break;
                                }
                                _ => {}
                            }
                        }
                        // Send to server
                        Some(daemon_msg) = outbound_rx.recv() => {
                            if let Ok(json) = serde_json::to_string(&daemon_msg) {
                                if write.send(Message::Text(json.into())).await.is_err() {
                                    warn!("failed to send to server");
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, backoff = ?backoff, "connection failed, retrying");
            }
        }

        // Reconnect with exponential backoff
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}
