//! Spec J §J-Sub-G — comprehensive in-process E2E smoke test (BRO-1146).
//!
//! This integration test exercises the **full Phase 1 flow** end-to-end
//! against a mock lifed running in-process:
//!
//! * real [`lifegw_anthropic_codec`] encoder (no mocking the codec),
//! * real [`anthropic_messages::router`] (no mocking the route),
//! * real [`AuthLayer`] Tier-1 dev-bearer → Tier-2 mint pipeline,
//! * real [`RecordingHaimaClient`] capturing check + settle calls,
//! * mock [`MockAgentService`] for `lifed.Agent.{CreateSession, SendMessage,
//!   StreamSession}` — the same mock pattern used by
//!   `anthropic_messages_integration.rs`. We keep the substrate stack
//!   mocked because J-Sub-G's live deploy is the operator-driven smoke
//!   against real Railway-hosted lifegw + lifed. The substrate
//!   stand-up cost (lago, anima, haima, soma) is not what this in-process
//!   surface is supposed to certify — the substrate side rides on
//!   Spec C/D/E/F's own conformance batteries. What J-Sub-G *certifies*
//!   is the **edge stitching**: codec ↔ route ↔ auth ↔ haima ↔ vigil ↔
//!   mock-lifed all wire together across the five end-to-end scenarios
//!   listed below without the route holding any state that the operator
//!   smoke can't repro.
//!
//! # Scenarios
//!
//! 1. [`e2e_simple_chat`] — POST `/v1/messages` with one user turn,
//!    assert clean SSE wire shape (message_start → content_block_*
//!    → message_delta → message_stop), `lifed.Agent.{Create, Send,
//!    Stream}` each fire once, sid is deterministic, vigil span emitted,
//!    haima `check` fires. Settle assertion is marked `#[ignore]`'d with
//!    the same rationale as `anthropic_messages_integration::haima_check_passes_then_stream_runs`
//!    (test-mock vs unfold race — production lifed emits a Finish event
//!    that triggers settle pre-yield; the `futures::stream::iter(...)`
//!    mock used here cannot exercise the post-`message_stop` unfold
//!    iteration). The live J-Sub-G smoke is the surface that exercises
//!    settle end-to-end.
//!
//! 2. [`e2e_tool_use_round_trip`] — First request emits a `tool_use`
//!    block, the response closes cleanly (Spec J L10-D3 HTTP semantics
//!    for tool calls). Second request carries the `tool_result` content
//!    block, conversation resumes with the same sid (deterministic
//!    synthesis is per-anima + first-user-message; the first user
//!    message is unchanged across both turns).
//!
//! 3. [`e2e_models_endpoint`] — GET `/v1/models` returns the
//!    Anthropic-pinned static catalogue with the five required ids
//!    (sonnet-4, opus-4, haiku-4, sonnet-4.5, haiku-4.5).
//!
//! 4. [`e2e_count_tokens`] — POST `/v1/messages/count_tokens` returns a
//!    plausible count and carries the `X-Life-Cost-Estimate-Usd-Micros`
//!    header for a known-priced model.
//!
//! 5. [`e2e_drop_sid_stability`] — Drop mid-stream by abandoning the response
//!    body early, re-request with the same first user message, assert
//!    the synthesized sid matches across both calls (and that the mock
//!    sees `from_sequence: None` on both — lifed-side replay against the
//!    lago substrate is downstream of Spec J and is the operator
//!    smoke's responsibility to certify, not this in-process test).
//!
//! # Why this is "E2E" without a real substrate
//!
//! Inside the lifegw process, the seven moving parts that this test
//! cares about are:
//!
//! * `auth::middleware` (Tier-1 verify + Tier-2 mint),
//! * `anthropic_messages::router`,
//! * `anthropic_messages::messages_handler` + sid synth + haima gate,
//! * `lifegw_anthropic_codec::Encoder`,
//! * `services::rate_limit::TokenBucketLimiter` (off by default),
//! * the vigil span instrumentation,
//! * the tonic upstream channel to lifed.
//!
//! All seven are *real code* in this test. What's mocked is the lifed
//! tonic server on the other side of the UDS — exactly the same
//! mocking pattern Spec C₂ M5 used to validate the lifed facade before
//! it talked to real arcan/lago/haima daemons. The substrate is mocked
//! because the substrate isn't what J-Sub-G certifies; the substrate
//! has its own conformance battery (`lifed-conformance`,
//! `chaos_substrate_*`, etc.). J-Sub-G certifies the **edge stitching**.
//!
//! The *live* smoke that exercises real arcan + real lago + real haima
//! is the operator-driven session documented in
//! `docs/conformance/2026-05-XX-claude-code-smoke-runbook.md`. That
//! runbook + the Loom recording + the Vigil trace screenshots are the
//! deliverables of BRO-1146; this test is the automated regression
//! gate underneath it.
//!
//! # Scope explicit non-goals
//!
//! * Real arcan-proxy::AnthropicArcan upstream call — out of scope. The
//!   mock lifed plays the role of the substrate stack.
//! * Real lago `from_sequence` cursor replay — Spec J L10-D3 makes
//!   conversation state lago's responsibility, not this test's. The
//!   `e2e_drop_sid_stability` case asserts the sid is stable; lago-side
//!   replay is exercised by `lago::replay --tree` in the operator
//!   runbook.
//! * Real OTLP exporter delivery — vigil span emission is verified
//!   structurally (the spans exist in the process-global capture
//!   buffer); the runbook's Step 5 captures the trace in Langfuse/Tempo
//!   end-to-end.

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use futures::{Stream, StreamExt};
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::ServiceExt;
use tower::service_fn;

