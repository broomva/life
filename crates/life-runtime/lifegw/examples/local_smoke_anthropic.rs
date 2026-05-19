//! BRO-1185 — real-Anthropic local smoke for lifegw (zero deploy, zero
//! Railway). The mirror of `local_smoke.rs` with one substantive change:
//! the upstream tonic `Agent` service forwards to
//! `arcan_proxy::anthropic::AnthropicArcan` instead of returning canned
//! events. Result: a real Claude Code ↔ lifegw ↔ `api.anthropic.com`
//! round-trip on `127.0.0.1:<port>` with only `ANTHROPIC_API_KEY` set.
//!
//! # Purpose
//!
//! Path 4 of the Spec J Phase 1 test-surface taxonomy (see
//! `docs/conformance/2026-05-18-claude-code-smoke-runbook.md`):
//!
//! 1. In-process E2E test (`cargo test -p lifegw --test spec_j_e2e_smoke`)
//!    — automated, mock upstream.
//! 2. `local_smoke` example — interactive, mock upstream. Validates the
//!    edge wire (codec / auth / rate-limit / Vigil) without burning a
//!    Railway slot.
//! 3. Railway staging deploy — live, real upstream, full saga. Produces
//!    the Phase 1 conformance evidence.
//! 4. **This example** — interactive, real upstream, no Railway slot.
//!    Daily dogfooding loop for the iterating engineer: every call hits
//!    `api.anthropic.com` and costs real money, but the iteration cycle
//!    is `cargo run` rather than `railway up`.
//!
//! # Honest divergence from production
//!
//! Production lifegw goes through `lifed.Agent.StreamSession` → real
//! saga (Tier-2 → Tier-3) → `arcan-proxy` → `AnthropicArcan`. This
//! example **shortcuts the `lifed` layer entirely**: the in-process
//! tonic `Agent` service built below dispatches directly to
//! `AnthropicArcan`. Useful for iterating on the gateway (codec, route,
//! Vigil) without paying Railway latency. **Does NOT validate** the
//! full production saga / arcan-substrate dial / lago events / haima
//! ledger. For that, run Path 3 (the Railway operator runbook).
//!
//! # What's real, what's not
//!
//! Real (in-process, same code as production):
//!
//! - the axum router built from `AnthropicMessagesState`,
//! - the `lifegw_anthropic_codec::Encoder`,
//! - the `JwksCache::dev_only()` Tier-1 verifier (dev-bearer shortcut),
//! - the `Tier2Minter` capability minter,
//! - the `TokenBucketLimiter` rate limiter,
//! - the tonic Channel dialled to the in-process Agent UDS,
//! - the `StubHaimaClient` cost gate (no-op, mirrors production today),
//! - the `AnthropicArcan` HTTP client (POST `/v1/messages` against
//!   `api.anthropic.com`, real SSE parse, real `Token`/`Finish` events).
//!
//! In-process (not mocked, but not lifed either):
//!
//! - `lifed.life.v1.Agent.{SendMessage, StreamSession}` is served by a
//!   minimal `AnthropicProxyAgentService` that captures the user
//!   content on `SendMessage` and replays it through
//!   `AnthropicArcan::dispatch_message` on the matching
//!   `StreamSession`. Multi-turn history is bookkept by
//!   `AnthropicArcan` itself.
//!
//! Substrate-free per Spec C₃ §11.2 — `arcan-proxy` is a
//! `[dev-dependencies]` entry, not a production dep. The
//! `verify_dependencies_lifegw.sh` script uses `cargo tree --edges
//! normal` which excludes dev-deps, so this remains compliant. The
//! carve-out matches the existing `lifed` dev-dep entry (used by
//! `tests/integration_proxy_passthrough.rs`).
//!
//! # 600 s SSE wall-clock cap
//!
//! The production `anthropic_messages::router` enforces
//! `HARD_STREAM_TIMEOUT = 600 s` on the SSE response body (see
//! `services/anthropic_messages.rs:272`). Because this example uses
//! `anthropic_messages::router(state)` directly, that cap is inherited
//! — no new timeout code at the example level. Long-running upstream
//! calls (e.g. a multi-minute Sonnet thinking turn) are bounded by the
//! same backstop the production gateway uses.
//!
//! # Lifecycle
//!
//! 1. Read `ANTHROPIC_API_KEY` from env (and `ANTHROPIC_MODEL`,
//!    `ANTHROPIC_BASE_URL`, `ANTHROPIC_MAX_TOKENS` if present).
//!    Bail fast with a clean error block if the key is missing.
//! 2. Build a tempdir + UDS path `<tempdir>/lifed.sock`.
//! 3. Spawn a tonic `Server` hosting the `AnthropicProxyAgentService`
//!    on that UDS.
//! 4. Dial a tonic `Channel` back over the UDS.
//! 5. Build `AnthropicMessagesState` with real auth + rate-limit +
//!    stub haima + the in-process channel.
//! 6. Bind axum on `127.0.0.1:0` (kernel-picked port).
//! 7. Print the URL + dev bearer + cost warning + curl recipes.
//! 8. Serve until `SIGINT` (Ctrl-C).
//! 9. Graceful drain: shut down axum, then the in-process UDS server,
//!    then drop the tempdir.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::Stream;
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
use tracing_subscriber::EnvFilter;

