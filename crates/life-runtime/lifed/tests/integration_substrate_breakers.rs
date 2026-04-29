//! Sub-phase E end-to-end smoke: each of the four substrate breakers
//! exercised by real handler traffic.
//!
//! Acceptance per Spec C₂ §7.2 + §15.2:
//!
//! > Killing each substrate independently must trip its own breaker and
//! > leave the other three unaffected. The dispatch path that drives
//! > each breaker is a real public-plane RPC against the running daemon.
//!
//! After Sub-phase E the pool bracketing lives inside each proxy crate's
//! `Pooled<C>` adapter — we no longer need test-only stopgaps to bump
//! breakers directly. Each test forces the relevant mock substrate into
//! sustained failure mode and issues `FAILURE_THRESHOLD` real RPCs that
//! touch the substrate. The breaker observably trips Open. The remaining
//! three breakers stay Closed.

#[path = "_support/mod.rs"]
mod _support;

use _support::test_env::TestEnv;
use lifed::routing::breaker::{BreakerState, FAILURE_THRESHOLD};

#[tokio::test]
async fn lago_breaker_opens_via_real_create_session_traffic() {
    let env = TestEnv::start_with_mocks().await;
    env.fail_lago();
    for _ in 0..FAILURE_THRESHOLD {
        let _ = env.create_session_dev("alice", "p", "lago-chaos").await;
    }
    assert_eq!(env.lago_breaker_state(), BreakerState::Open);
    assert_eq!(env.arcan_breaker_state(), BreakerState::Closed);
    assert_eq!(env.haima_breaker_state(), BreakerState::Closed);
    assert_eq!(env.anima_breaker_state(), BreakerState::Closed);
    env.shutdown().await;
}

#[tokio::test]
async fn arcan_breaker_opens_via_real_create_session_traffic() {
    let env = TestEnv::start_with_mocks().await;
    // Sustained arcan failure: every saga's first step fails.
    env.mocks.arcan.set_force_fail(true);
    for _ in 0..FAILURE_THRESHOLD {
        let _ = env.create_session_dev("alice", "p", "arcan-chaos").await;
    }
    assert_eq!(env.arcan_breaker_state(), BreakerState::Open);
    assert_eq!(env.lago_breaker_state(), BreakerState::Closed);
    assert_eq!(env.haima_breaker_state(), BreakerState::Closed);
    assert_eq!(env.anima_breaker_state(), BreakerState::Closed);
    env.shutdown().await;
}

#[tokio::test]
async fn haima_breaker_opens_via_real_create_session_traffic() {
    let env = TestEnv::start_with_mocks().await;
    env.mocks.haima.set_force_fail(true);
    for _ in 0..FAILURE_THRESHOLD {
        let _ = env.create_session_dev("alice", "p", "haima-chaos").await;
    }
    assert_eq!(env.haima_breaker_state(), BreakerState::Open);
    // arcan + lago succeed before the haima step; their breakers stay
    // Closed because successful calls reset the counter.
    assert_eq!(env.arcan_breaker_state(), BreakerState::Closed);
    assert_eq!(env.lago_breaker_state(), BreakerState::Closed);
    assert_eq!(env.anima_breaker_state(), BreakerState::Closed);
    env.shutdown().await;
}

#[tokio::test]
async fn anima_breaker_opens_via_real_create_session_traffic() {
    let env = TestEnv::start_with_mocks().await;
    env.mocks.anima.set_force_fail(true);
    for _ in 0..FAILURE_THRESHOLD {
        let _ = env.create_session_dev("alice", "p", "anima-chaos").await;
    }
    assert_eq!(env.anima_breaker_state(), BreakerState::Open);
    assert_eq!(env.arcan_breaker_state(), BreakerState::Closed);
    assert_eq!(env.lago_breaker_state(), BreakerState::Closed);
    assert_eq!(env.haima_breaker_state(), BreakerState::Closed);
    env.shutdown().await;
}
