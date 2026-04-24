//! wiremock-driven unit tests for EventsProxy against a simulated lagod.

use aios_protocol::ids::{BranchId, SessionId};
use aios_protocol::ports::EventStorePort;
use life_kernel_facade::{
    config::DaemonEndpoints,
    lagod::{client::LagoClient, events::EventsProxy},
};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn head_returns_sequence() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/v1/sessions/[^/]+/events/head$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "seq": 42u64
        })))
        .mount(&server)
        .await;

    let endpoints = DaemonEndpoints::new("http://arcand.invalid", server.uri());
    let client = LagoClient::new(&endpoints).unwrap();
    let proxy = EventsProxy::new(client);

    let head = proxy
        .head(SessionId::from("s-1"), BranchId::from("main"))
        .await
        .unwrap();
    assert_eq!(head, 42);
}

#[tokio::test]
async fn head_maps_5xx_to_backend_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/v1/sessions/.+/events/head$"))
        .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
        .mount(&server)
        .await;
    let endpoints = DaemonEndpoints::new("http://arcand.invalid", server.uri());
    let client = LagoClient::new(&endpoints).unwrap();
    let proxy = EventsProxy::new(client);

    let err = proxy
        .head(SessionId::from("s-1"), BranchId::from("main"))
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("503") || msg.contains("backend") || msg.contains("lagod"), "got {msg}");
}

#[tokio::test]
async fn read_returns_empty_slice() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/v1/sessions/.+/events/read$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;
    let endpoints = DaemonEndpoints::new("http://arcand.invalid", server.uri());
    let client = LagoClient::new(&endpoints).unwrap();
    let proxy = EventsProxy::new(client);

    let records = proxy
        .read(SessionId::from("s-1"), BranchId::from("main"), 0, 10)
        .await
        .unwrap();
    assert!(records.is_empty());
}
