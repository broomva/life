//! `VercelAiGatewayArcan` — `ArcanCall` impl that streams real
//! chat-completions tokens from any OpenAI-compatible HTTP endpoint
//! (Vercel AI Gateway, OpenAI direct, OpenRouter, …).
//!
//! ## Why this exists (Stage 3b — May 2026)
//!
//! The canonical `arcan-proxy::ArcanProxy` is a placeholder tonic
//! client over an `arcan-proto` service that arcand does not yet
//! publish. Until that ships, `lifed::Agent.SendMessage /
//! StreamSession` either streams canned events from `MockArcan` (no
//! tokens) or falls through to nothing. To get real LLM tokens
//! flowing end-to-end through the public-plane wire (lifegw → lifed →
//! ArcanCall → WS), this module bridges the `ArcanCall` trait
//! directly to a remote chat-completions endpoint.
//!
//! ## Architecture (transitional)
//!
//! ```text
//!   lifed.Agent.SendMessage
//!     ─▶ ArcanCall::dispatch_message(sid, content)
//!         ─▶ VercelAiGatewayArcan
//!             ─▶ POST {base_url}/chat/completions  (stream=true)
//!             ◀── SSE: data: { choices: [{ delta: { content: "Hi" } }] }
//!         ◀── AgentEvent { kind: TOKEN, payload: { text: "Hi" } }
//!     ◀── tonic::Response<Stream<AgentEvent>>
//! ```
//!
//! When `arcan-proto` ships, this module will be marked `#[deprecated]`
//! and the canonical path becomes `lifed → arcand UDS`. The `ArcanCall`
//! trait stays unchanged across the migration — only the implementation
//! swaps.
//!
//! ## Endpoint compatibility
//!
//! Vercel AI Gateway accepts the standard OpenAI Chat Completions
//! shape at `https://ai-gateway.vercel.sh/v1/chat/completions`. Any
//! OpenAI-compatible provider works (set `base_url` accordingly). The
//! `model` string can be a vendor-prefixed identifier like
//! `anthropic/claude-sonnet-4-6` (Vercel's gateway routes by prefix)
//! or a raw OpenAI model name.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use life_runtime_proto::life::v1::{AgentEvent, AgentEventKind, EventRecord};
use serde::{Deserialize, Serialize};

use crate::client::ArcanCall;
use crate::conversions::now_timestamp;
use crate::error::{ArcanProxyError, ArcanProxyResult};

/// Default base URL for Vercel AI Gateway. Operators override via
/// `VercelAiGatewayConfig::base_url` for OpenRouter / OpenAI direct /
/// any other OpenAI-compatible endpoint.
pub const DEFAULT_BASE_URL: &str = "https://ai-gateway.vercel.sh/v1";

/// Default model. `anthropic/claude-sonnet-4-6` is what the existing
/// Railway `arcan` service ships with — matches operator expectations.
pub const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4-6";

/// Default per-request HTTP deadline. Generous enough for a
/// multi-thousand-token streaming response, tight enough that a
/// hung upstream doesn't pin handler tasks indefinitely.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Configuration for [`VercelAiGatewayArcan`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct VercelAiGatewayConfig {
    /// HTTP base URL — Vercel AI Gateway, OpenAI, OpenRouter, etc.
    /// Trailing slash is stripped at construction.
    pub base_url: String,
    /// Bearer token. For Vercel AI Gateway this is a `vck_…` key.
    pub api_key: String,
    /// Model identifier. Vendor-prefixed for Vercel AI Gateway.
    pub model: String,
    /// Per-request timeout. Defaults to [`DEFAULT_REQUEST_TIMEOUT`].
    pub request_timeout: Duration,
    /// Optional system prompt prepended to every dispatch.
    pub system_prompt: Option<String>,
}

impl VercelAiGatewayConfig {
    /// Read configuration from environment variables matching the
    /// existing arcan-service convention:
    ///
    /// - `OPENAI_API_KEY` (required)
    /// - `OPENAI_BASE_URL` (default [`DEFAULT_BASE_URL`])
    /// - `OPENAI_MODEL` (default [`DEFAULT_MODEL`])
    /// - `LIFED_ARCAN_SYSTEM_PROMPT` (optional override; defaults to the
    ///   grounded [`crate::grounding::DEFAULT_CHAT_SYSTEM_PROMPT`])
    /// - `LIFED_ARCAN_REQUEST_TIMEOUT_SECS` (optional, default 120)
    pub fn from_env() -> ArcanProxyResult<Self> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
            ArcanProxyError::Transport(
                "VercelAiGatewayArcan requires OPENAI_API_KEY (Vercel AI Gateway token)"
                    .to_string(),
            )
        })?;
        let base_url = std::env::var("OPENAI_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let model = std::env::var("OPENAI_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        // Defaults to the grounded persona when the env var is unset/blank
        // so the live agent answers project FAQs factually instead of
        // "I don't have enough context"; an explicit override still wins.
        let system_prompt = crate::grounding::resolve_system_prompt();
        let request_timeout = std::env::var("LIFED_ARCAN_REQUEST_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT);
        Ok(Self {
            base_url,
            api_key,
            model,
            request_timeout,
            system_prompt,
        })
    }
}

/// `ArcanCall` impl backed by a streaming OpenAI-compatible endpoint.
///
/// Cloning is cheap — the underlying `reqwest::Client` is `Arc`-backed.
#[derive(Clone)]
pub struct VercelAiGatewayArcan {
    cfg: Arc<VercelAiGatewayConfig>,
    client: reqwest::Client,
}

impl VercelAiGatewayArcan {
    /// Build a new `VercelAiGatewayArcan` from a config. Constructs
    /// the `reqwest::Client` with the configured timeout. Returns an
    /// error if the URL is malformed or `api_key` is empty.
    pub fn new(cfg: VercelAiGatewayConfig) -> ArcanProxyResult<Self> {
        if cfg.api_key.trim().is_empty() {
            return Err(ArcanProxyError::Transport(
                "VercelAiGatewayConfig.api_key must not be empty".to_string(),
            ));
        }
        let normalized = cfg.base_url.trim_end_matches('/').to_string();
        if !normalized.starts_with("http://") && !normalized.starts_with("https://") {
            return Err(ArcanProxyError::Transport(format!(
                "VercelAiGatewayConfig.base_url must be http(s):// (got `{}`)",
                cfg.base_url
            )));
        }
        let client = reqwest::Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .map_err(|e| ArcanProxyError::Transport(format!("build reqwest client: {e}")))?;
        let cfg = VercelAiGatewayConfig {
            base_url: normalized,
            ..cfg
        };
        Ok(Self {
            cfg: Arc::new(cfg),
            client,
        })
    }

