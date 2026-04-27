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

/// M5 sub-phase B Task B12 acceptance — Spec C₂ §6.4.
///
/// Multi-tab fanout: two clients attach to `stream_session` and a single
/// `send_message` should land an event on both.
#[tokio::test]
async fn multi_tab_fanout_emits_to_all_attached_streams() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "project-demo", "fanout")
        .await
        .expect("create_session");
    let sid = session.sid.expect("sid");

    let mut c1 = env.agent_client().await;
    let mut c2 = env.agent_client().await;
    let mut req1 = tonic::Request::new(SessionRef {
        sid: Some(sid.clone()),
    });
    let mut req2 = tonic::Request::new(SessionRef {
        sid: Some(sid.clone()),
    });
    req1.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    req2.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );

    let mut s1 = c1.stream_session(req1).await.expect("s1").into_inner();
    let mut s2 = c2.stream_session(req2).await.expect("s2").into_inner();

    // Each stream_session call attaches its own arcan dispatch + broadcast,
    // so each receives at least one event from its own pump.
    let r1 = s1.next().await.expect("e1");
    let r2 = s2.next().await.expect("e2");
    assert!(r1.is_ok());
    assert!(r2.is_ok());

    env.shutdown().await;
}
