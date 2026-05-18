//! Anthropic Messages `POST /v1/messages` SSE route — Spec J §J-Sub-B.
//!
//! Mounts the Anthropic-compatible Messages endpoint on lifegw so
//! Claude Code (and any compatible client speaking the Anthropic
//! Messages wire) can drive a Life agent session over the same
//! `(Tier-1 → Tier-2 → lifed.Agent.*)` flow the WebSocket route already
//! uses. The novel surface, vs `services/agent_http.rs`, is the
//! response body: instead of a unary JSON reply we stream
//! `text/event-stream` synthesized by [`lifegw_anthropic_codec`].
//!
//! # Flow
//!
//! ```text
//! client                                  lifegw                       lifed
//!   │                                       │                            │
//!   │  POST /v1/messages                    │                            │
//!   │  Authorization: Bearer <Tier-1 JWS>   │                            │
//!   │  anthropic-version: 2023-06-01        │                            │
//!   │  Content-Type: application/json       │                            │
//!   │  Body: AnthropicMessagesRequest       │                            │
//!   ├──────────────────────────────────────▶│                            │
//!   │                                       │ verify Tier-1              │
//!   │                                       │ mint Tier-2                │
//!   │                                       │ synthesize sid             │
//!   │                                       │  Agent.CreateSession       │
//!   │                                       ├───────────────────────────▶│
//!   │                                       │  Agent.SendMessage         │
//!   │                                       ├───────────────────────────▶│
//!   │                                       │  Agent.StreamSession       │
//!   │                                       ├───────────────────────────▶│
//!   │                                       │  AgentEvent stream         │
//!   │                                       │◀───────────────────────────┤
//!   │   text/event-stream                   │ encode via codec           │
//!   │◀──────────────────────────────────────┤                            │
//! ```
//!
//! # Locked-decision wiring (Spec J §[Locked Decisions])
//!
//! - **L10-D1**: lifegw uses `lifegw_anthropic_codec` (workspace-internal
//!   crate, no substrate deps). `arcan-proxy::AnthropicArcan` is the
//!   adapter on lifed's south side — not pulled by this route.
//! - **L10-D2**: sid synthesis goes through
//!   [`lifegw_anthropic_codec::synthesize_sid`]. The function takes the
//!   parsed request + the caller's anima DID and produces a deterministic
//!   `claude-code:<16-hex>` sid. The canonicalisation algorithm is
//!   defined in the codec; this route only invokes it.
//! - **L10-D5**: an unknown `anthropic-version` header returns HTTP 400
//!   with an Anthropic-shape error body. Body-level forward compatibility
//!   stays the codec's choice (see codec's `request.rs` strictness policy).
//! - **L10-D6**: model-name resolution (Anthropic vs life-routed) is
//!   J-Sub-F's surface. This route passes `model` through to lifed
//!   verbatim.
//! - **L10-D7**: no token counting. Token semantics are Vigil/Haima
//!   surfaces (J-Sub-F).
//!
//! # Response semantics
//!
//! - SSE keep-alive: a `ping` event is emitted every 15 s while the
//!   upstream is idle (Spec J §[Wire protocol mapping]).
//! - Hard timeout: 600 s wall-clock cap. On timeout the encoder emits
//!   `message_delta{stop_reason:"stop_sequence"}` + `message_stop` and
//!   closes the stream.
//! - Errors during the stream become in-band `event: error` frames
//!   (Anthropic stream semantics); errors *before* the stream opens
//!   are HTTP 4xx/5xx with an Anthropic-shape JSON error body.

// Several upstream helpers return `Result<(), Response>` so callers can
// short-circuit with a fully-built HTTP error response on failure. The
// `Err`-variant is large (axum `Response` is `~128 B`); boxing it would
// flatten every call site at zero correctness benefit. The lint is
// scoped to this module to keep the trade-off local.
#![allow(clippy::result_large_err)]

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use bytes::Bytes;
use futures::stream::{self, Stream, StreamExt};
use lifegw_anthropic_codec::{
    AnthropicError, AnthropicErrorKind, AnthropicMessagesRequest, AnthropicSseEvent,
    AnthropicVersion, CodecError, Encoder, synthesize_sid,
};
use tonic::transport::Channel;
use uuid::Uuid;

use life_runtime_proto::life::v1::{self as pb, agent_client::AgentClient};

use crate::auth::jwks::JwksCache;
use crate::auth::tier2::Tier2Minter;

