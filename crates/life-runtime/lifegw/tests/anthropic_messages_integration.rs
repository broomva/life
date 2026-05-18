//! Integration tests for `services::anthropic_messages` (Spec J §J-Sub-B).
//!
//! Brings up:
//!
//! 1. A mock lifed `Agent` service over a tempdir UDS — the test
//!    controls what `CreateSession` / `SendMessage` / `StreamSession`
//!    return without standing up real arcan/lago/anima/haima
//!    substrates. The `Agent.StreamSession` body is the surface most
//!    of the assertions read.
//! 2. The `AnthropicMessagesState` directly — same handles + Channel
//!    shape `bootstrap.rs` builds at runtime. We `oneshot` the axum
//!    router with synthetic `POST /v1/messages` bodies; we do NOT
//!    stand up a TLS listener (the route handler is independent of
//!    transport — TLS is verified separately by Sub-phase B's lifegw
//!    end-to-end test rig).
//!
//! Tests cover:
//!
//! * `simple_chat_completion` — happy path, end-to-end SSE shape.
//! * `multi_turn_no_tools` — sid stability across two HTTP turns with
//!   the same first user message.
//! * `auth_missing_returns_401` / `auth_invalid_returns_401` — Tier-1
//!   verification gates.
//! * `unknown_anthropic_version_returns_400` — Spec J L10-D5 strictness.
//! * `rate_limit_engaged_returns_429` — upstream `ResourceExhausted`
//!   maps to HTTP 429 + Anthropic-shape `rate_limit_error` body.
//! * `connection_drop_resume` — re-requesting with the same body
//!   resolves the same sid (deterministic sid synthesis).
//! * `large_request_body` — 100K-character payload doesn't blow up
//!   the streaming parser.
//!
//! Scope caveats (mirroring Spec J §J-Sub-B):
//!
//! * **Tool use** — not exercised here. The mock lifed emits only
//!   Token + Finish events; ToolCallPending wiring is J-Sub-D's
//!   surface. Tests stick to text-only flows.
//! * **Real Anthropic upstream** — `arcan-proxy::AnthropicArcan` is
//!   not invoked. The mock lifed plays the role of the entire
//!   substrate stack. The adapter is tested under `arcan-proxy/src/anthropic.rs`.

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
use tonic::transport::{Endpoint, Uri};
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
use lifegw::services::anthropic_messages::{self, AnthropicMessagesState};

// ─── Mock lifed Agent service ───────────────────────────────────────────