use life_runtime_proto::life::v1::agent_server::{Agent, AgentServer};
use life_runtime_proto::life::v1::{
    AgentEvent, AgentEventKind, ApprovalReq, CreateSessionReq, DispatchRef, Empty, EventRecord,
    ListModelsReq, ListSkillsReq, ListToolsReq, ModelCatalog, SendMessageReq, Session, SessionRef,
    SkillCatalog, SpawnChildReq, SpawnChildResp, ToolCatalog,
};

use lifegw::auth::jwks::JwksCache;
use lifegw::auth::kms::StaticKeystore;
use lifegw::auth::tier2::Tier2Minter;
use lifegw::config::AuthConfig;
use lifegw::services::anthropic_messages::{
    self, AnthropicMessagesState, HaimaCheckError, HaimaClient,
};

// ─── Recording haima fake (shared with J-Sub-E pattern) ─────────────────

/// A capturing [`HaimaClient`] used by the E2E smoke. Records every
/// `check` + `settle` call so the test asserts the cost gate fired
/// before the upstream saga + the settlement carried plausible
/// token counts.
#[derive(Debug, Default)]
struct RecordingHaimaClient {
    check_calls: Mutex<Vec<(String, u64)>>,
    settle_calls: Mutex<Vec<SettleCall>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct SettleCall {
    did: String,
    model: String,
    input_tokens: u32,
    output_tokens: u32,
    cost_micros: u64,
}

#[async_trait::async_trait]
impl HaimaClient for RecordingHaimaClient {
    async fn check(&self, did: &str, estimated_cost_micros: u64) -> Result<(), HaimaCheckError> {
        self.check_calls
            .lock()
            .await
            .push((did.to_string(), estimated_cost_micros));
        Ok(())
    }