/// Wall-clock cap on a single `/v1/messages` response stream.
///
/// Spec J §[Wire protocol mapping]: lifegw matches Anthropic's
/// published 10-minute hard cap. When the timer fires, the encoder
/// finalises with `stop_sequence` so Claude Code sees a clean stop
/// rather than a torn-down connection.
const HARD_STREAM_TIMEOUT: Duration = Duration::from_secs(600);

/// Cadence of the synthetic `event: ping` keep-alive emitted while the
/// upstream is silent. Spec J §[Wire protocol mapping]: matches
/// free-claude-code's 15 s value so existing L7 idle-cutters (Cloudflare
/// 100 s default, Vercel edge 30 s, etc.) don't drop the stream.
const PING_INTERVAL: Duration = Duration::from_secs(15);

/// Per-RPC deadline for each upstream unary call (CreateSession +
/// SendMessage). StreamSession is deliberately not deadlined here —
/// it's a long-lived response stream bounded by [`HARD_STREAM_TIMEOUT`].
const UPSTREAM_RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Default `project_id` synthesised for Claude-Code sessions when no
/// override is supplied. Spec J §[Anima binding]: free-claude-code has
/// no concept of "projects", so we land all conversations under one
/// well-known id.
const CLAUDE_CODE_PROJECT_ID: &str = "claude-code-default";

/// Maximum length of the canonical-form `anthropic-version` header.
const MAX_VERSION_HEADER_LEN: usize = 64;

/// Maximum upstream request body size, in bytes. 8 MiB is well above
/// Claude Code's largest observed prompts and well below axum's
/// extractor defaults — we surface oversize bodies as a 413 with an
/// Anthropic-shape error rather than letting axum reject them with the
/// default text body.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

// ─── Router state ───────────────────────────────────────────────────────

/// Shared state threaded through the axum router.
#[derive(Clone)]
pub struct AnthropicMessagesState {
    /// Tier-1 verifier — the same handle `AuthLayer` uses.
    pub jwks: Arc<JwksCache>,
    /// Tier-2 minter — the same handle `AuthLayer` uses for tonic upstream.
    pub minter: Arc<Tier2Minter>,
    /// Pre-dialed lifed UDS channel. Cheap to clone (internally `Arc`'d).
    pub upstream: Channel,
}

/// Mount the route.
///
/// Per Spec J §J-Sub-B, the route lives at `/v1/messages`. We use
/// exact-route matching (not nesting) so the rest of the `/v1/*` space
/// stays free for the agent / events surfaces that the tonic stack
/// continues to serve.
pub fn router(state: AnthropicMessagesState) -> Router {
    Router::new()
        .route(
            "/v1/messages",
            post(messages_handler).options(probe).head(probe),
        )
        .with_state(state)
}

// ─── Probe (OPTIONS / HEAD) ─────────────────────────────────────────────

/// Probe response for `OPTIONS` + `HEAD`. Some Anthropic-shaped clients
/// pre-flight the route; we reply 204 with an `Allow` header so they
/// don't bounce off a 405.
async fn probe() -> Response {
    let mut resp = Response::new(Body::empty());
    *resp.status_mut() = StatusCode::NO_CONTENT;
    resp.headers_mut().insert(
        header::ALLOW,
        HeaderValue::from_static("POST, HEAD, OPTIONS"),
    );
    resp
}

// ─── POST handler ───────────────────────────────────────────────────────

