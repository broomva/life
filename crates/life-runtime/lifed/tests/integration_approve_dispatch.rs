//! M5 sub-phase C acceptance: Agent.ApproveDispatch first-responder-wins
//! per Spec C₂ §6.4.
//!
//! Two concurrent tabs racing for the same `(sid, dispatch_id)` should
//! produce exactly one winner; the loser sees `Status::AlreadyExists`.
//! `CancelDispatch` releases the slot so a subsequent re-approval succeeds.

#[path = "_support/mod.rs"]
mod _support;

use _support::test_env::TestEnv;

use life_runtime_proto::life::v1::{ApprovalReq, DispatchRef};

fn auth_req<T>(body: T, user: &str) -> tonic::Request<T> {
    let mut r = tonic::Request::new(body);
    r.metadata_mut().insert(
        "authorization",
        format!("Bearer test-token-for-{user}").parse().unwrap(),
    );
    r
}

#[tokio::test]
async fn approve_dispatch_wins_first_call_and_blocks_second() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "p", "label")
        .await
        .expect("create");
    let sid = session.sid.expect("sid");

    let mut client = env.agent_client().await;

    // First approval wins.
    client
        .approve_dispatch(auth_req(
            ApprovalReq {
                sid: Some(sid.clone()),
                dispatch_id: "dispatch-1".to_string(),
            },
            "alice",
        ))
        .await
        .expect("first approval ok");

    // Second concurrent approval (different dispatch_id) sees the slot
    // taken and is rejected.
    let err = client
        .approve_dispatch(auth_req(
            ApprovalReq {
                sid: Some(sid.clone()),
                dispatch_id: "dispatch-2".to_string(),
            },
            "alice",
        ))
        .await
        .expect_err("second approval blocked");
    assert_eq!(err.code(), tonic::Code::AlreadyExists);
    assert!(
        err.message().contains("dispatch-1"),
        "error names the prior winner: {}",
        err.message(),
    );

    env.shutdown().await;
}

#[tokio::test]
async fn cancel_dispatch_releases_slot_for_reapproval() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "p", "label")
        .await
        .expect("create");
    let sid = session.sid.expect("sid");

    let mut client = env.agent_client().await;

    client
        .approve_dispatch(auth_req(
            ApprovalReq {
                sid: Some(sid.clone()),
                dispatch_id: "d1".to_string(),
            },
            "alice",
        ))
        .await
        .expect("approve d1");
    client
        .cancel_dispatch(auth_req(
            DispatchRef {
                sid: Some(sid.clone()),
                dispatch_id: "d1".to_string(),
            },
            "alice",
        ))
        .await
        .expect("cancel d1");
    client
        .approve_dispatch(auth_req(
            ApprovalReq {
                sid: Some(sid.clone()),
                dispatch_id: "d2".to_string(),
            },
            "alice",
        ))
        .await
        .expect("re-approve after cancel");

    env.shutdown().await;
}

#[tokio::test]
async fn concurrent_approvals_have_exactly_one_winner() {
    let env = TestEnv::start_with_mocks().await;
    let session = env
        .create_session_dev("alice", "p", "label")
        .await
        .expect("create");
    let sid = session.sid.expect("sid");

    // Spawn 16 concurrent approvers racing for the same sid (different
    // dispatch_ids). Exactly one must win.
    let mut handles = Vec::new();
    for i in 0..16 {
        let mut client = env.agent_client().await;
        let sid_clone = sid.clone();
        handles.push(tokio::spawn(async move {
            client
                .approve_dispatch(auth_req(
                    ApprovalReq {
                        sid: Some(sid_clone),
                        dispatch_id: format!("dispatch-{i}"),
                    },
                    "alice",
                ))
                .await
        }));
    }

    let mut wins = 0;
    let mut conflicts = 0;
    for h in handles {
        match h.await.expect("join") {
            Ok(_) => wins += 1,
            Err(e) if e.code() == tonic::Code::AlreadyExists => conflicts += 1,
            Err(e) => panic!("unexpected error: {e:?}"),
        }
    }
    assert_eq!(wins, 1, "exactly one winner under concurrency");
    assert_eq!(conflicts, 15, "everyone else sees AlreadyExists");

    env.shutdown().await;
}

#[tokio::test]
async fn approve_dispatch_rejects_missing_sid() {
    let env = TestEnv::start_with_mocks().await;
    let mut client = env.agent_client().await;
    let err = client
        .approve_dispatch(auth_req(
            ApprovalReq {
                sid: None,
                dispatch_id: "x".to_string(),
            },
            "alice",
        ))
        .await
        .expect_err("missing sid");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    env.shutdown().await;
}
