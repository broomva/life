//! HTTP polling connection to broomva.tech relay edge.
//!
//! Replaces WebSocket with Vercel-compatible HTTP polling:
//! - POST /api/relay/connect — register node, get `node_id`
//! - GET  /api/relay/poll?nodeId=xxx — poll for commands (1-2s interval)
//! - POST /api/relay/events — push session output events

use std::time::Duration;

use life_relay_core::{DaemonMessage, ServerMessage};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[derive(Deserialize)]
struct ConnectResponse {
    #[serde(rename = "nodeId")]
    node_id: String,
    status: String,
}

#[derive(Deserialize)]
struct PollResponse {
    command: Option<ServerMessage>,
}

/// Register this node with the relay server.
pub async fn register_node(
    client: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
    hostname: &str,
) -> anyhow::Result<String> {
    let url = format!("{server_url}/api/relay/connect");
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .json(&serde_json::json!({
            "name": name,
            "hostname": hostname,
            "capabilities": ["claude-code", "codex", "arcan"],
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("register failed: {body}");
    }

    let data: ConnectResponse = resp.json().await?;
    info!(node_id = %data.node_id, status = %data.status, "node registered");
    Ok(data.node_id)
}

/// Poll for commands from the server (non-blocking).
async fn poll_commands(
    client: &reqwest::Client,
    server_url: &str,
    token: &str,
    node_id: &str,
) -> Option<ServerMessage> {
    let url = format!("{server_url}/api/relay/poll?nodeId={node_id}");
    match client.get(&url).bearer_auth(token).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<PollResponse>().await {
            Ok(data) => data.command,
            Err(e) => {
                warn!(error = %e, "failed to parse poll response");
                None
            }
        },
        Ok(resp) => {
            warn!(status = %resp.status(), "poll returned error");
            None
        }
        Err(e) => {
            warn!(error = %e, "poll request failed");
            None
        }
    }
}

/// Push events to the server.
pub async fn push_events(
    client: &reqwest::Client,
    server_url: &str,
    token: &str,
    node_id: &str,
    events: &[DaemonMessage],
) -> bool {
    let url = format!("{server_url}/api/relay/events");
    let payload = serde_json::json!({
        "nodeId": node_id,
        "events": events,
    });
    tracing::debug!(payload = %serde_json::to_string(&payload).unwrap_or_default(), "pushing events");
    match client
        .post(&url)
        .bearer_auth(token)
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => true,
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "push events failed");
            false
        }
        Err(e) => {
            warn!(error = %e, "push events request failed");
            false
        }
    }
}

/// Run the polling connection loop.
///
/// Uses adaptive polling: 200ms when streaming events are flowing,
/// 2s otherwise. Reverts to slow polling after 5s of no streaming events.
pub async fn run_polling_loop(
    client: &reqwest::Client,
    server_url: &str,
    token: &str,
    node_id: &str,
    outbound_rx: &mut mpsc::Receiver<DaemonMessage>,
    inbound_tx: &mpsc::Sender<ServerMessage>,
) {
    let slow_interval = Duration::from_secs(2);
    let fast_interval = Duration::from_millis(200);
    let fast_timeout = Duration::from_secs(5);

    let mut event_buffer: Vec<DaemonMessage> = Vec::new();
    let mut last_streaming_event = tokio::time::Instant::now() - fast_timeout;

    info!("starting polling loop (adaptive: {}ms / {}ms)", fast_interval.as_millis(), slow_interval.as_millis());

    loop {
        // 1. Poll for commands
        if let Some(cmd) = poll_commands(client, server_url, token, node_id).await {
            if inbound_tx.send(cmd).await.is_err() {
                error!("daemon receiver dropped");
                return;
            }
        }

        // 2. Drain outbound events and push them
        let had_events = !event_buffer.is_empty();
        while let Ok(msg) = outbound_rx.try_recv() {
            // Track whether any streaming events arrived this cycle
            if matches!(msg, DaemonMessage::ContentDelta { .. }
                | DaemonMessage::ContentBlockStart { .. }
                | DaemonMessage::ContentBlockStop { .. })
            {
                last_streaming_event = tokio::time::Instant::now();
            }
            event_buffer.push(msg);
        }

        // On failure, keep events in buffer for next cycle.
        if !event_buffer.is_empty()
            && push_events(client, server_url, token, node_id, &event_buffer).await
        {
            // Events arrived this cycle — mark as streaming if buffer was freshly populated
            if !had_events {
                last_streaming_event = tokio::time::Instant::now();
            }
            event_buffer.clear();
        }

        // 3. Adaptive wait: fast polling while streaming, slow otherwise
        let poll_interval = if last_streaming_event.elapsed() < fast_timeout {
            fast_interval
        } else {
            slow_interval
        };
        tokio::time::sleep(poll_interval).await;
    }
}
