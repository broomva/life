//! M5 sub-phase B Task B15 acceptance: saga compensation under deliberate
//! failure injection per Spec C₂ §15.3. One test per saga step.
//!
//! Saga step order (Spec C₂ §4.2):
//!   1. CreateAgent          (arcan)
//!   2. OpenLagoNamespace    (lago)
//!   3. BindWallet           (haima)
//!   4. RegisterAnimaSession (anima)
//!
//! When step N fails, steps 1..(N-1) MUST be compensated in reverse.

#[path = "_support/mod.rs"]
mod _support;

use _support::test_env::TestEnv;

#[tokio::test]
async fn create_session_compensates_on_anima_failure() {
    let env = TestEnv::start_with_mocks().await;
    env.inject_anima_fault();
    let res = env.create_session_dev("alice", "p", "test").await;
    assert!(res.is_err(), "saga must fail when anima fails");
    {
        // arcan, lago, haima all forwarded — all must be compensated.
        let arcan = env.mocks.arcan.destroy_agent_calls.lock();
        let lago = env.mocks.lago.close_namespace_calls.lock();
        let haima = env.mocks.haima.unbind_wallet_calls.lock();
        assert_eq!(arcan.len(), 1, "arcan destroyed");
        assert_eq!(lago.len(), 1, "lago closed");
        assert_eq!(haima.len(), 1, "haima unbound");
    }
    env.shutdown().await;
}

#[tokio::test]
async fn create_session_compensates_on_haima_failure() {
    let env = TestEnv::start_with_mocks().await;
    env.inject_haima_fault();
    let res = env.create_session_dev("alice", "p", "test").await;
    assert!(res.is_err(), "saga must fail when haima fails");
    {
        let arcan = env.mocks.arcan.destroy_agent_calls.lock();
        let lago = env.mocks.lago.close_namespace_calls.lock();
        let anima = env.mocks.anima.register_session_calls.lock();
        assert_eq!(arcan.len(), 1, "arcan destroyed");
        assert_eq!(lago.len(), 1, "lago closed");
        assert_eq!(anima.len(), 0, "anima never registered");
    }
    env.shutdown().await;
}

#[tokio::test]
async fn create_session_compensates_on_lago_failure() {
    let env = TestEnv::start_with_mocks().await;
    env.inject_lago_fault();
    let res = env.create_session_dev("alice", "p", "test").await;
    assert!(res.is_err(), "saga must fail when lago fails");
    {
        let arcan = env.mocks.arcan.destroy_agent_calls.lock();
        let haima = env.mocks.haima.bind_wallet_calls.lock();
        assert_eq!(arcan.len(), 1, "arcan destroyed");
        assert_eq!(haima.len(), 0, "haima never bound");
    }
    env.shutdown().await;
}

#[tokio::test]
async fn create_session_compensates_on_arcan_failure() {
    let env = TestEnv::start_with_mocks().await;
    env.inject_arcan_fault();
    let res = env.create_session_dev("alice", "p", "test").await;
    assert!(res.is_err(), "saga must fail when arcan fails");
    {
        // arcan failed first; nothing downstream should have been called.
        let lago = env.mocks.lago.open_namespace_calls.lock();
        let haima = env.mocks.haima.bind_wallet_calls.lock();
        assert_eq!(lago.len(), 0, "lago never opened");
        assert_eq!(haima.len(), 0, "haima never bound");
    }
    env.shutdown().await;
}