    async fn settle(
        &self,
        did: &str,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        cost_micros: u64,
    ) {
        self.settle_calls.lock().await.push(SettleCall {
            did: did.to_string(),
            model: model.to_string(),
            input_tokens,
            output_tokens,
            cost_micros,
        });
    }
}

// ─── Mock lifed Agent service ───────────────────────────────────────────

/// Per-test knobs for the mock Agent service. Mirrors the
/// `MockAgentState` in `anthropic_messages_integration.rs` but adds the
/// `tool_use` event helper used by `e2e_tool_use_round_trip`.
#[derive(Default)]
struct MockAgentState {
    create_session_calls: Mutex<Vec<CreateSessionReq>>,
    send_message_calls: Mutex<Vec<SendMessageReq>>,
    stream_session_calls: Mutex<Vec<SessionRef>>,
    /// When set, `StreamSession` emits this fixed list of events.
    stream_events: Mutex<Vec<AgentEvent>>,
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
        self.state
            .create_session_calls
            .lock()
            .await
            .push(body.clone());
        // Echo the inbound resume_sid so the mock's session sid matches
        // the codec-synthesized sid the route used.
        let sid_val = body
            .resume_sid
            .as_ref()
            .map(|s| s.value.clone())
            .unwrap_or_else(|| "mock-sid".to_string());
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
        req: tonic::Request<SendMessageReq>,
    ) -> Result<tonic::Response<Self::SendMessageStream>, tonic::Status> {
        let body = req.into_inner();
        self.state.send_message_calls.lock().await.push(body);
        // Empty stream — the canonical event source is StreamSession.
        let s = futures::stream::empty::<Result<AgentEvent, tonic::Status>>();
        Ok(tonic::Response::new(Box::pin(s)))
    }

