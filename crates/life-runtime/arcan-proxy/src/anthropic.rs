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
use crate::conversions::now_timestamp;
use crate::error::{ArcanProxyError, ArcanProxyResult};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";
pub const DEFAULT_MAX_TOKENS: u32 = 4096;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_HISTORY_MESSAGES: usize = 20;

/// `AnthropicArcanConfig` carries the credential + connection settings
/// for the Anthropic Messages-backed `ArcanCall` adapter.
///
/// **Security:** [`Debug`] is implemented manually to redact `api_key`.
/// Any future `tracing::debug!("{cfg:?}")` / `dbg!` / Vigil span attribute
/// MUST stay safe by construction — the rest of the workspace uses the
/// same redact-on-Debug pattern for secrets (`Zeroizing<String>` is the
/// other axis, used in Spec D KMS keystore).
#[derive(Clone)]
pub struct AnthropicArcanConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub max_tokens: u32,
    pub request_timeout: Duration,
    pub system_prompt: Option<String>,
}

impl std::fmt::Debug for AnthropicArcanConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicArcanConfig")
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("request_timeout", &self.request_timeout)
            .field("system_prompt", &self.system_prompt)
            .finish()
    }
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
            // Defaults to the grounded persona when the env var is
            // unset/blank (see `crate::grounding`); an explicit override
            // still wins wholesale.
            system_prompt: crate::grounding::resolve_system_prompt(),
        })
    }
}

#[derive(Clone)]
pub struct AnthropicArcan {
    cfg: Arc<AnthropicArcanConfig>,
    client: reqwest::Client,
    // TODO(BRO-1143): cross-sid eviction + LRU cap. The per-sid bound
    // (`MAX_HISTORY_MESSAGES`) is sufficient for J-Sub-B's text-only flows,
    // but production tool flows (J-Sub-D, when `AnthropicArcan` wires into
    // lifed as a real `ArcanCall` impl) will accumulate sids indefinitely.
    // Wrap in an LRU keyed by last-touch instant, or piggy-back on the
    // routing-cache idle-TTL pattern, when J-Sub-D lands.
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