// `ArcanCall` is the trait that provides `dispatch_message`; it's an
// `async_trait` so the method-on-trait-object pattern requires the
// trait to be in scope at the call site.
use arcan_proxy::ArcanCall;
use life_runtime_proto::life::v1::agent_server::{Agent, AgentServer};
use life_runtime_proto::life::v1::{
    AgentEvent, ApprovalReq, CreateSessionReq, DispatchRef, Empty, ListModelsReq, ListSkillsReq,
    ListToolsReq, ModelCatalog, SendMessageReq, Session, SessionRef, SkillCatalog, SpawnChildReq,
    SpawnChildResp, ToolCatalog,
};

use lifegw::auth::jwks::JwksCache;
use lifegw::auth::kms::StaticKeystore;
use lifegw::auth::tier2::Tier2Minter;
use lifegw::config::{AuthConfig, RateLimitConfig};
use lifegw::services::anthropic_messages::{
    self, AnthropicMessagesState, HaimaClient, StubHaimaClient,
};
use lifegw::services::rate_limit::TokenBucketLimiter;

// ─── In-process Anthropic-proxy Agent service ───────────────────────────
//
// Same shape as `tests/spec_j_e2e_smoke.rs::MockAgentService` and
// `examples/local_smoke.rs::MockAgentService`, but the
// `SendMessage` / `StreamSession` pair forwards real content to
// `arcan_proxy::anthropic::AnthropicArcan`. The split mirrors lifegw's
// production posture: lifegw calls `SendMessage` (no streaming), then
// `StreamSession` to consume events — the example honours that ordering
// by stashing the user content in `SendMessage` and replaying it through
// `AnthropicArcan::dispatch_message` on `StreamSession`.

#[derive(Clone)]
struct AnthropicProxyAgentService {
    /// Underlying real-upstream client. Cloned cheaply; the inner state
    /// (per-sid history, reqwest client) is `Arc`-wrapped inside.
    arcan: Arc<arcan_proxy::anthropic::AnthropicArcan>,
    /// `sid -> last user message`. `SendMessage` inserts; `StreamSession`
    /// removes and dispatches. A new `SendMessage` for the same sid
    /// overwrites — matches the production "one in-flight message per
    /// session" posture.
    pending: Arc<Mutex<HashMap<String, String>>>,
    /// Monotonic counter for synthesizing fresh `mock-sid-N` values when
    /// the caller does not supply a `resume_sid`. Same naming as
    /// `local_smoke.rs` so the operator's mental model is identical
    /// across the two examples.
    sequence: Arc<Mutex<u64>>,
}

