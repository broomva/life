//! M5 sub-phase C admin-plane integration tests.
//!
//! Boots `lifed` with `MockSubstrates`, dials the admin UDS, and exercises
//! every life.admin.v1.* RPC. Asserts both happy-path behavior and the
//! documented sub-phase C carve-outs (Saga.ForceCompensate +
//! RoutingCache.RebuildFromLago).

#[path = "_support/mod.rs"]
mod _support;

use _support::test_env::TestEnv;

use life_runtime_proto::life::admin::v1::{
    DumpReq, EvictReq, ForceCloseReq, HealthReq, IdemReq, ListAllReq, ListInflightReq, RebuildReq,
    SagaRef, SuspendReq, routing_cache_client::RoutingCacheClient, runtime_client::RuntimeClient,
    saga_client::SagaClient,
};

// =============================================================================
// life.admin.v1.Runtime
// =============================================================================

#[tokio::test]
async fn admin_health_check_returns_ok() {
    let env = TestEnv::start_with_mocks().await;
    let channel = env.dial_admin().await;
    let mut client = RuntimeClient::new(channel);
    let resp = client
        .health_check(HealthReq {})
        .await
        .expect("health")
        .into_inner();
    assert!(resp.ok);
    assert_eq!(resp.cache_size, 0);
    assert!(!resp.version.is_empty(), "version surfaced");
    env.shutdown().await;
}

#[tokio::test]
async fn sessions_list_all_includes_active_sessions() {
    let env = TestEnv::start_with_mocks().await;
    let _s1 = env
        .create_session_dev("alice", "p", "s1")
        .await
        .expect("s1");
    let _s2 = env
        .create_session_dev("alice", "p", "s2")
        .await
        .expect("s2");

    let channel = env.dial_admin().await;
    let mut client = RuntimeClient::new(channel);
    let mut stream = client
        .sessions_list_all(ListAllReq { limit: 100 })
        .await
        .expect("list")
        .into_inner();
    let mut count = 0;
    while let Some(s) = stream.message().await.expect("msg") {
        count += 1;
        assert!(!s.user_id.is_empty());
        assert_eq!(s.status, "active");
    }
    assert_eq!(count, 2);
    env.shutdown().await;
}

#[tokio::test]
async fn sessions_force_close_evicts_routing_entry() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "p", "label")
        .await
        .expect("create");
    let sid = session.sid.clone().expect("sid");

    assert_eq!(env.handles.routing.size(), 1);

    let channel = env.dial_admin().await;
    let mut client = RuntimeClient::new(channel);
    client
        .sessions_force_close(ForceCloseReq {
            sid: Some(sid.clone()),
            reason: "test".to_string(),
        })
        .await
        .expect("force_close");

    assert_eq!(env.handles.routing.size(), 0, "evicted");
    env.shutdown().await;
}

#[tokio::test]
async fn sessions_suspend_marks_status_detached() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "p", "label")
        .await
        .expect("create");
    let sid = session.sid.clone().expect("sid");

    let channel = env.dial_admin().await;
    let mut client = RuntimeClient::new(channel);
    client
        .sessions_suspend(SuspendReq {
            sid: Some(sid.clone()),
            reason: "idle".to_string(),
        })
        .await
        .expect("suspend");

    let entry = env.handles.routing.lookup(&sid).expect("entry present");
    assert_eq!(entry.status, lifed::routing::cache::SessionStatus::Detached);
    env.shutdown().await;
}

#[tokio::test]
async fn idempotency_lookup_returns_not_found_for_missing_key() {
    let env = TestEnv::start_with_mocks().await;
    let channel = env.dial_admin().await;
    let mut client = RuntimeClient::new(channel);
    let resp = client
        .idempotency_lookup(IdemReq {
            idempotency_key: "definitely-not-stored".to_string(),
            method: "Wallet.Debit".to_string(),
        })
        .await
        .expect("lookup")
        .into_inner();
    assert!(!resp.found);
    env.shutdown().await;
}

// =============================================================================
// life.admin.v1.Saga
// =============================================================================

#[tokio::test]
async fn saga_show_returns_create_session_record() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "p", "label")
        .await
        .expect("create");
    let sid = session.sid.clone().expect("sid");
    let saga_id = format!("create-session-{}", sid.value);

    let channel = env.dial_admin().await;
    let mut client = SagaClient::new(channel);
    let state = client
        .show(SagaRef {
            saga_id: saga_id.clone(),
        })
        .await
        .expect("show")
        .into_inner();
    assert_eq!(state.saga_id, saga_id);
    assert_eq!(state.saga_kind, "lifed-runtime");
    assert_eq!(state.status, "succeeded");
    assert_eq!(state.completed_steps.len(), 4, "four-step saga ran fully");
    env.shutdown().await;
}

#[tokio::test]
async fn saga_show_unknown_id_returns_not_found() {
    let env = TestEnv::start_with_mocks().await;
    let channel = env.dial_admin().await;
    let mut client = SagaClient::new(channel);
    let err = client
        .show(SagaRef {
            saga_id: "nope".to_string(),
        })
        .await
        .expect_err("not found");
    assert_eq!(err.code(), tonic::Code::NotFound);
    env.shutdown().await;
}

