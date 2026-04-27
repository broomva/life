//! M5 sub-phase B Task B14 acceptance: life.v1.Identity end-to-end.

#[path = "_support/mod.rs"]
mod _support;

use _support::test_env::TestEnv;
use life_runtime_proto::life::v1::{
    IdentityEmpty, IdentitySessionRef, ListSessionsReq, Profile, UpdateProfileReq,
};

fn auth_req<T>(body: T) -> tonic::Request<T> {
    let mut r = tonic::Request::new(body);
    r.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    r
}

#[tokio::test]
async fn me_returns_canned_account() {
    let env = TestEnv::start_with_mocks().await;
    let mut client = env.identity_client().await;
    let acct = client
        .me(auth_req(IdentityEmpty {}))
        .await
        .expect("me")
        .into_inner();
    assert_eq!(acct.user_id, "alice");
    assert!(acct.handle.starts_with('@'));
    env.shutdown().await;
}

#[tokio::test]
async fn update_profile_round_trips() {
    let env = TestEnv::start_with_mocks().await;
    let mut client = env.identity_client().await;
    let prof = Profile {
        bio: "hello".to_string(),
        ..Default::default()
    };
    let acct = client
        .update_profile(auth_req(UpdateProfileReq {
            profile: Some(prof),
        }))
        .await
        .expect("update")
        .into_inner();
    assert_eq!(acct.profile.expect("profile").bio, "hello");
    env.shutdown().await;
}

#[tokio::test]
async fn list_sessions_returns_empty() {
    let env = TestEnv::start_with_mocks().await;
    let mut client = env.identity_client().await;
    let resp = client
        .list_sessions(auth_req(ListSessionsReq {
            include_closed: true,
            limit: 10,
        }))
        .await
        .expect("list")
        .into_inner();
    assert_eq!(resp.sessions.len(), 0);
    env.shutdown().await;
}

#[tokio::test]
async fn revoke_session_evicts_routing_entry() {
    let env = TestEnv::start_with_mocks().await;
    // Open a session so the routing cache has an entry.
    let session = env
        .create_session_dev("alice", "project-demo", "to-revoke")
        .await
        .expect("create_session");
    let sid = session.sid.expect("sid");

    let mut client = env.identity_client().await;
    client
        .revoke_session(auth_req(IdentitySessionRef {
            sid: Some(sid.clone()),
        }))
        .await
        .expect("revoke");

    // Confirm the mock anima saw the revoke. Scope the lock so it drops
    // before `env.shutdown()` consumes `env`.
    {
        let revokes = env.mocks.anima.revoke_calls.lock();
        assert!(revokes.iter().any(|s| s == &sid.value));
    }
    env.shutdown().await;
}