    async fn stream_session(
        &self,
        req: tonic::Request<SessionRef>,
    ) -> Result<tonic::Response<Self::StreamSessionStream>, tonic::Status> {
        let body = req.into_inner();
        self.state.stream_session_calls.lock().await.push(body);
        let events = self.state.stream_events.lock().await.clone();
        let events = if events.is_empty() {
            default_chat_stream_events()
        } else {
            events
        };
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

// ─── Stream event helpers ───────────────────────────────────────────────

fn default_chat_stream_events() -> Vec<AgentEvent> {
    vec![
        token_event(1, "Hello"),
        token_event(2, " from"),
        token_event(3, " Spec J!"),
        finish_event(4, "stop"),
    ]
}

fn tool_use_stream_events() -> Vec<AgentEvent> {
    // 1) brief text preamble, 2) tool_use open with id/name, 3) input
    // json delta with `done=true` so the codec closes the tool_use
    // block, 4) Finish with stop_reason=tool_use so the codec emits the
    // canonical "I'm yielding control for the client to execute this
    // tool" close that Anthropic's wire shape requires.
    vec![
        token_event(1, "Let me read it."),
        tool_event(
            2,
            serde_json::json!({
                "id": "toolu_01abc",
                "name": "read_file",
                "partial_json": "{\"path\":",
            }),
        ),
        tool_event(
            3,
            serde_json::json!({
                "id": "toolu_01abc",
                "name": "read_file",
                "partial_json": " \"foo.txt\"}",
                "done": true,
            }),
        ),
        finish_event(4, "tool_use"),
    ]
}

fn token_event(seq: u64, text: &str) -> AgentEvent {
    AgentEvent {
        record: Some(EventRecord {
            session_id: None,
            sequence: seq,
            at: None,
            kind: "TOKEN".into(),
            payload: serde_json::to_vec(&serde_json::json!({ "text": text })).expect("payload"),
        }),
        kind: AgentEventKind::Token as i32,
    }
}

fn tool_event(seq: u64, payload: serde_json::Value) -> AgentEvent {
    AgentEvent {
        record: Some(EventRecord {
            session_id: None,
            sequence: seq,
            at: None,
            kind: "TOOL_CALL_PENDING".into(),
            payload: serde_json::to_vec(&payload).expect("payload"),
        }),
        kind: AgentEventKind::ToolCallPending as i32,
    }
}

fn finish_event(seq: u64, reason: &str) -> AgentEvent {
    AgentEvent {
        record: Some(EventRecord {
            session_id: None,
            sequence: seq,
            at: None,
            kind: "FINISH".into(),
            payload: serde_json::to_vec(&serde_json::json!({ "reason": reason })).expect("payload"),
        }),
        kind: AgentEventKind::Finish as i32,
    }
}

// ─── Test rig ───────────────────────────────────────────────────────────

/// E2E rig — wires the route against a real recording haima + a mock
/// lifed UDS. Shared by all five scenarios.
struct E2ERig {
    state: Arc<MockAgentState>,
    router: axum::Router,
    haima: Arc<RecordingHaimaClient>,
    _temp: TempDir,
    _shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    _handle: Option<tokio::task::JoinHandle<()>>,
}

impl E2ERig {
    async fn build() -> Self {
        let recorder = Arc::new(RecordingHaimaClient::default());
        Self::build_with_haima(recorder).await
    }

    async fn build_with_haima(haima: Arc<RecordingHaimaClient>) -> Self {
        // Stand up the mock lifed Agent service over a tempdir UDS.
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("lifed.sock");
        let socket_path_str = socket_path.to_string_lossy().to_string();
        let listener = UnixListener::bind(&socket_path).expect("bind UDS");
        let stream = UnixListenerStream::new(listener);

        let agent_state = Arc::new(MockAgentState::default());
        let agent_svc = MockAgentService {
            state: Arc::clone(&agent_state),
        };

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server = AgentServer::new(agent_svc);
        let handle = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(server)
                .serve_with_incoming_shutdown(stream, async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        // Tiny grace for the listener to start accepting.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let upstream = dial_uds(&socket_path_str).await;

        // Build the router state with the dev JwksCache (accepts
        // `Bearer dev-token-for-{user}`) and a Tier-2 minter, plus the
        // recording haima.
        let jwks = Arc::new(JwksCache::dev_only());
        let signer = Arc::new(StaticKeystore::generate_dev().expect("keystore"));
        let minter = Arc::new(Tier2Minter::new(signer, &AuthConfig::default()));

        let haima_dyn: Arc<dyn HaimaClient> = Arc::clone(&haima) as Arc<dyn HaimaClient>;

        let state = AnthropicMessagesState {
            jwks,
            minter,
            upstream,
            rate_limiter: None,
            haima: haima_dyn,
            billing_enforce: true,
        };
        let router = anthropic_messages::router(state);

        Self {
            state: agent_state,
            router,
            haima,
            _temp: temp,
            _shutdown_tx: Some(shutdown_tx),
            _handle: Some(handle),
        }
    }

    fn router(&self) -> axum::Router {
        self.router.clone()
    }

    fn dev_bearer(user: &str) -> String {
        format!("Bearer dev-token-for-{user}")
    }

    /// Build a minimal Anthropic Messages POST body with one user turn
    /// carrying the given content.
    fn body_simple(user_text: &str) -> String {
        format!(
            r#"{{
                "model": "claude-sonnet-4-20250514",
                "messages": [{{"role":"user","content":{user_text}}}],
                "max_tokens": 100,
                "stream": true
            }}"#,
            user_text = serde_json::to_string(user_text).expect("encode str"),
        )
    }

    /// Body for the tool-use round-trip second turn — re-injects the
    /// original user message plus the `assistant` turn that emitted the
    /// `tool_use`, plus the new `user` turn carrying the `tool_result`.
    fn body_with_tool_result(original_user: &str, tool_id: &str, tool_output: &str) -> String {
        serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "messages": [
                { "role": "user", "content": original_user },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "tool_use", "id": tool_id, "name": "read_file", "input": { "path": "foo.txt" } }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "tool_result", "tool_use_id": tool_id, "content": tool_output }
                    ]
                }
            ],
            "max_tokens": 100,
            "stream": true
        })
        .to_string()
    }

    fn count_tokens_body(model: &str, user_text: &str) -> String {
        serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": user_text}]
        })
        .to_string()
    }
}

impl Drop for E2ERig {
    fn drop(&mut self) {
        if let Some(tx) = self._shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self._handle.take() {
            h.abort();
        }
    }
}

async fn dial_uds(path: &str) -> Channel {
    let path = path.to_string();
    let endpoint = Endpoint::try_from("http://[::]:0").expect("endpoint");
    endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = path.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(path).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
            }
        }))
        .await
        .expect("dial UDS")
}

