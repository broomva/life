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
        tool_definitions: vec![],
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

/// Stage 3b-bis (May 2026): `stream_session` is a passive subscribe;
/// the pump only spawns from `send_message`. This test attaches the
/// stream first, then fires a `send_message` to drive the pump, then
/// verifies events flow back to the subscriber.
#[tokio::test]
async fn stream_session_returns_canned_events() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "project-demo", "stream session test")
        .await
        .expect("create_session");
    let sid = session.sid.expect("sid");

    let mut subscriber = env.agent_client().await;
    let mut req = tonic::Request::new(SessionRef {
        sid: Some(sid.clone()),
        from_sequence: None,
    });
    req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let mut stream = subscriber
        .stream_session(req)
        .await
        .expect("stream_session")
        .into_inner();

    // Drive a turn via send_message. Mock substrate emits Token+Finish.
    let mut driver = env.agent_client().await;
    let mut send_req = tonic::Request::new(life_runtime_proto::life::v1::SendMessageReq {
        sid: Some(sid),
        content: "drive".to_string(),
        attachment_blob_ref: vec![],
        tool_definitions: vec![],
    });
    send_req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let _ = driver
        .send_message(send_req)
        .await
        .expect("send_message")
        .into_inner();

    let _ = stream.next().await; // consume at least one
    env.shutdown().await;
}