async fn messages_handler(
    State(state): State<AnthropicMessagesState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 1. Validate `anthropic-version` header *before* doing any other
    //    work. Per Spec J L10-D5 unknown values are loud HTTP 400s.
    let version_raw = match headers
        .get("anthropic-version")
        .and_then(|v| v.to_str().ok())
    {
        Some(raw) if raw.len() <= MAX_VERSION_HEADER_LEN => raw,
        Some(_) => {
            return anthropic_http_error(
                StatusCode::BAD_REQUEST,
                AnthropicErrorKind::InvalidRequestError,
                "anthropic-version header exceeds length limit",
            );
        }
        None => {
            return anthropic_http_error(
                StatusCode::BAD_REQUEST,
                AnthropicErrorKind::InvalidRequestError,
                "missing anthropic-version header",
            );
        }
    };
    if let Err(err) = AnthropicVersion::parse(version_raw) {
        return anthropic_http_error(
            StatusCode::BAD_REQUEST,
            AnthropicErrorKind::InvalidRequestError,
            err.to_string(),
        );
    }

    // 2. Verify the Tier-1 bearer (same flow as `agent_http.rs`).
    let bearer = match headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    {
        Some(b) if !b.is_empty() => b,
        _ => {
            return anthropic_http_error(
                StatusCode::UNAUTHORIZED,
                AnthropicErrorKind::AuthenticationError,
                "missing Tier-1 bearer",
            );
        }
    };
    let tier1 = match state.jwks.verify(bearer) {
        Ok(c) => c,
        Err(e) => {
            return anthropic_http_error(
                StatusCode::UNAUTHORIZED,
                AnthropicErrorKind::AuthenticationError,
                format!("invalid Tier-1: {e}"),
            );
        }
    };

    // 3. Enforce body size.
    if body.len() > MAX_BODY_BYTES {
        return anthropic_http_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            AnthropicErrorKind::InvalidRequestError,
            format!("request body exceeds {MAX_BODY_BYTES} bytes"),
        );
    }

    // 4. Parse the request body via the codec (forward-compatible — see
    //    codec's `request.rs` strictness policy).
    let req: AnthropicMessagesRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => {
            return anthropic_http_error(
                StatusCode::BAD_REQUEST,
                AnthropicErrorKind::InvalidRequestError,
                format!("invalid JSON body: {e}"),
            );
        }
    };
    if let Err(e) = req.validate() {
        let status = match &e {
            CodecError::NoUserMessage | CodecError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::BAD_REQUEST,
        };
        return anthropic_http_error(
            status,
            AnthropicErrorKind::InvalidRequestError,
            e.to_string(),
        );
    }

    // 5. Synthesize the deterministic Life sid from the Tier-1 caller's
    //    anima DID + the canonical first user message (Spec J L10-D2).
    //    Today the Tier-1 claim set carries `user_id`; the DID is
    //    "did:life:<user_id>" until Spec D's full DID claim threads
    //    through the gateway. The wire algorithm is stable either way.
    let anima_did = format!("did:life:{}", tier1.user_id);
    let sid = match synthesize_sid(&req, &anima_did) {
        Ok(s) => s,
        Err(e) => {
            return anthropic_http_error(
                StatusCode::BAD_REQUEST,
                AnthropicErrorKind::InvalidRequestError,
                e.to_string(),
            );
        }
    };

    // 6. Mint a Tier-2 cap. Same posture as `agent_http.rs`.
    let tier2 = match state.minter.mint(&tier1) {
        Ok(t) => t,
        Err(e) => {
            return anthropic_http_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                AnthropicErrorKind::ApiError,
                format!("mint tier-2: {e}"),
            );
        }
    };

    // 7. Drive the upstream saga: CreateSession (resume_sid=Some(sid))
    //    → SendMessage(last user content) → StreamSession.
    let mut agent_client = AgentClient::new(state.upstream.clone());

    if let Err(err) = create_session(&mut agent_client, &tier2, &tier1.user_id, &sid).await {
        return err;
    }

    let last_user_content = extract_last_user_content(&req);
    if let Err(err) = send_message(&mut agent_client, &tier2, &sid, &last_user_content).await {
        return err;
    }

    let event_stream = match open_stream(&mut agent_client, &tier2, &sid).await {
        Ok(s) => s,
        Err(err) => return err,
    };

    // 8. Build the SSE response. The encoder is fresh; `message_id`
    //    is freshly synthesised in the Anthropic `msg_<hex>` shape so
    //    clients log a recognisable id even though Life sessions don't
    //    have a native equivalent.
    let message_id = format!("msg_{}", Uuid::new_v4().simple());
    let encoder = Encoder::new(message_id, req.model.clone());

    let sse_body = build_sse_body(encoder, event_stream);

    // 9. Compose response. Anthropic clients require a content-type of
    //    `text/event-stream`; `X-Accel-Buffering: no` neutralises nginx
    //    / Cloudflare proxy buffering so deltas surface immediately.
    let mut resp = Response::new(Body::from_stream(sse_body));
    *resp.status_mut() = StatusCode::OK;
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, no-transform"),
    );
    h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    h.insert(
        header::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    resp
}

// ─── Upstream helpers ───────────────────────────────────────────────────