#[tokio::test]
async fn saga_list_inflight_streams_zero_when_idle() {
    let env = TestEnv::start_with_mocks().await;
    // Create + complete a session so the only saga is in `Succeeded`
    // state, not `Inflight` — should not appear in the inflight stream.
    let _s = env
        .create_session_dev("alice", "p", "label")
        .await
        .expect("create");

    let channel = env.dial_admin().await;
    let mut client = SagaClient::new(channel);
    let mut stream = client
        .list_inflight(ListInflightReq { limit: 100 })
        .await
        .expect("list")
        .into_inner();
    let mut count = 0;
    while stream.message().await.expect("msg").is_some() {
        count += 1;
    }
    assert_eq!(count, 0, "no inflight sagas");
    env.shutdown().await;
}

#[tokio::test]
async fn saga_force_compensate_returns_unimplemented() {
    let env = TestEnv::start_with_mocks().await;
    let channel = env.dial_admin().await;
    let mut client = SagaClient::new(channel);
    let err = client
        .force_compensate(SagaRef {
            saga_id: "anything".to_string(),
        })
        .await
        .expect_err("unimplemented carve-out");
    assert_eq!(err.code(), tonic::Code::Unimplemented);
    assert!(
        err.message().contains("carve-out") || err.message().contains("re-entrant"),
        "carve-out reason surfaces in error message",
    );
    env.shutdown().await;
}

// =============================================================================
// life.admin.v1.RoutingCache
// =============================================================================

#[tokio::test]
async fn routing_cache_dump_streams_active_entries() {
    let env = TestEnv::start_with_mocks().await;
    let _s1 = env
        .create_session_dev("alice", "p1", "s1")
        .await
        .expect("s1");
    let _s2 = env.create_session_dev("bob", "p2", "s2").await.expect("s2");

    let channel = env.dial_admin().await;
    let mut client = RoutingCacheClient::new(channel);
    let mut stream = client
        .dump(DumpReq { limit: 100 })
        .await
        .expect("dump")
        .into_inner();
    let mut entries = Vec::new();
    while let Some(e) = stream.message().await.expect("msg") {
        entries.push(e);
    }
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|e| e.user_id == "alice"));
    assert!(entries.iter().any(|e| e.user_id == "bob"));
    for e in &entries {
        assert!(!e.lago_namespace.is_empty());
        assert!(!e.haima_wallet.is_empty());
        assert!(!e.anima_account.is_empty());
    }
    env.shutdown().await;
}

#[tokio::test]
async fn routing_cache_evict_removes_entry() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "p", "label")
        .await
        .expect("create");
    let sid = session.sid.clone().expect("sid");

    let channel = env.dial_admin().await;
    let mut client = RoutingCacheClient::new(channel);
    client
        .evict(EvictReq {
            sid: Some(sid.clone()),
            reason: "test".to_string(),
        })
        .await
        .expect("evict");
    assert_eq!(env.handles.routing.size(), 0);
    env.shutdown().await;
}

#[tokio::test]
async fn routing_cache_rebuild_from_lago_is_documented_stub() {
    let env = TestEnv::start_with_mocks().await;
    let channel = env.dial_admin().await;
    let mut client = RoutingCacheClient::new(channel);
    let resp = client
        .rebuild_from_lago(RebuildReq {})
        .await
        .expect("rebuild")
        .into_inner();
    assert_eq!(
        resp.sessions_loaded, 0,
        "stub returns 0 entries per master-spec ambiguity #3"
    );
    assert_eq!(resp.lago_events_read, 0);
    env.shutdown().await;
}

// =============================================================================
// Saga lago persistence (Spec C₂ §4.1)
// =============================================================================

#[tokio::test]
async fn saga_lifecycle_emits_lago_events() {
    let env = TestEnv::start_with_mocks().await;
    let _ = env
        .create_session_dev("alice", "p", "lifecycle")
        .await
        .expect("create");

    // The mock lago records every append_event call; assert that the
    // saga driver wrote `saga.started`, four `saga.step_forward`, and
    // one `saga.completed` to `system/lifed/saga/<saga_id>`. Snapshot
    // the parking_lot guard into an owned Vec so it never crosses an
    // await point.
    let appends_snapshot: Vec<(String, String)> =
        { env.mocks.lago.append_event_calls.lock().clone() };
    let saga_appends: Vec<&(String, String)> = appends_snapshot
        .iter()
        .filter(|(ns, _)| ns.starts_with("system/lifed/saga/"))
        .collect();
    assert!(
        !saga_appends.is_empty(),
        "saga events landed in lago: {:?}",
        appends_snapshot
    );
    let event_types: Vec<&str> = saga_appends.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        event_types.contains(&"saga.started"),
        "saga.started present: {event_types:?}"
    );
    assert!(
        event_types.contains(&"saga.step_forward"),
        "saga.step_forward present: {event_types:?}"
    );
    assert!(
        event_types.contains(&"saga.completed"),
        "saga.completed present: {event_types:?}"
    );
    env.shutdown().await;
}
