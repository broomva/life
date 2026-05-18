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
//!   J-Sub-F's surface. The `model` field on the inbound Anthropic body
//!   is **NOT** forwarded to lifed — `CreateSessionReq` and
//!   `SendMessageReq` have no `model` field. The route captures
//!   `req.model` only for the codec encoder, where it becomes
//!   `gen_ai.response.model` plus the `model` field on the `message_start`
//!   envelope. Backend selection (Anthropic upstream vs life-routed
//!   substrate) is derived from agent identity inside lifed; the wire
//!   model name only roundtrips through SSE framing today. J-Sub-F is
//!   where actual model-based routing lands; until then the comment in
//!   the PR body table that said "passes through to lifed" was wrong
//!   and is corrected here.
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bytes::Bytes;
use futures::stream::{self, Stream, StreamExt};
use lifegw_anthropic_codec::{
    AnthropicError, AnthropicErrorKind, AnthropicMessagesRequest, AnthropicSseEvent,
    AnthropicVersion, CodecError, Encoder, synthesize_sid,
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;
use tracing::Instrument;
use uuid::Uuid;

use life_runtime_proto::life::v1::{self as pb, agent_client::AgentClient};

use crate::auth::jwks::JwksCache;
use crate::auth::tier2::Tier2Minter;
use crate::services::rate_limit::TokenBucketLimiter;

// ─── Spec J §[Vigil span emission] — semconv constants ──────────────────

/// Child span: haima credit gate (pre-stream).
///
/// Note: other span names (root `life.anthropic.messages`, sid synthesis,
/// auth verify) are emitted directly as string literals inside `info_span!`
/// because the macro's first argument requires a literal, not a const ref.
/// This single const survives because it's passed into a helper that
/// accepts `&'static str`.
const SPAN_HAIMA_CHECK: &str = "life.anthropic.haima_check";
/// Child span: codec encoder construction.
const SPAN_CODEC_ENCODE: &str = "life.anthropic.codec_encode";

// Life-namespace attribute keys (Spec J §[Vigil span emission]).
// `_ATTR_LIFE_*` constants document the wire shape — the
// `tracing::info_span!` macro requires string literals for field names
// rather than const references, so the constants are reference-only
// (read by tests, asserted-on integration, and provide a single
// rename-friendly source of truth).
#[allow(dead_code)]
pub(crate) const ATTR_LIFE_SESSION_ID: &str = "life.session.id";
#[allow(dead_code)]
pub(crate) const ATTR_LIFE_ANIMA_DID: &str = "life.anima.did";
pub(crate) const ATTR_LIFE_HAIMA_COST_MICROS: &str = "life.haima.cost_usd_micros";
#[allow(dead_code)]
pub(crate) const ATTR_LIFE_BACKEND_ID: &str = "life.backend.id";
#[allow(dead_code)]
pub(crate) const ATTR_LIFE_BACKEND_KIND: &str = "life.backend.kind";

/// Phase 1 default backend kind. Spec E backends override this when
/// they ship (post-J-Sub-E).
const BACKEND_KIND_DEFAULT: &str = "anthropic-arcan";
/// Default backend id when the upstream substrate doesn't identify
/// itself in the request path.
const BACKEND_ID_DEFAULT: &str = "lifed-anthropic";

// ─── Spec J §[Cost gate] — x402 challenge ───────────────────────────────

/// x402 facilitator URL surfaced in the `X-Payment` challenge header
/// when `haima_check` rejects a request for insufficient credits.
const X402_FACILITATOR_DEFAULT: &str = "https://haima.broomva.dev/x402";

/// x402 challenge token (USDC on Base mainnet — Spec J §[Cost gate]).
const X402_CHAIN: &str = "base";
const X402_TOKEN: &str = "USDC";

/// Default x402 challenge amount (USD, decimal string). The amount is
/// the *minimum* that satisfies the spend gate — clients can top up
/// more. 0.10 USD matches Spec J's reference table.
const X402_AMOUNT_USD_DEFAULT: &str = "0.10";

// ─── Spec J §[Cost gate] — haima wire ───────────────────────────────────

/// Tiny abstraction over haima's per-call billing surface.
///
/// Spec J §J-Sub-E ships the *handler shape* and the Vigil span tree
/// regardless of whether haima exposes a live API yet. When the haima
/// daemon doesn't have a check/settle RPC (today), production runs the
/// [`StubHaimaClient`] which returns `Ok(_)` for every call. The trait
/// gives us:
///
/// 1. A single seam to swap a real `haimad` gRPC client in when the
///    wire shape lands (planned Phase F4 of haima's roadmap).
/// 2. A test seam — `TestHaimaClient` records calls and lets us
///    pre-arm a `HaimaCheckError::InsufficientCredits` rejection
///    without standing up a separate process.
///
/// `Send + Sync + 'static` so the trait is dyn-callable from inside
/// the per-request handler.
#[async_trait::async_trait]
pub trait HaimaClient: Send + Sync + 'static {
    /// Pre-call gate. `estimated_cost_micros` is the worst-case spend
    /// for the upcoming request derived from the model's pricing-table
    /// rate × `max_tokens`. Returns `Ok(())` when the DID has at least
    /// that much credit; `Err(InsufficientCredits)` triggers a 402 +
    /// x402 challenge body. Other errors map to 500 — they represent a
    /// haima-daemon outage and surface as `api_error`.
    async fn check(&self, did: &str, estimated_cost_micros: u64) -> Result<(), HaimaCheckError>;

    /// Post-call settlement. Called once the upstream stream has
    /// drained successfully. The token counts are an approximation
    /// from the encoder (no upstream-provided usage today) and the
    /// `actual_cost_micros` is the worst-case computed via the same
    /// pricing-table rate.
    ///
    /// Spec J §[Cost gate]: settlement records a `haima.charged` lago
    /// event. Stub implementations log only.
    async fn settle(
        &self,
        did: &str,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        actual_cost_micros: u64,
    );
}