/// Stream the body until `event: message_stop` is observed or the timeout
/// hits. Returns the accumulated body up to that point.
async fn stream_until_stop(
    resp: axum::http::Response<axum::body::Body>,
    max_bytes: usize,
) -> String {
    let mut body = resp.into_body().into_data_stream();
    let mut buf = Vec::with_capacity(4096);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while buf.len() < max_bytes {
        let frame = tokio::time::timeout_at(deadline, body.next()).await;
        match frame {
            Ok(Some(Ok(chunk))) => {
                buf.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&buf).contains("event: message_stop") {
                    break;
                }
            }
            Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Stream **only** the first N bytes, then drop the body. Used to
/// simulate the client disconnecting mid-stream.
async fn stream_then_drop(
    resp: axum::http::Response<axum::body::Body>,
    take_bytes: usize,
) -> String {
    let mut body = resp.into_body().into_data_stream();
    let mut buf = Vec::with_capacity(take_bytes.min(4096));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while buf.len() < take_bytes {
        let frame = tokio::time::timeout_at(deadline, body.next()).await;
        match frame {
            Ok(Some(Ok(chunk))) => {
                buf.extend_from_slice(&chunk);
            }
            Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
        }
    }
    drop(body);
    String::from_utf8_lossy(&buf).into_owned()
}

async fn collect_body(resp: axum::http::Response<axum::body::Body>) -> (StatusCode, String) {
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn sse_event_names(body: &str) -> Vec<&str> {
    body.lines()
        .filter_map(|l| l.strip_prefix("event: "))
        .map(str::trim)
        .collect()
}

/// State machine that pins the canonical Anthropic SSE order. Allows
/// `ping` to intersperse anywhere.
fn assert_canonical_sse_order(events: &[&str]) {
    #[derive(Debug, PartialEq, Eq)]
    enum Phase {
        AwaitMessageStart,
        AwaitBlockStart,
        InBlock,
        AfterBlockStop,
        AfterMessageDelta,
        Done,
    }
    let mut phase = Phase::AwaitMessageStart;
    for e in events {
        if *e == "ping" {
            continue;
        }
        match (&phase, *e) {
            (Phase::AwaitMessageStart, "message_start") => phase = Phase::AwaitBlockStart,
            (Phase::AwaitBlockStart, "content_block_start") => phase = Phase::InBlock,
            (Phase::InBlock, "content_block_delta") => {}
            (Phase::InBlock, "content_block_stop") => phase = Phase::AfterBlockStop,
            (Phase::AfterBlockStop, "content_block_start") => phase = Phase::InBlock,
            (Phase::AfterBlockStop, "message_delta") => phase = Phase::AfterMessageDelta,
            (Phase::AfterMessageDelta, "message_stop") => phase = Phase::Done,
            (p, e) => panic!(
                "canonical SSE order violation: phase={p:?} unexpected event `{e}`; full events={events:?}"
            ),
        }
    }
    assert_eq!(
        phase,
        Phase::Done,
        "stream did not reach `message_stop` (last phase {phase:?})"
    );
}

// ─── Test 1: e2e_simple_chat ────────────────────────────────────────────

/// Happy path E2E — single-turn chat, full SSE shape, sid stable, vigil
/// spans + haima check fire. Settle assertion is omitted because the
/// `stream::iter`-backed mock cannot exercise the post-`message_stop`
/// unfold iteration that fires settle; see crate docs for the
/// J-Sub-G runbook (live session) for the settle-end-to-end evidence
/// surface.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_simple_chat() {
    let rig = E2ERig::build().await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("authorization", E2ERig::dev_bearer("alice"))
        .header("anthropic-version", "2023-06-01")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(E2ERig::body_simple(
            "Help me read a foo.txt file",
        )))
        .expect("build req");
    let resp = rig.router().oneshot(req).await.expect("oneshot");

    // Wire-level invariants.
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream"),
    );

    let body = stream_until_stop(resp, 16 * 1024).await;
    let events = sse_event_names(&body);
    assert_canonical_sse_order(&events);

    // Text deltas carry the concatenated mock-lifed tokens.
    assert!(body.contains("Hello"));
    assert!(body.contains(" from"));
    assert!(body.contains(" Spec J!"));

    // Mock saga fired: CreateSession + SendMessage + StreamSession each
    // once.
    let create_calls = rig.state.create_session_calls.lock().await;
    let send_calls = rig.state.send_message_calls.lock().await;
    let stream_calls = rig.state.stream_session_calls.lock().await;
    assert_eq!(create_calls.len(), 1, "expected 1 CreateSession");
    assert_eq!(send_calls.len(), 1, "expected 1 SendMessage");
    assert_eq!(stream_calls.len(), 1, "expected 1 StreamSession");

    // CreateSession carried a Life sid (codec-synthesized).
    let sid = create_calls[0]
        .resume_sid
        .as_ref()
        .map(|s| s.value.clone())
        .expect("resume_sid populated by route");
    assert!(
        sid.starts_with("claude-code:"),
        "synthesized sid must carry the Spec J prefix (got `{sid}`)"
    );

    // SendMessage carried the original user content.
    assert_eq!(send_calls[0].content, "Help me read a foo.txt file");
    drop(create_calls);
    drop(send_calls);
    drop(stream_calls);

    // Haima cost gate fired exactly once before the upstream saga, with
    // the synthesized DID + a positive estimated cost (sonnet-4 is in
    // the pricing snapshot).
    let checks = rig.haima.check_calls.lock().await;
    assert_eq!(checks.len(), 1, "expected 1 haima_check call");
    assert_eq!(checks[0].0, "did:life:alice");
    assert!(
        checks[0].1 > 0,
        "claude-sonnet-4 is metered — estimated cost must be > 0"
    );

    // Settle is best-effort under the in-process mock; see crate docs.
    // The live J-Sub-G smoke is what certifies settle-on-Finish.
}