/// Run `lifed.Agent.CreateSession` with `resume_sid = Some(sid)`. lifed
/// returns the existing session on resume, or runs the create-session
/// saga on a cold sid. Either way the routing-cache entry is warm by
/// the time we open the stream.
async fn create_session(
    client: &mut AgentClient<Channel>,
    tier2: &str,
    user_id: &str,
    sid: &str,
) -> Result<(), Response> {
    let label = format!("cc:{}", short_sid_suffix(sid));
    let mut req = tonic::Request::new(pb::CreateSessionReq {
        user_id: user_id.to_string(),
        project_id: CLAUDE_CODE_PROJECT_ID.to_string(),
        label,
        resume_sid: Some(aios_proto::aios::v1::SessionId {
            value: sid.to_string(),
        }),
        inherit_policy: None,
    });
    attach_tier2(&mut req, tier2)?;

    match tokio::time::timeout(UPSTREAM_RPC_TIMEOUT, client.create_session(req)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(status)) => Err(anthropic_http_error(
            map_tonic_to_http(status.code()),
            anthropic_kind_for(status.code()),
            sanitize_upstream(status.message(), "lifed.Agent.CreateSession"),
        )),
        Err(_) => Err(anthropic_http_error(
            StatusCode::GATEWAY_TIMEOUT,
            AnthropicErrorKind::ApiError,
            format!(
                "lifed.Agent.CreateSession exceeded {}s deadline",
                UPSTREAM_RPC_TIMEOUT.as_secs()
            ),
        )),
    }
}

/// Send the last user message in the Anthropic request as a single
/// `lifed.Agent.SendMessage` call. lifed broadcasts the resulting
/// events through its fanout registry; we read them back via
/// `StreamSession`.
async fn send_message(
    client: &mut AgentClient<Channel>,
    tier2: &str,
    sid: &str,
    content: &str,
) -> Result<(), Response> {
    let mut req = tonic::Request::new(pb::SendMessageReq {
        sid: Some(aios_proto::aios::v1::SessionId {
            value: sid.to_string(),
        }),
        content: content.to_string(),
        attachment_blob_ref: Vec::new(),
    });
    attach_tier2(&mut req, tier2)?;

    let resp = match tokio::time::timeout(UPSTREAM_RPC_TIMEOUT, client.send_message(req)).await {
        Ok(Ok(r)) => r,
        Ok(Err(status)) => {
            return Err(anthropic_http_error(
                map_tonic_to_http(status.code()),
                anthropic_kind_for(status.code()),
                sanitize_upstream(status.message(), "lifed.Agent.SendMessage"),
            ));
        }
        Err(_) => {
            return Err(anthropic_http_error(
                StatusCode::GATEWAY_TIMEOUT,
                AnthropicErrorKind::ApiError,
                format!(
                    "lifed.Agent.SendMessage exceeded {}s deadline",
                    UPSTREAM_RPC_TIMEOUT.as_secs()
                ),
            ));
        }
    };

    // Drop the SendMessage event stream — like the WS dispatcher, we
    // rely on `Agent.StreamSession` as the canonical event source so
    // we don't double-emit on lifed's fanout registry. Spawning a
    // background drain keeps the upstream from blocking on us if it
    // expects to push the full reply through this RPC; we cap it at
    // the hard timeout to avoid lingering tasks on a misbehaving
    // upstream.
    tokio::spawn(async move {
        let mut s = resp.into_inner();
        let _ = tokio::time::timeout(HARD_STREAM_TIMEOUT, async {
            while s.next().await.is_some() {
                // Intentionally drop. StreamSession is canonical.
            }
        })
        .await;
    });

    Ok(())
}

/// Open `lifed.Agent.StreamSession{sid}`. Returns the event stream.
async fn open_stream(
    client: &mut AgentClient<Channel>,
    tier2: &str,
    sid: &str,
) -> Result<tonic::Streaming<pb::AgentEvent>, Response> {
    let mut req = tonic::Request::new(pb::SessionRef {
        sid: Some(aios_proto::aios::v1::SessionId {
            value: sid.to_string(),
        }),
        from_sequence: None,
    });
    attach_tier2(&mut req, tier2)?;

    match client.stream_session(req).await {
        Ok(r) => Ok(r.into_inner()),
        Err(status) => Err(anthropic_http_error(
            map_tonic_to_http(status.code()),
            anthropic_kind_for(status.code()),
            sanitize_upstream(status.message(), "lifed.Agent.StreamSession"),
        )),
    }
}