/// Failure modes for [`HaimaClient::check`].
#[derive(Clone, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HaimaCheckError {
    /// DID does not have enough micro-USDC credit to cover the
    /// estimated worst-case spend. Maps to HTTP 402 + x402 challenge.
    #[error("insufficient credits")]
    InsufficientCredits {
        /// Amount the gate required (micro-USDC).
        required_micros: u64,
        /// Amount available (micro-USDC), if the haima daemon
        /// returned it.
        available_micros: Option<u64>,
    },
    /// Generic haima-daemon outage — UDS unreachable, gRPC error, etc.
    /// Maps to HTTP 500.
    #[error("haima daemon unavailable: {0}")]
    Unavailable(String),
}

/// Default haima client used when the daemon isn't wired up yet.
///
/// Spec J J-Sub-E stop-condition: "If haima doesn't expose a
/// check/settle API yet, document the gap and ship a feature-flag-
/// gated stub that always returns Ok. The handler shape + vigil spans
/// still ship." This is that stub.
#[derive(Clone, Debug, Default)]
pub struct StubHaimaClient;

#[async_trait::async_trait]
impl HaimaClient for StubHaimaClient {
    async fn check(&self, _did: &str, _estimated_cost_micros: u64) -> Result<(), HaimaCheckError> {
        Ok(())
    }

    async fn settle(
        &self,
        did: &str,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        actual_cost_micros: u64,
    ) {
        tracing::debug!(
            target: "lifegw::anthropic_messages::haima",
            did = %did,
            model = %model,
            input_tokens,
            output_tokens,
            actual_cost_micros,
            "StubHaimaClient::settle (logged only — no wire)"
        );
    }
}

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
///
/// Fix-round 1 — I-4: this constant is enforced at the *router* level
/// via [`DefaultBodyLimit::max`] so axum rejects oversize bodies during
/// the body read (streaming-time) rather than after the full payload
/// has been buffered into a `Bytes` extractor. The post-extract size
/// check has been removed as redundant.
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
    /// Fix-round 1 — C-1: per-user + per-IP token-bucket limiter
    /// shared with `AuthLayer`. The handler consults the same limiter
    /// the tonic stack uses post–Tier-1-verify and pre–Tier-2-mint, so
    /// `/v1/messages` traffic is gated by the same budget. Until the
    /// limiter exists every Anthropic-shaped POST holds a streaming
    /// slot for up to `HARD_STREAM_TIMEOUT` (600 s) without back-pressure
    /// — strictly worse than the `agent_http` precedent (10 s unary).
    /// Tests opt out by leaving this `None`.
    pub rate_limiter: Option<TokenBucketLimiter>,
    /// Spec J J-Sub-E: haima per-call billing client. Production
    /// wires a real haimad gRPC handle; tests + the
    /// `cfg.billing.enforce = false` path wire [`StubHaimaClient`].
    /// `dyn` so the same field carries both the production client and
    /// a test fake without monomorphisation.
    pub haima: Arc<dyn HaimaClient>,
    /// Spec J J-Sub-E: when `false`, the haima check + settle calls
    /// are skipped (the vigil span still records usage). Mirrors
    /// `cfg.billing.enforce`.
    pub billing_enforce: bool,
}

/// Mount the route.
///
/// Per Spec J §J-Sub-B, the streaming route lives at `/v1/messages`. We
/// use exact-route matching (not nesting) so the rest of the `/v1/*`
/// space stays free for the agent / events surfaces that the tonic
/// stack continues to serve.
///
/// Spec J §J-Sub-F adds two siblings:
///
/// - `GET /v1/models` — Anthropic-compat model picker. Phase 1 returns
///   a static Anthropic-pinned list; Phase 2 will fan out to Spec E
///   backend discovery (`life/<backend>/<model>` ids + `-no-thinking`
///   variants). Unauthenticated probe — the picker is consulted by
///   clients before they have established a Tier-1 bearer in some
///   bootstrap flows (matching Anthropic's posture).
/// - `POST /v1/messages/count_tokens` — token-count probe. Tier-1
///   bearer required (mirrors `/v1/messages`); reuses Vigil's edge
///   estimator (Spec J L10-D7 — no new tokenizer crate).
pub fn router(state: AnthropicMessagesState) -> Router {
    Router::new()
        .route(
            "/v1/messages",
            post(messages_handler).options(probe).head(probe),
        )
        .route(
            "/v1/models",
            get(models_handler).options(models_probe).head(models_probe),
        )
        .route(
            "/v1/messages/count_tokens",
            post(count_tokens_handler)
                .options(count_tokens_probe)
                .head(count_tokens_probe),
        )
        // I-4 (fix-round 1): cap body size at the router boundary so
        // axum rejects oversized payloads during the read rather than
        // after the entire body has been buffered into `Bytes`. axum's
        // default ceiling is 2 MiB; we override to MAX_BODY_BYTES.
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

// ─── Probe (OPTIONS / HEAD) ─────────────────────────────────────────────

/// Probe response for `OPTIONS` + `HEAD` on `/v1/messages`. Some
/// Anthropic-shaped clients pre-flight the route; we reply 204 with an
/// `Allow` header so they don't bounce off a 405.
async fn probe() -> Response {
    probe_with_allow("POST, HEAD, OPTIONS")
}

/// Probe response for `/v1/models` — only `GET` is supported as the
/// real verb.
async fn models_probe() -> Response {
    probe_with_allow("GET, HEAD, OPTIONS")
}

/// Probe response for `/v1/messages/count_tokens` — `POST` only.
async fn count_tokens_probe() -> Response {
    probe_with_allow("POST, HEAD, OPTIONS")
}

fn probe_with_allow(allow: &'static str) -> Response {
    let mut resp = Response::new(Body::empty());
    *resp.status_mut() = StatusCode::NO_CONTENT;
    resp.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static(allow));
    resp
}

// ─── POST handler ───────────────────────────────────────────────────────

