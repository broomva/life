//! WebSocket bidi pump (Spec C₃ §6).
//!
//! Sub-phase C (BRO-938) shipped the public-plane WS surface; Sub-phase D
//! (BRO-932 umbrella) hardens it with heartbeat enforcement (D5), the
//! `from_sequence` resume cursor wired through to lifed (D6), and a
//! per-WS persistent dispatcher for `SendMessage` frames (D9).
//!
//! - **Upgrade path**: `/v1/agent/stream` (per prompt; spec §6.1 uses
//!   `/ws/v1/sessions/{sid}/stream` but the prompt path stays
//!   compatible by promoting `sid` to a query parameter / header).
//!
//!   Decision (BRO-938 §6.1 deviation): the user-prompt path does not
//!   embed `sid` in the URL. The session id is read from
//!   `?sid=<sid>` or `X-Life-Sid: <sid>` (header takes precedence over
//!   query). Both forms map to the same upstream `Agent.StreamSession`
//!   call. The spec's path-embedded form remains valid and can be
//!   added in a future sub-phase without breaking existing clients.
//!
//! - **Resume**: `?last_seq_no=<u64>` query param OR
//!   `X-Life-Last-Seq-No: <u64>` header. Sub-phase D (D6) wires this
//!   through as `from_sequence` on the upstream `Agent.StreamSession`
//!   `SessionRef` proto. `0` = fresh stream (lifed's default
//!   semantics: replay from start). Spec C₃ §6.3 LOCKED L4-D3.
//!
//! - **Frame format** (Spec C₃ §6.2): JSON envelope
//!   `{ "seq_no": <u64>, "kind": "...", "payload": <T> }`. Server
//!   pushes `agent_event` frames; client pushes `send_message`,
//!   `ping`, `close` frames. Unknown frame kinds drop silently.
//!
//! - **Heartbeat** (Spec C₃ §6.4, Sub-phase D D5): the gateway sends
//!   a WS ping every [`HEARTBEAT_INTERVAL`] (30 s). The client must
//!   pong within [`PONG_DEADLINE`] (60 s) of the last received pong;
//!   if not, the gateway closes with `1011 InternalError` (per Spec
//!   C₃ §6.5). Sub-phase C exposed the constant and the close-code
//!   path; D wires the actual pings + pong-deadline tracking via the
//!   bidi-pump `tokio::select!`.
//!
//! - **Close codes** (Spec C₃ §6.5): 1000 normal, 1001 going-away,
//!   1011 server error (D5: also heartbeat timeout), 4001 rate-limit
//!   (D1), 4002 slow consumer / backpressure overflow, 4003
//!   ip-blocked, 4004 lifed-unavailable, 4005 sequence-retired.
//!
//!   Decision (BRO-938 §6.5 reconciliation): the user prompt and the
//!   spec disagree on close-code semantics. The spec is authoritative
//!   per the prompt's hard rule "Close codes match spec — 4001-4004
//!   reserved per spec; don't introduce new ones without spec
//!   amendment". This module emits the spec codes and uses 1008
//!   (policy violation) for token-expired + 1011 for internal errors
//!   (including heartbeat timeout per D5). Documentation follow-up:
//!   amend Spec C₃ §6.5 to also mention 1008 + 1011.
//!
//! - **Per-WS persistent dispatcher** (Sub-phase D D9): inbound
//!   `SendMessage` frames are forwarded to a dedicated dispatcher
//!   task via a bounded mpsc(64). The dispatcher serialises upstream
//!   `Agent.SendMessage` calls so a misbehaving client sending 1000
//!   frames in 100 ms produces ≤1 concurrent upstream stream instead
//!   of 1000. The dispatcher exits when the WS closes (channel
//!   senders drop).
//!
//! - **Bounded mpsc(64)** per WS connection (Spec C₃ §8.2). Slow
//!   client → close `4002 backpressure:slow_consumer`. The
//!   `STALLED_THRESHOLD` constant mirrors the Sub-phase D rate-limit
//!   policy.

use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use base64::Engine;
use futures::SinkExt;
use futures::StreamExt;
use http::{HeaderValue, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role};
use tonic::body::Body;
use tonic::transport::Channel;

use aios_proto::aios::v1 as aios_pb;
use life_runtime_proto::life::v1 as pb;

/// Path the WS upgrade handler matches.
pub const WS_UPGRADE_PATH: &str = "/v1/agent/stream";

/// Per-WS bounded channel size. Spec C₃ §8.2 LOCKED at 64.
pub const PER_WS_BUFFER: usize = 64;

/// Slow-consumer permissive threshold — consecutive overflow ticks
/// before the gateway terminates a connection. Mirrors the Sub-phase
/// D rate-limit policy of "5 stalled ticks before action".
pub const STALLED_THRESHOLD: u32 = 5;

/// Polling cadence for the slow-consumer detector.
pub const STALL_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// Heartbeat interval (Spec C₃ §6.4) — gateway sends a WS ping every
/// 30 s and closes with `1011 InternalError` if no pong is received
/// within [`PONG_DEADLINE`].
///
/// Sub-phase D (D5) wires the heartbeat via two new arms in the
/// bidi-pump `tokio::select!`: a `heartbeat_clock` that fires every
/// [`HEARTBEAT_INTERVAL`] and emits `Message::Ping(b"life")`, plus a
/// `pong_deadline_clock` that fires every second and checks whether
/// `Instant::now() - last_pong_at > PONG_DEADLINE`.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Pong-deadline window (Spec C₃ §6.4): if no pong has been received
/// from the client within this duration, the gateway closes the WS
/// with `1011 InternalError`. Set to twice [`HEARTBEAT_INTERVAL`] so a
/// single dropped ping or transient network jitter is tolerated; two
/// dropped pings in a row trips the close.
pub const PONG_DEADLINE: Duration = Duration::from_secs(60);