#[tonic::async_trait]
impl Agent for AnthropicProxyAgentService {
    type SendMessageStream =
        Pin<Box<dyn Stream<Item = Result<AgentEvent, tonic::Status>> + Send + 'static>>;
    type StreamSessionStream =
        Pin<Box<dyn Stream<Item = Result<AgentEvent, tonic::Status>> + Send + 'static>>;

    async fn create_session(
        &self,
        req: tonic::Request<CreateSessionReq>,
    ) -> Result<tonic::Response<Session>, tonic::Status> {
        let body = req.into_inner();
        let sid_val = match body.resume_sid.as_ref().map(|s| s.value.clone()) {
            Some(v) if !v.is_empty() => v,
            _ => {
                let mut seq = self.sequence.lock().await;
                *seq += 1;
                format!("mock-sid-{}", *seq)
            }
        };
        Ok(tonic::Response::new(Session {
            sid: Some(aios_proto::aios::v1::SessionId { value: sid_val }),
            agent_id: Some(aios_proto::aios::v1::AgentId {
                value: "anthropic-arcan".to_string(),
            }),
            user_id: body.user_id,
            project_id: body.project_id,
            created_at: Some(prost_types::Timestamp {
                seconds: 0,
                nanos: 0,
            }),
        }))
    }

    async fn describe_session(
        &self,
        _: tonic::Request<SessionRef>,
    ) -> Result<tonic::Response<Session>, tonic::Status> {
        Err(tonic::Status::unimplemented("anthropic-proxy"))
    }

    async fn close_session(
        &self,
        _: tonic::Request<SessionRef>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        Ok(tonic::Response::new(Empty {}))
    }

    async fn send_message(
        &self,
        req: tonic::Request<SendMessageReq>,
    ) -> Result<tonic::Response<Self::SendMessageStream>, tonic::Status> {
        // Stash the user content for the matching `StreamSession`.
        // Production lifegw calls `SendMessage` (returns empty stream)
        // immediately followed by `StreamSession`; the example matches
        // that ordering.
        let body = req.into_inner();
        let sid = body
            .sid
            .as_ref()
            .map(|s| s.value.clone())
            .unwrap_or_default();
        let content = body.content;
        if !sid.is_empty() {
            self.pending.lock().await.insert(sid, content);
        }
        // Empty stream — matches the mock's posture: the canonical event
        // source is `StreamSession`, not `SendMessage`.
        let s = futures::stream::empty::<Result<AgentEvent, tonic::Status>>();
        Ok(tonic::Response::new(Box::pin(s)))
    }

    async fn stream_session(
        &self,
        req: tonic::Request<SessionRef>,
    ) -> Result<tonic::Response<Self::StreamSessionStream>, tonic::Status> {
        let sref = req.into_inner();
        let sid = sref
            .sid
            .as_ref()
            .map(|s| s.value.clone())
            .unwrap_or_default();
        let content = self.pending.lock().await.remove(&sid).unwrap_or_default();
        if content.is_empty() {
            // No content stashed — caller hit `StreamSession` before
            // `SendMessage`, or the sid does not match a pending entry.
            // Return an empty stream (matches the mock's posture for
            // empty replay).
            let s = futures::stream::empty::<Result<AgentEvent, tonic::Status>>();
            return Ok(tonic::Response::new(Box::pin(s)));
        }
        // Dispatch into the real Anthropic upstream. `AnthropicArcan`
        // bookkeeps multi-turn history per-sid internally
        // (`append_exchange` on `Finish`), so successive turns on the
        // same sid carry conversation context.
        let stream = self
            .arcan
            .dispatch_message(&sid, &content)
            .await
            .map_err(|e| tonic::Status::internal(format!("AnthropicArcan dispatch failed: {e}")))?;
        Ok(tonic::Response::new(stream))
    }

    async fn approve_dispatch(
        &self,
        _: tonic::Request<ApprovalReq>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        Ok(tonic::Response::new(Empty {}))
    }

