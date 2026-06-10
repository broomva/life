//! BRO-1165 — local interactive smoke for lifegw (zero deploy, zero Railway).
//!
//! Boots lifegw's Anthropic Messages router (and siblings: `/v1/models`,
//! `/v1/messages/count_tokens`) on a kernel-picked `127.0.0.1` port, wired
//! against an in-process mock lifed `Agent` service over a tempdir UDS. The
//! operator runs:
//!
//! ```sh
//! cargo run -p lifegw --example local_smoke
//! ```
//!
//! …and gets a URL + dev bearer printed to stdout. `curl`, `claude`, or any
//! Anthropic-compatible client can then exercise the route against a real
//! TCP socket, end to end, with real Vigil span emission and the real codec
//! pipeline.
//!
//! # Why this exists
//!
//! Spec J Phase 1 ships three test paths (`docs/STATUS.md` §J-Sub-G):
//!
//! 1. In-process E2E test (`cargo test -p lifegw --test spec_j_e2e_smoke`).
//! 2. Railway staging deploy per the operator runbook
//!    (`docs/conformance/2026-05-18-claude-code-smoke-runbook.md`).
//! 3. **This example** — local interactive smoke, ~30 s setup, no Railway
//!    slot burned.
//!
//! Path 3 is what an iterating engineer reaches for when validating a tweak
//! to `services/anthropic_messages.rs`, the codec, or the Vigil spans. The
//! full Railway deploy path is what produces the Phase 1 conformance
//! evidence (Loom + Vigil traces + lago replay).
//!
//! # What's mocked, what's real
//!
//! Real (in-process, same code as production):
//!
//! - the axum router built from `AnthropicMessagesState`,
//! - the `lifegw_anthropic_codec::Encoder`,
//! - the `JwksCache::dev_only()` Tier-1 verifier (dev-bearer shortcut),
//! - the `Tier2Minter` capability minter,
//! - the `TokenBucketLimiter` rate limiter,
//! - the tonic Channel dialled to the mock lifed UDS,
//! - the `StubHaimaClient` cost gate (no-op, mirrors production today).
//!
//! Mocked (over UDS, by this file):
//!
//! - `lifed.life.v1.Agent` — `CreateSession`, `SendMessage`, `StreamSession`
//!   produce a canned four-token chat reply per request, matching the
//!   pattern used by `tests/spec_j_e2e_smoke.rs`.
//!
//! Substrate-free per Spec J L10-D1 — no real arcan/lago/anima/haima deps.
//! Per the BRO-1165 scope-discipline rule, the mock is duplicated from
//! `tests/spec_j_e2e_smoke.rs` (option 2) rather than refactored into a
//! shared module: dev tooling has no business widening lifegw's public
//! `src/` surface.
//!
//! # Lifecycle
//!
//! 1. Build a tempdir + UDS path `<tempdir>/lifed.sock`.
//! 2. Spawn a tonic `Server` hosting the mock `Agent` on that UDS.
//! 3. Dial a tonic `Channel` back over the UDS.
//! 4. Build `AnthropicMessagesState` with real auth + rate-limit + stub
//!    haima + the mock-backed channel.
//! 5. Bind axum on `127.0.0.1:0` (kernel-picked port).
//! 6. Print the URL + dev bearer + curl recipes.
//! 7. Serve until `SIGINT` (Ctrl-C).
//! 8. Graceful drain: shut down axum, then the mock UDS server, then drop
//!    the tempdir.

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

use life_runtime_proto::life::v1::agent_server::{Agent, AgentServer};
use life_runtime_proto::life::v1::{
    AgentEvent, AgentEventKind, ApprovalReq, CreateSessionReq, DispatchRef, Empty, EventRecord,
    ListModelsReq, ListSkillsReq, ListToolsReq, ModelCatalog, SendMessageReq, Session, SessionRef,
    SkillCatalog, SpawnChildReq, SpawnChildResp, ToolCatalog,
};

use lifegw::auth::jwks::JwksCache;
use lifegw::auth::kms::StaticKeystore;
use lifegw::auth::tier2::Tier2Minter;
use lifegw::config::{AuthConfig, RateLimitConfig};
use lifegw::services::anthropic_messages::{
    self, AnthropicMessagesState, HaimaClient, StubHaimaClient,
};
use lifegw::services::rate_limit::TokenBucketLimiter;

// ─── Mock lifed Agent service ───────────────────────────────────────────
//
// Duplicated from `tests/spec_j_e2e_smoke.rs` (option 2 per BRO-1165 scope
// discipline). Kept intentionally minimal: a single fixed token stream
// (`Hello`, ` from`, ` lifegw`, ` local`, ` smoke!`) closing with a
// `stop` Finish. Tool-use round-trips and richer scenarios stay in the
// test suite — operators wanting to exercise those drive the example
// against the codec via the real `/v1/messages` route.

#[derive(Default)]
struct MockAgentState {
    /// Cumulative session-id counter so successive `CreateSession` calls
    /// echo distinct sids when the caller doesn't supply a `resume_sid`.
    sequence: Mutex<u64>,
}

#[derive(Clone)]
struct MockAgentService {
    state: Arc<MockAgentState>,
}