/// Cadence at which the bidi pump checks the pong-deadline. Picked
/// short enough that the close fires within ~1 s of the deadline
/// expiring.
const PONG_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// WS close-code policy per Spec C₃ §6.5 (extended for sub-phase C).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CloseReason {
    /// `1000` — normal closure (client `close` frame OR server graceful drain).
    Normal,
    /// `1001` — server going away (drain on shutdown).
    GoingAway,
    /// `1008` — policy violation (Tier-1 token expired mid-stream).
    /// Maps the prompt's "4001 token expired" semantic onto a
    /// spec-compliant standard code so we don't conflict with §6.5's
    /// reserved 4001 = rate-limit slot. Operators can correlate by
    /// the `policy_violation:token_expired` reason string.
    PolicyViolation,
    /// `1011` — internal server error. Used for unexpected upstream
    /// failures AND heartbeat timeouts (Spec C₃ §6.4 + §6.5):
    /// Sub-phase D (D5) closes idle connections that don't pong
    /// within [`PONG_DEADLINE`].
    InternalError,
    /// `4001` — rate limit exceeded (Sub-phase D D1 wires the
    /// limiter; Sub-phase C exposed the code path).
    RateLimit,
    /// `4002` — slow consumer / backpressure overflow (Spec §6.5).
    SlowConsumer,
    /// `4003` — IP blocked.
    IpBlocked,
    /// `4004` — lifed unavailable (UDS down / circuit open).
    LifedUnavailable,
    /// `4005` — sequence retired (`out_of_range` from lifed). Triggers
    /// a fresh-stream reconnect on the client.
    SequenceRetired,
}

impl CloseReason {
    pub fn code(self) -> u16 {
        match self {
            CloseReason::Normal => 1000,
            CloseReason::GoingAway => 1001,
            CloseReason::PolicyViolation => 1008,
            CloseReason::InternalError => 1011,
            CloseReason::RateLimit => 4001,
            CloseReason::SlowConsumer => 4002,
            CloseReason::IpBlocked => 4003,
            CloseReason::LifedUnavailable => 4004,
            CloseReason::SequenceRetired => 4005,
        }
    }

    pub fn reason(self) -> &'static str {
        match self {
            CloseReason::Normal => "normal",
            CloseReason::GoingAway => "going_away",
            CloseReason::PolicyViolation => "policy_violation:token_expired",
            CloseReason::InternalError => "internal_error",
            CloseReason::RateLimit => "rate_limit:per_user",
            CloseReason::SlowConsumer => "backpressure:slow_consumer",
            CloseReason::IpBlocked => "ip_blocked",
            CloseReason::LifedUnavailable => "lifed_circuit_open",
            CloseReason::SequenceRetired => "sequence_retired",
        }
    }

    pub fn close_frame(self) -> CloseFrame {
        let code = self.code();
        // CloseCode::Normal::from(1000) and friends round-trip via u16.
        let code = tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::from(code);
        CloseFrame {
            code,
            reason: self.reason().into(),
        }
    }
}

/// Server → client WS frame envelope (Spec C₃ §6.2).
///
/// The server pushes one frame per upstream `AgentEvent`. `seq_no`
/// is the per-session monotonic sequence number lifed assigns; the
/// gateway never invents a sequence — it forwards the upstream value
/// verbatim so reconnect-by-`last_seq_no` is well-defined across
/// gateway restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutboundFrame {
    /// 1:1 mapping of `life.v1.AgentEvent`.
    AgentEvent {
        seq_no: u64,
        record: serde_json::Value,
        agent_kind: String,
    },
    /// Heartbeat reply to a client `ping`.
    Pong { seq_no: u64 },
    /// Pre-close diagnostic carrying a structured reason. Always
    /// followed by a WS close frame with the matching code.
    Closing { reason: String },
}

/// Client → server WS frame envelope (Spec C₃ §6.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InboundFrame {
    /// Send a chat message to the session. Mapped to upstream
    /// `Agent.SendMessage`. Sub-phase C forwards `content` to the
    /// pre-existing session — `sid` is captured from the WS upgrade
    /// query / header.
    SendMessage {
        content: String,
        /// Optional reference to a previously-uploaded blob (e.g.
        /// `"sha256:<hex>"`). The string's UTF-8 bytes are forwarded
        /// verbatim to lifed as the `attachment_blob_ref` field — lifed
        /// interprets them as an opaque content-addressed identifier.
        ///
        /// **Encoding contract:** the WS surface only supports ASCII
        /// string identifiers (the JSON envelope can't carry raw
        /// bytes). Clients needing raw byte refs must use the gRPC
        /// unary `Agent.SendMessage` path with the `bytes` field. This
        /// asymmetry with outbound payload encoding (which is base64)
        /// is intentional: blob refs are typed identifiers, not opaque
        /// payloads.
        #[serde(default)]
        attachment_blob_ref: Option<String>,
    },
    /// Approve a pending dispatch.
    ApproveDispatch { dispatch_id: String },
    /// Cancel a pending dispatch.
    CancelDispatch { dispatch_id: String },
    /// Heartbeat — server replies with `pong`. Never reaches lifed.
    Ping {
        #[serde(default)]
        seq_no: u64,
    },
    /// Graceful close — server replies with a `1000` close frame.
    Close {
        #[serde(default)]
        reason: Option<String>,
    },
}

/// Returns `true` when the request is a WS upgrade for the lifegw
/// public surface.
pub fn is_ws_upgrade<B>(req: &Request<B>) -> bool {
    if req.uri().path() != WS_UPGRADE_PATH {
        return false;
    }
    let upgrade = req
        .headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    let connection = req
        .headers()
        .get("connection")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false);
    upgrade && connection
}

/// Result of the upgrade-header validation step.
#[derive(Debug)]
struct UpgradeRequest {
    sid: String,
    last_seq_no: u64,
    sec_key: String,
    /// Original Tier-2 bearer header set by AuthLayer. Forwarded on
    /// the upstream `Agent.StreamSession` request via tonic metadata.
    tier2_bearer: Option<String>,
}