    async fn cancel_dispatch(
        &self,
        _: tonic::Request<DispatchRef>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        Ok(tonic::Response::new(Empty {}))
    }

    async fn list_skills(
        &self,
        _: tonic::Request<ListSkillsReq>,
    ) -> Result<tonic::Response<SkillCatalog>, tonic::Status> {
        Err(tonic::Status::unimplemented("anthropic-proxy"))
    }

    async fn list_models(
        &self,
        _: tonic::Request<ListModelsReq>,
    ) -> Result<tonic::Response<ModelCatalog>, tonic::Status> {
        Err(tonic::Status::unimplemented("anthropic-proxy"))
    }

    async fn list_tools(
        &self,
        _: tonic::Request<ListToolsReq>,
    ) -> Result<tonic::Response<ToolCatalog>, tonic::Status> {
        Err(tonic::Status::unimplemented("anthropic-proxy"))
    }

    async fn spawn_child(
        &self,
        _: tonic::Request<SpawnChildReq>,
    ) -> Result<tonic::Response<SpawnChildResp>, tonic::Status> {
        Err(tonic::Status::unimplemented("anthropic-proxy"))
    }
}

async fn dial_uds(path: &str) -> Channel {
    let path = path.to_string();
    let endpoint = Endpoint::try_from("http://[::]:0").expect("endpoint scheme");
    endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = path.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(path).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
            }
        }))
        .await
        .expect("dial in-process Anthropic-proxy UDS")
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Logging — default to `lifegw=debug,info` so Vigil span emission is
    // visible to the operator. `RUST_LOG` overrides the default.
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("lifegw=debug,info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    // 0. Fail fast on missing API key. The error from
    // `AnthropicArcan::from_env` already says "AnthropicArcan requires
    // ANTHROPIC_API_KEY" (no key value in the message); the printed
    // operator instructions never echo the key either.
    let arcan = match arcan_proxy::anthropic::AnthropicArcan::from_env() {
        Ok(a) => a,
        Err(err) => {
            eprintln!();
            eprintln!("ERROR: {err}");
            eprintln!();
            eprintln!("local_smoke_anthropic requires ANTHROPIC_API_KEY in the environment.");
            eprintln!("Set it (Haiku is the cheapest model for iteration) and re-run:");
            eprintln!();
            eprintln!("    export ANTHROPIC_API_KEY=sk-...");
            eprintln!(
                "    export ANTHROPIC_MODEL=claude-haiku-4-5-20251001  # optional, defaults to Sonnet 4.5"
            );
            eprintln!("    cargo run -p lifegw --example local_smoke_anthropic");
            eprintln!();
            std::process::exit(1);
        }
    };
    let arcan = Arc::new(arcan);

    // Surface which model is in use so the operator sees what they're
    // about to pay for. `AnthropicArcan` itself parses `ANTHROPIC_MODEL`
    // (defaulting to `claude-sonnet-4-5-20250929`); reading the env var
    // here mirrors that derivation without touching the redacted Debug.
    let active_model = std::env::var("ANTHROPIC_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| arcan_proxy::ANTHROPIC_DEFAULT_MODEL.to_string());

    // 1. Stand up the in-process Anthropic-proxy Agent service over a
    // tempdir UDS.
    let temp: TempDir = tempfile::tempdir()?;
    let socket_path = temp.path().join("lifed.sock");
    let socket_path_str = socket_path.to_string_lossy().to_string();
    let listener = UnixListener::bind(&socket_path)?;
    let uds_stream = UnixListenerStream::new(listener);

    tracing::info!(uds = %socket_path_str, "in-process Anthropic-proxy UDS bound");

    let agent_svc = AnthropicProxyAgentService {
        arcan: Arc::clone(&arcan),
        pending: Arc::new(Mutex::new(HashMap::new())),
        sequence: Arc::new(Mutex::new(0)),
    };
    let agent_server = AgentServer::new(agent_svc);

    let (uds_shutdown_tx, uds_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let uds_handle = tokio::spawn(async move {
        if let Err(err) = tonic::transport::Server::builder()
            .add_service(agent_server)
            .serve_with_incoming_shutdown(uds_stream, async move {
                let _ = uds_shutdown_rx.await;
            })
            .await
        {
            tracing::warn!(error = %err, "in-process Anthropic-proxy server exited with error");
        }
    });

    // Tiny grace for the listener to start accepting (mirrors the
    // mock-upstream example — connect_with_connector retries once but a
    // brief sleep makes the first dial deterministic).
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 2. Dial the in-process Anthropic-proxy over UDS.
    let upstream = dial_uds(&socket_path_str).await;

    // 3. Build the router state — same shape lifegw uses in production,
    // with the dev JWKS shortcut + a freshly-generated keystore + a
    // default-config rate limiter + the stub haima cost gate.
    let auth_cfg = AuthConfig::default();
    let jwks = Arc::new(JwksCache::dev_only());
    let signer = Arc::new(StaticKeystore::generate_dev()?);
    let minter = Arc::new(Tier2Minter::new(signer, &auth_cfg));
    let rate_limiter = TokenBucketLimiter::from_config(&RateLimitConfig::default());
    let haima: Arc<dyn HaimaClient> = Arc::new(StubHaimaClient);

    let state = AnthropicMessagesState {
        jwks,
        minter,
        upstream,
        rate_limiter: Some(rate_limiter),
        haima,
        billing_enforce: false,
    };
    // The 600 s `HARD_STREAM_TIMEOUT` SSE wall-clock cap is enforced
    // inside `anthropic_messages::router(state)` itself — see
    // `services/anthropic_messages.rs:272`. The example inherits the
    // production cap unchanged.
    let app = anthropic_messages::router(state);

    // 4. Bind axum on 127.0.0.1:0 — kernel picks the port so concurrent
    // example runs don't collide.
    let bind_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let local_addr = bind_listener.local_addr()?;

    let dev_bearer = "dev-token-for-broomva";

    println!();
    println!("lifegw local smoke (real Anthropic upstream) ready:");
    println!("  URL:    http://{local_addr}");
    println!("  Bearer: {dev_bearer}");
    println!("  Model:  {active_model}");
    println!();
    println!("WARNING: this binds to api.anthropic.com using your ANTHROPIC_API_KEY.");
    println!("         Every /v1/messages call costs real money on the active model.");
    println!("         Haiku (claude-haiku-4-5-20251001) is the cheapest; switch to");
    println!("         it for iteration via ANTHROPIC_MODEL=...");
    println!();
    println!("Try:");
    println!("  curl http://{local_addr}/v1/models | jq .");
    println!();
    println!("  curl -N -H \"Authorization: Bearer {dev_bearer}\" \\");
    println!("       -H \"anthropic-version: 2023-06-01\" \\");
    println!("       -H \"Content-Type: application/json\" \\");
    println!(
        "       -d '{{\"model\":\"claude-haiku-4-5-20251001\",\"messages\":\
[{{\"role\":\"user\",\"content\":\"reply with the single word OK\"}}],\"max_tokens\":20,\"stream\":true}}' \\"
    );
    println!("       http://{local_addr}/v1/messages");
    println!();
    println!("Press Ctrl-C to exit.");
    println!();

    // 5. Serve until SIGINT. axum::serve takes ownership of the listener
    // and runs the connection accept loop; the shutdown future fires on
    // Ctrl-C and the server drains in-flight requests.
    let shutdown = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %err, "ctrl_c handler failed; exiting anyway");
        }
        tracing::info!("Ctrl-C received; shutting down lifegw local smoke (real upstream)");
    };

    let serve_result = axum::serve(bind_listener, app)
        .with_graceful_shutdown(shutdown)
        .await;

    // 6. Tear down the in-process UDS server and the tempdir.
    let _ = uds_shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), uds_handle).await;
    drop(temp);

    serve_result.map_err(Into::into)
}
