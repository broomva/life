//! M5 sub-phase A acceptance: Agent.CreateSession round-trip against the
//! mock arcan substrate. Validates the handler shape, Tier-2 token check,
//! Tier-3 mint, and substrate dispatch.

#[path = "_support/mod.rs"]
mod _support;

use _support::test_env::TestEnv;

#[tokio::test]
async fn create_session_round_trips_against_mock_arcan() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "project-demo", "test session")
        .await
        .expect("create_session");

    assert_eq!(session.user_id, "alice");
    assert_eq!(session.project_id, "project-demo");
    assert!(!session.sid.expect("sid").value.is_empty());

    // Sub-phase A: at least one mock arcan call recorded.
    assert_eq!(env.mocks.arcan.create_agent_calls.lock().len(), 1);

    env.shutdown().await;
}