/// BRO-1206: end-to-end model-override plumbing through lifed.
///
/// 1. `Agent.CreateSession` with `model: Some("anthropic/claude-opus-4-7")`
///    persists the override on the routing-cache entry.
/// 2. `Agent.SendMessage` looks it up and passes it to
///    `MockArcan::dispatch_message`.
/// 3. The mock records `(sid, Some("anthropic/claude-opus-4-7"))` on
///    `dispatch_models`, proving the wire path is closed.
#[tokio::test]
async fn send_message_passes_per_session_model_override_to_arcan() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev_with_model(
            "alice",
            "project-demo",
            "model-override",
            Some("anthropic/claude-opus-4-7"),
        )
        .await
        .expect("create_session");
    let sid = session.sid.expect("sid");

    let mut client = env.agent_client().await;
    let mut req = tonic::Request::new(SendMessageReq {
        sid: Some(sid.clone()),
        content: "hi".to_string(),
        attachment_blob_ref: vec![],
        tool_definitions: vec![],
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
    // Drain a few events so the pump task has run dispatch_message.
    for _ in 0..2 {
        if stream.next().await.is_none() {
            break;
        }
    }
    // Give the spawned pump a moment to record the dispatch.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let models = env.mocks.arcan.dispatch_models.lock().clone();
    assert!(
        !models.is_empty(),
        "expected at least one dispatch_message call recorded"
    );
    assert_eq!(models[0].0, sid.value);
    assert_eq!(
        models[0].1.as_deref(),
        Some("anthropic/claude-opus-4-7"),
        "model override should travel from CreateSession through routing cache to dispatch_message"
    );

    env.shutdown().await;
}

/// Client tool definitions travel from `Agent.SendMessage`
/// (`tool_definitions` JSON bytes) through lifed's fanout pump to
/// `ArcanCall::dispatch_message`:
///
/// 1. `SendMessageReq.tool_definitions` carries one JSON-bytes entry
///    per tool (AI-SDK / OpenAI function shape).
/// 2. lifed decodes them and passes `&[serde_json::Value]` to the
///    substrate-call boundary.
/// 3. `MockArcan` records `(sid, tools)` on `dispatch_tools`, proving
///    the wire path is closed. Malformed entries are skipped.
#[tokio::test]
async fn send_message_passes_tool_definitions_to_arcan() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "project-demo", "tools")
        .await
        .expect("create_session");
    let sid = session.sid.expect("sid");

    let tool = serde_json::json!({
        "name": "get_weather",
        "description": "Look up the weather",
        "parameters": {"type": "object", "properties": {"city": {"type": "string"}}},
    });
    let mut client = env.agent_client().await;
    let mut req = tonic::Request::new(SendMessageReq {
        sid: Some(sid.clone()),
        content: "what's the weather in Nicosia?".to_string(),
        attachment_blob_ref: vec![],
        tool_definitions: vec![
            serde_json::to_vec(&tool).expect("serialize tool"),
            // Malformed entry — must be skipped, not fail the dispatch.
            b"not-json".to_vec(),
        ],
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
    for _ in 0..2 {
        if stream.next().await.is_none() {
            break;
        }
    }
    // Give the spawned pump a moment to record the dispatch.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let recorded = env.mocks.arcan.dispatch_tools.lock().clone();
    assert!(
        !recorded.is_empty(),
        "expected at least one dispatch_message call recorded"
    );
    assert_eq!(recorded[0].0, sid.value);
    assert_eq!(
        recorded[0].1,
        vec![tool],
        "tool definitions should travel from SendMessage through the pump \
         to dispatch_message; malformed entries are dropped"
    );

    env.shutdown().await;
}

/// BRO-1206: no `model` field on `Agent.CreateSession` → backends see
/// `None` on `dispatch_message` → env fallback applies.
#[tokio::test]
async fn send_message_passes_none_when_no_model_override() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "project-demo", "no-override")
        .await
        .expect("create_session");
    let sid = session.sid.expect("sid");

    let mut client = env.agent_client().await;
    let mut req = tonic::Request::new(SendMessageReq {
        sid: Some(sid.clone()),
        content: "hi".to_string(),
        attachment_blob_ref: vec![],
        tool_definitions: vec![],
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
    for _ in 0..2 {
        if stream.next().await.is_none() {
            break;
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let models = env.mocks.arcan.dispatch_models.lock().clone();
    assert!(!models.is_empty());
    assert!(
        models[0].1.is_none(),
        "no model on CreateSession → None on dispatch_message (env fallback)"
    );

    env.shutdown().await;
}

/// BRO-1206: empty / whitespace `model` is normalised to None at the
/// routing-cache layer so backends see a single "not set" representation.
#[tokio::test]
async fn empty_model_field_normalises_to_none() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev_with_model("alice", "project-demo", "empty-model", Some("   "))
        .await
        .expect("create_session");
    let sid = session.sid.expect("sid");

    let mut client = env.agent_client().await;
    let mut req = tonic::Request::new(SendMessageReq {
        sid: Some(sid.clone()),
        content: "hi".to_string(),
        attachment_blob_ref: vec![],
        tool_definitions: vec![],
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
    for _ in 0..2 {
        if stream.next().await.is_none() {
            break;
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let models = env.mocks.arcan.dispatch_models.lock().clone();
    assert!(!models.is_empty());
    assert!(
        models[0].1.is_none(),
        "whitespace-only model should normalise to None at the routing-cache boundary"
    );

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
        from_sequence: None,
    });
    let mut req2 = tonic::Request::new(SessionRef {
        sid: Some(sid.clone()),
        from_sequence: None,
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

    // Stage 3b-bis (May 2026): `stream_session` is a passive subscribe.
    // Drive a single `send_message` to kick off the pump; lifed's
    // fanout broadcasts the resulting events to BOTH attached
    // subscribers (one pump, two attached tabs — Spec C₂ §6.4).
    let mut driver = env.agent_client().await;
    let mut send_req = tonic::Request::new(life_runtime_proto::life::v1::SendMessageReq {
        sid: Some(sid),
        content: "fanout driver".to_string(),
        attachment_blob_ref: vec![],
        tool_definitions: vec![],
    });
    send_req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let _ = driver
        .send_message(send_req)
        .await
        .expect("send_message")
        .into_inner();

    // Both attached streams receive the broadcasted Token+Finish events
    // from the single shared pump — one pump, two readers (Spec C₂ §6.4).
    let r1 = s1.next().await.expect("e1");
    let r2 = s2.next().await.expect("e2");
    assert!(r1.is_ok());
    assert!(r2.is_ok());

    env.shutdown().await;
}
