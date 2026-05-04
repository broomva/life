//! Smoke test — constructs every public type in the crate.
//!
//! This is the v0.0 scaffold's only integration test. It confirms the
//! public API surface compiles and is reachable from a downstream crate.
//! Real per-level integration tests (asserting `λ̂_0 > 0` on a sandboxed
//! Life run, etc.) land with v0.1.

use std::time::Duration;

use life_perturb::injector::{
    L0ProviderInjector, L1AutonomicInjector, L2AutoanyInjector, L3PolicyInjector,
};
use life_perturb::lyapunov::{V0Plant, V1Autonomic, V2Autoany, V3Governance};
use life_perturb::{
    Injector, LambdaEstimator, Level, LyapunovFn, LyapunovSample, Perturbation, PerturbationHandle,
    PerturbationId, PerturbationKind, RecoveryFit, Severity, SystemSnapshot,
};

#[test]
fn public_types_compile_and_construct() {
    let _: Level = Level::L0;
    let _: Severity = Severity::Mild;
    let _: PerturbationId = PerturbationId::new();

    let p = Perturbation::new(
        PerturbationKind::RateLimitStorm {
            rps: 100.0,
            duration: Duration::from_secs(60),
        },
        Duration::from_secs(60),
    );
    assert_eq!(p.level, Level::L0);

    let h = PerturbationHandle::for_perturbation(&p);
    assert_eq!(h.perturbation_id, p.id);

    let _: V0Plant = V0Plant::default();
    let _: V1Autonomic = V1Autonomic::default();
    let _: V2Autoany = V2Autoany::default();
    let _: V3Governance = V3Governance::default();

    let snap = SystemSnapshot::default();
    assert_eq!(V0Plant::default().compute(&snap), 0.0);

    let _ = RecoveryFit::default();
    let _ = LyapunovSample::new(0, 1.0);

    let mut est = LambdaEstimator::new(Level::L1, PerturbationId::new());
    est.push(LyapunovSample::new(0, 1.0));
    est.push(LyapunovSample::new(100, 0.95));
    est.push(LyapunovSample::new(200, 0.90));
    let fit = est.fit_recovery().expect("3 positive samples is enough");
    assert!(fit.r_squared >= 0.0);

    // Stub injectors compile and report their levels.
    assert_eq!(L0ProviderInjector.level(), Level::L0);
    assert_eq!(L1AutonomicInjector.level(), Level::L1);
    assert_eq!(L2AutoanyInjector.level(), Level::L2);
    assert_eq!(L3PolicyInjector.level(), Level::L3);
}