    /// Build the Anthropic Messages API request body.
    ///
    /// BRO-1206: when `model_override` is `Some(non_empty)` the override
    /// wins over `self.cfg.model` (env-bound default — `ANTHROPIC_MODEL`
    /// or [`DEFAULT_MODEL`]). Empty / whitespace / `None` falls back to
    /// the env default. Per-call override means a single backend can
    /// serve sessions on different Claude models without re-construction.
    fn request_body(
        &self,
        sid: &str,
        content: &str,
        model_override: Option<&str>,
    ) -> serde_json::Value {
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
        let model: &str = model_override
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(self.cfg.model.as_str());
        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": self.cfg.max_tokens,
            "messages": messages,
            "stream": true
        });
        if let Some(system) = &self.cfg.system_prompt
            && !system.trim().is_empty()
        {
            // Emit `system` as a cacheable content block so Anthropic
            // prompt caching amortizes the (now always-present) grounding
            // persona across a multi-turn conversation instead of
            // re-billing it on every turn. The Messages API accepts both
            // the bare-string and block-array forms for `system`; only the
            // array form can carry `cache_control`.
            body["system"] = serde_json::json!([{
                "type": "text",
                "text": system,
                "cache_control": { "type": "ephemeral" },
            }]);
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
        model: Option<&str>,
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
            // BRO-1206: per-call override or env-bound default in the
            // outbound `model` field.
            .json(&self.request_body(sid, content, model))
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

/// State carried across SSE parse iterations.
///
/// Tracks upstream content-block indices that are bound to `tool_use`
/// blocks so that subsequent `content_block_delta` / `content_block_stop`
/// frames can look up the tool_use id without re-parsing the original
/// start frame.
///
/// J-Sub-D (BRO-1143) extension: previously this parser was text-only;
/// for the Anthropic tool-use bridge it must thread `(index → tool_use_id)`
/// state across frames so that `input_json_delta` events know which tool
/// the partial JSON belongs to.
#[derive(Default)]
struct ParserState {
    /// Maps upstream `content_block` index → Anthropic `tool_use_id` for
    /// every currently-open tool_use block.
    tool_use_by_index: std::collections::HashMap<u64, String>,
    /// Most recent stop_reason carried on `message_delta`. Surfaces in
    /// the synthesized `Finish` event so the encoder downstream can
    /// emit Anthropic's `message_delta {stop_reason: "tool_use"}`.
    last_stop_reason: Option<String>,
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
        (
            body,
            Vec::new(),
            0_u64,
            false,
            session_id,
            ParserState::default(),
        ),
        |(mut body, mut buffer, mut sequence, done, session_id, mut state)| async move {
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
                            Some("content_block_start") => {
                                // J-Sub-D: tool_use blocks open here.
                                // Track the upstream index → id mapping
                                // and synthesize a `ToolCallPending`
                                // event the encoder uses to open the
                                // Anthropic-side tool_use content block.
                                if let Some((idx, id, name)) = parse_tool_use_start(&v) {
                                    state.tool_use_by_index.insert(idx, id.clone());
                                    sequence += 1;
                                    return Some((
                                        Ok(event(
                                            AgentEventKind::ToolCallPending,
                                            "TOOL_CALL_PENDING",
                                            &session_id,
                                            sequence,
                                            serde_json::json!({
                                                "id": id,
                                                "name": name,
                                                "input": {},
                                            }),
                                        )),
                                        (body, buffer, sequence, false, session_id, state),
                                    ));
                                }
                            }
                            Some("content_block_delta") => {
                                // Text deltas — existing path.
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
                                        (body, buffer, sequence, false, session_id, state),
                                    ));
                                }
                                // J-Sub-D: `input_json_delta` carries the
                                // streamed tool_use input JSON. Look up
                                // the tool_use id by the upstream index
                                // (stashed at content_block_start) and
                                // emit a ToolCallPending event with the
                                // partial JSON fragment.
                                //
                                // If the index is unknown (upstream is
                                // misbehaving) the delta is dropped
                                // silently rather than panicking — keep
                                // the stream healthy.
                                if let Some((idx, partial)) = parse_input_json_delta(&v)
                                    && let Some(id) = state.tool_use_by_index.get(&idx)
                                {
                                    sequence += 1;
                                    let id = id.clone();
                                    return Some((
                                        Ok(event(
                                            AgentEventKind::ToolCallPending,
                                            "TOOL_CALL_PENDING",
                                            &session_id,
                                            sequence,
                                            serde_json::json!({
                                                "id": id,
                                                "name": "",
                                                "partial_json": partial,
                                            }),
                                        )),
                                        (body, buffer, sequence, false, session_id, state),
                                    ));
                                }
                            }
                            Some("content_block_stop") => {
                                // J-Sub-D: close tool_use block. Emit
                                // ToolCallPending with `done: true` so
                                // the encoder closes the Anthropic-side
                                // content block. Text/thinking block
                                // boundaries are synthesized at the
                                // encoder layer, not here.
                                if let Some(idx) = v.get("index").and_then(|i| i.as_u64())
                                    && let Some(id) = state.tool_use_by_index.remove(&idx)
                                {
                                    sequence += 1;
                                    return Some((
                                        Ok(event(
                                            AgentEventKind::ToolCallPending,
                                            "TOOL_CALL_PENDING",
                                            &session_id,
                                            sequence,
                                            serde_json::json!({
                                                "id": id,
                                                "name": "",
                                                "done": true,
                                            }),
                                        )),
                                        (body, buffer, sequence, false, session_id, state),
                                    ));
                                }
                            }
                            Some("message_delta") => {
                                // J-Sub-D: capture stop_reason so the
                                // synthesized Finish event downstream
                                // carries it. Anthropic's upstream emits
                                // `stop_reason: "tool_use"` on a
                                // tool_use-terminated turn.
                                if let Some(reason) = v
                                    .get("delta")
                                    .and_then(|d| d.get("stop_reason"))
                                    .and_then(|s| s.as_str())
                                {
                                    state.last_stop_reason = Some(reason.to_string());
                                }
                                // message_delta itself produces no
                                // synthesized AgentEvent — the Finish
                                // event on `message_stop` carries the
                                // reason.
                            }
                            Some("message_stop") => {
                                let reason = state
                                    .last_stop_reason
                                    .clone()
                                    .unwrap_or_else(|| "stop".to_string());
                                sequence += 1;
                                return Some((
                                    Ok(event(
                                        AgentEventKind::Finish,
                                        "FINISH",
                                        &session_id,
                                        sequence,
                                        serde_json::json!({"reason": reason}),
                                    )),
                                    (body, buffer, sequence, true, session_id, state),
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
                            (body, buffer, sequence, true, session_id, state),
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
                            (body, buffer, sequence, true, session_id, state),
                        ));
                    }
                }
            }
        },
    ))
}

