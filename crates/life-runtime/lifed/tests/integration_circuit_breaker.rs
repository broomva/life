//! Sub-phase D7 / E1: chaos test for the per-substrate circuit breaker.
//!
//! The acceptance criterion (Spec C₂ §7.2 + sub-phase E acceptance):
//!
//! > Killing lago does not cascade to arcan calls; the lago breaker
//! > opens within 30 s; recovers when lago returns.
//!
//! In the test harness we don't actually kill a process; we set
//! `MockLago::force_fail = true` so every lago RPC returns
//! `Unavailable`. Sub-phase E removed the `TestEnv::record_lago_failures`
//! stopgap — pool bracketing now lives inside `lago_proxy::Pooled<...>`
//! so every `Agent.CreateSession` call drives the lago breaker through
//! the real saga round-trip. After `FAILURE_THRESHOLD` failed
//! `Agent.CreateSession` attempts the lago breaker observably trips
//! Open without touching the arcan breaker.

#[path = "_support/mod.rs"]
mod _support;

use _support::test_env::TestEnv;

#[tokio::test]
async fn lago_failure_opens_breaker_without_cascading_to_arcan() {
    let env = TestEnv::start_with_mocks().await;

    // Baseline: every breaker is Closed.
    assert_eq!(
        env.lago_breaker_state(),
        lifed::routing::breaker::BreakerState::Closed,
        "lago breaker starts Closed",
    );
    assert_eq!(
        env.arcan_breaker_state(),
        lifed::routing::breaker::BreakerState::Closed,
        "arcan breaker starts Closed",
    );

    // Simulate a sustained lago outage: every RPC returns Unavailable.
    // Sub-phase E: pool bracketing is inside `lago_proxy::Pooled<...>`,
    // so every saga's `lago.open_namespace` call records a failure on
    // the lago breaker. After FAILURE_THRESHOLD attempts the breaker
    // trips Open. Each attempt also touches arcan.create_agent — but
    // arcan keeps returning Ok, so its breaker stays Closed.
    env.fail_lago();
    for _ in 0..lifed::routing::breaker::FAILURE_THRESHOLD {
        let _ = env.create_session_dev("alice", "p", "chaos").await;
    }

    assert_eq!(
        env.lago_breaker_state(),
        lifed::routing::breaker::BreakerState::Open,
        "lago breaker Open after threshold failed dispatches",
    );

    // The arcan breaker is unaffected — no cascade.
    assert_eq!(
        env.arcan_breaker_state(),
        lifed::routing::breaker::BreakerState::Closed,
        "arcan breaker stayed Closed during lago outage",
    );

    // Recover: the lago breaker remains Open until the 10 s window
    // elapses (Spec C₂ §7.2). Force the open window past via the
    // life-runtime-pool test-support helper so we don't sleep in CI.
    env.force_lago_open_window_elapsed();
    env.recover_lago();
    let _ok = env
        .create_session_dev("alice", "p", "post-recovery")
        .await
        .expect("session after lago recovery");
    // A successful arcan call keeps the arcan breaker Closed.
    assert_eq!(
        env.arcan_breaker_state(),
        lifed::routing::breaker::BreakerState::Closed,
    );

    env.shutdown().await;
}

#[tokio::test]
async fn breaker_state_is_observable_for_metric_export() {
    let env = TestEnv::start_with_mocks().await;
    use lifed::routing::breaker::BreakerState;
    assert_eq!(env.lago_breaker_state().as_metric_value(), 0);
    // Sub-phase E: drive the breaker through real handler traffic
    // rather than the removed `record_lago_failures` stopgap.
    env.fail_lago();
    for _ in 0..lifed::routing::breaker::FAILURE_THRESHOLD {
        let _ = env.create_session_dev("alice", "p", "metric").await;
    }
    assert_eq!(env.lago_breaker_state().as_metric_value(), 2);
    let _ = BreakerState::Open;
    env.shutdown().await;
}