    /// Convenience: construct from environment variables.
    pub fn from_env() -> ArcanProxyResult<Self> {
        Self::new(VercelAiGatewayConfig::from_env()?)
    }

    /// Build the chat-completions request body for a single user
    /// dispatch. The system prompt (if any) is prepended.
    ///
    /// BRO-1206: when `model_override` is `Some(non_empty)` the override
    /// wins; otherwise we fall back to `self.cfg.model` (today's behavior
    /// — derived at construction from `OPENAI_MODEL` env or
    /// [`DEFAULT_MODEL`]). The override is per-call (per-session in
    /// practice) so a single backend can serve users on different
    /// models without rebuilding.
    ///
    /// `tools` carries client-supplied tool definitions. Non-empty
    /// definitions are attached as the OpenAI-shape `tools` array:
    /// `{"type": "function", "function": {name, description, parameters}}`.
    fn build_request_body(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        tools: &[serde_json::Value],
    ) -> serde_json::Value {
        let mut messages: Vec<serde_json::Value> = Vec::with_capacity(2);
        if let Some(sys) = &self.cfg.system_prompt
            && !sys.trim().is_empty()
        {
            messages.push(serde_json::json!({
                "role": "system",
                "content": sys,
            }));
        }
        messages.push(serde_json::json!({
            "role": "user",
            "content": user_message,
        }));
        let model: &str = model_override
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(self.cfg.model.as_str());
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        if !tools.is_empty() {
            let tools_json: Vec<serde_json::Value> = tools.iter().map(openai_tool_shape).collect();
            body["tools"] = serde_json::Value::Array(tools_json);
        }
        body
    }
}

/// Translate one client tool definition into the OpenAI chat-completions
/// `tools` entry shape: `{"type": "function", "function": {name,
/// description, parameters}}`.
///
/// Accepted inputs:
/// - AI-SDK / bare shape: `{"name", "description"?, "parameters"?}` (wrapped)
/// - OpenAI envelope: `{"type": "function", "function": {…}}` (passed through)
/// - Anthropic-native: `{"name", "input_schema"}` (schema lifted into
///   `parameters`)
///
/// Missing schemas default to an empty object schema so the gateway
/// never sees a structurally-invalid tool entry.
fn openai_tool_shape(tool: &serde_json::Value) -> serde_json::Value {
    if tool.get("type").and_then(|t| t.as_str()) == Some("function")
        && tool.get("function").is_some()
    {
        return tool.clone();
    }
    let parameters = tool
        .get("parameters")
        .or_else(|| tool.get("input_schema"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
    serde_json::json!({
        "type": "function",
        "function": {
            "name": tool.get("name").cloned().unwrap_or_else(|| serde_json::json!("")),
            "description": tool.get("description").cloned().unwrap_or_else(|| serde_json::json!("")),
            "parameters": parameters,
        },
    })
}

#[async_trait]
impl ArcanCall for VercelAiGatewayArcan {
    async fn create_agent(&self, sid: &str) -> ArcanProxyResult<String> {
        // Stateless on the gateway — there's nothing to "create" until
        // the first dispatch. Return an opaque agent_id that mirrors
        // the canonical `ArcanProxy::create_agent` shape so callers
        // can't detect the backend.
        Ok(format!("agent-{sid}"))
    }

    async fn destroy_agent(&self, _sid: &str) -> ArcanProxyResult<()> {
        // Nothing to destroy — the gateway holds no per-agent state.
        Ok(())
    }

    async fn dispatch_message(
        &self,
        sid: &str,
        content: &str,
        model: Option<&str>,
        tools: &[serde_json::Value],
    ) -> ArcanProxyResult<Pin<Box<dyn Stream<Item = Result<AgentEvent, tonic::Status>> + Send>>>
    {
        let url = format!("{}/chat/completions", self.cfg.base_url);
        // BRO-1206: per-call model override → outbound request body.
        // `None` / empty falls back to `self.cfg.model` (env-bound).
        // Client tool definitions ride along as the OpenAI-shape
        // `tools` array.
        let body = self.build_request_body(content, model, tools);

        // BRO-1234: trace the substrate boundary so a silent gateway
        // hang is distinguishable from a substrate-never-fires hang in
        // production logs. The previous incarnation of this module
        // emitted no log on entry / response → the broomva.tech-side
        // dogfood saw "3 frames then silence" with no upstream signal
        // to disambiguate H4 (gateway stalls) / H5 (parser bug) /
        // H6 (ws-pump drops). These info lines + the per-chunk +
        // per-emit logs in `parse_sse_token_stream` give every step
        // of the body pump a name.
        let t_start = std::time::Instant::now();
        let resolved_model = model
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&self.cfg.model);
        tracing::info!(
            sid = sid,
            model = resolved_model,
            content_len = content.len(),
            tool_count = tools.len(),
            url = %url,
            "arcan.dispatch_message: starting",
        );
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.cfg.api_key)
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(
                    sid = sid,
                    elapsed_ms = t_start.elapsed().as_millis() as u64,
                    error = %e,
                    "arcan.dispatch_message: gateway send failed",
                );
                ArcanProxyError::Transport(format!("POST {url}: {e}"))
            })?;
        let resp_elapsed_ms = t_start.elapsed().as_millis() as u64;
        let status = resp.status();
        if !status.is_success() {
            let bytes = resp.bytes().await.unwrap_or_default();
            let body_preview = String::from_utf8_lossy(&bytes);
            tracing::warn!(
                sid = sid,
                status = status.as_u16(),
                elapsed_ms = resp_elapsed_ms,
                body_preview = %truncate_body(&body_preview, 128),
                "arcan.dispatch_message: gateway returned non-2xx",
            );
            return Err(ArcanProxyError::Transport(format!(
                "POST {url} returned HTTP {status}: {}",
                truncate_body(&body_preview, 256)
            )));
        }
        tracing::info!(
            sid = sid,
            status = status.as_u16(),
            elapsed_ms = resp_elapsed_ms,
            "arcan.dispatch_message: gateway responded — attaching SSE parser",
        );

        // Stream parser: SSE-style lines `data: {json}\n\n` with
        // `data: [DONE]\n\n` as the terminal marker.
        let session_id = aios_proto::aios::v1::SessionId {
            value: sid.to_string(),
        };
        let body_stream = resp.bytes_stream();
        let parsed = parse_sse_token_stream(body_stream, session_id);
        Ok(Box::pin(parsed))
    }
}