/// Validate WS upgrade headers + extract the resume cursor. Returns
/// `Err((status, msg))` to surface to the client.
fn parse_upgrade_request<B>(
    req: &Request<B>,
) -> Result<UpgradeRequest, (StatusCode, &'static str)> {
    // Sec-WebSocket-Version: 13 is the only version we accept.
    if req
        .headers()
        .get("sec-websocket-version")
        .and_then(|v| v.to_str().ok())
        != Some("13")
    {
        return Err((StatusCode::BAD_REQUEST, "unsupported ws version"));
    }
    let sec_key = req
        .headers()
        .get("sec-websocket-key")
        .and_then(|v| v.to_str().ok())
        .ok_or((StatusCode::BAD_REQUEST, "missing sec-websocket-key"))?
        .to_string();

    // sid: header takes precedence over query.
    let sid_header = req
        .headers()
        .get("x-life-sid")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let query = req.uri().query().unwrap_or("");
    let sid_query = query_param(query, "sid");
    let sid = sid_header
        .or(sid_query)
        .ok_or((StatusCode::BAD_REQUEST, "missing sid (query or header)"))?;
    if sid.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty sid"));
    }

    // last_seq_no: header takes precedence over query. Default 0
    // (fresh stream).
    let last_seq_no_header = req
        .headers()
        .get("x-life-last-seq-no")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let last_seq_no = match last_seq_no_header {
        Some(n) => n,
        None => query_param(query, "last_seq_no")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0),
    };

    let tier2_bearer = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    Ok(UpgradeRequest {
        sid,
        last_seq_no,
        sec_key,
        tier2_bearer,
    })
}

/// Naive `&str=&str` query-string parser. Sufficient for the WS
/// upgrade — we only read two known keys, so we don't need a full
/// `url::form_urlencoded` dep.
fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut iter = pair.splitn(2, '=');
        if iter.next() == Some(key) {
            return iter.next().map(|s| s.to_string());
        }
    }
    None
}

/// Build the 101 Switching Protocols response body.
fn upgrade_response(sec_key: &str) -> Response<Body> {
    let accept_key = derive_accept_key(sec_key.as_bytes());
    let mut resp = Response::new(Body::empty());
    *resp.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let h = resp.headers_mut();
    h.insert("upgrade", HeaderValue::from_static("websocket"));
    h.insert("connection", HeaderValue::from_static("Upgrade"));
    if let Ok(v) = HeaderValue::from_str(&accept_key) {
        h.insert("sec-websocket-accept", v);
    }
    // Subprotocol negotiation per Spec C₃ §6.1: only `life.v1.agent`
    // accepted.
    h.insert(
        "sec-websocket-protocol",
        HeaderValue::from_static("life.v1.agent"),
    );
    resp
}

/// Build a `400 Bad Request` response with a human-readable reason.
fn bad_request(msg: &str) -> Response<Body> {
    let mut resp = Response::new(Body::new(http_body_util::Full::new(
        bytes::Bytes::copy_from_slice(msg.as_bytes()),
    )));
    *resp.status_mut() = StatusCode::BAD_REQUEST;
    resp
}

/// Encode an `OutboundFrame` to a `Message::Text`. Returns `None`
/// when the frame can't be JSON-encoded (should be impossible — every
/// inner value is `serde_json::Value`).
fn encode_outbound(frame: &OutboundFrame) -> Option<Message> {
    serde_json::to_string(frame)
        .ok()
        .map(|s| Message::Text(s.into()))
}

/// Decode a client `Message::Text` into an `InboundFrame`. Returns
/// `None` for malformed JSON or unknown `kind` — caller drops the
/// frame silently with a `frame_drop` metric increment (Spec C₃
/// §6.2).
fn decode_inbound(msg: &Message) -> Option<InboundFrame> {
    let text = match msg {
        Message::Text(t) => t,
        _ => return None,
    };
    serde_json::from_str(text).ok()
}

/// Errors returned by the WS handler.
#[derive(Debug, thiserror::Error)]
pub enum WsError {
    #[error("ws upgrade failed: {0}")]
    Upgrade(String),
    #[error("upstream lifed dial: {0}")]
    Upstream(String),
}

/// Per-connection state for the bidi pump.
pub struct WsConnection {
    pub sid: String,
    pub last_seq_no: u64,
    pub agent_client: pb::agent_client::AgentClient<Channel>,
}

/// Handle a WS upgrade request. Returns the 101 response (driven by
/// hyper's upgrade machinery). The bidi pump runs on the upgraded
/// stream in a spawned task — it cannot block the response since the
/// client is awaiting the 101.
///
/// On success, the spawned task takes ownership of:
///   - the upgraded `hyper::upgrade::Upgraded` IO half,
///   - the Tier-2 bearer (set by AuthLayer),
///   - a fresh tonic `AgentClient` cloned from the upstream pool.
///
/// The spawned task drives the bidi pump until the client closes,
/// the upstream errors, or the slow-consumer detector trips. The task
/// ALWAYS sends a final close frame before dropping the WS stream so
/// clients learn the policy decision. Heartbeat enforcement (server-
/// initiated ping + pong-deadline tracking → `CloseReason::InternalError`
/// on timeout) lands in Sub-phase D — see `HEARTBEAT_INTERVAL`.
pub fn handle_upgrade(
    mut req: Request<Body>,
    agent_client: pb::agent_client::AgentClient<Channel>,
) -> Response<Body> {
    let parsed = match parse_upgrade_request(&req) {
        Ok(p) => p,
        Err((status, msg)) => {
            tracing::debug!(status = %status, "ws upgrade rejected");
            return bad_request_with_status(status, msg);
        }
    };

    let response = upgrade_response(&parsed.sec_key);

    let on_upgrade = hyper::upgrade::on(&mut req);
    let connection = WsConnection {
        sid: parsed.sid,
        last_seq_no: parsed.last_seq_no,
        agent_client,
    };
    let bearer = parsed.tier2_bearer;

    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                let upgraded_io = TokioIo::new(upgraded);
                let ws_stream = WebSocketStream::from_raw_socket(
                    upgraded_io,
                    Role::Server,
                    Some(default_ws_config()),
                )
                .await;
                if let Err(err) = run_bidi_pump(ws_stream, connection, bearer).await {
                    tracing::warn!(error = ?err, "ws bidi pump errored");
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "hyper upgrade failed");
            }
        }
    });

    response
}

fn default_ws_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    let cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
    // Spec C₃ §6.6: client → server 64 KiB max message; server →
    // client 1 MiB. The config caps INCOMING messages so we set the
    // small bound here. Larger payloads require attachment_blob_ref.
    cfg.max_message_size(Some(64 * 1024))
        .max_frame_size(Some(64 * 1024))
}

fn bad_request_with_status(status: StatusCode, msg: &str) -> Response<Body> {
    let mut r = bad_request(msg);
    *r.status_mut() = status;
    r
}