async fn messages_handler(
    State(state): State<AnthropicMessagesState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Spec J §[Vigil span emission] (J-Sub-E): open the root
    // `life.anthropic.messages` span at request entry. Required attrs
    // per the spec table are populated as soon as the underlying value
    // is known — `gen_ai.request.*` after body parse, `life.session.id`
    // after sid synthesis, `life.haima.cost_usd_micros` +
    // `gen_ai.usage.*` at settlement. Fields not yet known declare
    // `tracing::field::Empty` so `record()` can stamp them later.
    let request_id = Uuid::new_v4().simple().to_string();
    let root_span = tracing::info_span!(
        "life.anthropic.messages",
        "gen_ai.system" = "life",
        "gen_ai.operation.name" = "chat",
        "gen_ai.request.model" = tracing::field::Empty,
        "gen_ai.request.max_tokens" = tracing::field::Empty,
        "gen_ai.request.temperature" = tracing::field::Empty,
        "gen_ai.usage.input_tokens" = tracing::field::Empty,
        "gen_ai.usage.output_tokens" = tracing::field::Empty,
        "life.session.id" = tracing::field::Empty,
        "life.anima.did" = tracing::field::Empty,
        "life.haima.cost_usd_micros" = tracing::field::Empty,
        "life.backend.id" = BACKEND_ID_DEFAULT,
        "life.backend.kind" = BACKEND_KIND_DEFAULT,
        "vigil.llm.request_id" = %request_id,
    );
    messages_handler_inner(state, headers, body, root_span.clone())
        .instrument(root_span)
        .await
}

