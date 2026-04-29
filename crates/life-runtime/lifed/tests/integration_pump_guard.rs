//! Sub-phase E: panic-safe `PumpGuard` integration.
//!
//! Acceptance: a panicking pump task releases the per-session pump slot
//! via the RAII `Drop` impl on [`lifed::services::agent::PumpGuard`].
//! Spec C₂ §6.4 invariant: at most one upstream arcan dispatch pump per
//! session.

use std::sync::Arc;

use lifed::routing::fanout::FanoutRegistry;
use lifed::services::agent::PumpGuard;

#[tokio::test]
async fn pump_guard_releases_slot_on_drop() {
    let registry = Arc::new(FanoutRegistry::new());
    {
        let guard = PumpGuard::try_claim(Arc::clone(&registry), "sid-x".to_string())
            .expect("first claim wins");
        assert!(registry.is_pump_active(), "claim sets the active flag");
        // Test the documented Drop release path — guard goes out of scope.
        drop(guard);
    }
    assert!(!registry.is_pump_active(), "Drop releases the pump slot");
    assert!(
        PumpGuard::try_claim(Arc::clone(&registry), "sid-x".to_string()).is_some(),
        "slot reusable after release",
    );
}

#[tokio::test]
async fn second_concurrent_claim_loses_pump_slot() {
    let registry = Arc::new(FanoutRegistry::new());
    let _first =
        PumpGuard::try_claim(Arc::clone(&registry), "sid-y".to_string()).expect("first claim wins");
    let second = PumpGuard::try_claim(Arc::clone(&registry), "sid-y".to_string());
    assert!(second.is_none(), "second concurrent claim loses");
}

/// Sub-phase E: under task panic the slot must still release.
///
/// Spawns a task that claims the pump slot and panics. After the join
/// completes, the registry's pump-active flag must be `false` so a
/// subsequent claim succeeds. This proves the RAII `Drop` on PumpGuard
/// runs even on unwind.
#[tokio::test]
async fn pump_guard_releases_slot_on_panic() {
    let registry = Arc::new(FanoutRegistry::new());
    let registry_in_task = Arc::clone(&registry);
    let handle = tokio::spawn(async move {
        let _guard = PumpGuard::try_claim(registry_in_task, "sid-panic".to_string())
            .expect("first claim wins");
        // Yield once so the runtime materialises the spawn boundary.
        tokio::task::yield_now().await;
        panic!("simulated pump-task panic");
    });
    // Spawn-task panic surfaces as `Err(JoinError { panicked })`.
    assert!(handle.await.is_err(), "panicking task observed");
    // Slot must release because PumpGuard's Drop ran on unwind.
    assert!(
        !registry.is_pump_active(),
        "Drop ran during unwind and released the slot"
    );
    assert!(
        PumpGuard::try_claim(Arc::clone(&registry), "sid-panic".to_string()).is_some(),
        "next claim wins because the pump slot is free again",
    );
}