#[tonic::async_trait]
impl Agent for MockAgentService {
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
                let mut seq = self.state.sequence.lock().await;
                *seq += 1;
                format!("mock-sid-{}", *seq)
            }
        };
        Ok(tonic::Response::new(Session {
            sid: Some(aios_proto::aios::v1::SessionId { value: sid_val }),
            agent_id: Some(aios_proto::aios::v1::AgentId {
                value: "mock-agent".to_string(),
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
        Err(tonic::Status::unimplemented("mock"))
    }

    async fn close_session(
        &self,
        _: tonic::Request<SessionRef>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        Ok(tonic::Response::new(Empty {}))
    }

    async fn send_message(
        &self,
        _req: tonic::Request<SendMessageReq>,
    ) -> Result<tonic::Response<Self::SendMessageStream>, tonic::Status> {
        // Empty — the canonical event source is StreamSession (matches
        // the in-process E2E test pattern).
        let s = futures::stream::empty::<Result<AgentEvent, tonic::Status>>();
        Ok(tonic::Response::new(Box::pin(s)))
    }

    async fn stream_session(
        &self,
        _req: tonic::Request<SessionRef>,
    ) -> Result<tonic::Response<Self::StreamSessionStream>, tonic::Status> {
        let events = default_chat_stream_events();
        let s = futures::stream::iter(events.into_iter().map(Ok::<_, tonic::Status>));
        Ok(tonic::Response::new(Box::pin(s)))
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
        Err(tonic::Status::unimplemented("mock"))
    }

    async fn list_models(
        &self,
        _: tonic::Request<ListModelsReq>,
    ) -> Result<tonic::Response<ModelCatalog>, tonic::Status> {
        Err(tonic::Status::unimplemented("mock"))
    }

    async fn list_tools(
        &self,
        _: tonic::Request<ListToolsReq>,
    ) -> Result<tonic::Response<ToolCatalog>, tonic::Status> {
        Err(tonic::Status::unimplemented("mock"))
    }

    async fn spawn_child(
        &self,
        _: tonic::Request<SpawnChildReq>,
    ) -> Result<tonic::Response<SpawnChildResp>, tonic::Status> {
        Err(tonic::Status::unimplemented("mock"))
    }
}

fn default_chat_stream_events() -> Vec<AgentEvent> {
    vec![
        token_event(1, "Hello"),
        token_event(2, " from"),
        token_event(3, " lifegw"),
        token_event(4, " local"),
        token_event(5, " smoke!"),
        finish_event(6, "stop"),
    ]
}

fn token_event(seq: u64, text: &str) -> AgentEvent {
    AgentEvent {
        record: Some(EventRecord {
            session_id: None,
            sequence: seq,
            at: None,
            kind: "TOKEN".into(),
            payload: serde_json::to_vec(&serde_json::json!({ "text": text }))
                .expect("encode token payload"),
        }),
        kind: AgentEventKind::Token as i32,
    }
}

fn finish_event(seq: u64, reason: &str) -> AgentEvent {
    AgentEvent {
        record: Some(EventRecord {
            session_id: None,
            sequence: seq,
            at: None,
            kind: "FINISH".into(),
            payload: serde_json::to_vec(&serde_json::json!({ "reason": reason }))
                .expect("encode finish payload"),
        }),
        kind: AgentEventKind::Finish as i32,
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
        .expect("dial mock lifed UDS")
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

    // 1. Stand up the mock lifed Agent service over a tempdir UDS.
    let temp: TempDir = tempfile::tempdir()?;
    let socket_path = temp.path().join("lifed.sock");
    let socket_path_str = socket_path.to_string_lossy().to_string();
    let listener = UnixListener::bind(&socket_path)?;
    let uds_stream = UnixListenerStream::new(listener);

    tracing::info!(uds = %socket_path_str, "mock lifed UDS bound");

    let agent_state = Arc::new(MockAgentState::default());
    let agent_svc = MockAgentService {
        state: Arc::clone(&agent_state),
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
            tracing::warn!(error = %err, "mock lifed server exited with error");
        }
    });

    // Tiny grace for the listener to start accepting (mirrors the test rig
    // — connect_with_connector retries once but a brief sleep makes the
    // first dial deterministic).
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 2. Dial the mock lifed over UDS.
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
    let app = anthropic_messages::router(state);

    // 4. Bind axum on 127.0.0.1:0 — kernel picks the port so concurrent
    // example runs don't collide.
    let bind_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let local_addr = bind_listener.local_addr()?;

    let dev_bearer = "dev-token-for-broomva";

    println!();
    println!("lifegw local smoke ready:");
    println!("  URL:    http://{local_addr}");
    println!("  Bearer: {dev_bearer}");
    println!();
    println!("Try:");
    println!("  curl http://{local_addr}/v1/models | jq .");
    println!();
    println!("  curl -N -H \"Authorization: Bearer {dev_bearer}\" \\");
    println!("       -H \"anthropic-version: 2023-06-01\" \\");
    println!("       -H \"Content-Type: application/json\" \\");
    println!(
        "       -d '{{\"model\":\"claude-sonnet-4-20250514\",\"messages\":\
[{{\"role\":\"user\",\"content\":\"hello\"}}],\"max_tokens\":100,\"stream\":true}}' \\"
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
        tracing::info!("Ctrl-C received; shutting down lifegw local smoke");
    };

    let serve_result = axum::serve(bind_listener, app)
        .with_graceful_shutdown(shutdown)
        .await;

    // 6. Tear down the mock UDS server and the tempdir.
    let _ = uds_shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), uds_handle).await;
    drop(temp);

    serve_result.map_err(Into::into)
}
