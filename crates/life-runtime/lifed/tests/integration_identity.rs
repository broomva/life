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

/// Verifies that `Identity.RevokeSession` propagates the revocation to all
/// three places per Spec C₂ §5.4 + §6.3:
/// 1. anima — the substrate-of-record for session revocation.
/// 2. local `RevokedSidSet` — blocklist that substrates poll for the
///    30 s revocation gap before Tier-3 tokens expire.
/// 3. local `RoutingCache` — evicting the entry guarantees no further
///    substrate dispatches for that sid land on lifed's hot path.
///
/// Sub-phase C widens the assertion (BRO-933) using the new
/// `LifedHandles` accessors.
#[tokio::test]
async fn revoke_session_propagates_to_anima() {
    let env = TestEnv::start_with_mocks().await;
    // Open a session so the routing cache has an entry.
    let session = env
        .create_session_dev("alice", "project-demo", "to-revoke")
        .await
        .expect("create_session");
    let sid = session.sid.expect("sid");

    // Pre-conditions: routing entry present, blocklist empty.
    assert_eq!(env.handles.routing.size(), 1, "session opened");
    assert!(
        !env.handles.revoked.contains(&sid),
        "blocklist starts empty",
    );

    let mut client = env.identity_client().await;
    client
        .revoke_session(auth_req(IdentitySessionRef {
            sid: Some(sid.clone()),
        }))
        .await
        .expect("revoke");

    // 1. Anima saw the revoke.
    {
        let revokes = env.mocks.anima.revoke_calls.lock();
        assert!(revokes.iter().any(|s| s == &sid.value));
    }
    // 2. Local blocklist now contains the sid.
    assert!(env.handles.revoked.contains(&sid), "sid added to blocklist",);
    // 3. Routing cache no longer has an entry.
    assert_eq!(env.handles.routing.size(), 0, "routing entry evicted");
    assert!(env.handles.routing.lookup(&sid).is_none());

    env.shutdown().await;
}