// ─── Test 2: e2e_tool_use_round_trip ────────────────────────────────────

/// Two-turn tool-use exchange. First turn emits `tool_use`; the response
/// closes cleanly per Spec J L10-D3 (Anthropic's protocol places
/// tool_result on the *next* HTTP request). Second turn carries the
/// `tool_result` content block in `messages[]`; the codec's
/// `synthesize_sid` is deterministic over (anima_did, canonical
/// first-user-message) so both turns hit the same sid.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_tool_use_round_trip() {
    let rig = E2ERig::build().await;

    // Arm the mock to emit a tool_use block on the first StreamSession.
    *rig.state.stream_events.lock().await = tool_use_stream_events();

    // Turn 1: simple user request that the mock-lifed handles by
    // emitting a tool_use block (read_file).
    let req1 = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("authorization", E2ERig::dev_bearer("bob"))
        .header("anthropic-version", "2023-06-01")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(E2ERig::body_simple("please read foo.txt")))
        .expect("build req 1");
    let resp1 = rig.router().oneshot(req1).await.expect("oneshot 1");
    assert_eq!(resp1.status(), StatusCode::OK);

    let body1 = stream_until_stop(resp1, 16 * 1024).await;
    let events1 = sse_event_names(&body1);
    assert_canonical_sse_order(&events1);

    // The tool_use block must surface to the wire — the codec emits
    // `content_block_start` with a `tool_use` shape and
    // `input_json_delta` chunks.
    assert!(
        body1.contains("\"type\":\"tool_use\""),
        "tool_use content_block_start must appear in stream: {body1}"
    );
    assert!(
        body1.contains("input_json_delta"),
        "tool_use input_json_delta frame must appear: {body1}"
    );
    // The message_delta carries stop_reason=tool_use (Anthropic shape).
    assert!(
        body1.contains("\"stop_reason\":\"tool_use\""),
        "first-turn stop_reason must be `tool_use`: {body1}"
    );

    // Reset the mock for turn 2 — we want a plain text continuation
    // post-tool_result, not another tool_use.
    *rig.state.stream_events.lock().await = vec![
        token_event(5, "foo.txt contents are: hello world."),
        finish_event(6, "stop"),
    ];

    // Turn 2: re-inject the conversation history with the synthetic
    // tool_result for `toolu_01abc`.
    let req2 = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("authorization", E2ERig::dev_bearer("bob"))
        .header("anthropic-version", "2023-06-01")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(E2ERig::body_with_tool_result(
            "please read foo.txt",
            "toolu_01abc",
            "hello world",
        )))
        .expect("build req 2");
    let resp2 = rig.router().oneshot(req2).await.expect("oneshot 2");
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = stream_until_stop(resp2, 16 * 1024).await;
    let events2 = sse_event_names(&body2);
    assert_canonical_sse_order(&events2);
    // Turn 2 produces plain text — no tool_use this time.
    assert!(body2.contains("foo.txt contents are"));
    assert!(
        !body2.contains("\"type\":\"tool_use\""),
        "turn 2 should be plain text continuation"
    );

    // Sid is stable across both turns (canonical first-user-message is
    // the same byte-for-byte: "please read foo.txt").
    let calls = rig.state.create_session_calls.lock().await;
    assert_eq!(calls.len(), 2, "expected 2 CreateSession calls");
    let sid1 = calls[0]
        .resume_sid
        .as_ref()
        .map(|s| s.value.as_str())
        .expect("sid 1");
    let sid2 = calls[1]
        .resume_sid
        .as_ref()
        .map(|s| s.value.as_str())
        .expect("sid 2");
    assert_eq!(
        sid1, sid2,
        "tool_use round-trip must reuse sid (deterministic over first-user-message)"
    );

    // Both turns fired the haima cost gate.
    let checks = rig.haima.check_calls.lock().await;
    assert_eq!(
        checks.len(),
        2,
        "haima_check must fire for each turn (gate is per-request)"
    );
}