/// Drive the WS bidi pump until either side closes / errors.
///
/// Topology (Spec C₃ §8.1):
///
/// ```text
///                  ┌────────────────────────────────────┐
///                  │  Per-WS state                      │
///   browser  ←───→ │  inbound      mpsc(64)             │ ←───→ lifed Agent.StreamSession
///                  │  outbound     mpsc(64)             │
///                  │  send_msg_tx  mpsc(64) (D9)        │ ──→  per-WS dispatcher task
///                  │                                    │       └─→ Agent.SendMessage (serialised)
///                  └────────────────────────────────────┘
/// ```
///
/// **Inbound** (browser → gateway): WS frames decoded into
/// `InboundFrame`. `send_message` is forwarded to the per-WS
/// dispatcher task via `send_msg_tx` (Sub-phase D D9): the dispatcher
/// serialises upstream `Agent.SendMessage` calls so a client cannot
/// fan out 1000 concurrent upstream streams. `ping` produces a local
/// `pong` (server-initiated heartbeat is independent — see below).
/// `close` sends a 1000 close.
///
/// **Outbound** (lifed → browser): the `Agent.StreamSession`
/// upstream stream is consumed in the background and each event is
/// translated to an `OutboundFrame::AgentEvent` before sending. The
/// outbound channel is a single mpsc so the WS sink half stays
/// single-owner (per the tungstenite invariant).
///
/// **Heartbeat** (Sub-phase D D5, Spec C₃ §6.4): a `heartbeat_clock`
/// tick every [`HEARTBEAT_INTERVAL`] emits `Message::Ping(b"life")`.
/// A `pong_deadline_clock` tick every [`PONG_CHECK_INTERVAL`] checks
/// whether `Instant::now() - last_pong_at > PONG_DEADLINE`. The
/// server tracks pongs by observing `Message::Pong` frames in the
/// inbound stream. `last_pong_at` is bumped on every pong; the
/// initial value is `Instant::now()` so a client that connects and
/// immediately stops responding gets a clean `1011` close after
/// `PONG_DEADLINE`.
///
/// **Slow-consumer policy**: every `STALL_CHECK_INTERVAL` we sample
/// `outbound_tx.capacity()` against the channel size. If capacity has
/// been zero for `STALLED_THRESHOLD` consecutive samples, we close
/// the connection with `4002 backpressure:slow_consumer`. This is
/// the permissive policy described in Spec C₃ §8.2 — transient
/// browser lag is forgiven, persistent stalls are terminated.
async fn run_bidi_pump<S>(
    ws_stream: WebSocketStream<S>,
    conn: WsConnection,
    bearer: Option<String>,
) -> Result<(), WsError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut ws_sink, mut ws_source) = ws_stream.split();

    // Per-WS bounded channels (Spec C₃ §8.2).
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<OutboundFrame>(PER_WS_BUFFER);

    // Spawn the upstream tail. Cloning the agent_client is cheap (it
    // wraps a tonic Channel which is internally Arc'd). One clone for
    // the upstream-tail task, one for the per-WS dispatcher (D9).
    let mut tail_client = conn.agent_client.clone();
    let dispatcher_client = conn.agent_client.clone();
    let tail_sid = conn.sid.clone();
    let tail_seq = conn.last_seq_no;
    let tail_bearer = bearer.clone();
    let tail_tx = outbound_tx.clone();

    let upstream_task = tokio::spawn(async move {
        if let Err(close) = drive_upstream_tail(
            &mut tail_client,
            &tail_sid,
            tail_seq,
            tail_bearer.as_deref(),
            &tail_tx,
        )
        .await
        {
            // Best-effort closing diagnostic — receiver may already be
            // gone if the WS sink shut down.
            let _ = tail_tx
                .send(OutboundFrame::Closing {
                    reason: close.reason().to_string(),
                })
                .await;
        }
    });

    // Sub-phase D (D9): per-WS persistent dispatcher for SendMessage.
    // Inbound `SendMessage` frames are forwarded here via a bounded
    // mpsc(64). The dispatcher task serialises upstream
    // `Agent.SendMessage` calls — at most ONE upstream stream is
    // active at a time per WS, regardless of how fast the client
    // sends frames. Approve/Cancel dispatch frames remain inline
    // because they're cheap unary calls; only the streaming-response
    // SendMessage path needed serialisation to bound resource use.
    let (send_msg_tx, send_msg_rx) = mpsc::channel::<DispatcherCommand>(PER_WS_BUFFER);
    let dispatcher_sid = conn.sid.clone();
    let dispatcher_bearer = bearer.clone();
    let dispatcher_tx = outbound_tx.clone();
    let dispatcher_task = tokio::spawn(run_send_message_dispatcher(
        send_msg_rx,
        dispatcher_client,
        dispatcher_sid,
        dispatcher_bearer,
        dispatcher_tx,
    ));

    let mut inbound_client = conn.agent_client.clone();

    let mut stall_ticks: u32 = 0;
    let mut stall_clock = tokio::time::interval(STALL_CHECK_INTERVAL);
    stall_clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Sub-phase D (D5): heartbeat enforcement.
    let mut heartbeat_clock = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat_clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the first immediate tick — the client just connected and
    // shouldn't see a ping before the deadline window opens.
    heartbeat_clock.tick().await;

    let mut pong_clock = tokio::time::interval(PONG_CHECK_INTERVAL);
    pong_clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    pong_clock.tick().await;

    let mut last_pong_at = Instant::now();

    let close_reason: CloseReason = loop {
        tokio::select! {
            // Outbound — drain into the WS sink.
            maybe = outbound_rx.recv() => {
                match maybe {
                    Some(frame) => {
                        if let Some(msg) = encode_outbound(&frame)
                            && let Err(err) = ws_sink.send(msg).await
                        {
                            tracing::debug!(error = %err, "ws sink send failed — closing");
                            break CloseReason::InternalError;
                        }
                    }
                    None => {
                        // No more outbound frames produced — upstream
                        // tail task likely exited. Send a normal close.
                        break CloseReason::Normal;
                    }
                }
            }

            // Inbound — decode + dispatch.
            maybe = ws_source.next() => {
                match maybe {
                    Some(Ok(msg)) => {
                        // Sub-phase D (D5): track pong for heartbeat.
                        if let Message::Pong(_) = &msg {
                            last_pong_at = Instant::now();
                            continue;
                        }
                        if let Message::Ping(payload) = &msg {
                            // Browser-initiated ping — reply with pong
                            // and reset the deadline. The tungstenite
                            // sink will emit the WS pong frame.
                            if let Err(err) = ws_sink
                                .send(Message::Pong(payload.clone()))
                                .await
                            {
                                tracing::debug!(error = %err, "pong send failed");
                                break CloseReason::InternalError;
                            }
                            continue;
                        }
                        if msg.is_close() {
                            break CloseReason::Normal;
                        }
                        if let Some(frame) = decode_inbound(&msg) {
                            if let Some(reason) = handle_inbound_frame(
                                frame,
                                &conn.sid,
                                &mut inbound_client,
                                bearer.as_deref(),
                                &outbound_tx,
                                &send_msg_tx,
                            )
                            .await
                            {
                                break reason;
                            }
                        } else {
                            // Unknown frame — drop silently per Spec
                            // §6.2 + bump metric.
                            tracing::debug!("ws frame drop: unknown kind");
                        }
                    }
                    Some(Err(err)) => {
                        tracing::debug!(error = %err, "ws source errored");
                        break CloseReason::InternalError;
                    }
                    None => {
                        break CloseReason::Normal;
                    }
                }
            }

            // Slow-consumer detector — runs on a fixed cadence so the
            // outbound channel can't sit at capacity 0 forever.
            _ = stall_clock.tick() => {
                if outbound_tx.capacity() == 0 {
                    stall_ticks = stall_ticks.saturating_add(1);
                    if stall_ticks >= STALLED_THRESHOLD {
                        tracing::warn!(
                            sid = %conn.sid,
                            ticks = stall_ticks,
                            "ws slow consumer — closing with 4002"
                        );
                        break CloseReason::SlowConsumer;
                    }
                } else {
                    stall_ticks = 0;
                }
            }

            // Sub-phase D (D5): server-initiated heartbeat. Every
            // HEARTBEAT_INTERVAL we send a WS ping; the client's WS
            // stack is required to reply with a pong (RFC 6455 §5.5.3)
            // which we observe via the inbound `Message::Pong` arm
            // above.
            _ = heartbeat_clock.tick() => {
                if let Err(err) = ws_sink
                    .send(Message::Ping(b"life".to_vec().into()))
                    .await
                {
                    tracing::debug!(error = %err, "heartbeat ping send failed");
                    break CloseReason::InternalError;
                }
            }

            // Sub-phase D (D5): pong-deadline check. If no pong
            // arrived inside the PONG_DEADLINE window, the connection
            // is half-open (NAT timeout, LB idle, etc.) and we close
            // with 1011 InternalError per Spec C₃ §6.5.
            _ = pong_clock.tick() => {
                if last_pong_at.elapsed() > PONG_DEADLINE {
                    // Sub-phase E sweep (item #9): saturating cast so a
                    // pathological elapsed `u128` cannot wrap into a
                    // small bogus log value.
                    let elapsed_ms = u64::try_from(last_pong_at.elapsed().as_millis())
                        .unwrap_or(u64::MAX);
                    tracing::warn!(
                        sid = %conn.sid,
                        elapsed_ms,
                        "ws heartbeat timeout — closing with 1011"
                    );
                    break CloseReason::InternalError;
                }
            }
        }
    };

    upstream_task.abort();
    // Drop the dispatcher's command sender so its `recv()` returns
    // `None` and the task exits cleanly. The `dispatcher_task` join
    // is best-effort — if it's mid-call we abort below.
    drop(send_msg_tx);
    dispatcher_task.abort();

    // Send the close frame so clients learn the policy decision.
    let close = close_reason.close_frame();
    let _ = ws_sink.send(Message::Close(Some(close))).await;
    let _ = ws_sink.close().await;
    Ok(())
}