/// Attach the Tier-2 cap to the outgoing tonic metadata. Mirrors the
/// pattern from `agent_http.rs`.
fn attach_tier2<T>(req: &mut tonic::Request<T>, tier2: &str) -> Result<(), Response> {
    let value: tonic::metadata::MetadataValue<_> = match format!("Bearer {tier2}").parse() {
        Ok(v) => v,
        Err(e) => {
            return Err(anthropic_http_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                AnthropicErrorKind::ApiError,
                format!("encode tier-2 metadata: {e}"),
            ));
        }
    };
    req.metadata_mut().insert("authorization", value);
    Ok(())
}

// ─── SSE body assembly ──────────────────────────────────────────────────

/// Build the streaming SSE body: a `Stream<Item = Result<Bytes,
/// Infallible>>` that pipes pb::AgentEvents through the codec's
/// `Encoder`, multiplexes 15 s keep-alive pings, and enforces the
/// 600 s hard timeout.
///
/// `Infallible` as the stream's error type matches axum's
/// `Body::from_stream` requirement and pairs with the codec's discipline
/// of converting mid-stream faults into in-band `event: error` frames.
fn build_sse_body(
    encoder: Encoder,
    upstream: tonic::Streaming<pb::AgentEvent>,
) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    // Box the upstream so the stream::unfold state type stays sized.
    let upstream: std::pin::Pin<
        Box<dyn Stream<Item = Result<pb::AgentEvent, tonic::Status>> + Send>,
    > = Box::pin(upstream);

    // We model the stream as a small async state machine:
    //
    //   loop:
    //     select! {
    //         hard_timeout => emit force_finalize + END
    //         ping_tick    => emit Ping frame
    //         event = upstream.next() =>
    //             Some(Ok(evt))  -> codec.encode(evt) -> emit frames
    //             Some(Err(s))   -> emit error frame + force_finalize + END
    //             None           -> emit force_finalize + END
    //     }
    //
    // `stream::unfold` keeps the state owned inline.

    let start = tokio::time::Instant::now();
    let deadline = start + HARD_STREAM_TIMEOUT;

    struct StreamState {
        encoder: Encoder,
        upstream:
            std::pin::Pin<Box<dyn Stream<Item = Result<pb::AgentEvent, tonic::Status>> + Send>>,
        deadline: tokio::time::Instant,
        ping_interval: tokio::time::Interval,
        /// Queued frames waiting to flush (a single upstream event can
        /// produce multiple SSE frames).
        queued: std::collections::VecDeque<AnthropicSseEvent>,
        /// Terminal flag — once true the stream returns None on next call.
        done: bool,
    }

    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    // First tick fires immediately; consume it so the first real ping
    // is at +PING_INTERVAL, not at t=0 alongside `message_start`.
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let state = StreamState {
        encoder,
        upstream,
        deadline,
        ping_interval,
        queued: std::collections::VecDeque::new(),
        done: false,
    };

    stream::unfold(state, |mut s| async move {
        if s.done {
            return None;
        }
        // If there are queued frames from a prior upstream event, emit
        // them one by one before polling the upstream again.
        if let Some(evt) = s.queued.pop_front() {
            let bytes = Bytes::from(evt.to_sse_frame());
            return Some((Ok::<Bytes, Infallible>(bytes), s));
        }

        loop {
            let next_event = s.upstream.next();
            let ping = s.ping_interval.tick();
            let sleep_to_deadline = tokio::time::sleep_until(s.deadline);

            tokio::select! {
                biased;
                _ = sleep_to_deadline => {
                    // Hard timeout — finalise with `stop_sequence` so
                    // Claude Code interprets as a clean stop.
                    let final_frames = s.encoder.force_finalize();
                    for f in final_frames {
                        s.queued.push_back(f);
                    }
                    s.done = true;
                    if let Some(first) = s.queued.pop_front() {
                        let bytes = Bytes::from(first.to_sse_frame());
                        return Some((Ok(bytes), s));
                    }
                    return None;
                }
                evt = next_event => {
                    match evt {
                        Some(Ok(event)) => {
                            match s.encoder.encode(&event) {
                                Ok(frames) => {
                                    for f in frames {
                                        s.queued.push_back(f);
                                    }
                                }
                                Err(e) => {
                                    // Codec rejected the upstream event
                                    // (structural mismatch). Emit an
                                    // in-band error + finalise.
                                    let err = Encoder::emit_top_level_error(
                                        AnthropicErrorKind::ApiError,
                                        e.to_string(),
                                    );
                                    s.queued.push_back(err);
                                    for f in s.encoder.force_finalize() {
                                        s.queued.push_back(f);
                                    }
                                    s.done = true;
                                }
                            }
                            if let Some(first) = s.queued.pop_front() {
                                let bytes = Bytes::from(first.to_sse_frame());
                                return Some((Ok(bytes), s));
                            }
                            // Empty translation — keep polling.
                            continue;
                        }
                        Some(Err(status)) => {
                            // Upstream tonic error mid-stream. Convert
                            // to Anthropic-shape error then finalise.
                            let err = Encoder::emit_top_level_error(
                                anthropic_kind_for(status.code()),
                                sanitize_upstream(
                                    status.message(),
                                    "lifed.Agent.StreamSession",
                                ),
                            );
                            s.queued.push_back(err);
                            for f in s.encoder.force_finalize() {
                                s.queued.push_back(f);
                            }
                            s.done = true;
                            if let Some(first) = s.queued.pop_front() {
                                let bytes = Bytes::from(first.to_sse_frame());
                                return Some((Ok(bytes), s));
                            }
                            return None;
                        }
                        None => {
                            // Upstream EOF without explicit Finish —
                            // force_finalize so the protocol stays valid.
                            for f in s.encoder.force_finalize() {
                                s.queued.push_back(f);
                            }
                            s.done = true;
                            if let Some(first) = s.queued.pop_front() {
                                let bytes = Bytes::from(first.to_sse_frame());
                                return Some((Ok(bytes), s));
                            }
                            return None;
                        }
                    }
                }
                _ = ping => {
                    let bytes = Bytes::from(Encoder::ping().to_sse_frame());
                    return Some((Ok(bytes), s));
                }
            }
        }
    })
}

