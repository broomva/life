//! M5 sub-phase A acceptance: Agent.SendMessage server-stream round-trips
//! against the mock arcan stream.

#[path = "_support/mod.rs"]
mod _support;

use _support::test_env::TestEnv;
use futures::StreamExt;
use life_runtime_proto::life::v1::{AgentEventKind, SendMessageReq, SessionRef};

#[tokio::test]
async fn send_message_streams_at_least_one_event() {
    let env = TestEnv::start_with_mocks().await;

    // Open a session so the routing cache has an entry.
    let session = env
        .create_session_dev("alice", "project-demo", "stream test")
        .await
        .expect("create_session");
    let sid = session.sid.expect("sid");

    let mut client = env.agent_client().await;
    let mut req = tonic::Request::new(SendMessageReq {
        sid: Some(sid.clone()),
        content: "Hello, lifed".to_string(),
        attachment_blob_ref: vec![],
    });
    req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let mut stream = client
        .send_message(req)
        .await
        .expect("send_message")
        .into_inner();

    let mut events = Vec::new();
    while let Some(evt) = stream.next().await {
        events.push(evt.expect("event ok"));
        if events.len() >= 2 {
            break;
        }
    }
    assert!(events.len() >= 2);
    assert_eq!(events[0].kind, AgentEventKind::Token as i32);
    assert_eq!(events[1].kind, AgentEventKind::Finish as i32);

    env.shutdown().await;
}

#[tokio::test]
async fn stream_session_returns_canned_events() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "project-demo", "stream session test")
        .await
        .expect("create_session");
    let sid = session.sid.expect("sid");

    let mut client = env.agent_client().await;
    let mut req = tonic::Request::new(SessionRef { sid: Some(sid) });
    req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let mut stream = client
        .stream_session(req)
        .await
        .expect("stream_session")
        .into_inner();

    let _ = stream.next().await; // consume at least one
    env.shutdown().await;
}