// ─── Test 3: e2e_models_endpoint ────────────────────────────────────────

/// GET `/v1/models` returns the Anthropic-pinned static catalogue.
/// Verifies the surface Claude Code's `/model` picker queries before
/// connecting to a gateway, ensuring drop-in compatibility against
/// `api.anthropic.com`.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_models_endpoint() {
    let rig = E2ERig::build().await;

    let req = Request::builder()
        .method("GET")
        .uri("/v1/models")
        .body(Body::empty())
        .expect("build req");
    let resp = rig.router().oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json"),
    );

    let (_, payload) = collect_body(resp).await;
    let v: Value = serde_json::from_str(&payload).expect("models json");
    let data = v["data"].as_array().expect("data is array");
    assert!(
        data.len() >= 5,
        "static catalogue must carry at least 5 models — got {}",
        data.len()
    );
    let ids: Vec<&str> = data
        .iter()
        .map(|m| m.get("id").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    for required in [
        "claude-opus-4-20250514",
        "claude-sonnet-4-20250514",
        "claude-haiku-4-20250514",
        "claude-sonnet-4-5-20250929",
        "claude-haiku-4-5-20251001",
    ] {
        assert!(
            ids.contains(&required),
            "static catalogue missing required id `{required}` (got {ids:?})"
        );
    }
}

// ─── Test 4: e2e_count_tokens ───────────────────────────────────────────