// ─── Helpers ────────────────────────────────────────────────────────────

/// Pull a short, sid-derived suffix for `label`. We slice the hex part
/// of the sid (everything after the `"claude-code:"` prefix) and keep
/// the first 8 hex chars — operators recognise it without leaking the
/// full sid into log labels.
fn short_sid_suffix(sid: &str) -> &str {
    let hex = sid
        .strip_prefix(lifegw_anthropic_codec::SID_PREFIX)
        .unwrap_or(sid);
    let take = hex.len().min(8);
    &hex[..take]
}

/// Extract the last user-role message text. Phase 1 sends one
/// SendMessage per HTTP turn (the conversation history is already on
/// the lifed side via session resume).
fn extract_last_user_content(req: &AnthropicMessagesRequest) -> String {
    use lifegw_anthropic_codec::Role;
    req.messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.content.plain_text())
        .unwrap_or_default()
}

/// Build a JSON Anthropic-shape error response with the given HTTP
/// status. Used for failures that happen *before* the SSE body starts.
fn anthropic_http_error(
    status: StatusCode,
    kind: AnthropicErrorKind,
    message: impl Into<String>,
) -> Response {
    let err = AnthropicError::new(kind, message);
    let body = err.to_sse_data();
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    resp
}

/// Map an upstream `tonic::Code` to the closest HTTP status. Aligned
/// with `agent_http.rs::map_tonic_to_http`.
fn map_tonic_to_http(code: tonic::Code) -> StatusCode {
    use tonic::Code;
    match code {
        Code::Unauthenticated | Code::PermissionDenied => StatusCode::UNAUTHORIZED,
        Code::InvalidArgument | Code::FailedPrecondition => StatusCode::BAD_REQUEST,
        Code::NotFound => StatusCode::NOT_FOUND,
        Code::AlreadyExists => StatusCode::CONFLICT,
        Code::ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
        Code::Unavailable => StatusCode::BAD_GATEWAY,
        Code::DeadlineExceeded => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Map an upstream tonic code to the closest Anthropic `error.type`.
fn anthropic_kind_for(code: tonic::Code) -> AnthropicErrorKind {
    use tonic::Code;
    match code {
        Code::Unauthenticated => AnthropicErrorKind::AuthenticationError,
        Code::PermissionDenied => AnthropicErrorKind::PermissionError,
        Code::InvalidArgument | Code::FailedPrecondition => AnthropicErrorKind::InvalidRequestError,
        Code::NotFound => AnthropicErrorKind::NotFoundError,
        Code::ResourceExhausted => AnthropicErrorKind::RateLimitError,
        Code::Unavailable => AnthropicErrorKind::OverloadedError,
        _ => AnthropicErrorKind::ApiError,
    }
}

/// Sanitise upstream error messages so we don't leak internal-only
/// details (UDS paths, sql fragments, etc.) into the public response.
/// Mirrors `agent_http.rs::sanitize_upstream` (same posture).
fn sanitize_upstream(msg: &str, fallback_context: &str) -> String {
    if msg.is_empty() {
        return format!("{fallback_context} failed");
    }
    let cleaned: String = msg
        .split_whitespace()
        .map(|tok| {
            if tok.starts_with('/') {
                "<path-redacted>"
            } else {
                tok
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.len() > 256 {
        format!("{}…", &cleaned[..256])
    } else {
        cleaned
    }
}

// Compile-time guard: IntoResponse on the handler's actual return type.
#[doc(hidden)]
fn _impl_response_compile_check() {
    fn _check<T: IntoResponse>() {}
    _check::<Response>();
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_tonic_codes_to_http_status() {
        assert_eq!(
            map_tonic_to_http(tonic::Code::Unauthenticated),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            map_tonic_to_http(tonic::Code::ResourceExhausted),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            map_tonic_to_http(tonic::Code::Unavailable),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn maps_tonic_codes_to_anthropic_kind() {
        assert!(matches!(
            anthropic_kind_for(tonic::Code::Unauthenticated),
            AnthropicErrorKind::AuthenticationError
        ));
        assert!(matches!(
            anthropic_kind_for(tonic::Code::ResourceExhausted),
            AnthropicErrorKind::RateLimitError
        ));
        assert!(matches!(
            anthropic_kind_for(tonic::Code::Unavailable),
            AnthropicErrorKind::OverloadedError
        ));
        // Unknown defaults to api_error.
        assert!(matches!(
            anthropic_kind_for(tonic::Code::Internal),
            AnthropicErrorKind::ApiError
        ));
    }

    #[test]
    fn sanitize_redacts_paths_and_bounds_length() {
        let s = sanitize_upstream(
            "connect failed at /run/life/life.sock with EADDRNOTAVAIL",
            "lifed.Agent.CreateSession",
        );
        assert!(!s.contains("/run/life"));
        assert!(s.contains("EADDRNOTAVAIL"));

        let long = "x".repeat(1000);
        let trimmed = sanitize_upstream(&long, "lifed.Agent.CreateSession");
        // 256 ASCII + 3 bytes for `…` (U+2026 = e2 80 a6) = 259 bytes.
        assert!(trimmed.len() <= 259);
        assert!(trimmed.ends_with('…'));
    }

    #[test]
    fn anthropic_http_error_carries_anthropic_shape_body() {
        let resp = anthropic_http_error(
            StatusCode::TOO_MANY_REQUESTS,
            AnthropicErrorKind::RateLimitError,
            "burst exceeded",
        );
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.to_str().unwrap_or("").to_string()),
            Some("application/json".to_string())
        );
    }

    #[test]
    fn short_sid_suffix_extracts_hex_prefix() {
        let sid = format!("{}deadbeefcafef00d", lifegw_anthropic_codec::SID_PREFIX);
        assert_eq!(short_sid_suffix(&sid), "deadbeef");
    }

    #[test]
    fn short_sid_suffix_handles_no_prefix() {
        assert_eq!(short_sid_suffix("abc"), "abc");
    }

    #[test]
    fn extract_last_user_content_skips_assistant_turns() {
        use lifegw_anthropic_codec::request::MessageContent;
        use lifegw_anthropic_codec::{Message, Role};
        let req = AnthropicMessagesRequest {
            model: "m".into(),
            messages: vec![
                Message {
                    role: Role::User,
                    content: MessageContent::Text("first".into()),
                },
                Message {
                    role: Role::Assistant,
                    content: MessageContent::Text("between".into()),
                },
                Message {
                    role: Role::User,
                    content: MessageContent::Text("latest".into()),
                },
            ],
            system: None,
            max_tokens: 1,
            stop_sequences: vec![],
            stream: true,
            temperature: None,
            top_p: None,
            top_k: None,
            metadata: None,
            tools: vec![],
            tool_choice: None,
            thinking: None,
        };
        assert_eq!(extract_last_user_content(&req), "latest");
    }
}