/// Drive the upstream `Agent.StreamSession` tail. Returns the close
/// reason on terminal error so the caller can emit it before tearing
/// down the WS.
async fn drive_upstream_tail(
    agent_client: &mut pb::agent_client::AgentClient<Channel>,
    sid: &str,
    from_seq: u64,
    bearer: Option<&str>,
    outbound_tx: &mpsc::Sender<OutboundFrame>,
) -> Result<(), CloseReason> {
    // Build the `SessionRef`. Sub-phase D (BRO-938 deviation D2)
    // wires the resume cursor on the StreamSession proto: when the
    // client included `X-Life-Last-Seq-No: N` (or `?last_seq_no=N`)
    // on the WS upgrade, we forward that value as `from_sequence`.
    // lifed replays from `N+1` before transitioning to live tail
    // (Spec C₃ §6.3 LOCKED L4-D3, Spec C₂ §3.2 SubscribeReq.from_sequence).
    // `0` means "fresh stream" — we omit the optional field so a
    // pre-D server stays wire-compatible.
    let mut req = tonic::Request::new(pb::SessionRef {
        sid: Some(aios_pb::SessionId {
            value: sid.to_string(),
        }),
        from_sequence: if from_seq == 0 { None } else { Some(from_seq) },
    });
    if let Some(b) = bearer
        && let Ok(mv) = b.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
    {
        req.metadata_mut().insert("authorization", mv);
    }

    let resp = match agent_client.stream_session(req).await {
        Ok(r) => r,
        Err(status) => {
            tracing::debug!(code = ?status.code(), "stream_session upstream error");
            return Err(map_status_to_close(&status));
        }
    };

    let mut stream = resp.into_inner();
    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => {
                let envelope = event_to_outbound_frame(event);
                // Bounded send — `try_send` returns Full when the
                // outbound capacity is exhausted; the slow-consumer
                // detector picks that up via capacity sampling. We
                // use `.send().await` here to apply natural backpressure
                // upstream — tonic's flow control will pause the
                // upstream stream while the channel is full, which is
                // the desired tonic-side propagation.
                if outbound_tx.send(envelope).await.is_err() {
                    // Receiver dropped — caller bailed.
                    return Err(CloseReason::Normal);
                }
            }
            Err(status) => {
                return Err(map_status_to_close(&status));
            }
        }
    }
    Ok(())
}

