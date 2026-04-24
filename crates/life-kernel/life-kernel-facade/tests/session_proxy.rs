//! wiremock-driven unit tests for SessionProxy against a simulated arcand.

use aios_protocol::{
    ids::{BranchId, SessionId},
    ports::SessionPort,
    session::{CreateSessionRequest, SessionManifest},
};
use life_kernel_facade::{
    arcand::{client::ArcanClient, session::SessionProxy},
    config::DaemonEndpoints,
};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_manifest_json(id: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": id,
        "owner": "test",
        "created_at": "2026-04-24T00:00:00Z",
        "workspace_root": "/tmp/test",
        "model_routing": {
            "primary_model": "claude-sonnet-4-5-20250929",
            "fallback_models": ["gpt-4.1"],
            "temperature": 0.2
        },
        "policy": null
    })
}

#[tokio::test]
async fn create_returns_manifest() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/sessions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(make_manifest_json("s-123")),
        )
        .mount(&server)
        .await;

    let client =
        ArcanClient::new(&DaemonEndpoints::new(server.uri(), "http://lagod.invalid")).unwrap();
    let proxy = SessionProxy::new(client);

    // CreateSessionRequest is #[non_exhaustive] — construct via serde.
    let req: CreateSessionRequest = serde_json::from_value(serde_json::json!({
        "owner": "test"
    }))
    .unwrap();
    let got: SessionManifest = proxy.create(req).await.unwrap();
    assert_eq!(got.session_id, SessionId::from("s-123"));
    assert_eq!(got.owner, "test");
}

#[tokio::test]
async fn list_returns_empty_vec() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/sessions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;

    let client =
        ArcanClient::new(&DaemonEndpoints::new(server.uri(), "http://lagod.invalid")).unwrap();
    let proxy = SessionProxy::new(client);

    let result = proxy
        .list(Default::default())
        .await
        .unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn get_finds_session_in_list() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/sessions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "session_id": "s-999", "owner": "alice", "created_at": "2026-04-24T00:00:00Z" }
        ])))
        .mount(&server)
        .await;

    let client =
        ArcanClient::new(&DaemonEndpoints::new(server.uri(), "http://lagod.invalid")).unwrap();
    let proxy = SessionProxy::new(client);

    let got = proxy.get(SessionId::from("s-999")).await.unwrap();
    assert_eq!(got.owner, "alice");
}

#[tokio::test]
#[ignore = "SSE decoding requires exact EventEnvelope JSON shape — exercised in Task 18 integration harness"]
async fn stream_events_decodes_one_frame() {
    let server = MockServer::start().await;

    // Minimal EventEnvelope JSON — fill all required fields.
    let event = serde_json::json!({
        "event_id": "evt-1",
        "session_id": "s-1",
        "agent_id": "agent-default",
        "branch_id": "main",
        "seq": 1u64,
        "ts_ms": 1745500000000000u64,
        "actor": { "type": "system", "component": "arcand" },
        "schema": { "name": "aios-protocol", "version": "0.1.0" },
        "kind": { "type": "session_created", "owner": "test" }
    });
    let body = format!("data: {}\n\n", event);

    Mock::given(method("GET"))
        .and(path_regex(r"^/sessions/s-1/events/stream$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(body, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let client =
        ArcanClient::new(&DaemonEndpoints::new(server.uri(), "http://lagod.invalid")).unwrap();
    let proxy = SessionProxy::new(client);

    use futures::StreamExt;
    let mut stream = proxy
        .stream_events(
            SessionId::from("s-1"),
            BranchId::from("main"),
            0,
        )
        .await
        .unwrap();
    let next = stream.next().await.expect("frame").expect("ok");
    let _ = next;
}
