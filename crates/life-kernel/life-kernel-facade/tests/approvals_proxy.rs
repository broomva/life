//! wiremock-driven unit tests for ApprovalsProxy against a simulated arcand.

use aios_protocol::{
    ids::{ApprovalId, SessionId},
    ports::ApprovalPort,
};
use life_kernel_facade::{
    arcand::{approvals::ApprovalsProxy, client::ArcanClient},
    config::DaemonEndpoints,
};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn list_pending_returns_empty() {
    let server = MockServer::start().await;

    // No mocks needed — list_pending returns empty in Phase 1.
    let client =
        ArcanClient::new(&DaemonEndpoints::new(server.uri(), "http://lagod.invalid")).unwrap();
    let proxy = ApprovalsProxy::new(client);

    let result = proxy.list_pending(SessionId::from("s-1")).await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn resolve_calls_correct_endpoint() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r"^/sessions/[^/]+/approvals/[^/]+$"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let client =
        ArcanClient::new(&DaemonEndpoints::new(server.uri(), "http://lagod.invalid")).unwrap();
    let proxy = ApprovalsProxy::new(client);

    let resolution = proxy
        .resolve(ApprovalId::from("test-approval-id"), true, "human".into())
        .await
        .unwrap();
    assert!(resolution.approved);
    assert_eq!(resolution.actor, "human");
}

#[tokio::test]
async fn enqueue_returns_unsupported_error() {
    let server = MockServer::start().await;
    let client =
        ArcanClient::new(&DaemonEndpoints::new(server.uri(), "http://lagod.invalid")).unwrap();
    let proxy = ApprovalsProxy::new(client);

    // arcand has no enqueue endpoint in v0 — expect an error.
    use aios_protocol::ports::ApprovalRequest;
    let req: ApprovalRequest = serde_json::from_value(serde_json::json!({
        "session_id": "s-1",
        "call_id": "call-1",
        "tool_name": "bash",
        "capability": "exec:cmd:bash",
        "reason": "needs approval"
    }))
    .unwrap();
    let err = proxy.enqueue(req).await.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("arcand") || msg.contains("enqueue") || msg.contains("direct"),
        "got: {msg}"
    );
}