/// Extract `(index, id, name)` from a `content_block_start` event whose
/// `content_block.type == "tool_use"`. Returns `None` for non-tool_use
/// block_start frames (text / thinking are synthesized at the encoder,
/// not parsed from upstream).
fn parse_tool_use_start(v: &serde_json::Value) -> Option<(u64, String, String)> {
    let idx = v.get("index").and_then(|i| i.as_u64())?;
    let block = v.get("content_block")?;
    if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
        return None;
    }
    let id = block.get("id").and_then(|s| s.as_str())?.to_string();
    let name = block.get("name").and_then(|s| s.as_str())?.to_string();
    Some((idx, id, name))
}

/// Extract `(index, partial_json)` from a `content_block_delta` whose
/// `delta.type == "input_json_delta"`. Returns `None` for non-tool_use
/// deltas.
fn parse_input_json_delta(v: &serde_json::Value) -> Option<(u64, String)> {
    let idx = v.get("index").and_then(|i| i.as_u64())?;
    let delta = v.get("delta")?;
    if delta.get("type").and_then(|t| t.as_str()) != Some("input_json_delta") {
        return None;
    }
    let partial = delta
        .get("partial_json")
        .and_then(|s| s.as_str())?
        .to_string();
    Some((idx, partial))
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

    /// Fix-round 1 — C-2 regression guard. Asserts that the `Debug`
    /// impl on `AnthropicArcanConfig` redacts the api_key field so a
    /// future `tracing::debug!("{cfg:?}")` or Vigil span attribute can
    /// not leak the credential into telemetry.
    #[test]
    fn debug_impl_redacts_api_key() {
        let cfg = AnthropicArcanConfig {
            api_key: "secret-key-value".to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            system_prompt: Some("you are a helpful assistant".to_string()),
        };
        let rendered = format!("{cfg:?}");
        assert!(
            rendered.contains("<redacted>"),
            "Debug must print `<redacted>` placeholder: {rendered}"
        );
        assert!(
            !rendered.contains("secret-key-value"),
            "Debug must NOT contain the api_key value: {rendered}"
        );
        // Non-secret fields still appear.
        assert!(rendered.contains(DEFAULT_MODEL));
        assert!(rendered.contains("helpful assistant"));
    }

    /// BRO-1206: `request_body` honors model overrides per-call.
    /// Empty / whitespace / `None` falls back to `cfg.model`.
    #[test]
    fn request_body_honors_model_override() {
        let cfg = AnthropicArcanConfig {
            api_key: "k".to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            model: "claude-sonnet-4-5-default".to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            system_prompt: None,
        };
        let arc = AnthropicArcan::new(cfg).expect("build");
        // Override wins.
        let body = arc.request_body("sid", "hi", Some("claude-opus-4-7"));
        assert_eq!(body["model"], "claude-opus-4-7");
        // Empty override falls back to cfg.model.
        let body = arc.request_body("sid", "hi", Some(""));
        assert_eq!(body["model"], "claude-sonnet-4-5-default");
        // Whitespace falls back too.
        let body = arc.request_body("sid", "hi", Some("   "));
        assert_eq!(body["model"], "claude-sonnet-4-5-default");
        // None falls back.
        let body = arc.request_body("sid", "hi", None);
        assert_eq!(body["model"], "claude-sonnet-4-5-default");
    }

    /// The grounded default (used when `LIFED_ARCAN_SYSTEM_PROMPT` is
    /// unset) must reach the Anthropic request body as a cacheable system
    /// block. The Anthropic path serializes `system` differently from the
    /// Vercel path, so it needs its own end-to-end grounding assertion.
    /// Uses the pure resolver so the test never touches the process env.
    #[test]
    fn default_grounding_flows_into_anthropic_system_block() {
        let cfg = AnthropicArcanConfig {
            api_key: "k".to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            system_prompt: crate::grounding::resolve_system_prompt_from(None),
        };
        let arc = AnthropicArcan::new(cfg).expect("build");
        let body = arc.request_body("sid", "Who is Carlos Escobar-Valbuena?", None);
        let system = body["system"].as_array().expect("system is a block array");
        assert_eq!(system.len(), 1, "single grounding system block");
        assert_eq!(system[0]["type"], "text");
        assert_eq!(
            system[0]["cache_control"]["type"], "ephemeral",
            "system block must be marked cacheable",
        );
        let text = system[0]["text"].as_str().expect("system text");
        assert!(text.contains("broomva.tech"));
        assert!(text.contains("Carlos"));
        assert!(text.contains("Life Agent OS"));
    }

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

    /// J-Sub-D (BRO-1143): upstream `content_block_start` for a tool_use
    /// block must emit a `ToolCallPending` event carrying `{id, name}`.
    #[tokio::test]
    async fn parses_tool_use_start() {
        let body = b"event: content_block_start\n\
                     data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_01\",\"name\":\"read_file\",\"input\":{}}}\n\n\
                     event: message_stop\n\
                     data: {\"type\":\"message_stop\"}\n\n";
        let mut s = parse_sse(
            stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(body.to_vec()))]),
            aios_proto::aios::v1::SessionId { value: "s".into() },
        );
        let first = s.next().await.unwrap().unwrap();
        assert_eq!(first.kind(), AgentEventKind::ToolCallPending);
        let record = first.record.as_ref().unwrap();
        let payload: serde_json::Value =
            serde_json::from_slice(&record.payload).expect("payload is valid json");
        assert_eq!(payload["id"], "toolu_01");
        assert_eq!(payload["name"], "read_file");
        // Finish closes the stream.
        let second = s.next().await.unwrap().unwrap();
        assert_eq!(second.kind(), AgentEventKind::Finish);
    }

    /// J-Sub-D (BRO-1143): `content_block_delta` carrying
    /// `input_json_delta` re-uses the index → tool_use_id binding to
    /// emit one event per partial-JSON chunk.
    #[tokio::test]
    async fn parses_input_json_delta_chunks() {
        let body = b"event: content_block_start\n\
                     data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_01\",\"name\":\"read_file\",\"input\":{}}}\n\n\
                     event: content_block_delta\n\
                     data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n\
                     event: content_block_delta\n\
                     data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\" \\\"foo.txt\\\"}\"}}\n\n\
                     event: content_block_stop\n\
                     data: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
                     event: message_stop\n\
                     data: {\"type\":\"message_stop\"}\n\n";
        let mut s = parse_sse(
            stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(body.to_vec()))]),
            aios_proto::aios::v1::SessionId { value: "s".into() },
        );
        // Open: id + name carried.
        let open = s.next().await.unwrap().unwrap();
        assert_eq!(open.kind(), AgentEventKind::ToolCallPending);
        // Partial 1.
        let p1 = s.next().await.unwrap().unwrap();
        assert_eq!(p1.kind(), AgentEventKind::ToolCallPending);
        let v1: serde_json::Value =
            serde_json::from_slice(&p1.record.as_ref().unwrap().payload).unwrap();
        assert_eq!(v1["id"], "toolu_01");
        assert_eq!(v1["partial_json"], "{\"path\":");
        // Partial 2.
        let p2 = s.next().await.unwrap().unwrap();
        let v2: serde_json::Value =
            serde_json::from_slice(&p2.record.as_ref().unwrap().payload).unwrap();
        assert_eq!(v2["id"], "toolu_01");
        assert_eq!(v2["partial_json"], " \"foo.txt\"}");
        // Stop: done=true.
        let stop = s.next().await.unwrap().unwrap();
        let vs: serde_json::Value =
            serde_json::from_slice(&stop.record.as_ref().unwrap().payload).unwrap();
        assert_eq!(vs["id"], "toolu_01");
        assert_eq!(vs["done"], true);
        // Finish at message_stop.
        let fin = s.next().await.unwrap().unwrap();
        assert_eq!(fin.kind(), AgentEventKind::Finish);
    }

    /// J-Sub-D (BRO-1143): `message_delta {stop_reason: "tool_use"}`
    /// propagates into the synthesized `Finish` event so the encoder
    /// can render Anthropic's `message_delta {stop_reason: "tool_use"}`.
    #[tokio::test]
    async fn parses_message_delta_stop_reason_tool_use() {
        let body = b"event: content_block_start\n\
                     data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_02\",\"name\":\"do_x\",\"input\":{}}}\n\n\
                     event: content_block_stop\n\
                     data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
                     event: message_delta\n\
                     data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":47}}\n\n\
                     event: message_stop\n\
                     data: {\"type\":\"message_stop\"}\n\n";
        let mut s = parse_sse(
            stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(body.to_vec()))]),
            aios_proto::aios::v1::SessionId { value: "s".into() },
        );
        // open + stop + (no message_delta event surfaced) + finish.
        let mut events = Vec::new();
        while let Some(item) = s.next().await {
            events.push(item.unwrap());
        }
        let kinds: Vec<AgentEventKind> = events.iter().map(|e| e.kind()).collect();
        // Two ToolCallPending (open + done) + one Finish.
        assert_eq!(
            kinds,
            vec![
                AgentEventKind::ToolCallPending,
                AgentEventKind::ToolCallPending,
                AgentEventKind::Finish,
            ]
        );
        // The Finish event carries reason="tool_use" from message_delta.
        let finish = events.last().unwrap();
        let payload: serde_json::Value =
            serde_json::from_slice(&finish.record.as_ref().unwrap().payload).unwrap();
        assert_eq!(payload["reason"], "tool_use");
    }

    /// J-Sub-D (BRO-1143): two interleaved tool_use blocks (parallel
    /// tool_use is rare from Anthropic today but the protocol allows
    /// it). Verify that index→id state stays correctly partitioned.
    #[tokio::test]
    async fn parses_multi_tool_use_with_distinct_indices() {
        let body = b"event: content_block_start\n\
                     data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_A\",\"name\":\"a\",\"input\":{}}}\n\n\
                     event: content_block_start\n\
                     data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_B\",\"name\":\"b\",\"input\":{}}}\n\n\
                     event: content_block_delta\n\
                     data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n\n\
                     event: content_block_delta\n\
                     data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n\n\
                     event: message_stop\n\
                     data: {\"type\":\"message_stop\"}\n\n";
        let mut s = parse_sse(
            stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(body.to_vec()))]),
            aios_proto::aios::v1::SessionId { value: "s".into() },
        );
        let mut events = Vec::new();
        while let Some(item) = s.next().await {
            events.push(item.unwrap());
        }
        // 2 starts + 2 deltas + 1 finish = 5 events.
        assert_eq!(events.len(), 5);
        let payload = |i: usize| -> serde_json::Value {
            serde_json::from_slice(&events[i].record.as_ref().unwrap().payload).unwrap()
        };
        assert_eq!(payload(0)["id"], "toolu_A");
        assert_eq!(payload(0)["name"], "a");
        assert_eq!(payload(1)["id"], "toolu_B");
        assert_eq!(payload(1)["name"], "b");
        // Order of partial deltas: index 1 first (toolu_B), then index 0.
        assert_eq!(payload(2)["id"], "toolu_B");
        assert_eq!(payload(3)["id"], "toolu_A");
        assert_eq!(events[4].kind(), AgentEventKind::Finish);
    }
}
