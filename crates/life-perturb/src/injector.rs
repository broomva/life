//! Per-level injector trait + stub implementations.
//!
//! Real injectors hook the daemons (`arcand`, `autonomicd`, future
//! `autoanyd`) under a `--perturb-mode` feature flag. The v0.0 scaffold
//! ships only the trait surface and four stub structs that return
//! `PerturbError::NotImplemented` so consumers can compile against the
//! shape without the runtime hooks landing yet.
//!
//! See spec §4 + §6 for the integration contract.

use async_trait::async_trait;

use crate::error::{PerturbError, PerturbResult};
use crate::perturbation::{Level, Perturbation};

/// Opaque revert token returned by `Injector::inject` and consumed by
/// `Injector::revert`. Implementations may stash arbitrary state here
/// (Tower middleware handles, `Arc<Mutex<…>>` overrides, etc.).
#[derive(Debug, Clone)]
pub struct PerturbationHandle {
    /// The perturbation that produced this handle.
    pub perturbation_id: crate::perturbation::PerturbationId,
    /// Wall-clock time the perturbation took effect.
    pub injected_at: chrono::DateTime<chrono::Utc>,
}

impl PerturbationHandle {
    /// Construct a handle tagged with `now()`.
    pub fn for_perturbation(p: &Perturbation) -> Self {
        Self {
            perturbation_id: p.id,
            injected_at: chrono::Utc::now(),
        }
    }
}

/// One implementation per level. Concrete injectors live in this crate as
/// stubs and will be filled in by the per-level workstreams (v0.1 → v1.0).
#[async_trait]
pub trait Injector: Send + Sync {
    /// The hierarchy level this injector targets.
    fn level(&self) -> Level;

    /// Apply `p` to the live runtime. The returned handle MUST be passed
    /// to [`Injector::revert`] to restore the runtime to baseline.
    async fn inject(&self, p: &Perturbation) -> PerturbResult<PerturbationHandle>;

    /// Reverse the perturbation identified by `handle`.
    async fn revert(&self, handle: PerturbationHandle) -> PerturbResult<()>;
}

// ─── Stub implementations ────────────────────────────────────────────────

/// L0 injector — wires into `arcan-provider` middleware. v0.1 target.
#[derive(Debug, Clone, Default)]
pub struct L0ProviderInjector;

#[async_trait]
impl Injector for L0ProviderInjector {
    fn level(&self) -> Level {
        Level::L0
    }
    async fn inject(&self, p: &Perturbation) -> PerturbResult<PerturbationHandle> {
        // v0.0 scaffold: signal "shape is right, body deferred".
        Err(PerturbError::NotImplemented {
            level: Level::L0,
            kind: p.kind.name(),
        })
    }
    async fn revert(&self, _handle: PerturbationHandle) -> PerturbResult<()> {
        Ok(())
    }
}

/// L1 injector — wires into `autonomic-controller` HysteresisGate. v0.5 target.
#[derive(Debug, Clone, Default)]
pub struct L1AutonomicInjector;

#[async_trait]
impl Injector for L1AutonomicInjector {
    fn level(&self) -> Level {
        Level::L1
    }
    async fn inject(&self, p: &Perturbation) -> PerturbResult<PerturbationHandle> {
        Err(PerturbError::NotImplemented {
            level: Level::L1,
            kind: p.kind.name(),
        })
    }
    async fn revert(&self, _handle: PerturbationHandle) -> PerturbResult<()> {
        Ok(())
    }
}

/// L2 injector — wires into `autoany-core::loop_engine`. v1.0 target.
#[derive(Debug, Clone, Default)]
pub struct L2AutoanyInjector;

#[async_trait]
impl Injector for L2AutoanyInjector {
    fn level(&self) -> Level {
        Level::L2
    }
    async fn inject(&self, p: &Perturbation) -> PerturbResult<PerturbationHandle> {
        Err(PerturbError::NotImplemented {
            level: Level::L2,
            kind: p.kind.name(),
        })
    }
    async fn revert(&self, _handle: PerturbationHandle) -> PerturbResult<()> {
        Ok(())
    }
}

/// L3 injector — sandbox-only writer for `.control/policy.yaml`. v1.0 target.
#[derive(Debug, Clone, Default)]
pub struct L3PolicyInjector;

#[async_trait]
impl Injector for L3PolicyInjector {
    fn level(&self) -> Level {
        Level::L3
    }
    async fn inject(&self, p: &Perturbation) -> PerturbResult<PerturbationHandle> {
        Err(PerturbError::NotImplemented {
            level: Level::L3,
            kind: p.kind.name(),
        })
    }
    async fn revert(&self, _handle: PerturbationHandle) -> PerturbResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perturbation::PerturbationKind;
    use std::time::Duration;

    #[test]
    fn levels_match_injector_kind() {
        assert_eq!(L0ProviderInjector.level(), Level::L0);
        assert_eq!(L1AutonomicInjector.level(), Level::L1);
        assert_eq!(L2AutoanyInjector.level(), Level::L2);
        assert_eq!(L3PolicyInjector.level(), Level::L3);
    }

    #[tokio::test]
    async fn stub_injectors_return_not_implemented() {
        let p = Perturbation::new(
            PerturbationKind::RateLimitStorm {
                rps: 1.0,
                duration: Duration::from_secs(1),
            },
            Duration::from_secs(1),
        );
        let inj = L0ProviderInjector;
        let err = inj.inject(&p).await.expect_err("scaffold returns error");
        match err {
            PerturbError::NotImplemented { level, .. } => assert_eq!(level, Level::L0),
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