/// Per-test knobs for the mock Agent service.
#[derive(Default)]
struct MockAgentState {
    /// Records of every `CreateSession` call.
    create_session_calls: Mutex<Vec<CreateSessionReq>>,
    /// Records of every `SendMessage` call.
    send_message_calls: Mutex<Vec<SendMessageReq>>,
    /// Records of every `StreamSession` call.
    stream_session_calls: Mutex<Vec<SessionRef>>,
    /// When set, `CreateSession` returns the given status instead of
    /// the canned happy-path response.
    force_create_status: Mutex<Option<tonic::Status>>,
    /// When set, `SendMessage` returns the given status.
    force_send_status: Mutex<Option<tonic::Status>>,
    /// When set, `StreamSession` returns the given status.
    force_stream_status: Mutex<Option<tonic::Status>>,
    /// When set, `StreamSession` emits this fixed list of events instead
    /// of the default minimal Token+Finish pair.
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
        if let Some(s) = self.state.force_create_status.lock().await.take() {
            return Err(s);
        }
        // Echo the inbound resume_sid (the route always sets it from
        // synthesize_sid) so the response sid matches what the codec
        // saw and downstream assertions stay tight.
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
        if let Some(s) = self.state.force_send_status.lock().await.take() {
            return Err(s);
        }
        // Empty stream — the route drops SendMessage events on the
        // floor; the canonical event source is StreamSession.
        let s = futures::stream::empty::<Result<AgentEvent, tonic::Status>>();
        Ok(tonic::Response::new(Box::pin(s)))
    }

    async fn stream_session(
        &self,
        req: tonic::Request<SessionRef>,
    ) -> Result<tonic::Response<Self::StreamSessionStream>, tonic::Status> {
        let body = req.into_inner();
        self.state.stream_session_calls.lock().await.push(body);
        if let Some(s) = self.state.force_stream_status.lock().await.take() {
            return Err(s);
        }
        let events = self.state.stream_events.lock().await.clone();
        let events = if events.is_empty() {
            default_stream_events()
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

fn default_stream_events() -> Vec<AgentEvent> {
    // Three Token deltas → Finish. Codec emits a single text
    // content_block containing the concatenated text.
    vec![
        token_event(1, "Hello"),
        token_event(2, " world"),
        token_event(3, "!"),
        finish_event(4, "stop"),
    ]
}

fn token_event(seq: u64, text: &str) -> AgentEvent {
    AgentEvent {
        record: Some(EventRecord {
            session_id: None,
            sequence: seq,
            at: None,
            kind: "TOKEN".into(),
            payload: serde_json::to_vec(&serde_json::json!({"text": text})).expect("payload"),
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
            payload: serde_json::to_vec(&serde_json::json!({"reason": reason})).expect("payload"),
        }),
        kind: AgentEventKind::Finish as i32,
    }
}

// ─── Test rig ───────────────────────────────────────────────────────────

struct TestRig {
    state: Arc<MockAgentState>,
    router: axum::Router,
    _temp: TempDir,
    _shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    _handle: Option<tokio::task::JoinHandle<()>>,
}

impl TestRig {
    async fn build() -> Self {
        // Mock lifed Agent service over a tempdir UDS.
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

        // Dial the UDS as a tonic Channel — this is the same shape
        // `bootstrap::dial_upstream` produces in production.
        let path = socket_path_str.clone();
        let endpoint = Endpoint::try_from("http://[::]:0").expect("endpoint");
        let upstream = endpoint
            .connect_with_connector(service_fn(move |_: Uri| {
                let path = path.clone();
                async move {
                    let stream = tokio::net::UnixStream::connect(path).await?;
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
                }
            }))
            .await
            .expect("dial UDS");

        // Build the router state with the dev JwksCache (accepts
        // `Bearer dev-token-for-{user}` shortcut) and a Tier-2 minter.
        let jwks = Arc::new(JwksCache::dev_only());
        let signer = Arc::new(StaticKeystore::generate_dev().expect("keystore"));
        let minter = Arc::new(Tier2Minter::new(signer, &AuthConfig::default()));

        let state = AnthropicMessagesState {
            jwks,
            minter,
            upstream,
        };
        let router = anthropic_messages::router(state);

        Self {
            state: agent_state,
            router,
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

    /// Build a minimal Anthropic Messages POST request body.
    fn body(user_text: &str) -> String {
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
}

impl Drop for TestRig {
    fn drop(&mut self) {
        if let Some(tx) = self._shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self._handle.take() {
            h.abort();
        }
    }
}

/// Collect the full SSE body as a String (test bodies are small).
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

/// Helper that streams the response body and stops once `message_stop`
/// is observed. Used by tests that need the SSE-shape assertions
/// (`event: message_stop\n` is the terminal marker).
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

// ─── Tests ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn simple_chat_completion() {
    let rig = TestRig::build().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("authorization", TestRig::dev_bearer("alice"))
        .header("anthropic-version", "2023-06-01")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(TestRig::body("hello")))
        .expect("build req");
    let resp = rig.router().oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap_or("").to_string())
            .as_deref(),
        Some("text/event-stream"),
    );

    let body = stream_until_stop(resp, 8 * 1024).await;
    assert!(
        body.contains("event: message_start"),
        "missing message_start in body: {body}"
    );
    assert!(
        body.contains("event: content_block_start"),
        "missing content_block_start: {body}"
    );
    assert!(body.contains("Hello"));
    assert!(body.contains(" world"));
    assert!(body.contains("event: message_stop"), "missing terminal");

    // Upstream observed: create + send + stream.
    assert_eq!(rig.state.create_session_calls.lock().await.len(), 1);
    assert_eq!(rig.state.send_message_calls.lock().await.len(), 1);
    assert_eq!(rig.state.stream_session_calls.lock().await.len(), 1);

    let send = rig.state.send_message_calls.lock().await;
    assert_eq!(send[0].content, "hello");
    let stream = rig.state.stream_session_calls.lock().await;
    assert!(stream[0].sid.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_turn_no_tools() {
    // Two HTTP turns with the same first user message: the codec's sid
    // synthesizer is deterministic over (anima_did, canon-first-user)
    // so both turns hit the same sid. The mock echoes the inbound
    // resume_sid, so we can read the sid back via CreateSession's
    // recorded inputs.
    let rig = TestRig::build().await;
    for _turn in 0..2 {
        let body = TestRig::body("read foo.txt");
        let req = Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", TestRig::dev_bearer("alice"))
            .header("anthropic-version", "2023-06-01")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("build req");
        let resp = rig.router().oneshot(req).await.expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        // Drain so the upstream call completes before the next turn.
        let _ = stream_until_stop(resp, 8 * 1024).await;
    }
    let calls = rig.state.create_session_calls.lock().await;
    assert_eq!(calls.len(), 2);
    let sid1 = calls[0].resume_sid.as_ref().map(|s| s.value.clone());
    let sid2 = calls[1].resume_sid.as_ref().map(|s| s.value.clone());
    assert!(sid1.is_some());
    assert_eq!(
        sid1, sid2,
        "sid synthesis must be deterministic across turns"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_missing_returns_401() {
    let rig = TestRig::build().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("anthropic-version", "2023-06-01")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(TestRig::body("hi")))
        .expect("build req");
    let resp = rig.router().oneshot(req).await.expect("oneshot");
    let (status, body) = collect_body(resp).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let v: Value = serde_json::from_str(&body).expect("anthropic-shape body");
    assert_eq!(v["type"], "error");
    assert_eq!(v["error"]["type"], "authentication_error");
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_invalid_returns_401() {
    let rig = TestRig::build().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("authorization", "Bearer garbage-not-a-token")
        .header("anthropic-version", "2023-06-01")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(TestRig::body("hi")))
        .expect("build req");
    let resp = rig.router().oneshot(req).await.expect("oneshot");
    let (status, body) = collect_body(resp).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("authentication_error"));
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_anthropic_version_returns_400() {
    let rig = TestRig::build().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("authorization", TestRig::dev_bearer("alice"))
        .header("anthropic-version", "future-2099")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(TestRig::body("hi")))
        .expect("build req");
    let resp = rig.router().oneshot(req).await.expect("oneshot");
    let (status, body) = collect_body(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).expect("anthropic-shape body");
    assert_eq!(v["error"]["type"], "invalid_request_error");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("future-2099")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limit_engaged_returns_429() {
    let rig = TestRig::build().await;
    // Pre-arm the mock so the very next CreateSession returns
    // ResourceExhausted — same shape as a real upstream rate-limit hit.
    *rig.state.force_create_status.lock().await =
        Some(tonic::Status::resource_exhausted("rate_limit:per_user"));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("authorization", TestRig::dev_bearer("alice"))
        .header("anthropic-version", "2023-06-01")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(TestRig::body("hi")))
        .expect("build req");
    let resp = rig.router().oneshot(req).await.expect("oneshot");
    let (status, body) = collect_body(resp).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let v: Value = serde_json::from_str(&body).expect("anthropic-shape body");
    assert_eq!(v["error"]["type"], "rate_limit_error");
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_drop_resume() {
    // Drive two POSTs back-to-back with the same body. The deterministic
    // sid synthesis means both hit the same Life sid — the mock's
    // CreateSession echoes the inbound resume_sid, so the recorded
    // sids must match. This mirrors a Claude-Code-side disconnect +
    // re-send.
    let rig = TestRig::build().await;

    let body = TestRig::body("resume-marker");
    for _ in 0..2 {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", TestRig::dev_bearer("alice"))
            .header("anthropic-version", "2023-06-01")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.clone()))
            .expect("build req");
        let resp = rig.router().oneshot(req).await.expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = stream_until_stop(resp, 8 * 1024).await;
    }
    let calls = rig.state.create_session_calls.lock().await;
    assert_eq!(calls.len(), 2);
    let sid_a = calls[0].resume_sid.as_ref().map(|s| s.value.clone());
    let sid_b = calls[1].resume_sid.as_ref().map(|s| s.value.clone());
    assert_eq!(
        sid_a, sid_b,
        "resume must reuse sid via deterministic synthesis"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn large_request_body() {
    // 100K-character user message. The codec parses it lazily;
    // synthesize_sid hashes it; the route forwards only the last user
    // message (still 100K bytes). Goal: assert the handler doesn't OOM
    // and produces a clean 200 OK + message_stop terminal.
    let rig = TestRig::build().await;
    let huge = "X".repeat(100_000);
    let body = TestRig::body(&huge);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("authorization", TestRig::dev_bearer("alice"))
        .header("anthropic-version", "2023-06-01")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("build req");
    let resp = rig.router().oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let drained = stream_until_stop(resp, 32 * 1024).await;
    assert!(drained.contains("event: message_stop"));
    // The 100K user content lands in SendMessage's `content` field
    // (the canonical-form first-user-message used for sid synthesis is
    // also 100K + whitespace-normalised, but plain "X..." has no
    // whitespace to collapse, so it should be byte-identical at that
    // path too).
    let send = rig.state.send_message_calls.lock().await;
    assert_eq!(send.len(), 1);
    assert_eq!(send[0].content.len(), 100_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn options_probe_returns_204() {
    // Some clients pre-flight; lifegw replies 204 No Content with an
    // `Allow` header so they don't bounce off a 405.
    let rig = TestRig::build().await;
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/v1/messages")
        .body(Body::empty())
        .expect("build req");
    let resp = rig.router().oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let allow = resp
        .headers()
        .get(header::ALLOW)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(allow.contains("POST"));
}