// ─── SSE parser ───────────────────────────────────────────────────────

/// OpenAI streaming chat-completions delta envelope. We only need the
/// pieces relevant to lifed's AgentEvent emission.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatCompletionChoice {
    #[serde(default)]
    delta: ChatCompletionDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ChatCompletionDelta {
    #[serde(default)]
    content: Option<String>,
    /// OpenAI streaming tool-call deltas. The first fragment for an
    /// `index` carries `id` (+ usually `function.name`); subsequent
    /// fragments stream `function.arguments` JSON pieces.
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ToolCallDelta {
    #[serde(default)]
    index: u64,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: ToolCallFunctionDelta,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ToolCallFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Per-stream observability state threaded through the unfold
/// closure. BRO-1234: gives every body-pump + emit boundary a named
/// log line so a production dogfood can distinguish "gateway stops
/// sending bytes" from "parser stops emitting events" from
/// "downstream ws pump drops frames."
struct ParserMetrics {
    /// `sid` extracted from `session_id.value` so every log line
    /// correlates with the lifegw saga / WS upgrade for the same chat.
    sid: String,
    /// Wall clock at which `parse_sse_token_stream` started consuming.
    /// `Instant::elapsed()` measures time-to-first-chunk and time
    /// between chunks — the key signal for distinguishing a gateway-
    /// side stall from a parser bug.
    t_start: std::time::Instant,
    /// Set on each `body.next() → Some(Ok(chunk))`. `None` before the
    /// first chunk arrives. The `pre-await` log line surfaces
    /// `ms_since_last_chunk` so a long gap is immediately visible
    /// even if the await itself never resolves.
    last_chunk_at: Option<std::time::Instant>,
    chunk_count: u64,
    emit_count: u64,
    total_bytes: u64,
}

impl ParserMetrics {
    fn new(sid: String) -> Self {
        Self {
            sid,
            t_start: std::time::Instant::now(),
            last_chunk_at: None,
            chunk_count: 0,
            emit_count: 0,
            total_bytes: 0,
        }
    }

    fn ms_since_start(&self) -> u64 {
        self.t_start.elapsed().as_millis() as u64
    }

    /// Returns `ms_since_last_chunk` when at least one chunk has
    /// arrived, otherwise `ms_since_start` (so the first pre-await
    /// log still shows useful elapsed time).
    fn ms_since_last_chunk_or_start(&self) -> u64 {
        self.last_chunk_at
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or_else(|| self.ms_since_start())
    }
}

/// Response-side tool-call bookkeeping for the OpenAI-compatible
/// streaming wire.
///
/// Maps `delta.tool_calls` fragments onto the existing TOOL_CALL event
/// vocabulary (the same payload shapes the Anthropic-backed path and
/// the #1686 real-arcand path emit) so the client sees structured
/// tool-call events instead of dropped frames:
///
/// - open:     `{"id", "name", "input": {}}`
/// - fragment: `{"id", "name": "", "partial_json": "…"}`
/// - close:    `{"id", "name": "", "done": true}` (on finish)
///
/// The unfold yields one item per poll, but a single upstream chunk
/// can synthesize several events — they queue on `pending` and drain
/// first on subsequent polls.
struct ToolCallTracker {
    pending: std::collections::VecDeque<AgentEvent>,
    /// `tool_calls[].index` → tool-call id for every currently-open
    /// call. BTreeMap so close-out order is deterministic.
    open_by_index: std::collections::BTreeMap<u64, String>,
}

impl ToolCallTracker {
    fn new() -> Self {
        Self {
            pending: std::collections::VecDeque::new(),
            open_by_index: std::collections::BTreeMap::new(),
        }
    }

    /// Ingest one `delta.tool_calls[]` entry, queueing the synthesized
    /// TOOL_CALL_PENDING events. Returns the updated sequence counter.
    fn ingest_delta(
        &mut self,
        tc: ToolCallDelta,
        session_id: &aios_proto::aios::v1::SessionId,
        mut sequence: u64,
    ) -> u64 {
        if let Some(id) = tc.id.filter(|s| !s.is_empty()) {
            self.open_by_index.insert(tc.index, id.clone());
            let name = tc.function.name.clone().unwrap_or_default();
            sequence += 1;
            self.pending.push_back(tool_pending_event(
                session_id,
                sequence,
                serde_json::json!({"id": id, "name": name, "input": {}}),
            ));
        }
        if let Some(args) = tc.function.arguments.filter(|s| !s.is_empty())
            && let Some(id) = self.open_by_index.get(&tc.index)
        {
            sequence += 1;
            self.pending.push_back(tool_pending_event(
                session_id,
                sequence,
                serde_json::json!({"id": id, "name": "", "partial_json": args}),
            ));
        }
        sequence
    }

    /// Close every open tool call (`{"id", "done": true}` events) then
    /// queue the terminal FINISH carrying `reason`. Returns the updated
    /// sequence counter.
    fn queue_close_then_finish(
        &mut self,
        reason: &str,
        session_id: &aios_proto::aios::v1::SessionId,
        mut sequence: u64,
    ) -> u64 {
        for (_, id) in std::mem::take(&mut self.open_by_index) {
            sequence += 1;
            self.pending.push_back(tool_pending_event(
                session_id,
                sequence,
                serde_json::json!({"id": id, "name": "", "done": true}),
            ));
        }
        sequence += 1;
        self.pending.push_back(AgentEvent {
            record: Some(EventRecord {
                session_id: Some(session_id.clone()),
                sequence,
                at: now_timestamp(),
                kind: "FINISH".to_string(),
                payload: serde_json::to_vec(&serde_json::json!({"reason": reason}))
                    .unwrap_or_default(),
            }),
            kind: AgentEventKind::Finish as i32,
        });
        sequence
    }
}

fn tool_pending_event(
    session_id: &aios_proto::aios::v1::SessionId,
    sequence: u64,
    payload: serde_json::Value,
) -> AgentEvent {
    AgentEvent {
        record: Some(EventRecord {
            session_id: Some(session_id.clone()),
            sequence,
            at: now_timestamp(),
            kind: "TOOL_CALL_PENDING".to_string(),
            payload: serde_json::to_vec(&payload).unwrap_or_default(),
        }),
        kind: AgentEventKind::ToolCallPending as i32,
    }
}

fn queued_token_event(
    session_id: &aios_proto::aios::v1::SessionId,
    sequence: u64,
    text: &str,
) -> AgentEvent {
    AgentEvent {
        record: Some(EventRecord {
            session_id: Some(session_id.clone()),
            sequence,
            at: now_timestamp(),
            kind: "TOKEN".to_string(),
            payload: serde_json::to_vec(&serde_json::json!({"text": text})).unwrap_or_default(),
        }),
        kind: AgentEventKind::Token as i32,
    }
}

/// Convert a streaming HTTP body into a stream of `AgentEvent`s.
///
/// SSE framing: `\n\n`-delimited records, each line beginning with
/// `data: ` carries a JSON chunk (or the literal `[DONE]` sentinel).
/// We hand-roll the parser to keep the dep surface narrow — `eventsource-stream`
/// is in the workspace but pulls its own framing assumptions.
fn parse_sse_token_stream<S>(
    body: S,
    session_id: aios_proto::aios::v1::SessionId,
) -> Pin<Box<dyn Stream<Item = Result<AgentEvent, tonic::Status>> + Send>>
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    use futures::stream;
    let buffer = Vec::new();
    let sequence: u64 = 0;
    let done = false;
    let metrics = ParserMetrics::new(session_id.value.clone());
    let tools_state = ToolCallTracker::new();
    let body: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>> =
        Box::pin(body);

    Box::pin(stream::unfold(
        (
            body,
            buffer,
            sequence,
            done,
            session_id,
            metrics,
            tools_state,
        ),
        move |(
            mut body,
            mut buffer,
            mut sequence,
            mut done,
            session_id,
            mut metrics,
            mut tools_state,
        )| async move {
            if done {
                return None;
            }
            loop {
                // Drain queued tool-lifecycle / terminal events first —
                // a single upstream chunk can synthesize several events
                // but the unfold yields one item per poll.
                if let Some(evt) = tools_state.pending.pop_front() {
                    metrics.emit_count += 1;
                    let is_finish = evt.kind == AgentEventKind::Finish as i32;
                    tracing::info!(
                        sid = %metrics.sid,
                        emit_count = metrics.emit_count,
                        kind = evt.kind,
                        is_finish,
                        elapsed_ms = metrics.ms_since_start(),
                        "arcan.parse: emit queued event",
                    );
                    if is_finish {
                        done = true;
                    }
                    return Some((
                        Ok(evt),
                        (
                            body,
                            buffer,
                            sequence,
                            done,
                            session_id,
                            metrics,
                            tools_state,
                        ),
                    ));
                }
                // Try to extract a full SSE record from the buffer.
                if let Some(record_end) = find_double_newline(&buffer) {
                    let record_bytes = buffer.drain(..record_end + 2).collect::<Vec<u8>>();
                    let record = String::from_utf8_lossy(&record_bytes);
                    for line in record.lines() {
                        let line = line.trim_end();
                        let payload = match line
                            .strip_prefix("data: ")
                            .or_else(|| line.strip_prefix("data:"))
                        {
                            Some(p) => p.trim(),
                            None => continue,
                        };
                        if payload == "[DONE]" {
                            // Close any open tool calls + queue the
                            // terminal FINISH. The top-of-loop drain
                            // yields them one per poll.
                            sequence =
                                tools_state.queue_close_then_finish("stop", &session_id, sequence);
                            tracing::info!(
                                sid = %metrics.sid,
                                seq = sequence,
                                chunk_count = metrics.chunk_count,
                                total_bytes = metrics.total_bytes,
                                elapsed_ms = metrics.ms_since_start(),
                                "arcan.parse: queue FINISH (reason=stop, [DONE] sentinel)",
                            );
                            continue;
                        }
                        if payload.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<ChatCompletionChunk>(payload) {
                            Ok(chunk) => {
                                let mut delta_text = String::new();
                                let mut finish_reason: Option<String> = None;
                                for choice in chunk.choices {
                                    if let Some(c) = choice.delta.content {
                                        delta_text.push_str(&c);
                                    }
                                    // Tool-call deltas queue structured
                                    // TOOL_CALL_PENDING events instead of
                                    // being silently dropped.
                                    for tc in choice.delta.tool_calls {
                                        sequence =
                                            tools_state.ingest_delta(tc, &session_id, sequence);
                                    }
                                    if let Some(reason) = choice.finish_reason {
                                        finish_reason = Some(reason);
                                    }
                                }
                                if !delta_text.is_empty() && tools_state.pending.is_empty() {
                                    sequence += 1;
                                    metrics.emit_count += 1;
                                    let delta_len = delta_text.len();
                                    tracing::info!(
                                        sid = %metrics.sid,
                                        seq = sequence,
                                        emit_count = metrics.emit_count,
                                        delta_len,
                                        chunk_count = metrics.chunk_count,
                                        total_bytes = metrics.total_bytes,
                                        elapsed_ms = metrics.ms_since_start(),
                                        "arcan.parse: emit TOKEN",
                                    );
                                    let token_event =
                                        queued_token_event(&session_id, sequence, &delta_text);
                                    return Some((
                                        Ok(token_event),
                                        (
                                            body,
                                            buffer,
                                            sequence,
                                            done,
                                            session_id,
                                            metrics,
                                            tools_state,
                                        ),
                                    ));
                                }
                                if !delta_text.is_empty() {
                                    // Mixed chunk (text + tool deltas —
                                    // rare): keep emission order by
                                    // queueing the TOKEN behind the tool
                                    // events already pending.
                                    sequence += 1;
                                    tools_state.pending.push_back(queued_token_event(
                                        &session_id,
                                        sequence,
                                        &delta_text,
                                    ));
                                }
                                if let Some(reason) = finish_reason {
                                    // Close open tool calls + queue the
                                    // terminal FINISH; the top-of-loop
                                    // drain yields them in order.
                                    sequence = tools_state.queue_close_then_finish(
                                        &reason,
                                        &session_id,
                                        sequence,
                                    );
                                    tracing::info!(
                                        sid = %metrics.sid,
                                        seq = sequence,
                                        reason = %reason,
                                        chunk_count = metrics.chunk_count,
                                        total_bytes = metrics.total_bytes,
                                        elapsed_ms = metrics.ms_since_start(),
                                        "arcan.parse: queue FINISH (finish_reason from delta)",
                                    );
                                }
                            }
                            Err(err) => {
                                tracing::warn!(
                                    sid = %metrics.sid,
                                    error = %err,
                                    payload = %truncate_body(payload, 128),
                                    chunk_count = metrics.chunk_count,
                                    total_bytes = metrics.total_bytes,
                                    "arcan.parse: skip malformed SSE chunk",
                                );
                            }
                        }
                    }
                    // Record consumed but produced no event (e.g. only role-only delta) — loop.
                    continue;
                }
                // No full record yet — pull more bytes. The `pre-await`
                // log line is the smoking-gun signal for H4 (gateway
                // stops sending bytes): if logs show this line but the
                // matching "got chunk" / "closed" / "stream error" log
                // never follows, the `body.next().await` future is
                // blocked indefinitely on upstream silence.
                tracing::info!(
                    sid = %metrics.sid,
                    chunk_count = metrics.chunk_count,
                    emit_count = metrics.emit_count,
                    total_bytes = metrics.total_bytes,
                    buffer_len = buffer.len(),
                    ms_since_last_chunk = metrics.ms_since_last_chunk_or_start(),
                    "arcan.parse: awaiting next upstream body chunk",
                );
                match body.next().await {
                    Some(Ok(chunk)) => {
                        let bytes = chunk.len() as u64;
                        metrics.chunk_count += 1;
                        metrics.total_bytes += bytes;
                        metrics.last_chunk_at = Some(std::time::Instant::now());
                        tracing::info!(
                            sid = %metrics.sid,
                            chunk_count = metrics.chunk_count,
                            bytes,
                            total_bytes = metrics.total_bytes,
                            emit_count = metrics.emit_count,
                            elapsed_ms = metrics.ms_since_start(),
                            "arcan.parse: got upstream body chunk",
                        );
                        buffer.extend_from_slice(&chunk);
                    }
                    Some(Err(err)) => {
                        tracing::warn!(
                            sid = %metrics.sid,
                            chunk_count = metrics.chunk_count,
                            total_bytes = metrics.total_bytes,
                            emit_count = metrics.emit_count,
                            elapsed_ms = metrics.ms_since_start(),
                            error = %err,
                            "arcan.parse: upstream body stream error",
                        );
                        return Some((
                            Err(tonic::Status::internal(format!(
                                "VercelAiGatewayArcan: stream read error: {err}"
                            ))),
                            (
                                body,
                                buffer,
                                sequence,
                                true,
                                session_id,
                                metrics,
                                tools_state,
                            ),
                        ));
                    }
                    None => {
                        // Upstream closed without [DONE] — close any
                        // open tool calls + synthesize a finish so the
                        // downstream pump exits cleanly. The top-of-loop
                        // drain yields the queued events; pending is
                        // guaranteed non-empty here so the loop cannot
                        // poll the (exhausted) body again before the
                        // FINISH flips `done`.
                        tracing::info!(
                            sid = %metrics.sid,
                            chunk_count = metrics.chunk_count,
                            total_bytes = metrics.total_bytes,
                            emit_count = metrics.emit_count,
                            elapsed_ms = metrics.ms_since_start(),
                            bytes_remaining = buffer.len(),
                            "arcan.parse: upstream closed (synthesising FINISH reason=upstream_closed)",
                        );
                        sequence = tools_state.queue_close_then_finish(
                            "upstream_closed",
                            &session_id,
                            sequence,
                        );
                        continue;
                    }
                }
            }
        },
    ))
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2)
        .enumerate()
        .find(|(_, w)| *w == b"\n\n")
        .map(|(i, _)| i)
}