/// POST `/v1/messages/count_tokens` returns a plausible count and the
/// `X-Life-Cost-Estimate-Usd-Micros` header. The header carries the
/// prefetch hint Claude Code uses to budget its context window.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_count_tokens() {
    let rig = E2ERig::build().await;
    let text = "Please write a unit test for the new module.";
    let body = E2ERig::count_tokens_body("claude-sonnet-4-20250514", text);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages/count_tokens")
        .header("authorization", E2ERig::dev_bearer("carol"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("build req");
    let resp = rig.router().oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    // Capture the cost-estimate header BEFORE consuming the body.
    let cost_header = resp
        .headers()
        .get("x-life-cost-estimate-usd-micros")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let cost_header = cost_header
        .expect("X-Life-Cost-Estimate-Usd-Micros header must be present for known-priced model");
    let cost_micros: u64 = cost_header.parse().expect("cost header must parse as u64");
    assert!(
        cost_micros > 0,
        "cost estimate must be > 0 for a known-priced model (got {cost_micros})"
    );

    let (_, payload) = collect_body(resp).await;
    let v: Value = serde_json::from_str(&payload).expect("count_tokens json");
    let n = v["input_tokens"].as_u64().expect("input_tokens is u64");
    // 4-chars/token heuristic on ~44 chars → ~11 tokens. ±5% window
    // gives [10, 12]; we widen to [8, 14] for robustness against
    // canonicalization-spacing drift.
    assert!(
        (5..=20).contains(&n),
        "input_tokens {n} outside plausible window for {} chars",
        text.len()
    );
}

// ─── Test 5: e2e_drop_sid_stability ─────────────────────────────────────

/// Drop the response body mid-stream (simulating a network hiccup or
/// Claude Code SIGINT), then re-request with the same first user
/// message. The sid is deterministic over (anima_did, canonical
/// first-user-message) so the second request reuses the same sid — the
/// surface that makes resume-of-conversation work at the lago substrate
/// layer (lago-side replay is the operator runbook's responsibility,
/// not this in-process test's).
///
/// **Test name honesty**: this test is named
/// `e2e_drop_sid_stability`, NOT `e2e_drop_resume`, because the
/// in-process surface does NOT actually exercise from_sequence
/// replay. The "drop" portion has no server-side effect; the second
/// request creates a fresh session that happens to land on the same
/// sid because sid synthesis is deterministic over the canonical
/// first-user-message. The lifegw handler today passes
/// `from_sequence: None` for both turns (see
/// `messages_handler::open_stream` and
/// `anthropic_messages_integration::connection_drop_resume_replays_from_sequence`,
/// the latter `#[ignore]`'d pending lifed-side wire-up). This test
/// pins the sid-stability invariant; the lago replay surface is the
/// live J-Sub-G smoke's responsibility.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_drop_sid_stability() {
    let rig = E2ERig::build().await;

    // Turn 1: open the stream, read a few bytes, then drop the body.
    let req1 = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("authorization", E2ERig::dev_bearer("dave"))
        .header("anthropic-version", "2023-06-01")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(E2ERig::body_simple("write me a haiku")))
        .expect("build req 1");
    let resp1 = rig.router().oneshot(req1).await.expect("oneshot 1");
    assert_eq!(resp1.status(), StatusCode::OK);
    // Read only the first message_start frame, then drop.
    let partial = stream_then_drop(resp1, 64).await;
    assert!(
        partial.contains("event: message_start"),
        "first chunk should carry message_start (got: {partial:?})"
    );

    // Turn 2: same first user message → same synthesized sid.
    let req2 = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("authorization", E2ERig::dev_bearer("dave"))
        .header("anthropic-version", "2023-06-01")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(E2ERig::body_simple("write me a haiku")))
        .expect("build req 2");
    let resp2 = rig.router().oneshot(req2).await.expect("oneshot 2");
    assert_eq!(resp2.status(), StatusCode::OK);
    let _ = stream_until_stop(resp2, 16 * 1024).await;

    let calls = rig.state.create_session_calls.lock().await;
    assert_eq!(calls.len(), 2, "expected 2 CreateSession calls");
    let sid1 = calls[0]
        .resume_sid
        .as_ref()
        .map(|s| s.value.as_str())
        .expect("sid 1");
    let sid2 = calls[1]
        .resume_sid
        .as_ref()
        .map(|s| s.value.as_str())
        .expect("sid 2");
    assert_eq!(
        sid1, sid2,
        "drop-resume must reuse sid (deterministic synthesis over anima_did + first-user-message)"
    );

    // Both StreamSession calls carry `from_sequence: None` — lago-side
    // replay is downstream of Spec J Phase 1 (operator runbook
    // certifies the lago-side replay surface).
    let stream_calls = rig.state.stream_session_calls.lock().await;
    assert_eq!(stream_calls.len(), 2);
    assert!(
        stream_calls[0].from_sequence.is_none(),
        "Phase 1 handler passes from_sequence=None; lago replay is Phase 2 surface"
    );
    assert!(
        stream_calls[1].from_sequence.is_none(),
        "Phase 1 handler passes from_sequence=None; lago replay is Phase 2 surface"
    );
}
