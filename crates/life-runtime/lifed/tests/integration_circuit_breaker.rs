//! Sub-phase D7: chaos test for the per-substrate circuit breaker.
//!
//! The acceptance criterion (Spec C₂ §7.2 + sub-phase D9):
//!
//! > Killing lago does not cascade to arcan calls; the lago breaker
//! > opens within 30 s; recovers when lago returns.
//!
//! In the test harness we don't actually kill a process; we set
//! `MockLago::force_fail = true` so every lago RPC returns
//! `Unavailable`. Using the `record_lago_failures` accessor we drive
//! the pool's hand-rolled breaker to Open after 5 failures and assert
//! the rest of the daemon (and other substrates) keep working.

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

    // Simulate a sustained lago outage: every RPC fails. Then drive the
    // pool's breaker to Open by recording 5 consecutive failures.
    env.fail_lago();
    env.record_lago_failures(lifed::routing::breaker::FAILURE_THRESHOLD);

    assert_eq!(
        env.lago_breaker_state(),
        lifed::routing::breaker::BreakerState::Open,
        "lago breaker Open after threshold failures",
    );

    // The arcan breaker is unaffected — no cascade.
    assert_eq!(
        env.arcan_breaker_state(),
        lifed::routing::breaker::BreakerState::Closed,
        "arcan breaker stayed Closed during lago outage",
    );

    // Recover: lago returns, future arcan dispatches should still work.
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
    // Each substrate must expose a numeric metric value (0/1/2 for
    // Closed/HalfOpen/Open) for the `life.daemon.breaker_state` series.
    use lifed::routing::breaker::BreakerState;
    assert_eq!(env.lago_breaker_state().as_metric_value(), 0);
    env.record_lago_failures(lifed::routing::breaker::FAILURE_THRESHOLD);
    assert_eq!(env.lago_breaker_state().as_metric_value(), 2);
    let _ = BreakerState::Open;
    env.shutdown().await;
}
