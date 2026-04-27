//! M5 sub-phase B Task B13 acceptance: life.v1.Wallet end-to-end against
//! the mock haima substrate.

#[path = "_support/mod.rs"]
mod _support;

use _support::test_env::TestEnv;
use life_runtime_proto::life::v1::{DebitReq, WalletRef};

#[tokio::test]
async fn get_balance_returns_canned_value() {
    let env = TestEnv::start_with_mocks().await;
    let mut client = env.wallet_client().await;
    let mut req = tonic::Request::new(WalletRef {
        user_id: "alice".to_string(),
        project_id: "p".to_string(),
    });
    req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let bal = client.get_balance(req).await.expect("balance").into_inner();
    assert!(bal.micros > 0);
    env.shutdown().await;
}

#[tokio::test]
async fn debit_with_idempotency_key_replays() {
    let env = TestEnv::start_with_mocks().await;
    let mut client = env.wallet_client().await;
    let req = || {
        let mut r = tonic::Request::new(DebitReq {
            wallet: Some(WalletRef {
                user_id: "alice".to_string(),
                project_id: "p".to_string(),
            }),
            amount_micros: 1000,
            sid: "sid-1".to_string(),
            reason: "test".to_string(),
        });
        r.metadata_mut().insert(
            "authorization",
            "Bearer test-token-for-alice".parse().unwrap(),
        );
        r.metadata_mut()
            .insert("idempotency-key", "key-1".parse().unwrap());
        r
    };
    let r1 = client.debit(req()).await.expect("first").into_inner();
    let r2 = client.debit(req()).await.expect("replay").into_inner();
    assert_eq!(r1.entry_id, r2.entry_id, "replay returns same entry_id");
    env.shutdown().await;
}

#[tokio::test]
async fn debit_without_idempotency_key_fails_precondition() {
    let env = TestEnv::start_with_mocks().await;
    let mut client = env.wallet_client().await;
    let mut req = tonic::Request::new(DebitReq {
        wallet: Some(WalletRef {
            user_id: "alice".to_string(),
            project_id: "p".to_string(),
        }),
        amount_micros: 1000,
        sid: "sid-1".to_string(),
        reason: "test".to_string(),
    });
    req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let err = client.debit(req).await.expect_err("missing idem-key");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    env.shutdown().await;
}

#[tokio::test]
async fn transfer_returns_both_balances() {
    use life_runtime_proto::life::v1::TransferReq;
    let env = TestEnv::start_with_mocks().await;
    let mut client = env.wallet_client().await;
    let mut req = tonic::Request::new(TransferReq {
        from: Some(WalletRef {
            user_id: "alice".to_string(),
            project_id: "p".to_string(),
        }),
        to: Some(WalletRef {
            user_id: "bob".to_string(),
            project_id: "p".to_string(),
        }),
        amount_micros: 1000,
        memo: "test".to_string(),
    });
    req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let out = client.transfer(req).await.expect("transfer").into_inner();
    assert!(out.from_balance.is_some());
    assert!(out.to_balance.is_some());
    env.shutdown().await;
}