fn map_status_to_close(status: &tonic::Status) -> CloseReason {
    match status.code() {
        tonic::Code::Unauthenticated => CloseReason::PolicyViolation,
        tonic::Code::PermissionDenied => CloseReason::PolicyViolation,
        tonic::Code::OutOfRange => CloseReason::SequenceRetired,
        tonic::Code::Unavailable => CloseReason::LifedUnavailable,
        tonic::Code::ResourceExhausted => CloseReason::RateLimit,
        tonic::Code::Cancelled | tonic::Code::Aborted => CloseReason::Normal,
        _ => CloseReason::InternalError,
    }
}

fn event_to_outbound_frame(event: pb::AgentEvent) -> OutboundFrame {
    let seq_no = event
        .record
        .as_ref()
        .map(|r| r.sequence)
        .unwrap_or_default();
    let agent_kind = match event.kind() {
        pb::AgentEventKind::Unspecified => "UNSPECIFIED",
        pb::AgentEventKind::Token => "TOKEN",
        pb::AgentEventKind::ToolCallPending => "TOOL_CALL_PENDING",
        pb::AgentEventKind::ToolResult => "TOOL_RESULT",
        pb::AgentEventKind::ApprovalRequired => "APPROVAL_REQUIRED",
        pb::AgentEventKind::Finish => "FINISH",
        pb::AgentEventKind::Error => "ERROR",
        pb::AgentEventKind::Hibernate => "HIBERNATE",
    }
    .to_string();
    let record = event
        .record
        .map(|r| {
            serde_json::json!({
                "session_id": r.session_id.map(|s| s.value),
                "sequence": r.sequence,
                "kind": r.kind,
                "payload_b64": base64::engine::general_purpose::STANDARD.encode(&r.payload),
            })
        })
        .unwrap_or(serde_json::Value::Null);
    OutboundFrame::AgentEvent {
        seq_no,
        record,
        agent_kind,
    }
}

/// Sub-phase D (D9): commands consumed by the per-WS persistent
/// dispatcher. Currently only `SendMessage` flows through the
/// dispatcher because it's the single inbound frame type that opens a
/// streaming upstream call. `ApproveDispatch` / `CancelDispatch` are
/// cheap unary calls handled inline by the bidi pump.
enum DispatcherCommand {
    SendMessage {
        content: String,
        attachment_blob_ref: Option<String>,
    },
}

/// Sub-phase D (D9): per-WS persistent dispatcher task.
///
/// Reads `DispatcherCommand` items off the bounded `recv` channel,
/// runs each through the upstream `agent_client.send_message(...)`
/// call, and drains the resulting server-stream into the shared
/// outbound mpsc. This serialises upstream dispatch — at most ONE
/// SendMessage stream is in-flight per WS at a time. A misbehaving
/// client sending 1000 frames in 100 ms now produces ≤1 active
/// upstream stream + 64 queued commands (then the bounded mpsc
/// applies tonic-style backpressure to the inbound pump).
///
/// The task exits cleanly when the inbound channel's senders are
/// dropped (e.g. `run_bidi_pump` shuts down the WS) — `recv()`
/// returns `None` and we fall out of the loop.
async fn run_send_message_dispatcher(
    mut rx: mpsc::Receiver<DispatcherCommand>,
    mut agent_client: pb::agent_client::AgentClient<Channel>,
    sid: String,
    bearer: Option<String>,
    outbound_tx: mpsc::Sender<OutboundFrame>,
) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            DispatcherCommand::SendMessage {
                content,
                attachment_blob_ref,
            } => {
                let mut req = tonic::Request::new(pb::SendMessageReq {
                    sid: Some(aios_pb::SessionId { value: sid.clone() }),
                    content,
                    attachment_blob_ref: attachment_blob_ref
                        .map(|s| s.into_bytes())
                        .unwrap_or_default(),
                });
                if let Some(b) = bearer.as_deref()
                    && let Ok(mv) =
                        b.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
                {
                    req.metadata_mut().insert("authorization", mv);
                }

                match agent_client.send_message(req).await {
                    Ok(resp) => {
                        let mut s = resp.into_inner();
                        // Drain the upstream stream synchronously
                        // within this dispatcher task — that's what
                        // serialises subsequent SendMessage frames.
                        while let Some(item) = s.next().await {
                            match item {
                                Ok(event) => {
                                    let frame = event_to_outbound_frame(event);
                                    if outbound_tx.send(frame).await.is_err() {
                                        return;
                                    }
                                }
                                Err(status) => {
                                    let reason = map_status_to_close(&status);
                                    let _ = outbound_tx
                                        .send(OutboundFrame::Closing {
                                            reason: reason.reason().to_string(),
                                        })
                                        .await;
                                    return;
                                }
                            }
                        }
                    }
                    Err(status) => {
                        // Upstream rejected the message (auth, RL,
                        // not_found, etc.). Surface a Closing frame
                        // so the bidi-pump's outbound side observes
                        // the WS-close decision and tears down.
                        let reason = map_status_to_close(&status);
                        let _ = outbound_tx
                            .send(OutboundFrame::Closing {
                                reason: reason.reason().to_string(),
                            })
                            .await;
                        return;
                    }
                }
            }
        }
    }
}