fn truncate_body(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        s.to_string()
    } else {
        let mut s = s.to_string();
        s.truncate(limit);
        s.push('…');
        s
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cfg(base_url: String) -> VercelAiGatewayConfig {
        VercelAiGatewayConfig {
            base_url,
            api_key: "test-key".to_string(),
            model: "test-model".to_string(),
            request_timeout: Duration::from_secs(5),
            system_prompt: None,
        }
    }

    #[test]
    fn config_rejects_empty_api_key() {
        let mut c = cfg("http://localhost:8080".to_string());
        c.api_key = "   ".to_string();
        match VercelAiGatewayArcan::new(c) {
            Ok(_) => panic!("must reject empty api_key"),
            Err(ArcanProxyError::Transport(m)) => assert!(m.contains("api_key")),
            Err(other) => panic!("expected Transport error, got {other:?}"),
        }
    }

    #[test]
    fn config_rejects_non_http_url() {
        let c = cfg("ftp://oops".to_string());
        match VercelAiGatewayArcan::new(c) {
            Ok(_) => panic!("must reject non-http url"),
            Err(ArcanProxyError::Transport(m)) => assert!(m.contains("http")),
            Err(other) => panic!("expected Transport error, got {other:?}"),
        }
    }

    #[test]
    fn config_strips_trailing_slash() {
        let arc =
            VercelAiGatewayArcan::new(cfg("http://localhost:8080/".to_string())).expect("build");
        assert_eq!(arc.cfg.base_url, "http://localhost:8080");
    }

    #[test]
    fn build_request_body_contains_model_and_user_message() {
        let arc =
            VercelAiGatewayArcan::new(cfg("http://localhost:8080".to_string())).expect("build");
        let body = arc.build_request_body("hello world", None, &[]);
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["stream"], true);
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "hello world");
    }

    #[test]
    fn build_request_body_includes_system_prompt_when_set() {
        let mut c = cfg("http://localhost:8080".to_string());
        c.system_prompt = Some("be helpful".to_string());
        let arc = VercelAiGatewayArcan::new(c).expect("build");
        let body = arc.build_request_body("hello", None, &[]);
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "be helpful");
        assert_eq!(messages[1]["role"], "user");
    }

    /// The grounded default (used when `LIFED_ARCAN_SYSTEM_PROMPT` is
    /// unset) must reach the outbound request body as the system message —
    /// otherwise the live agent stays ungrounded. Uses the pure resolver
    /// (`resolve_system_prompt_from(None)`) so the test never touches the
    /// process environment.
    #[test]
    fn default_grounding_flows_into_request_body() {
        let mut c = cfg("http://localhost:8080".to_string());
        c.system_prompt = crate::grounding::resolve_system_prompt_from(None);
        let arc = VercelAiGatewayArcan::new(c).expect("build");
        let body = arc.build_request_body("Who is Carlos Escobar-Valbuena?", None, &[]);
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 2, "system + user");
        assert_eq!(messages[0]["role"], "system");
        let system = messages[0]["content"].as_str().expect("system content");
        assert!(system.contains("broomva.tech"));
        assert!(system.contains("Carlos"));
        assert!(system.contains("Life Agent OS"));
    }

    /// BRO-1206: per-call model override wins over `cfg.model`.
    #[test]
    fn build_request_body_honors_model_override() {
        let arc =
            VercelAiGatewayArcan::new(cfg("http://localhost:8080".to_string())).expect("build");
        // Override wins.
        let body = arc.build_request_body("hi", Some("openai/gpt-4o-mini"), &[]);
        assert_eq!(body["model"], "openai/gpt-4o-mini");
        // Empty override falls back to cfg.model.
        let body = arc.build_request_body("hi", Some("   "), &[]);
        assert_eq!(body["model"], "test-model");
        // None falls back to cfg.model.
        let body = arc.build_request_body("hi", None, &[]);
        assert_eq!(body["model"], "test-model");
    }

    /// Client tool definitions (AI-SDK bare shape) are wrapped into the
    /// OpenAI `tools` array: `{"type":"function","function":{…}}`.
    #[test]
    fn build_request_body_emits_openai_tools_array() {
        let arc =
            VercelAiGatewayArcan::new(cfg("http://localhost:8080".to_string())).expect("build");
        let tools = vec![serde_json::json!({
            "name": "get_weather",
            "description": "Look up the weather",
            "parameters": {"type": "object", "properties": {"city": {"type": "string"}}},
        })];
        let body = arc.build_request_body("hi", None, &tools);
        let tools_json = body["tools"].as_array().expect("tools array");
        assert_eq!(tools_json.len(), 1);
        assert_eq!(tools_json[0]["type"], "function");
        assert_eq!(tools_json[0]["function"]["name"], "get_weather");
        assert_eq!(
            tools_json[0]["function"]["description"],
            "Look up the weather"
        );
        assert_eq!(
            tools_json[0]["function"]["parameters"]["properties"]["city"]["type"],
            "string"
        );
    }

    /// Already-OpenAI-shaped definitions pass through unchanged; missing
    /// schemas default to an empty object schema.
    #[test]
    fn build_request_body_tool_shape_passthrough_and_defaults() {
        let arc =
            VercelAiGatewayArcan::new(cfg("http://localhost:8080".to_string())).expect("build");
        let envelope = serde_json::json!({
            "type": "function",
            "function": {"name": "wrapped", "description": "d", "parameters": {"type": "object"}},
        });
        let bare = serde_json::json!({"name": "schemaless"});
        let body = arc.build_request_body("hi", None, &[envelope.clone(), bare]);
        let tools_json = body["tools"].as_array().expect("tools array");
        assert_eq!(tools_json[0], envelope, "OpenAI envelope passes through");
        assert_eq!(tools_json[1]["function"]["name"], "schemaless");
        assert_eq!(
            tools_json[1]["function"]["parameters"],
            serde_json::json!({"type": "object", "properties": {}}),
            "missing schema defaults to empty object schema",
        );
    }

    /// No client tools → no `tools` key at all (the gateway treats an
    /// empty array differently from an absent field on some providers).
    #[test]
    fn build_request_body_omits_tools_when_empty() {
        let arc =
            VercelAiGatewayArcan::new(cfg("http://localhost:8080".to_string())).expect("build");
        let body = arc.build_request_body("hi", None, &[]);
        assert!(body.get("tools").is_none());
    }

    /// Tool definitions reach the outbound HTTP body. wiremock only
    /// serves the mock when the wrapped `tools` array matches, proving
    /// the wire shape end-to-end.
    #[tokio::test]
    async fn dispatch_message_tools_reach_outbound_body() {
        let server = MockServer::start().await;
        let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                        data: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_partial_json(serde_json::json!({
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "description": "Look up the weather",
                        "parameters": {"type": "object", "properties": {}},
                    },
                }],
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body),
            )
            .mount(&server)
            .await;

        let arc = VercelAiGatewayArcan::new(cfg(server.uri())).expect("build");
        let tools = vec![serde_json::json!({
            "name": "get_weather",
            "description": "Look up the weather",
            "parameters": {"type": "object", "properties": {}},
        })];
        let stream = arc
            .dispatch_message("sess_x", "hi", None, &tools)
            .await
            .expect("tools reach body");
        let mut s = Box::pin(stream);
        let first = s.next().await.expect("event").expect("ok");
        assert_eq!(first.kind(), AgentEventKind::Token);
    }

    /// Response side: `delta.tool_calls` fragments map onto the existing
    /// TOOL_CALL event vocabulary — open `{"id","name","input":{}}`,
    /// fragments `{"id","partial_json"}`, close `{"id","done":true}` —
    /// then FINISH carries `reason: "tool_calls"`.
    #[tokio::test]
    async fn parse_sse_maps_tool_call_deltas_to_tool_events() {
        let body = b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]}}]}\n\n\
                     data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n\n\
                     data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Nicosia\\\"}\"}}]}}]}\n\n\
                     data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
                     data: [DONE]\n\n";
        let body_stream = stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(body.to_vec()))]);
        let session_id = aios_proto::aios::v1::SessionId {
            value: "s".to_string(),
        };
        let mut events = Vec::new();
        let mut s = parse_sse_token_stream(body_stream, session_id);
        while let Some(evt) = s.next().await {
            events.push(evt.expect("ok event"));
        }
        let payload = |i: usize| -> serde_json::Value {
            serde_json::from_slice(&events[i].record.as_ref().unwrap().payload).unwrap()
        };
        // open + 2 fragments + close + FINISH(tool_calls).
        assert_eq!(events.len(), 5, "events: {events:?}");
        assert_eq!(events[0].kind(), AgentEventKind::ToolCallPending);
        assert_eq!(payload(0)["id"], "call_1");
        assert_eq!(payload(0)["name"], "get_weather");
        assert_eq!(events[1].kind(), AgentEventKind::ToolCallPending);
        assert_eq!(payload(1)["partial_json"], "{\"city\":");
        assert_eq!(events[2].kind(), AgentEventKind::ToolCallPending);
        assert_eq!(payload(2)["partial_json"], "\"Nicosia\"}");
        assert_eq!(events[3].kind(), AgentEventKind::ToolCallPending);
        assert_eq!(payload(3)["id"], "call_1");
        assert_eq!(payload(3)["done"], true);
        assert_eq!(events[4].kind(), AgentEventKind::Finish);
        assert_eq!(payload(4)["reason"], "tool_calls");
        // Sequences are strictly increasing across the synthesized events.
        let seqs: Vec<u64> = events
            .iter()
            .map(|e| e.record.as_ref().unwrap().sequence)
            .collect();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "seqs: {seqs:?}");
    }

    #[tokio::test]
    async fn parse_sse_emits_token_per_delta_then_finish() {
        // Three tokens then [DONE]. The parser should emit 3 TOKEN events
        // each with the correct text + a FINISH.
        let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
                     data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n\
                     data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}\n\n\
                     data: [DONE]\n\n";
        let body_stream = stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(body.to_vec()))]);
        let session_id = aios_proto::aios::v1::SessionId {
            value: "sess_x".to_string(),
        };
        let mut events = parse_sse_token_stream(body_stream, session_id);
        let mut tokens = Vec::new();
        let mut finish_seen = false;
        let mut s = Box::pin(events.by_ref());
        while let Some(evt) = s.next().await {
            let evt = evt.expect("ok event");
            match evt.kind() {
                AgentEventKind::Token => {
                    let payload = evt.record.as_ref().unwrap().payload.clone();
                    let v: serde_json::Value = serde_json::from_slice(&payload).unwrap();
                    tokens.push(v["text"].as_str().unwrap_or_default().to_string());
                }
                AgentEventKind::Finish => {
                    finish_seen = true;
                    break;
                }
                _ => panic!("unexpected event kind"),
            }
        }
        assert_eq!(tokens, vec!["Hello", " world", "!"]);
        assert!(finish_seen, "FINISH event emitted");
    }

    #[tokio::test]
    async fn parse_sse_skips_malformed_chunks() {
        let body = b"data: not-json\n\n\
                     data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                     data: [DONE]\n\n";
        let body_stream = stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(body.to_vec()))]);
        let session_id = aios_proto::aios::v1::SessionId {
            value: "s".to_string(),
        };
        let mut events = parse_sse_token_stream(body_stream, session_id);
        let mut s = Box::pin(events.by_ref());
        let first = s.next().await.expect("event").expect("ok");
        assert_eq!(first.kind(), AgentEventKind::Token);
        let payload: serde_json::Value =
            serde_json::from_slice(&first.record.as_ref().unwrap().payload).unwrap();
        assert_eq!(payload["text"], "ok");
        let second = s.next().await.expect("event").expect("ok");
        assert_eq!(second.kind(), AgentEventKind::Finish);
    }

    #[tokio::test]
    async fn parse_sse_handles_finish_via_finish_reason_field() {
        // OpenAI emits a final chunk with `finish_reason: "stop"` and no
        // delta content, sometimes WITHOUT a trailing `[DONE]`. We
        // should treat that as the terminator.
        let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
                     data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n";
        let body_stream = stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(body.to_vec()))]);
        let session_id = aios_proto::aios::v1::SessionId {
            value: "s".to_string(),
        };
        let mut events = parse_sse_token_stream(body_stream, session_id);
        let mut s = Box::pin(events.by_ref());
        let first = s.next().await.expect("event").expect("ok");
        assert_eq!(first.kind(), AgentEventKind::Token);
        let second = s.next().await.expect("event").expect("ok");
        assert_eq!(second.kind(), AgentEventKind::Finish);
        let payload: serde_json::Value =
            serde_json::from_slice(&second.record.as_ref().unwrap().payload).unwrap();
        assert_eq!(payload["reason"], "stop");
    }

    #[tokio::test]
    async fn parse_sse_synthesizes_finish_when_upstream_closes_early() {
        // No [DONE], no finish_reason — just two tokens then EOF.
        // Parser must synthesize a FINISH so the downstream pump exits.
        let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n\
                     data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n";
        let body_stream = stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(body.to_vec()))]);
        let session_id = aios_proto::aios::v1::SessionId {
            value: "s".to_string(),
        };
        let mut events = parse_sse_token_stream(body_stream, session_id);
        let mut kinds = Vec::new();
        let mut s = Box::pin(events.by_ref());
        while let Some(evt) = s.next().await {
            kinds.push(evt.expect("ok").kind());
        }
        assert_eq!(
            kinds,
            vec![
                AgentEventKind::Token,
                AgentEventKind::Token,
                AgentEventKind::Finish
            ],
            "synthesized FINISH closes the stream",
        );
    }

    #[tokio::test]
    async fn end_to_end_against_wiremock_streams_real_http_response() {
        let server = MockServer::start().await;
        let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
                        data: {\"choices\":[{\"delta\":{\"content\":\" from\"}}]}\n\n\
                        data: {\"choices\":[{\"delta\":{\"content\":\" wiremock\"}}]}\n\n\
                        data: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body),
            )
            .mount(&server)
            .await;

        let arc = VercelAiGatewayArcan::new(cfg(server.uri())).expect("build");
        let stream = arc
            .dispatch_message("sess_x", "hello?", None, &[])
            .await
            .expect("dispatch");
        let mut tokens = Vec::new();
        let mut finish_seen = false;
        let mut s = Box::pin(stream);
        while let Some(evt) = s.next().await {
            let evt = evt.expect("ok event");
            match evt.kind() {
                AgentEventKind::Token => {
                    let payload: serde_json::Value =
                        serde_json::from_slice(&evt.record.as_ref().unwrap().payload).unwrap();
                    tokens.push(payload["text"].as_str().unwrap_or_default().to_string());
                }
                AgentEventKind::Finish => {
                    finish_seen = true;
                }
                _ => panic!("unexpected event kind"),
            }
        }
        assert_eq!(tokens, vec!["Hello", " from", " wiremock"]);
        assert!(finish_seen);
    }

    /// BRO-1206: when `dispatch_message` is called with `Some(model)`,
    /// the outbound HTTP body's `model` field MUST equal the override.
    /// Verified via `wiremock::matchers::body_partial_json` — wiremock
    /// only serves the mock when the `model` field matches the override,
    /// proving the wire shape end-to-end.
    #[tokio::test]
    async fn dispatch_message_override_reaches_outbound_body() {
        let server = MockServer::start().await;
        let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                        data: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_partial_json(serde_json::json!({
                "model": "openai/gpt-4o-mini"
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body),
            )
            .mount(&server)
            .await;

        let arc = VercelAiGatewayArcan::new(cfg(server.uri())).expect("build");
        // Override should win over `cfg.model = "test-model"`.
        let stream = arc
            .dispatch_message("sess_x", "hi", Some("openai/gpt-4o-mini"), &[])
            .await
            .expect("override reaches body");
        let mut s = Box::pin(stream);
        let first = s.next().await.expect("event").expect("ok");
        assert_eq!(first.kind(), AgentEventKind::Token);
    }

    /// BRO-1206: when `dispatch_message` is called with `None` (or empty
    /// override), the outbound HTTP body's `model` field MUST equal
    /// `cfg.model` — i.e. the env-bound default. Mock will only respond
    /// when the env-default flows through.
    #[tokio::test]
    async fn dispatch_message_env_fallback_reaches_outbound_body() {
        let server = MockServer::start().await;
        let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                        data: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_partial_json(serde_json::json!({
                "model": "test-model"
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body),
            )
            .mount(&server)
            .await;

        let arc = VercelAiGatewayArcan::new(cfg(server.uri())).expect("build");
        // No override → cfg.model is "test-model" (from `cfg(...)` helper).
        let stream = arc
            .dispatch_message("sess_x", "hi", None, &[])
            .await
            .expect("env fallback reaches body");
        let mut s = Box::pin(stream);
        let first = s.next().await.expect("event").expect("ok");
        assert_eq!(first.kind(), AgentEventKind::Token);
    }

    #[tokio::test]
    async fn dispatch_propagates_http_errors_with_actionable_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error":"bad key"}"#))
            .mount(&server)
            .await;

        let arc = VercelAiGatewayArcan::new(cfg(server.uri())).expect("build");
        match arc.dispatch_message("sess_x", "hi", None, &[]).await {
            Ok(_) => panic!("must surface 401"),
            Err(ArcanProxyError::Transport(m)) => {
                assert!(m.contains("401"), "msg: {m}");
                assert!(m.contains("bad key"), "msg: {m}");
            }
            Err(other) => panic!("expected Transport error, got {other:?}"),
        }
    }

    #[test]
    fn create_agent_returns_opaque_agent_id() {
        let arc =
            VercelAiGatewayArcan::new(cfg("http://localhost:8080".to_string())).expect("build");
        let id = futures::executor::block_on(arc.create_agent("sess_xyz")).expect("ok");
        assert_eq!(id, "agent-sess_xyz");
    }

    #[test]
    fn destroy_agent_is_a_noop() {
        let arc =
            VercelAiGatewayArcan::new(cfg("http://localhost:8080".to_string())).expect("build");
        futures::executor::block_on(arc.destroy_agent("sess_xyz")).expect("ok");
    }

    #[test]
    fn from_env_reads_required_api_key() {
        // Snapshot + restore env across this test.
        let prev_key = std::env::var("OPENAI_API_KEY").ok();
        // SAFETY: rust 2024 marks env mutators unsafe. Tests in this
        // module are not parallel against this var.
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
        }
        match VercelAiGatewayConfig::from_env() {
            Ok(_) => panic!("must reject missing OPENAI_API_KEY"),
            Err(ArcanProxyError::Transport(m)) => {
                assert!(m.contains("OPENAI_API_KEY"), "msg: {m}");
            }
            Err(other) => panic!("expected Transport error, got {other:?}"),
        }
        // Restore so other tests keep working.
        if let Some(v) = prev_key {
            unsafe {
                std::env::set_var("OPENAI_API_KEY", v);
            }
        }
    }

    #[test]
    fn from_env_falls_back_to_defaults_when_optionals_missing() {
        // Snapshot + restore env.
        let prev_key = std::env::var("OPENAI_API_KEY").ok();
        let prev_base = std::env::var("OPENAI_BASE_URL").ok();
        let prev_model = std::env::var("OPENAI_MODEL").ok();
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "vck_test");
            std::env::remove_var("OPENAI_BASE_URL");
            std::env::remove_var("OPENAI_MODEL");
        }
        let cfg = VercelAiGatewayConfig::from_env().expect("ok");
        assert_eq!(cfg.api_key, "vck_test");
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert_eq!(cfg.model, DEFAULT_MODEL);
        // Restore.
        unsafe {
            match prev_key {
                Some(v) => std::env::set_var("OPENAI_API_KEY", v),
                None => std::env::remove_var("OPENAI_API_KEY"),
            }
            match prev_base {
                Some(v) => std::env::set_var("OPENAI_BASE_URL", v),
                None => std::env::remove_var("OPENAI_BASE_URL"),
            }
            match prev_model {
                Some(v) => std::env::set_var("OPENAI_MODEL", v),
                None => std::env::remove_var("OPENAI_MODEL"),
            }
        }
    }
}