async fn messages_handler_inner(
    state: AnthropicMessagesState,
    headers: HeaderMap,
    body: Bytes,
    root_span: tracing::Span,
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

    // 2. Verify the Tier-1 bearer (same flow as `agent_http.rs`). Run
    //    inside the `life.anthropic.auth_verify` child span so the
    //    span tree mirrors the spec's named sites.
    let tier1 = {
        let auth_span = tracing::info_span!("life.anthropic.auth_verify");
        let _g = auth_span.enter();
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
        match state.jwks.verify(bearer) {
            Ok(c) => c,
            Err(e) => {
                return anthropic_http_error(
                    StatusCode::UNAUTHORIZED,
                    AnthropicErrorKind::AuthenticationError,
                    format!("invalid Tier-1: {e}"),
                );
            }
        }
    };

    // C-1 (fix-round 1): rate-limit check AFTER Tier-1 verify (so we
    // have a real `user_id` to key the bucket on) and BEFORE Tier-2
    // mint + the upstream saga (so over-budget traffic doesn't pay
    // the JWS-mint CPU cost or hold a streaming slot). The limiter is
    // the same handle `AuthLayer` uses for the tonic stack, so a user
    // who exhausts their bucket across `/v1/agent/*` will also hit the
    // limit on `/v1/messages` (single shared budget per user).
    //
    // Per the prompt's hard rule, rate-limit failures map to HTTP 429
    // + Anthropic-shape `rate_limit_error` body (the body shape lifed
    // would otherwise produce via `ResourceExhausted` → 429 mapping
    // for an upstream-side rejection).
    if let Some(limiter) = state.rate_limiter.as_ref() {
        let peer_ip = peer_ip_from_request(&headers)
            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        let decision = limiter.check(&tier1.user_id, peer_ip);
        if decision.is_reject() {
            tracing::debug!(
                user = %tier1.user_id,
                ip = %peer_ip,
                reason = decision.reason(),
                "rate limit rejected /v1/messages request"
            );
            return anthropic_http_error(
                StatusCode::TOO_MANY_REQUESTS,
                AnthropicErrorKind::RateLimitError,
                decision.reason().to_string(),
            );
        }
        // Suppress unused-decision warning when reason is informational only.
        let _ = decision;
    }

    // 3. Body size is enforced at the router level via
    //    `DefaultBodyLimit::max(MAX_BODY_BYTES)` — axum rejects
    //    oversized requests with a 413 during the body read before
    //    this handler is even called. See `router(...)` below.

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

    // Spec J §[Vigil span emission]: stamp `gen_ai.request.*` on the
    // root span now that the body is parsed.
    root_span.record("gen_ai.request.model", req.model.as_str());
    root_span.record("gen_ai.request.max_tokens", req.max_tokens);
    if let Some(t) = req.temperature {
        root_span.record("gen_ai.request.temperature", f64::from(t));
    }

    // 5. Synthesize the deterministic Life sid from the Tier-1 caller's
    //    anima DID + the canonical first user message (Spec J L10-D2).
    //    Today the Tier-1 claim set carries `user_id`; the DID is
    //    "did:life:<user_id>" until Spec D's full DID claim threads
    //    through the gateway. The wire algorithm is stable either way.
    let anima_did = format!("did:life:{}", tier1.user_id);
    root_span.record("life.anima.did", anima_did.as_str());
    let sid = {
        let sid_span = tracing::info_span!("life.anthropic.sid_synthesis");
        let _g = sid_span.enter();
        match synthesize_sid(&req, &anima_did) {
            Ok(s) => s,
            Err(e) => {
                return anthropic_http_error(
                    StatusCode::BAD_REQUEST,
                    AnthropicErrorKind::InvalidRequestError,
                    e.to_string(),
                );
            }
        }
    };
    root_span.record("life.session.id", sid.as_str());

    // Spec J §[Cost gate]: estimate worst-case spend BEFORE the upstream
    // saga fires. The estimate uses `life-vigil::pricing::lookup_pricing`
    // — no new tokenizer, just the static rate table. When the model
    // isn't metered we log a warning + fall back to "free tier" (Ok(0)).
    let estimated_input_tokens = approximate_input_tokens(&req);
    let cost_estimate = estimate_request_cost_micros(&req, estimated_input_tokens);

    // Spec J §[Cost gate]: run `haima_check(did, estimated_cost)`
    // BEFORE opening the upstream stream. On `InsufficientCredits` we
    // emit a 402 + x402 challenge body. Other errors map to 500.
    // When `billing_enforce = false`, the check is skipped (the
    // settlement path still records usage on the vigil span).
    if state.billing_enforce {
        let haima_span = tracing::info_span!(
            SPAN_HAIMA_CHECK,
            "life.haima.estimated_cost_micros" = cost_estimate,
        );
        let check_result = state
            .haima
            .check(&anima_did, cost_estimate)
            .instrument(haima_span)
            .await;
        match check_result {
            Ok(()) => {}
            Err(HaimaCheckError::InsufficientCredits {
                required_micros, ..
            }) => {
                tracing::info!(
                    target: "lifegw::anthropic_messages::billing",
                    did = %anima_did,
                    required_micros,
                    "haima_check rejected — emitting 402 + x402 challenge"
                );
                return x402_payment_required_response(required_micros);
            }
            Err(HaimaCheckError::Unavailable(msg)) => {
                tracing::warn!(
                    target: "lifegw::anthropic_messages::billing",
                    did = %anima_did,
                    error = %msg,
                    "haima_check failed (daemon outage) — surfacing 500"
                );
                return anthropic_http_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    AnthropicErrorKind::ApiError,
                    format!("haima unavailable: {msg}"),
                );
            }
        }
    } else {
        tracing::debug!(
            target: "lifegw::anthropic_messages::billing",
            did = %anima_did,
            "cfg.billing.enforce = false — skipping haima_check"
        );
    }

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

    // I-5 (fix-round 1): a `CancellationToken` ties the SendMessage
    // upstream drain task to the SSE response body lifecycle. The
    // body-stream holds the matching `DropGuard`; when axum drops the
    // body (client disconnect, hyper teardown, error), the guard fires
    // `cancel()` and the drain stops cleanly instead of burning a
    // connection-pool slot for up to HARD_STREAM_TIMEOUT.
    let cancel = CancellationToken::new();

    let last_user_content = extract_last_user_content(&req);
    if let Err(err) = send_message(
        &mut agent_client,
        &tier2,
        &sid,
        &last_user_content,
        cancel.clone(),
    )
    .await
    {
        return err;
    }

    let event_stream = match open_stream(&mut agent_client, &tier2, &sid).await {
        Ok(s) => s,
        Err(err) => return err,
    };

    // 8. Build the SSE response. The encoder is fresh; `message_id`
    //    is freshly synthesised in the Anthropic `msg_<hex>` shape so
    //    clients log a recognisable id even though Life sessions don't
    //    have a native equivalent. Span-wrapped per Spec J §[Vigil
    //    span emission] — the `life.anthropic.codec_encode` span
    //    covers encoder construction (the SSE stream itself runs
    //    under the root span via `Instrument`).
    let message_id = format!("msg_{}", Uuid::new_v4().simple());
    let encoder = {
        let codec_span = tracing::info_span!(SPAN_CODEC_ENCODE);
        let _g = codec_span.enter();
        Encoder::new(message_id, req.model.clone())
    };

    // Spec J §[Cost gate]: per-stream usage telemetry. The
    // `output_chars` counter is incremented inside the SSE-body
    // state machine on every Token event. On stream complete the
    // settlement task converts that to a token estimate, records the
    // result on the root span, and calls `haima_settle`.
    let usage_telemetry = Arc::new(UsageTelemetry {
        output_chars: AtomicU64::new(0),
        finished: AtomicU64::new(0),
    });
    let settlement_ctx = SettlementCtx {
        haima: Arc::clone(&state.haima),
        billing_enforce: state.billing_enforce,
        anima_did: anima_did.clone(),
        model: req.model.clone(),
        estimated_input_tokens,
        root_span: root_span.clone(),
        usage: Arc::clone(&usage_telemetry),
    };

    let sse_body = build_sse_body(
        encoder,
        event_stream,
        cancel,
        Arc::clone(&usage_telemetry),
        Some(settlement_ctx),
    );

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
///
/// The upstream `SendMessage` returns its own event stream which we
/// intentionally drop (`StreamSession` is canonical — see the WS
/// dispatcher's same invariant). I-5 fix-round 1: the drain task obeys
/// the caller-provided `cancel` token, so a client disconnect cancels
/// the drain immediately instead of holding the upstream slot for up
/// to `HARD_STREAM_TIMEOUT`.
async fn send_message(
    client: &mut AgentClient<Channel>,
    tier2: &str,
    sid: &str,
    content: &str,
    cancel: CancellationToken,
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

    // Spawn a background drain that pulls (and drops) the SendMessage
    // reply stream. We rely on `Agent.StreamSession` as the canonical
    // event source so we don't double-emit on lifed's fanout registry.
    //
    // The drain stops on any of three signals (whichever wins):
    //   - `cancel.cancelled()` — client disconnected / response body
    //     dropped (the new behaviour).
    //   - upstream EOF / error — natural stream end.
    //   - HARD_STREAM_TIMEOUT — backstop against pathologically slow
    //     upstreams that never close.
    //
    // Without the cancel arm, a client-side disconnect at second 5
    // would still pump for up to 595 more seconds, holding the tonic
    // client + upstream pool slot.
    tokio::spawn(async move {
        let mut s = resp.into_inner();
        let drain = async {
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    next = s.next() => {
                        if next.is_none() {
                            break;
                        }
                        // Intentionally drop the frame. StreamSession is canonical.
                    }
                }
            }
        };
        let _ = tokio::time::timeout(HARD_STREAM_TIMEOUT, drain).await;
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
///
/// I-5 (fix-round 1): the stream owns the `DropGuard` derived from
/// `cancel`. When axum drops the response body (client disconnect,
/// hyper teardown, error path), the guard fires `cancel()` and the
/// upstream `SendMessage` drain task spawned by [`send_message`] stops
/// immediately.
fn build_sse_body(
    encoder: Encoder,
    upstream: tonic::Streaming<pb::AgentEvent>,
    cancel: CancellationToken,
    usage: Arc<UsageTelemetry>,
    settlement: Option<SettlementCtx>,
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
        /// I-5 (fix-round 1): RAII guard that cancels the upstream
        /// `SendMessage` drain task when this state machine is
        /// dropped. The guard's `Drop` impl calls `cancel.cancel()`,
        /// which wakes the drain's `cancel.cancelled()` arm.
        ///
        /// The field is kept alive for the full life of the SSE body —
        /// when axum drops the body (any termination path: hyper
        /// finished, client disconnect, error), the guard fires.
        _drop_guard: tokio_util::sync::DropGuard,
        /// Spec J J-Sub-E: cumulative output-token telemetry. Updated
        /// from each Token event's payload `text` len. Shared with
        /// the settlement context so the post-stream callback reads
        /// the final value.
        usage: Arc<UsageTelemetry>,
        /// Spec J J-Sub-E: settlement context — fired once at stream
        /// termination (any path: clean Finish, upstream error, hard
        /// timeout, client drop via DropGuard). Wrapped in `Option`
        /// so it can be `take()`n out on first fire to guarantee
        /// at-most-once semantics.
        settlement: Option<SettlementCtx>,
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
        _drop_guard: cancel.drop_guard(),
        usage,
        settlement,
    };

    stream::unfold(state, |mut s| async move {
        if s.done {
            // J-Sub-E: at terminal, fire settlement exactly once.
            if let Some(ctx) = s.settlement.take() {
                let output_chars = s.usage.output_chars.load(Ordering::Relaxed);
                ctx.settle_now(output_chars).await;
            }
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
                    if let Some(ctx) = s.settlement.take() {
                        let output_chars = s.usage.output_chars.load(Ordering::Relaxed);
                        ctx.settle_now(output_chars).await;
                    }
                    return None;
                }
                evt = next_event => {
                    match evt {
                        Some(Ok(event)) => {
                            // J-Sub-E: count Token text characters for
                            // output-token approximation. We inspect
                            // the proto payload directly (the codec
                            // doesn't expose its internal accounting).
                            tally_token_chars(&event, &s.usage);
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
                            if s.done {
                                if let Some(ctx) = s.settlement.take() {
                                    let output_chars =
                                        s.usage.output_chars.load(Ordering::Relaxed);
                                    ctx.settle_now(output_chars).await;
                                }
                                return None;
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
                            if let Some(ctx) = s.settlement.take() {
                                let output_chars =
                                    s.usage.output_chars.load(Ordering::Relaxed);
                                ctx.settle_now(output_chars).await;
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
                            if let Some(ctx) = s.settlement.take() {
                                let output_chars =
                                    s.usage.output_chars.load(Ordering::Relaxed);
                                ctx.settle_now(output_chars).await;
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

// ─── J-Sub-E billing + telemetry helpers ────────────────────────────────

/// Per-request running counters shared between the SSE state machine
/// (writer) and the post-stream settlement context (reader).
#[derive(Debug)]
pub(crate) struct UsageTelemetry {
    /// Cumulative output character count, tallied from each Token
    /// event's payload `text` length. Translated to a token estimate
    /// (chars / 4) at settlement.
    pub(crate) output_chars: AtomicU64,
    /// Reserved for future use — a non-zero value signals that
    /// settlement has already fired so the on-drop fallback can skip.
    pub(crate) finished: AtomicU64,
}

/// Context captured at handler entry and consumed once at stream
/// termination. Records `gen_ai.usage.*` + `life.haima.cost_usd_micros`
/// on the root span and calls `haima_settle`.
pub(crate) struct SettlementCtx {
    pub(crate) haima: Arc<dyn HaimaClient>,
    pub(crate) billing_enforce: bool,
    pub(crate) anima_did: String,
    pub(crate) model: String,
    pub(crate) estimated_input_tokens: u32,
    pub(crate) root_span: tracing::Span,
    pub(crate) usage: Arc<UsageTelemetry>,
}

impl SettlementCtx {
    /// Finalise telemetry + settle haima exactly once.
    async fn settle_now(self, output_chars: u64) {
        let SettlementCtx {
            haima,
            billing_enforce,
            anima_did,
            model,
            estimated_input_tokens,
            root_span,
            usage,
        } = self;
        // Cap output_chars at u32::MAX so the chars/4 conversion can't
        // overflow. In practice no Anthropic response approaches this.
        let chars = output_chars.min(u64::from(u32::MAX));
        // Approximate output tokens from char count (chars / 4 ±). The
        // 4-bytes-per-token rule is documented in Anthropic's tokenizer
        // FAQ as the canonical fallback when a real tokenizer is
        // unavailable. We round up so users are not undercharged.
        let approximate_output_tokens = chars.div_ceil(4).min(u64::from(u32::MAX)) as u32;
        let actual_cost_micros =
            compute_cost_micros(&model, estimated_input_tokens, approximate_output_tokens);

        // Spec J §[Vigil span emission]: stamp final usage + cost on
        // the root span. These are the load-bearing attributes for
        // cost-attribution dashboards.
        root_span.record("gen_ai.usage.input_tokens", estimated_input_tokens);
        root_span.record("gen_ai.usage.output_tokens", approximate_output_tokens);
        root_span.record(ATTR_LIFE_HAIMA_COST_MICROS, actual_cost_micros);

        // Mark settlement fired (defensive — the at-most-once invariant
        // already lives in the StreamState `Option::take`).
        usage.finished.fetch_add(1, Ordering::Relaxed);

        if billing_enforce {
            haima
                .settle(
                    &anima_did,
                    &model,
                    estimated_input_tokens,
                    approximate_output_tokens,
                    actual_cost_micros,
                )
                .await;
        }
    }
}

/// Approximate the input-token count from the request body. Spec J
/// J-Sub-E anti-rationalization: "Don't add a new tokenizer — use
/// `life-vigil::pricing::lookup_model`'s side data for cost estimate".
/// We approximate via the universal char/4 fallback over the
/// concatenated system + messages text.
fn approximate_input_tokens(req: &AnthropicMessagesRequest) -> u32 {
    use lifegw_anthropic_codec::request::SystemPrompt;
    let mut chars: u64 = 0;
    if let Some(sys) = req.system.as_ref() {
        chars += match sys {
            SystemPrompt::Text(s) => s.len() as u64,
            SystemPrompt::Blocks(b) => b.iter().map(|x| x.text.len() as u64).sum(),
        };
    }
    for m in &req.messages {
        chars += m.content.plain_text().len() as u64;
    }
    // chars / 4 rounded up, capped at u32.
    chars.div_ceil(4).min(u64::from(u32::MAX)) as u32
}

/// Worst-case cost estimate: prompt rate × estimated input + output
/// rate × `max_tokens`. Used as the gate amount for `haima_check`.
fn estimate_request_cost_micros(
    req: &AnthropicMessagesRequest,
    estimated_input_tokens: u32,
) -> u64 {
    // max_tokens is the upper bound the *client* asked for; without a
    // real tokenizer it's the only honest worst-case ceiling we have.
    compute_cost_micros(&req.model, estimated_input_tokens, req.max_tokens)
}

/// Convert a (model, input_tokens, output_tokens) tuple to a cost in
/// micro-USDC (1 USDC = 1_000_000 micro-USDC). Returns `0` when the
/// model isn't in the pricing snapshot — Spec J anti-rationalization:
/// "model not metered, defaulting to free tier" with a Vigil warning.
fn compute_cost_micros(model: &str, input_tokens: u32, output_tokens: u32) -> u64 {
    let Some(pricing) = life_vigil::pricing::lookup_pricing(model) else {
        tracing::warn!(
            target: "lifegw::anthropic_messages::billing",
            model = %model,
            "model not in life-vigil pricing snapshot — defaulting to free tier"
        );
        return 0;
    };
    // pricing.{input,output}_per_million is USD per 1M tokens.
    // micros = (tokens / 1_000_000) * usd_per_million * 1_000_000
    //        = tokens * usd_per_million
    // i.e. tokens × USD-per-million == micro-USD.
    let input_micros = (f64::from(input_tokens) * pricing.input_per_million).max(0.0);
    let output_micros = (f64::from(output_tokens) * pricing.output_per_million).max(0.0);
    let total = input_micros + output_micros;
    if total.is_finite() && total >= 0.0 {
        // Clamp to u64 — for any realistic request this is far below
        // u64::MAX (claude-opus-4 max-tokens=4096 ≈ 320 µUSD).
        total.min(f64::from(u32::MAX) * 1.0e6) as u64
    } else {
        0
    }
}

/// Increment the per-stream `output_chars` counter from a Token-kind
/// AgentEvent's payload `text` field. Other event kinds are ignored.
fn tally_token_chars(evt: &pb::AgentEvent, usage: &Arc<UsageTelemetry>) {
    if evt.kind != pb::AgentEventKind::Token as i32 {
        return;
    }
    let Some(record) = evt.record.as_ref() else {
        return;
    };
    // The payload is JSON `{"text":"..."}` for text tokens and
    // `{"thinking":"..."}` for thinking deltas. We tally only the
    // user-visible `text` field — thinking blocks aren't charged.
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&record.payload) else {
        return;
    };
    if let Some(s) = payload.get("text").and_then(serde_json::Value::as_str) {
        usage
            .output_chars
            .fetch_add(s.len() as u64, Ordering::Relaxed);
    }
}

/// Build the 402 + x402 challenge response per Spec J §[Cost gate].
///
/// The body is the Anthropic-shape `billing_error` JSON; the
/// `X-Payment` header carries the x402 challenge so x402-aware clients
/// can auto-pay and retry. The amount header is the required deposit
/// surfaced via the pricing snapshot; defaults to `0.10 USD` when the
/// upstream rate is unknown.
fn x402_payment_required_response(required_micros: u64) -> Response {
    // Spec J §[Cost gate] body shape:
    //   { "type": "error",
    //     "error": { "type": "billing_error",
    //                "message": "Insufficient credits" } }
    let err = AnthropicError::new(AnthropicErrorKind::BillingError, "Insufficient credits");
    let body_json = err.to_sse_data();

    // Convert micros → decimal-string USD. When the gate amount is
    // unknown (model not metered) we fall back to the published
    // 0.10 USD default so x402 still has a deposit number to show.
    let amount_usd = if required_micros == 0 {
        X402_AMOUNT_USD_DEFAULT.to_string()
    } else {
        // 1 USDC = 1_000_000 micro-USDC; format with 6-decimal precision.
        format!("{:.6}", (required_micros as f64) / 1.0e6)
    };
    let payment_challenge = serde_json::json!({
        "chain": X402_CHAIN,
        "token": X402_TOKEN,
        "amount": amount_usd,
        "facilitator": X402_FACILITATOR_DEFAULT,
    });
    let payment_challenge_str = payment_challenge.to_string();

    let mut resp = Response::new(Body::from(body_json));
    *resp.status_mut() = StatusCode::PAYMENT_REQUIRED;
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    // X-Payment is the x402 challenge envelope. We `from_maybe_shared`
    // since the JSON contains characters that are not in the strict
    // header-value charset — `HeaderValue::from_str` would reject
    // braces. The challenge body is bounded (≈100 bytes) so the value
    // is always a valid header value byte-wise.
    if let Ok(hv) = HeaderValue::from_maybe_shared(payment_challenge_str.into_bytes()) {
        h.insert(http::HeaderName::from_static("x-payment"), hv);
    }
    resp
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

/// Resolve the request's peer IP for the rate-limiter's per-IP bucket.
///
/// Mirrors the precedence rules from `auth::middleware::peer_ip_from_request`
/// but reads only the `HeaderMap` axum hands us (axum's `ConnectInfo`
/// would have to be wired through a separate extractor, which would mean
/// rebuilding the route mount point; XFF + Forwarded headers cover the
/// only deployment topology that matters — lifegw behind Vercel edge —
/// and the L7 in front sets one of those two headers for every request).
///
/// Order of precedence:
/// 1. `X-Forwarded-For` (leftmost non-empty value).
/// 2. `Forwarded` (RFC 7239 `for=<ip>` token).
/// 3. `None` — caller falls back to `0.0.0.0` so the limiter still
///    enforces a defence-in-depth single-shared-bucket budget.
fn peer_ip_from_request(headers: &HeaderMap) -> Option<std::net::IpAddr> {
    if let Some(hv) = headers.get("x-forwarded-for")
        && let Ok(s) = hv.to_str()
        && let Some(first) = s.split(',').map(str::trim).find(|x| !x.is_empty())
        && let Some(ip) = crate::auth::middleware::parse_ip_or_socket(first)
    {
        return Some(ip);
    }
    if let Some(hv) = headers.get("forwarded")
        && let Ok(s) = hv.to_str()
    {
        for part in s.split(';') {
            for kv in part.split(',') {
                let trimmed = kv.trim();
                let rest = trimmed
                    .strip_prefix("for=")
                    .or_else(|| trimmed.strip_prefix("For="));
                if let Some(rest) = rest {
                    let raw = rest.trim_matches('"');
                    if let Some(ip) = crate::auth::middleware::parse_ip_or_socket(raw) {
                        return Some(ip);
                    }
                }
            }
        }
    }
    None
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

// ─── J-Sub-F: GET /v1/models ────────────────────────────────────────────

/// One row in the Anthropic-shape `data: [...]` list returned by
/// `GET /v1/models`. Matches Anthropic's [public response shape]
/// for Claude Code's `/model` picker.
///
/// [public response shape]: https://docs.anthropic.com/en/api/models-list
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    /// Stable model identifier (e.g. `claude-sonnet-4-20250514`).
    pub id: String,
    /// Human-readable display name surfaced in pickers.
    pub display_name: String,
    /// Release timestamp in RFC-3339 / ISO-8601. Anthropic uses
    /// midnight-UTC for these.
    pub created_at: String,
    /// Always `"model"` per Anthropic's wire shape — exposed so
    /// downstream clients that pattern-match on `type` work without
    /// custom serde. `String` rather than `&'static str` so the type
    /// is `Deserialize` (round-trippable through the integration test
    /// rig and any future replay machinery).
    #[serde(rename = "type", default = "model_kind_default")]
    pub kind: String,
}

/// Default `kind` value for [`ModelInfo`] deserialization — always
/// `"model"` per Anthropic's wire shape.
fn model_kind_default() -> String {
    "model".to_string()
}

/// Anthropic-shape `GET /v1/models` response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListResponse {
    /// Ordered list of available models. Anthropic's hosted endpoint
    /// returns the newest entries first; we mirror that ordering.
    pub data: Vec<ModelInfo>,
    /// First `id` in `data` (or empty if `data` is empty). Anthropic's
    /// pagination envelope.
    pub first_id: String,
    /// Whether more models are available beyond this page. Phase 1 is
    /// always `false` — the static list fits comfortably in a single
    /// response.
    pub has_more: bool,
    /// Last `id` in `data`.
    pub last_id: String,
}

/// Build the Phase 1 static model list per Spec J §[Model picker].
///
/// The pinned Anthropic identifiers are the autocomplete defaults
/// Claude Code ships with — keeping `/model` recognise these IDs makes
/// the gateway a drop-in for `api.anthropic.com` even before Spec E
/// backend fan-out lands.
///
/// **Phase 2 placeholder** — when Spec E's `InferenceRouter` exposes a
/// discoverable backend catalogue, this function will additionally:
///
/// 1. Query the router for the active backend set.
/// 2. For each backend `<b>` and model `<m>`, emit `id =
///    "life/<b>/<m>"` plus, where the backend declares thinking
///    support, a `<id>-no-thinking` companion (matching
///    free-claude-code's `gateway_model_id` /
///    `no_thinking_gateway_model_id` pattern).
/// 3. Keep the Anthropic-pinned list first so Claude Code's default
///    `/model` autocomplete still picks Anthropic identifiers.
///
/// That extension is **NOT** wired in this PR — Phase 2 surface area
/// only. See `docs/superpowers/specs/2026-05-18-spec-j-claude-code-interop.md`
/// §L10-D6 for the locked behaviour.
fn static_model_catalogue() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "claude-opus-4-20250514".to_string(),
            display_name: "Claude Opus 4".to_string(),
            created_at: "2025-05-14T00:00:00Z".to_string(),
            kind: "model".to_string(),
        },
        ModelInfo {
            id: "claude-sonnet-4-20250514".to_string(),
            display_name: "Claude Sonnet 4".to_string(),
            created_at: "2025-05-14T00:00:00Z".to_string(),
            kind: "model".to_string(),
        },
        ModelInfo {
            id: "claude-haiku-4-20250514".to_string(),
            display_name: "Claude Haiku 4".to_string(),
            created_at: "2025-05-14T00:00:00Z".to_string(),
            kind: "model".to_string(),
        },
        ModelInfo {
            id: "claude-sonnet-4-5-20250929".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            created_at: "2025-09-29T00:00:00Z".to_string(),
            kind: "model".to_string(),
        },
        ModelInfo {
            id: "claude-haiku-4-5-20251001".to_string(),
            display_name: "Claude Haiku 4.5".to_string(),
            created_at: "2025-10-01T00:00:00Z".to_string(),
            kind: "model".to_string(),
        },
    ]
}

/// Build the `ModelListResponse` envelope from a `Vec<ModelInfo>`.
fn build_model_list_response(models: Vec<ModelInfo>) -> ModelListResponse {
    let first_id = models.first().map(|m| m.id.clone()).unwrap_or_default();
    let last_id = models.last().map(|m| m.id.clone()).unwrap_or_default();
    ModelListResponse {
        data: models,
        first_id,
        has_more: false,
        last_id,
    }
}

/// `GET /v1/models` handler. Returns the Phase 1 static catalogue as a
/// JSON body in Anthropic's wire shape.
///
/// Unauthenticated by design — the picker is consulted at client
/// bootstrap before a Tier-1 bearer has been negotiated in some
/// deployments. The catalogue carries no per-tenant data; exposing it
/// is equivalent to publishing the docs page.
async fn models_handler(State(_state): State<AnthropicMessagesState>) -> Response {
    let body = build_model_list_response(static_model_catalogue());
    let payload = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(e) => {
            return anthropic_http_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                AnthropicErrorKind::ApiError,
                format!("encode models response: {e}"),
            );
        }
    };
    let mut resp = Response::new(Body::from(payload));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    resp
}

// ─── J-Sub-F: POST /v1/messages/count_tokens ────────────────────────────

/// Inbound body for `POST /v1/messages/count_tokens`. Mirrors
/// Anthropic's published shape — `model` + `messages[]` are mandatory;
/// `system` + `tools` are accepted but currently ignored by the
/// estimator (tool definitions are excluded from text-count, matching
/// Anthropic's own definition of "prompt tokens consumed by the
/// caller-visible text").
///
/// The shape is forward-compatible — `serde(default)` on the missing
/// envelope means new Anthropic-side fields don't break clients.
#[derive(Debug, Clone, Deserialize)]
struct CountTokensRequest {
    /// Model identifier. Honoured for pricing lookup + the Vigil span
    /// `gen_ai.request.model` attribute; does NOT influence the
    /// estimator (the 4-chars/token heuristic is model-agnostic at
    /// edge — Spec J L10-D7).
    model: String,
    /// Conversation history to estimate. Reused via the canonical
    /// codec request shape so the same parsing rules apply
    /// (`string`-or-`array` `content`, `tool_use` / `tool_result`
    /// blocks contribute the empty string for sid synthesis, etc.).
    messages: Vec<lifegw_anthropic_codec::Message>,
    /// Optional system prompt — counted into the token estimate when
    /// present.
    #[serde(default)]
    system: Option<lifegw_anthropic_codec::SystemPrompt>,
    /// Tools available to the model — currently ignored by the
    /// estimator (Anthropic's published count includes tool-def JSON
    /// schema text, but that's a Phase-2 refinement; matching their
    /// number to ±5% is sufficient for compact-window budgeting).
    #[serde(default)]
    #[allow(dead_code)]
    tools: Vec<serde_json::Value>,
}

/// Response body for `POST /v1/messages/count_tokens`. Strict
/// Anthropic-compat — no extra fields.
#[derive(Debug, Clone, Serialize)]
struct CountTokensResponse {
    input_tokens: usize,
}

/// Concatenate all messages into a single text string for estimator
/// input. Tool-use blocks are stripped (their schema is metadata, not
/// caller-visible prompt text); tool-result content is included via
/// `plain_text()`'s text-block-only contract.
///
/// System prompt text is prepended (Anthropic-aligned: the system
/// prompt is part of the prompt-token count).
fn canonicalize_messages_for_count(req: &CountTokensRequest) -> String {
    let mut out = String::new();
    if let Some(sys) = req.system.as_ref() {
        use lifegw_anthropic_codec::SystemPrompt;
        match sys {
            SystemPrompt::Text(t) => out.push_str(t),
            SystemPrompt::Blocks(blocks) => {
                for b in blocks {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&b.text);
                }
            }
        }
    }
    for m in &req.messages {
        let text = m.content.plain_text();
        if text.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&text);
    }
    out
}

/// `POST /v1/messages/count_tokens` handler. Returns an Anthropic-shape
/// `{"input_tokens": <usize>}` body computed via the 4-chars/token edge
/// estimator (Spec J L10-D7). Emits a `life.anthropic.count_tokens`
/// Vigil span carrying GenAI semconv attributes plus
/// `life.estimated_cost_usd_micros` when the model is in the pricing
/// snapshot, and a `X-Life-Cost-Estimate-Usd-Micros` response header.
async fn count_tokens_handler(
    State(state): State<AnthropicMessagesState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 1. Verify the Tier-1 bearer (same flow as `/v1/messages`).
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

    // 2. Rate-limit (same shared-budget posture as /v1/messages).
    if let Some(limiter) = state.rate_limiter.as_ref() {
        let peer_ip = peer_ip_from_request(&headers)
            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        let decision = limiter.check(&tier1.user_id, peer_ip);
        if decision.is_reject() {
            return anthropic_http_error(
                StatusCode::TOO_MANY_REQUESTS,
                AnthropicErrorKind::RateLimitError,
                decision.reason().to_string(),
            );
        }
    }

    // 3. Parse the body.
    let req: CountTokensRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => {
            return anthropic_http_error(
                StatusCode::BAD_REQUEST,
                AnthropicErrorKind::InvalidRequestError,
                format!("invalid JSON body: {e}"),
            );
        }
    };
    if req.model.trim().is_empty() {
        return anthropic_http_error(
            StatusCode::BAD_REQUEST,
            AnthropicErrorKind::InvalidRequestError,
            "model must not be empty",
        );
    }
    if req.messages.is_empty() {
        return anthropic_http_error(
            StatusCode::BAD_REQUEST,
            AnthropicErrorKind::InvalidRequestError,
            "messages must contain at least one entry",
        );
    }

    // 4. Canonicalise + estimate.
    let text = canonicalize_messages_for_count(&req);
    let input_tokens = life_vigil::tokens::estimate_tokens(&text);

    // 5. Look up pricing for the cost-estimate side-band. Vigil's
    //    `lookup_pricing` accepts exact + substring matches so dated
    //    variants resolve cleanly.
    let pricing = life_vigil::pricing::lookup_pricing(&req.model);
    let cost_micros: Option<u64> = pricing.map(|p| {
        // Cost in USD micros (10⁻⁶ USD). `input_per_million` is USD per
        // 1e6 tokens; we want micros per `input_tokens` tokens:
        //   micros = tokens * (USD/1e6 tokens) * (1e6 micros/USD)
        //          = tokens * input_per_million
        // The factors cancel into a simple multiplication.
        let micros_f = (input_tokens as f64) * p.input_per_million;
        // Saturating cast — pricing snapshot keeps numbers small (max
        // ~75.0 per million tokens for Opus output), so a 1 GB user
        // message produces ~1.9e10 tokens → ~1.4e12 micros, well inside
        // u64 range. Saturating is defensive against future bumps.
        micros_f.max(0.0).min(u64::MAX as f64) as u64
    });

    // 6. Emit the Vigil span. We use `info_span!` so the
    //    `tracing-opentelemetry` layer picks it up into OTLP. The
    //    `life.estimated_cost_usd_micros` field is created up-front
    //    with `tracing::field::Empty` so the conditional `record(...)`
    //    call below has somewhere to write.
    let anima_did = format!("did:life:{}", tier1.user_id);
    let span = tracing::info_span!(
        "life.anthropic.count_tokens",
        gen_ai.system = "life",
        gen_ai.operation.name = "count_tokens",
        gen_ai.usage.input_tokens = input_tokens,
        gen_ai.request.model = %req.model,
        life.anima.did = %anima_did,
        life.estimated_cost_usd_micros = tracing::field::Empty,
    );
    if let Some(c) = cost_micros {
        span.record("life.estimated_cost_usd_micros", c);
    }
    let _enter = span.enter();
    tracing::debug!(
        user = %tier1.user_id,
        model = %req.model,
        input_tokens,
        cost_micros = ?cost_micros,
        "count_tokens estimate",
    );
    drop(_enter);

    // 7. Build the response. Always JSON body (Anthropic-compat); when
    //    pricing exists, surface the cost estimate via the
    //    `X-Life-Cost-Estimate-Usd-Micros` header so haima-aware
    //    clients see it without parsing the trace.
    let payload = match serde_json::to_vec(&CountTokensResponse { input_tokens }) {
        Ok(b) => b,
        Err(e) => {
            return anthropic_http_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                AnthropicErrorKind::ApiError,
                format!("encode count_tokens response: {e}"),
            );
        }
    };
    let mut resp = Response::new(Body::from(payload));
    *resp.status_mut() = StatusCode::OK;
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Some(c) = cost_micros
        && let Ok(v) = HeaderValue::from_str(&c.to_string())
    {
        h.insert(
            header::HeaderName::from_static("x-life-cost-estimate-usd-micros"),
            v,
        );
    }
    resp
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
