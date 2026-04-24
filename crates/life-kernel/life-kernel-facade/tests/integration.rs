//! Phase 1 v0 integration harness.
//!
//! Validates the full wire surface end-to-end without requiring the
//! (as-yet uncreated) `lifed` binary:
//!
//! 1. `wiremock` stands in for `lagod` + `arcand`.
//! 2. The three v0 proxies (`EventsProxy`, `SessionProxy`,
//!    `ApprovalsProxy`) are wired over those fakes.
//! 3. An in-process `tonic` server binds a temp Unix socket and mounts
//!    all 5 v0 adapters plus the 3 v0.2 stubs.
//! 4. `life-client` connects over the same socket and drives a
//!    round-trip, confirming the typed handles speak the generated
//!    wire surface cleanly.
//!
//! When Spec A Phase 2 lands and `lifed` starts hosting these services
//! itself, the harness becomes the template for that binary's
//! `server.rs`.

use aios_protocol::ids::{BranchId, SessionId};
use life_client::{LifeClient, LifeTransport};
use life_kernel_facade::{
    arcand::{approvals::ApprovalsProxy, client::ArcanClient, session::SessionProxy},
    config::DaemonEndpoints,
    lagod::{client::LagoClient, events::EventsProxy},
    services,
};
use life_kernel_proto::{
    approvals::approvals_service_server::ApprovalsServiceServer,
    events::events_service_server::EventsServiceServer,
    model::model_service_server::ModelServiceServer,
    relay::relay_service_server::RelayServiceServer,
    session::session_service_server::SessionServiceServer,
    tools::tools_service_server::ToolsServiceServer,
};
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn v0_events_head_roundtrip_via_unix_socket() {
    // 1. Fake lagod — respond to Events.Head with a known sequence number.
    let lagod = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/v1/sessions/[^/]+/events/head$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "seq": 42u64 })))
        .mount(&lagod)
        .await;

    // 2. arcand fake — not exercised in this specific round-trip, but
    //    the Session / Approvals services need a base URL that parses.
    let arcand = MockServer::start().await;

    // 3. Build proxies.
    let endpoints = DaemonEndpoints::new(arcand.uri(), lagod.uri());
    let lago_client = LagoClient::new(&endpoints).expect("lago client");
    let arcan_client = ArcanClient::new(&endpoints).expect("arcan client");
    let events_proxy = Arc::new(EventsProxy::new(lago_client));
    let session_proxy = Arc::new(SessionProxy::new(arcan_client.clone()));
    let approvals_proxy = Arc::new(ApprovalsProxy::new(arcan_client));

    // 4. Bind an in-process tonic server on a temp Unix socket.
    let tempdir = tempfile::tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("lifed.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind unix");
    let incoming = UnixListenerStream::new(listener);

    let router = Server::builder()
        .add_service(EventsServiceServer::new(
            services::events::EventsService::new(events_proxy),
        ))
        .add_service(SessionServiceServer::new(
            services::session::SessionService::new(session_proxy),
        ))
        .add_service(ApprovalsServiceServer::new(
            services::approvals::ApprovalsService::new(approvals_proxy),
        ))
        // v0.2 reserved stubs are present on the wire so a v0 client
        // talking to a v0.2-lit server never sees a new service appear.
        .add_service(ToolsServiceServer::new(services::v0_2::ToolsService))
        .add_service(ModelServiceServer::new(services::v0_2::ModelService))
        .add_service(RelayServiceServer::new(services::v0_2::RelayService));

    let server_handle = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.ok();
    });

    // Small yield so the server is accepting before we dial.
    tokio::task::yield_now().await;

    // 5. Connect a life-client over the same Unix socket and drive a
    //    round-trip.
    let client = LifeClient::connect(LifeTransport::Unix(socket_path.clone()))
        .await
        .expect("connect");

    let head = client
        .events()
        .head(SessionId::from("s-1"), BranchId::from("main"))
        .await
        .expect("head rpc");
    assert_eq!(head, 42, "round-tripped head sequence");

    server_handle.abort();
}

#[tokio::test]
async fn v0_2_tools_returns_unimplemented() {
    // Ensure the v0.2 reserved-stub adapter is actually registered and
    // returns the Unimplemented status the spec requires.
    let lagod = MockServer::start().await;
    let arcand = MockServer::start().await;
    let endpoints = DaemonEndpoints::new(arcand.uri(), lagod.uri());
    let lago_client = LagoClient::new(&endpoints).unwrap();
    let arcan_client = ArcanClient::new(&endpoints).unwrap();

    let events_proxy = Arc::new(EventsProxy::new(lago_client));
    let session_proxy = Arc::new(SessionProxy::new(arcan_client.clone()));
    let approvals_proxy = Arc::new(ApprovalsProxy::new(arcan_client));

    let tempdir = tempfile::tempdir().unwrap();
    let socket_path = tempdir.path().join("lifed-v02.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let incoming = UnixListenerStream::new(listener);

    let router = Server::builder()
        .add_service(EventsServiceServer::new(
            services::events::EventsService::new(events_proxy),
        ))
        .add_service(SessionServiceServer::new(
            services::session::SessionService::new(session_proxy),
        ))
        .add_service(ApprovalsServiceServer::new(
            services::approvals::ApprovalsService::new(approvals_proxy),
        ))
        .add_service(ToolsServiceServer::new(services::v0_2::ToolsService))
        .add_service(ModelServiceServer::new(services::v0_2::ModelService))
        .add_service(RelayServiceServer::new(services::v0_2::RelayService));

    let server_handle = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.ok();
    });
    tokio::task::yield_now().await;

    let client = LifeClient::connect(LifeTransport::Unix(socket_path.clone()))
        .await
        .unwrap();

    // Drive through the raw generated client since life-client's v0.2
    // handles are not exposed by design — this is exactly how Spec B.1
    // Phase 4 will light them up later.
    use life_kernel_proto::tools::tools_service_client::ToolsServiceClient;
    let mut tools = ToolsServiceClient::new(client.channel());
    let err: tonic::Status = tools
        .execute(life_kernel_proto::tools::ExecuteRequest {
            attribution: None,
            request_json: vec![],
        })
        .await
        .expect_err("tools.Execute must return Unimplemented in v0");
    assert_eq!(
        err.code(),
        tonic::Code::Unimplemented,
        "got {:?}: {}",
        err.code(),
        err.message()
    );

    server_handle.abort();
}