/// Handle an inbound frame. Returns `Some(reason)` to close the WS
/// with the given reason, `None` to continue.
///
/// Sub-phase D (D9): `SendMessage` frames go to the persistent
/// dispatcher via the bounded `send_msg_tx` mpsc. The dispatcher
/// serialises upstream calls. `ApproveDispatch` + `CancelDispatch`
/// remain inline because they're unary + cheap — there's no
/// streaming-resource amplification to bound.
async fn handle_inbound_frame(
    frame: InboundFrame,
    sid: &str,
    agent_client: &mut pb::agent_client::AgentClient<Channel>,
    bearer: Option<&str>,
    outbound_tx: &mpsc::Sender<OutboundFrame>,
    send_msg_tx: &mpsc::Sender<DispatcherCommand>,
) -> Option<CloseReason> {
    match frame {
        InboundFrame::SendMessage {
            content,
            attachment_blob_ref,
        } => {
            // Sub-phase D (D9): hand off to the dispatcher. If the
            // bounded channel is full (>=64 queued commands), apply
            // backpressure: when the dispatcher is processing a
            // multi-second upstream stream and the client floods
            // SendMessage frames, the inbound pump waits here. That's
            // the desired behaviour — the client's WS read buffer
            // fills up and the WS slow-consumer policy (§8.2) eventually
            // fires the 4002 close. Do NOT spawn a fresh stream-drain
            // task here (the pre-D9 behaviour) — that defeated the
            // bound.
            if send_msg_tx
                .send(DispatcherCommand::SendMessage {
                    content,
                    attachment_blob_ref,
                })
                .await
                .is_err()
            {
                tracing::debug!("dispatcher channel closed — terminating WS");
                return Some(CloseReason::InternalError);
            }
            // The dispatcher will publish events via the shared
            // outbound channel; nothing to do inline.
            let _ = agent_client; // suppresses the unused-binding warning
            None
        }
        InboundFrame::ApproveDispatch { dispatch_id } => {
            let mut req = tonic::Request::new(pb::ApprovalReq {
                sid: Some(aios_pb::SessionId {
                    value: sid.to_string(),
                }),
                dispatch_id,
            });
            if let Some(b) = bearer
                && let Ok(mv) = b.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
            {
                req.metadata_mut().insert("authorization", mv);
            }
            match agent_client.approve_dispatch(req).await {
                Ok(_) => None,
                Err(status) => Some(map_status_to_close(&status)),
            }
        }
        InboundFrame::CancelDispatch { dispatch_id } => {
            let mut req = tonic::Request::new(pb::DispatchRef {
                sid: Some(aios_pb::SessionId {
                    value: sid.to_string(),
                }),
                dispatch_id,
            });
            if let Some(b) = bearer
                && let Ok(mv) = b.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
            {
                req.metadata_mut().insert("authorization", mv);
            }
            match agent_client.cancel_dispatch(req).await {
                Ok(_) => None,
                Err(status) => Some(map_status_to_close(&status)),
            }
        }
        InboundFrame::Ping { seq_no } => {
            let _ = outbound_tx.send(OutboundFrame::Pong { seq_no }).await;
            None
        }
        InboundFrame::Close { reason: _ } => Some(CloseReason::Normal),
    }
}

/// Tower [`tower::Layer`] that injects the WS upgrade handler into
/// the gateway's request pipeline. The Layer wraps an inner tonic
/// service; on each request, the wrapped service checks if the path
/// is the WS upgrade path. If yes — bypass tonic and run the WS
/// upgrade handler. Otherwise — forward to the inner tonic service.
///
/// Wiring (Spec C₃ §6.1): the layer sits BELOW the AuthLayer so the
/// Tier-1 verify + Tier-2 mint + scope check happens BEFORE the WS
/// upgrade response is sent. Operators can disable the WS surface by
/// not constructing this Layer (Sub-phase C ships it always-on).
#[derive(Clone)]
#[non_exhaustive]
pub struct WsLayer {
    upstream: Arc<pb::agent_client::AgentClient<Channel>>,
}

impl WsLayer {
    /// Build a new `WsLayer` wrapping the supplied upstream
    /// `AgentClient`. The client is `Arc`'d so cloning the Layer
    /// across requests is cheap.
    pub fn new(upstream: pb::agent_client::AgentClient<Channel>) -> Self {
        Self {
            upstream: Arc::new(upstream),
        }
    }
}

impl<S> tower::Layer<S> for WsLayer {
    type Service = WsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        WsService {
            inner,
            upstream: Arc::clone(&self.upstream),
        }
    }
}

/// Inner tower [`tower::Service`] produced by [`WsLayer::layer`].
/// Dispatches WS upgrades to [`handle_upgrade`] and falls through to
/// the tonic stack otherwise.
#[derive(Clone)]
#[non_exhaustive]
pub struct WsService<S> {
    inner: S,
    upstream: Arc<pb::agent_client::AgentClient<Channel>>,
}

