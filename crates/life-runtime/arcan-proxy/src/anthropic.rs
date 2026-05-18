//! Anthropic Messages API backed `ArcanCall` for lifed.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use life_runtime_proto::life::v1::{AgentEvent, AgentEventKind, EventRecord};
use serde::{Deserialize, Serialize};

use crate::client::ArcanCall;
use crate::error::{ArcanProxyError, ArcanProxyResult};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";
pub const DEFAULT_MAX_TOKENS: u32 = 4096;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_HISTORY_MESSAGES: usize = 20;

#[derive(Debug, Clone)]
pub struct AnthropicArcanConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub max_tokens: u32,
    pub request_timeout: Duration,
    pub system_prompt: Option<String>,
}

impl AnthropicArcanConfig {
    pub fn from_env() -> ArcanProxyResult<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
            ArcanProxyError::Transport("AnthropicArcan requires ANTHROPIC_API_KEY".to_string())
        })?;
        Ok(Self {
            api_key,
            base_url: std::env::var("ANTHROPIC_BASE_URL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model: std::env::var("ANTHROPIC_MODEL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            max_tokens: std::env::var("ANTHROPIC_MAX_TOKENS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_MAX_TOKENS),
            request_timeout: std::env::var("LIFED_ARCAN_REQUEST_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_REQUEST_TIMEOUT),
            system_prompt: std::env::var("LIFED_ARCAN_SYSTEM_PROMPT").ok(),
        })
    }
}

#[derive(Clone)]
pub struct AnthropicArcan {
    cfg: Arc<AnthropicArcanConfig>,
    client: reqwest::Client,
    history: Arc<Mutex<HashMap<String, Vec<ChatMessage>>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

impl AnthropicArcan {
    pub fn new(cfg: AnthropicArcanConfig) -> ArcanProxyResult<Self> {
        if cfg.api_key.trim().is_empty() {
            return Err(ArcanProxyError::Transport(
                "AnthropicArcanConfig.api_key must not be empty".to_string(),
            ));
        }
        let base_url = cfg.base_url.trim_end_matches('/').to_string();
        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            return Err(ArcanProxyError::Transport(format!(
                "AnthropicArcanConfig.base_url must be http(s):// (got `{}`)",
                cfg.base_url
            )));
        }
        let client = reqwest::Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .map_err(|e| ArcanProxyError::Transport(format!("build reqwest client: {e}")))?;
        Ok(Self {
            cfg: Arc::new(AnthropicArcanConfig { base_url, ..cfg }),
            client,
            history: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn from_env() -> ArcanProxyResult<Self> {
        Self::new(AnthropicArcanConfig::from_env()?)
    }

    fn request_body(&self, sid: &str, content: &str) -> serde_json::Value {
        let prior = self
            .history
            .lock()
            .ok()
            .and_then(|h| h.get(sid).cloned())
            .unwrap_or_default();
        let mut messages: Vec<serde_json::Value> = prior
            .into_iter()
            .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
            .collect();
        messages.push(serde_json::json!({"role": "user", "content": content}));
        let mut body = serde_json::json!({
            "model": self.cfg.model,
            "max_tokens": self.cfg.max_tokens,
            "messages": messages,
            "stream": true
        });
        if let Some(system) = &self.cfg.system_prompt
            && !system.trim().is_empty()
        {
            body["system"] = serde_json::json!(system);
        }
        body
    }

    fn append_exchange(&self, sid: &str, user: String, assistant: String) {
        let Ok(mut history) = self.history.lock() else {
            return;
        };
        let entry = history.entry(sid.to_string()).or_default();
        entry.push(ChatMessage {
            role: "user".to_string(),
            content: user,
        });
        if !assistant.is_empty() {
            entry.push(ChatMessage {
                role: "assistant".to_string(),
                content: assistant,
            });
        }
        if entry.len() > MAX_HISTORY_MESSAGES {
            entry.drain(0..entry.len() - MAX_HISTORY_MESSAGES);
        }
    }
}

#[async_trait]
impl ArcanCall for AnthropicArcan {
    async fn create_agent(&self, sid: &str) -> ArcanProxyResult<String> {
        Ok(format!("agent-{sid}"))
    }

    async fn destroy_agent(&self, _sid: &str) -> ArcanProxyResult<()> {
        Ok(())
    }

    async fn dispatch_message(
        &self,
        sid: &str,
        content: &str,
    ) -> ArcanProxyResult<Pin<Box<dyn Stream<Item = Result<AgentEvent, tonic::Status>> + Send>>>
    {
        let url = format!("{}/v1/messages", self.cfg.base_url);
        let resp = self
            .client
            .post(&url)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("x-api-key", &self.cfg.api_key)
            .json(&self.request_body(sid, content))
            .send()
            .await
            .map_err(|e| ArcanProxyError::Transport(format!("POST {url}: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            return Err(ArcanProxyError::Transport(format!(
                "POST {url} returned HTTP {status}: {}",
                truncate(&String::from_utf8_lossy(&bytes), 256)
            )));
        }
        let parsed = parse_sse(
            resp.bytes_stream(),
            aios_proto::aios::v1::SessionId {
                value: sid.to_string(),
            },
        );
        Ok(record_response(
            parsed,
            self.clone(),
            sid.to_string(),
            content.to_string(),
        ))
    }
}

fn record_response(
    stream: Pin<Box<dyn Stream<Item = Result<AgentEvent, tonic::Status>> + Send>>,
    arcan: AnthropicArcan,
    sid: String,
    user: String,
) -> Pin<Box<dyn Stream<Item = Result<AgentEvent, tonic::Status>> + Send>> {
    use futures::stream;
    Box::pin(stream::unfold(
        (stream, arcan, sid, user, String::new(), false),
        |(mut stream, arcan, sid, user, mut assistant, mut recorded)| async move {
            match stream.next().await {
                Some(Ok(evt)) => {
                    if evt.kind() == AgentEventKind::Token
                        && let Some(record) = evt.record.as_ref()
                        && let Ok(payload) =
                            serde_json::from_slice::<serde_json::Value>(&record.payload)
                        && let Some(text) = payload.get("text").and_then(|v| v.as_str())
                    {
                        assistant.push_str(text);
                    }
                    if evt.kind() == AgentEventKind::Finish && !recorded {
                        arcan.append_exchange(&sid, user.clone(), assistant.clone());
                        recorded = true;
                    }
                    Some((Ok(evt), (stream, arcan, sid, user, assistant, recorded)))
                }
                Some(Err(err)) => Some((Err(err), (stream, arcan, sid, user, assistant, recorded))),
                None => {
                    if !recorded {
                        arcan.append_exchange(&sid, user, assistant);
                    }
                    None
                }
            }
        },
    ))
}

fn parse_sse<S>(
    body: S,
    session_id: aios_proto::aios::v1::SessionId,
) -> Pin<Box<dyn Stream<Item = Result<AgentEvent, tonic::Status>> + Send>>
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    use futures::stream;
    let body: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>> =
        Box::pin(body);
    Box::pin(stream::unfold(
        (body, Vec::new(), 0_u64, false, session_id),
        |(mut body, mut buffer, mut sequence, done, session_id)| async move {
            if done {
                return None;
            }
            loop {
                if let Some(end) = double_newline(&buffer) {
                    let record =
                        String::from_utf8_lossy(&buffer.drain(..end + 2).collect::<Vec<_>>())
                            .to_string();
                    for line in record.lines() {
                        let Some(data) = line
                            .strip_prefix("data: ")
                            .or_else(|| line.strip_prefix("data:"))
                        else {
                            continue;
                        };
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(data.trim()) else {
                            continue;
                        };
                        match v.get("type").and_then(|t| t.as_str()) {
                            Some("content_block_delta") => {
                                if let Some(text) = v
                                    .get("delta")
                                    .and_then(|d| d.get("text"))
                                    .and_then(|t| t.as_str())
                                    && !text.is_empty()
                                {
                                    sequence += 1;
                                    return Some((
                                        Ok(event(
                                            AgentEventKind::Token,
                                            "TOKEN",
                                            &session_id,
                                            sequence,
                                            serde_json::json!({"text": text}),
                                        )),
                                        (body, buffer, sequence, false, session_id),
                                    ));
                                }
                            }
                            Some("message_stop") => {
                                sequence += 1;
                                return Some((
                                    Ok(event(
                                        AgentEventKind::Finish,
                                        "FINISH",
                                        &session_id,
                                        sequence,
                                        serde_json::json!({"reason": "stop"}),
                                    )),
                                    (body, buffer, sequence, true, session_id),
                                ));
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                match body.next().await {
                    Some(Ok(chunk)) => buffer.extend_from_slice(&chunk),
                    Some(Err(err)) => {
                        return Some((
                            Err(tonic::Status::internal(format!(
                                "AnthropicArcan: stream read error: {err}"
                            ))),
                            (body, buffer, sequence, true, session_id),
                        ));
                    }
                    None => {
                        sequence += 1;
                        return Some((
                            Ok(event(
                                AgentEventKind::Finish,
                                "FINISH",
                                &session_id,
                                sequence,
                                serde_json::json!({"reason": "upstream_closed"}),
                            )),
                            (body, buffer, sequence, true, session_id),
                        ));
                    }
                }
            }
        },
    ))
}

fn event(
    kind: AgentEventKind,
    record_kind: &str,
    session_id: &aios_proto::aios::v1::SessionId,
    sequence: u64,
    payload: serde_json::Value,
) -> AgentEvent {
    AgentEvent {
        record: Some(EventRecord {
            session_id: Some(session_id.clone()),
            sequence,
            at: now_timestamp(),
            kind: record_kind.to_string(),
            payload: serde_json::to_vec(&payload).unwrap_or_default(),
        }),
        kind: kind as i32,
    }
}

fn double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2)
        .enumerate()
        .find(|(_, w)| *w == b"\n\n")
        .map(|(i, _)| i)
}

fn now_timestamp() -> Option<prost_types::Timestamp> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(prost_types::Timestamp {
        seconds: now.as_secs() as i64,
        nanos: now.subsec_nanos() as i32,
    })
}

fn truncate(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        s.to_string()
    } else {
        format!("{}...", &s[..limit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;

    #[tokio::test]
    async fn parses_text_delta_and_finish() {
        let body = b"event: content_block_delta\n\
                     data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n\
                     event: message_stop\n\
                     data: {\"type\":\"message_stop\"}\n\n";
        let mut s = parse_sse(
            stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(body.to_vec()))]),
            aios_proto::aios::v1::SessionId { value: "s".into() },
        );
        assert_eq!(
            s.next().await.unwrap().unwrap().kind(),
            AgentEventKind::Token
        );
        assert_eq!(
            s.next().await.unwrap().unwrap().kind(),
            AgentEventKind::Finish
        );
    }
}
