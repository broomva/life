//! M5 sub-phase A: SpawnChild returns Status::unimplemented per Spec C₂ §13.3.

#[path = "_support/mod.rs"]
mod _support;

use _support::test_env::TestEnv;
use life_runtime_proto::life::v1::{ChildPolicy, CreateSessionReq, SpawnChildReq};

#[tokio::test]
async fn spawn_child_returns_unimplemented() {
    let env = TestEnv::start_with_mocks().await;
    let parent = env
        .create_session_dev("alice", "project-demo", "spawn-child test")
        .await
        .expect("parent session");

    let mut client = env.agent_client().await;
    let mut req = tonic::Request::new(SpawnChildReq {
        parent_sid: parent.sid,
        spec: Some(CreateSessionReq {
            user_id: "alice".to_string(),
            project_id: "project-demo".to_string(),
            label: "child".to_string(),
            resume_sid: None,
            inherit_policy: None,
        }),
        budget_cap_micros: 100_000,
        inherit_policy: Some(ChildPolicy {
            inherit_skills: true,
            inherit_models: true,
            depth_cap: 5,
            fanout_cap: 32,
        }),
    });
    req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let err = client.spawn_child(req).await.expect_err("must err");
    assert_eq!(err.code(), tonic::Code::Unimplemented);
    assert!(err.message().contains("Spec C") || err.message().contains("BRO-926"));

    env.shutdown().await;
}