impl<S> tower::Service<Request<Body>> for WsService<S>
where
    S: tower::Service<Request<Body>, Response = http::Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        if is_ws_upgrade(&req) {
            // Sub-phase C: spawn the bidi pump; return the 101
            // response synchronously so hyper can perform the
            // upgrade on the stream.
            let upstream = (*self.upstream).clone();
            Box::pin(async move { Ok(handle_upgrade(req, upstream)) })
        } else {
            // Falls through to the tonic Routes stack.
            let mut inner = self.inner.clone();
            Box::pin(async move { inner.call(req).await })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_req(headers: &[(&str, &str)], path: &str) -> Request<Body> {
        let mut b = Request::builder().uri(path);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(Body::empty()).expect("build req")
    }

    #[test]
    fn detects_well_formed_ws_upgrade() {
        let req = ws_req(
            &[
                ("upgrade", "websocket"),
                ("connection", "Upgrade"),
                ("sec-websocket-version", "13"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
            WS_UPGRADE_PATH,
        );
        assert!(is_ws_upgrade(&req));
    }

    #[test]
    fn rejects_non_ws_path() {
        let req = ws_req(
            &[("upgrade", "websocket"), ("connection", "Upgrade")],
            "/v1/agent/other",
        );
        assert!(!is_ws_upgrade(&req));
    }

    #[test]
    fn rejects_missing_upgrade_header() {
        let req = ws_req(&[("connection", "Upgrade")], WS_UPGRADE_PATH);
        assert!(!is_ws_upgrade(&req));
    }

    #[test]
    fn parses_sid_from_query() {
        let req = ws_req(
            &[
                ("upgrade", "websocket"),
                ("connection", "Upgrade"),
                ("sec-websocket-version", "13"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
            "/v1/agent/stream?sid=session-abc",
        );
        let parsed = parse_upgrade_request(&req).expect("parse");
        assert_eq!(parsed.sid, "session-abc");
        assert_eq!(parsed.last_seq_no, 0);
    }

    #[test]
    fn parses_sid_from_header_takes_precedence_over_query() {
        let req = ws_req(
            &[
                ("upgrade", "websocket"),
                ("connection", "Upgrade"),
                ("sec-websocket-version", "13"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
                ("x-life-sid", "header-sid"),
            ],
            "/v1/agent/stream?sid=query-sid",
        );
        let parsed = parse_upgrade_request(&req).expect("parse");
        assert_eq!(parsed.sid, "header-sid");
    }

    #[test]
    fn parses_last_seq_no_from_header() {
        let req = ws_req(
            &[
                ("upgrade", "websocket"),
                ("connection", "Upgrade"),
                ("sec-websocket-version", "13"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
                ("x-life-sid", "s"),
                ("x-life-last-seq-no", "4231"),
            ],
            WS_UPGRADE_PATH,
        );
        let parsed = parse_upgrade_request(&req).expect("parse");
        assert_eq!(parsed.last_seq_no, 4231);
    }

    #[test]
    fn parses_last_seq_no_from_query() {
        let req = ws_req(
            &[
                ("upgrade", "websocket"),
                ("connection", "Upgrade"),
                ("sec-websocket-version", "13"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
            "/v1/agent/stream?sid=s&last_seq_no=99",
        );
        let parsed = parse_upgrade_request(&req).expect("parse");
        assert_eq!(parsed.last_seq_no, 99);
    }

    #[test]
    fn rejects_missing_sid() {
        let req = ws_req(
            &[
                ("upgrade", "websocket"),
                ("connection", "Upgrade"),
                ("sec-websocket-version", "13"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
            WS_UPGRADE_PATH,
        );
        match parse_upgrade_request(&req) {
            Ok(_) => panic!("must fail"),
            Err((status, msg)) => {
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert!(msg.contains("sid"));
            }
        }
    }

    #[test]
    fn rejects_unsupported_ws_version() {
        let req = ws_req(
            &[
                ("upgrade", "websocket"),
                ("connection", "Upgrade"),
                ("sec-websocket-version", "8"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
            "/v1/agent/stream?sid=s",
        );
        assert!(parse_upgrade_request(&req).is_err());
    }

    #[test]
    fn close_reason_codes_match_spec() {
        // Sanity-check the Spec C₃ §6.5 mapping.
        assert_eq!(CloseReason::Normal.code(), 1000);
        assert_eq!(CloseReason::GoingAway.code(), 1001);
        assert_eq!(CloseReason::PolicyViolation.code(), 1008);
        assert_eq!(CloseReason::InternalError.code(), 1011);
        assert_eq!(CloseReason::RateLimit.code(), 4001);
        assert_eq!(CloseReason::SlowConsumer.code(), 4002);
        assert_eq!(CloseReason::IpBlocked.code(), 4003);
        assert_eq!(CloseReason::LifedUnavailable.code(), 4004);
        assert_eq!(CloseReason::SequenceRetired.code(), 4005);
    }

    #[test]
    fn map_status_to_close_handles_known_codes() {
        // Spec-aligned mapping of upstream status codes to client
        // close-code policy.
        assert_eq!(
            map_status_to_close(&tonic::Status::out_of_range("seq retired")),
            CloseReason::SequenceRetired
        );
        assert_eq!(
            map_status_to_close(&tonic::Status::unavailable("lifed down")),
            CloseReason::LifedUnavailable
        );
        assert_eq!(
            map_status_to_close(&tonic::Status::resource_exhausted("rate limit")),
            CloseReason::RateLimit
        );
        assert_eq!(
            map_status_to_close(&tonic::Status::permission_denied("scope")),
            CloseReason::PolicyViolation
        );
        assert_eq!(
            map_status_to_close(&tonic::Status::unauthenticated("expired")),
            CloseReason::PolicyViolation
        );
        assert_eq!(
            map_status_to_close(&tonic::Status::internal("oops")),
            CloseReason::InternalError
        );
    }

    #[test]
    fn outbound_frame_serializes_to_seq_no_envelope() {
        // Spec C₃ §6.2: server frames carry a top-level `seq_no` for
        // client-side resume tracking.
        let f = OutboundFrame::AgentEvent {
            seq_no: 42,
            record: serde_json::json!({"kind": "TOKEN"}),
            agent_kind: "TOKEN".to_string(),
        };
        let s = serde_json::to_string(&f).expect("serialize");
        assert!(s.contains("\"seq_no\":42"));
        assert!(s.contains("\"kind\":\"agent_event\""));
        assert!(s.contains("\"agent_kind\":\"TOKEN\""));
    }

    #[test]
    fn inbound_frame_decodes_send_message() {
        let raw = r#"{"kind":"send_message","content":"Hello"}"#;
        let m = Message::Text(raw.into());
        let parsed = decode_inbound(&m).expect("decode");
        match parsed {
            InboundFrame::SendMessage {
                content,
                attachment_blob_ref,
            } => {
                assert_eq!(content, "Hello");
                assert!(attachment_blob_ref.is_none());
            }
            other => panic!("expected SendMessage, got {other:?}"),
        }
    }

    #[test]
    fn inbound_frame_decodes_ping_close() {
        let p = decode_inbound(&Message::Text(r#"{"kind":"ping","seq_no":5}"#.into())).unwrap();
        assert!(matches!(p, InboundFrame::Ping { seq_no: 5 }));
        let c = decode_inbound(&Message::Text(r#"{"kind":"close"}"#.into())).unwrap();
        assert!(matches!(c, InboundFrame::Close { .. }));
    }

    #[test]
    fn inbound_frame_drops_unknown_kind() {
        // Unknown `kind` value must produce None — caller logs +
        // increments `frame_drop` metric.
        let m = Message::Text(r#"{"kind":"bogus","payload":1}"#.into());
        assert!(decode_inbound(&m).is_none());
    }

    #[test]
    fn inbound_frame_drops_non_text_message() {
        // Binary frames are explicitly dropped on the JSON
        // subprotocol per Spec C₃ §6.5 close code 1003 semantics.
        // Sub-phase C: drop silently (caller bumps metric); D adds
        // the explicit close.
        let m = Message::Binary(vec![1, 2, 3].into());
        assert!(decode_inbound(&m).is_none());
    }

    #[test]
    fn heartbeat_constants_match_spec() {
        // Sub-phase D (D5): heartbeat constants must match Spec C₃ §6.4.
        assert_eq!(HEARTBEAT_INTERVAL, Duration::from_secs(30));
        assert_eq!(PONG_DEADLINE, Duration::from_secs(60));
        assert_eq!(PONG_CHECK_INTERVAL, Duration::from_secs(1));
    }

    #[test]
    fn pong_deadline_is_two_heartbeat_intervals() {
        // Sub-phase D (D5) design rationale: PONG_DEADLINE = 2 ×
        // HEARTBEAT_INTERVAL. A single dropped ping is tolerated; two
        // in a row trip the close.
        assert_eq!(PONG_DEADLINE, HEARTBEAT_INTERVAL * 2);
    }
}
