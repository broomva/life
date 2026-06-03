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
    req.metadata_mut()
        .insert("idempotency-key", "transfer-1".parse().unwrap());
    let out = client.transfer(req).await.expect("transfer").into_inner();
    assert!(out.from_balance.is_some());
    assert!(out.to_balance.is_some());
    env.shutdown().await;
}

/// Sub-phase D8 follow-up #3: Wallet.Transfer is now idempotent.
/// Replaying with the same idempotency-key returns the cached receipt
/// instead of issuing a second transfer.
#[tokio::test]
async fn transfer_with_idempotency_key_replays() {
    use life_runtime_proto::life::v1::TransferReq;
    let env = TestEnv::start_with_mocks().await;
    let mut client = env.wallet_client().await;
    let req = || {
        let mut r = tonic::Request::new(TransferReq {
            from: Some(WalletRef {
                user_id: "alice".to_string(),
                project_id: "p".to_string(),
            }),
            to: Some(WalletRef {
                user_id: "bob".to_string(),
                project_id: "p".to_string(),
            }),
            amount_micros: 1500,
            memo: "test".to_string(),
        });
        r.metadata_mut().insert(
            "authorization",
            "Bearer test-token-for-alice".parse().unwrap(),
        );
        r.metadata_mut()
            .insert("idempotency-key", "transfer-replay".parse().unwrap());
        r
    };
    let r1 = client.transfer(req()).await.expect("first").into_inner();
    let r2 = client.transfer(req()).await.expect("replay").into_inner();
    assert_eq!(
        r1.entry_id, r2.entry_id,
        "Wallet.Transfer replay returns same entry_id (idempotent)",
    );
    // The mock records every transfer call. With idempotency, only the
    // first call should reach haima.
    let transfer_count = env.mocks.haima.transfer_calls.lock().len();
    assert_eq!(
        transfer_count, 1,
        "haima.transfer called exactly once across two replays"
    );
    env.shutdown().await;
}

/// BRO-1354: life.v1.Wallet.X402Pay end-to-end against the mock haima
/// substrate. The mock returns a canned base-sepolia "settled" outcome,
/// so this proves the lifed handler marshals the request to haima-proxy
/// and maps the X402PayOutcome onto the wire response.
#[tokio::test]
async fn x402_pay_returns_settled_outcome() {
    use life_runtime_proto::life::v1::X402PayReq;
    let env = TestEnv::start_with_mocks().await;
    let mut client = env.wallet_client().await;
    let mut req = tonic::Request::new(X402PayReq {
        user_id: "alice".to_string(),
        project_id: "p".to_string(),
        resource_url: "https://example.test/api/data".to_string(),
        network: "base-sepolia".to_string(),
        max_amount_micros: None,
    });
    req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let out = client.x402_pay(req).await.expect("x402_pay").into_inner();
    assert_eq!(out.status, "settled");
    assert!(out.settled);
    assert_eq!(out.network, "eip155:84532");
    // The handler reached the haima substrate exactly once with the
    // caller's (user, project).
    {
        let calls = env.mocks.haima.x402_pay_calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "alice");
        assert_eq!(calls[0].1, "p");
    }
    env.shutdown().await;
}

/// BRO-1354 hardening (P20 cross-review): a capability for `alice`
/// cannot initiate an x402 payment naming `bob` as the payer — the
/// handler binds the payer to the authenticated subject.
#[tokio::test]
async fn x402_pay_rejects_cross_user_payer() {
    use life_runtime_proto::life::v1::X402PayReq;
    let env = TestEnv::start_with_mocks().await;
    let mut client = env.wallet_client().await;
    let mut req = tonic::Request::new(X402PayReq {
        user_id: "bob".to_string(), // ≠ the token subject (alice)
        project_id: "p".to_string(),
        resource_url: "https://example.test/api/data".to_string(),
        network: "base-sepolia".to_string(),
        max_amount_micros: None,
    });
    req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let err = client
        .x402_pay(req)
        .await
        .expect_err("cross-user must reject");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    // The substrate was never reached.
    assert_eq!(env.mocks.haima.x402_pay_calls.lock().len(), 0);
    env.shutdown().await;
}

#[tokio::test]
async fn transfer_without_idempotency_key_fails_precondition() {
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
        amount_micros: 100,
        memo: "test".to_string(),
    });
    req.metadata_mut().insert(
        "authorization",
        "Bearer test-token-for-alice".parse().unwrap(),
    );
    let err = client.transfer(req).await.expect_err("missing idem-key");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    env.shutdown().await;
}
