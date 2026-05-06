//! L0 plant-level perturbation injection (v0.1).
//!
//! This module implements the first concrete `Injector` + `LyapunovFn`
//! pair on the path to closing the construct-validity gap with the RCS
//! paper's analytic λ_i values.
//!
//! ## Scope (v0.1-L0)
//!
//! - **One** perturbation kind: [`PerturbationKind::ToolLatencyJitter`].
//!   Picked over `RateLimitStorm` / `RequestDrop` because it is the
//!   cleanest, most reversible knob (mean + jitter) and exercises the
//!   tool-latency residual term in `V_0Plant`.
//! - Real V_0 probe derived from autonomic [`HomeostaticState`] —
//!   `tool_density` and `context_pressure` together drive the L0 plant
//!   stress proxy until the full `arcan-provider` middleware integration
//!   lands in v0.5.
//! - Simulation tick harness so a closed loop (`inject → tick → sample
//!   V_0(t) → fit λ̂_0`) can be exercised end-to-end without the live
//!   daemon dependency. The `arcand` Tower-layer wiring is explicitly
//!   deferred per spec §7 (v0.5).
//!
//! Gated behind the `inject-l0` Cargo feature so the default build of
//! `life-perturb` stays a passive scaffold.
//!
//! ## End-to-end flow
//!
//! ```ignore
//! # #[cfg(feature = "inject-l0")] {
//! use std::sync::Arc;
//! use std::time::Duration;
//! use life_perturb::{
//!     L0LatencyInjectionState, L0ProviderInjector, Injector,
//!     L0SimRuntime, LambdaEstimator, Level, Perturbation,
//!     PerturbationKind, V0Plant,
//! };
//!
//! # tokio_test::block_on(async {
//! let state = Arc::new(L0LatencyInjectionState::default());
//! let injector = L0ProviderInjector::new(Arc::clone(&state));
//! let v0 = V0Plant { w_latency: 1.0, w_drop: 0.0, w_health: 0.0 };
//!
//! let p = Perturbation::new(
//!     PerturbationKind::ToolLatencyJitter {
//!         mean_ms: 250, jitter_ms: 50, duration: Duration::from_secs(5),
//!     },
//!     Duration::from_secs(5),
//! );
//! let handle = injector.inject(&p).await.unwrap();
//!
//! let mut sim = L0SimRuntime::new(Arc::clone(&state), v0);
//! let mut est = LambdaEstimator::new(Level::L0, p.id);
//! for _ in 0..40 { est.push(sim.tick(Duration::from_millis(100))); }
//! injector.revert(handle).await.unwrap();
//! let fit = est.fit_recovery().unwrap();
//! assert!(fit.lambda_hat > 0.0);
//! # });
//! # }
//! ```

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rand::{Rng, SeedableRng, rngs::StdRng};

use autonomic_core::gating::HomeostaticState;
use autonomic_core::rcs_budget::MarginEstimator;

use crate::error::{PerturbError, PerturbResult};
use crate::injector::{Injector, PerturbationHandle};
use crate::lyapunov::{L0Probe, LyapunovFn, LyapunovSample, SystemSnapshot, V0Plant};
use crate::perturbation::{Level, Perturbation, PerturbationKind};

// ─── Shared injection state ─────────────────────────────────────────────

/// Mutable state advertised by the L0 latency injector. Whatever fronts
/// the provider boundary (Tower middleware in v0.5, the simulation
/// runtime in v0.1) reads these atomics on every tool call to decide
/// whether and by how much to delay.
#[derive(Debug)]
pub struct L0LatencyInjectionState {
    /// Whether a latency injection is currently active.
    active: AtomicBool,
    /// Current target mean delay (ms) — read into the simulation/middleware.
    mean_ms: AtomicU32,
    /// Current jitter half-width (ms).
    jitter_ms: AtomicU32,
    /// Most-recent injection identifier (UUID-low) for diagnostics.
    seed: AtomicU32,
}

impl Default for L0LatencyInjectionState {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(false),
            mean_ms: AtomicU32::new(0),
            jitter_ms: AtomicU32::new(0),
            seed: AtomicU32::new(0),
        }
    }
}

impl L0LatencyInjectionState {
    /// Whether an injection is currently active.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Current target mean delay in milliseconds (0 if not active).
    pub fn mean_ms(&self) -> u32 {
        self.mean_ms.load(Ordering::Acquire)
    }

    /// Current jitter half-width in milliseconds.
    pub fn jitter_ms(&self) -> u32 {
        self.jitter_ms.load(Ordering::Acquire)
    }

    fn arm(&self, mean_ms: u32, jitter_ms: u32, seed: u32) {
        self.mean_ms.store(mean_ms, Ordering::Release);
        self.jitter_ms.store(jitter_ms, Ordering::Release);
        self.seed.store(seed, Ordering::Release);
        self.active.store(true, Ordering::Release);
    }

    fn disarm(&self) {
        self.active.store(false, Ordering::Release);
        self.mean_ms.store(0, Ordering::Release);
        self.jitter_ms.store(0, Ordering::Release);
    }
}

// ─── Real L0 injector ───────────────────────────────────────────────────

/// Concrete L0 perturbation injector. v0.1 supports
/// [`PerturbationKind::ToolLatencyJitter`]; other L0 kinds remain
/// `NotImplemented` per spec §7.
#[derive(Debug, Clone)]
pub struct L0ProviderInjector {
    state: Arc<L0LatencyInjectionState>,
}

impl L0ProviderInjector {
    /// Construct an injector that publishes its decisions to `state`.
    /// The provider boundary (or simulation runtime) reads from the same
    /// `Arc` to decide whether to delay tool calls.
    pub fn new(state: Arc<L0LatencyInjectionState>) -> Self {
        Self { state }
    }

    /// Inspect the shared injection state — useful for tests and
    /// observability.
    pub fn state(&self) -> &Arc<L0LatencyInjectionState> {
        &self.state
    }
}

#[async_trait]
impl Injector for L0ProviderInjector {
    fn level(&self) -> Level {
        Level::L0
    }

    async fn inject(&self, p: &Perturbation) -> PerturbResult<PerturbationHandle> {
        match &p.kind {
            PerturbationKind::ToolLatencyJitter {
                mean_ms, jitter_ms, ..
            } => {
                // Use the bottom 32 bits of the ULID as a diagnostic seed.
                let seed = (p.id.0.0 & 0xFFFF_FFFF) as u32;
                self.state.arm(*mean_ms, *jitter_ms, seed);
                tracing::info!(
                    target: "life_perturb::l0",
                    perturbation_id = %p.id.0,
                    mean_ms = *mean_ms,
                    jitter_ms = *jitter_ms,
                    "L0 latency injection armed",
                );
                Ok(PerturbationHandle::for_perturbation(p))
            }
            // Other L0 variants land in v0.5 per spec §7. Failing fast
            // here keeps the contract obvious during the v0.1 ramp.
            other => Err(PerturbError::NotImplemented {
                level: Level::L0,
                kind: other.name(),
            }),
        }
    }

    async fn revert(&self, handle: PerturbationHandle) -> PerturbResult<()> {
        self.state.disarm();
        tracing::info!(
            target: "life_perturb::l0",
            perturbation_id = %handle.perturbation_id.0,
            "L0 latency injection disarmed",
        );
        Ok(())
    }
}

// ─── V_0 probe derived from autonomic HomeostaticState ──────────────────

/// Derives a [`L0Probe`] from autonomic [`HomeostaticState`].
///
/// Mapping (see spec §3 V_0 proxy table):
///
/// - `tool_latency_residual` ← excess latency over baseline (in seconds),
///   computed by the runtime caller (we do not pull live timings here).
/// - `request_drop_rate`     ← `operational.error_streak / max(1, total_actions)`.
/// - `provider_health`       ← `1 − context_pressure` (clamped). v0.5
///   replaces this with a real provider-error-rate signal.
///
/// The probe is intentionally cheap and read-only; it does not allocate
/// once constructed and is `Send + Sync` so it can live behind an `Arc`
/// in the daemon's tick loop.
#[derive(Debug)]
pub struct L0AutonomicProbe {
    /// Most recent observed tool-latency residual (seconds), set by the
    /// runtime that wraps the provider.
    pub tool_latency_residual_s: Mutex<f64>,
}

impl Default for L0AutonomicProbe {
    fn default() -> Self {
        Self {
            tool_latency_residual_s: Mutex::new(0.0),
        }
    }
}

impl L0AutonomicProbe {
    /// Update the externally-observed tool latency residual (seconds).
    /// This is what the runtime reports after each tool call.
    pub fn record_tool_latency_residual(&self, residual_s: f64) {
        let mut guard = self
            .tool_latency_residual_s
            .lock()
            .expect("L0AutonomicProbe latency lock poisoned");
        *guard = residual_s.max(0.0);
    }

    /// Produce a snapshot suitable for [`V0Plant::compute`].
    pub fn snapshot(&self, state: &HomeostaticState) -> SystemSnapshot {
        let total_actions =
            state.operational.total_successes as f64 + state.operational.total_errors as f64;
        let drop_rate = if total_actions > 0.0 {
            state.operational.error_streak as f64 / total_actions
        } else {
            0.0
        };
        let provider_health = (1.0 - (state.cognitive.context_pressure as f64)).clamp(0.0, 1.0);

        let residual = *self
            .tool_latency_residual_s
            .lock()
            .expect("L0AutonomicProbe latency lock poisoned");

        SystemSnapshot {
            l0: Some(L0Probe {
                tool_latency_residual: residual,
                request_drop_rate: drop_rate,
                provider_health,
            }),
            ..SystemSnapshot::default()
        }
    }
}

// ─── Simulation runtime (test harness for the v0.1 closed loop) ─────────

/// In-process simulation that closes the loop for v0.1: it pretends to
/// be the provider boundary, observes the injection state on every tick,
/// applies the latency knob with a baseline → spike → recovery dynamic,
/// and emits `LyapunovSample`s suitable for [`crate::LambdaEstimator`].
///
/// This is *not* the production wiring — that lives in `arcan-provider`
/// behind `--perturb-mode` and lands in v0.5. The simulation gives us a
/// reproducible end-to-end test that the full pipeline (inject →
/// telemetry → fit) compiles, runs, and produces a positive λ̂_0.
pub struct L0SimRuntime {
    state: Arc<L0LatencyInjectionState>,
    v0: V0Plant,
    probe: L0AutonomicProbe,
    homeostatic: HomeostaticState,
    margin: MarginEstimator,
    rng: StdRng,
    /// Current observed tool-latency residual (seconds).
    current_residual_s: f64,
    /// Decay constant per tick (1 = instant restore, 0 = never recover).
    /// Drives the synthetic recovery curve when the injection is reverted.
    decay_per_s: f64,
    /// Baseline latency (seconds) the runtime reports under no injection.
    baseline_s: f64,
    /// Wall-clock cursor (ms).
    t_ms: u64,
}

impl L0SimRuntime {
    /// Construct a runtime tied to the given injection state and V_0
    /// computer. Defaults: baseline 50 ms, recovery τ ≈ 1 s.
    pub fn new(state: Arc<L0LatencyInjectionState>, v0: V0Plant) -> Self {
        let homeostatic = HomeostaticState::for_agent("life-perturb-sim");
        Self::with_seed(state, v0, homeostatic, 0xC0FF_EE42)
    }

    /// Construct with explicit deterministic RNG seed and baseline
    /// homeostatic state.
    pub fn with_seed(
        state: Arc<L0LatencyInjectionState>,
        v0: V0Plant,
        homeostatic: HomeostaticState,
        seed: u64,
    ) -> Self {
        let margin = MarginEstimator::for_l1(homeostatic.clone());
        Self {
            state,
            v0,
            probe: L0AutonomicProbe::default(),
            homeostatic,
            margin,
            rng: StdRng::seed_from_u64(seed),
            current_residual_s: 0.0,
            decay_per_s: 1.0,
            baseline_s: 0.050,
            t_ms: 0,
        }
    }

    /// Override the recovery decay rate (1/seconds). Higher = faster
    /// snap-back to baseline once the injector is reverted.
    pub fn set_decay_per_s(mut self, decay_per_s: f64) -> Self {
        self.decay_per_s = decay_per_s.max(0.0);
        self
    }

    /// Override the no-injection baseline latency (seconds).
    pub fn set_baseline_s(mut self, baseline_s: f64) -> Self {
        self.baseline_s = baseline_s.max(0.0);
        self
    }

    /// Borrow the embedded MarginEstimator (handy for assertions in
    /// integration tests that want to verify autonomic-side observations
    /// are flowing).
    pub fn margin_estimator(&self) -> &MarginEstimator {
        &self.margin
    }

    /// Advance one tick of `dt`. Reads the injection state, computes a
    /// fresh tool-latency residual (with stochastic jitter), updates the
    /// embedded autonomic projection, and returns the corresponding
    /// `V_0(t)` sample.
    pub fn tick(&mut self, dt: Duration) -> LyapunovSample {
        let dt_s = dt.as_secs_f64();
        self.t_ms = self.t_ms.saturating_add(dt.as_millis() as u64);

        let target_s = if self.state.is_active() {
            let mean_ms = self.state.mean_ms() as f64;
            let jitter_ms = self.state.jitter_ms() as f64;
            let jitter_s = if jitter_ms > 0.0 {
                let raw: f64 = self.rng.gen_range(-1.0..1.0);
                raw * (jitter_ms / 1000.0)
            } else {
                0.0
            };
            ((mean_ms / 1000.0) + jitter_s).max(self.baseline_s)
        } else {
            self.baseline_s
        };

        // First-order lag toward the target. When the injector is active
        // the residual climbs toward the elevated target; once it
        // reverts, the same dynamic recovers exponentially toward
        // baseline — exactly the curve we want to fit λ̂_0 against.
        let alpha = (1.0 - (-self.decay_per_s * dt_s).exp()).clamp(0.0, 1.0);
        self.current_residual_s += alpha * (target_s - self.current_residual_s);

        // The "residual" component for V_0 is the excess over baseline.
        let residual_excess = (self.current_residual_s - self.baseline_s).max(0.0);
        self.probe.record_tool_latency_residual(residual_excess);

        // Mirror the observation into autonomic: pretend each tick is a
        // tool call with elevated latency, so cognitive.tool_density
        // tracks the perturbation. This is what the real arcand wiring
        // would do — we just do it inline here.
        self.homeostatic.last_event_ms = self.t_ms;
        self.homeostatic.last_event_seq = self.homeostatic.last_event_seq.saturating_add(1);
        // Map current residual into context pressure ∈ [0, 1] for
        // provider health. Saturates at +200 ms above baseline.
        let pressure = (residual_excess / 0.2).clamp(0.0, 1.0) as f32;
        self.homeostatic.cognitive.context_pressure = pressure;
        self.homeostatic.cognitive.tool_density =
            (self.homeostatic.cognitive.tool_density * 0.9) + (residual_excess * 10.0);
        self.margin.observe(&self.homeostatic);

        let snapshot = self.probe.snapshot(&self.homeostatic);
        let v = self.v0.compute(&snapshot);

        LyapunovSample::new(self.t_ms, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::estimator::LambdaEstimator;
    use crate::perturbation::PerturbationKind;

    fn make_jitter_perturbation(mean_ms: u32, jitter_ms: u32) -> Perturbation {
        Perturbation::new(
            PerturbationKind::ToolLatencyJitter {
                mean_ms,
                jitter_ms,
                duration: Duration::from_secs(2),
            },
            Duration::from_secs(2),
        )
    }

    #[tokio::test]
    async fn injector_arms_and_disarms_state() {
        let state = Arc::new(L0LatencyInjectionState::default());
        let injector = L0ProviderInjector::new(Arc::clone(&state));
        assert!(!state.is_active());

        let p = make_jitter_perturbation(300, 25);
        let handle = injector.inject(&p).await.expect("inject ok");
        assert!(state.is_active());
        assert_eq!(state.mean_ms(), 300);
        assert_eq!(state.jitter_ms(), 25);

        injector.revert(handle).await.expect("revert ok");
        assert!(!state.is_active());
        assert_eq!(state.mean_ms(), 0);
        assert_eq!(state.jitter_ms(), 0);
    }

    #[tokio::test]
    async fn injector_rejects_unimplemented_kinds() {
        let state = Arc::new(L0LatencyInjectionState::default());
        let injector = L0ProviderInjector::new(state);

        let p = Perturbation::new(
            PerturbationKind::RateLimitStorm {
                rps: 100.0,
                duration: Duration::from_secs(1),
            },
            Duration::from_secs(1),
        );
        let err = injector.inject(&p).await.expect_err("rate-limit deferred");
        match err {
            PerturbError::NotImplemented { level, kind } => {
                assert_eq!(level, Level::L0);
                assert_eq!(kind, "RateLimitStorm");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn probe_clamps_negative_latency() {
        let probe = L0AutonomicProbe::default();
        probe.record_tool_latency_residual(-1.0);
        let state = HomeostaticState::for_agent("test");
        let snap = probe.snapshot(&state);
        assert_eq!(snap.l0.unwrap().tool_latency_residual, 0.0);
    }

    #[test]
    fn probe_derives_drop_rate_and_health() {
        let probe = L0AutonomicProbe::default();
        probe.record_tool_latency_residual(0.075);
        let mut state = HomeostaticState::for_agent("test");
        state.operational.error_streak = 2;
        state.operational.total_errors = 4;
        state.operational.total_successes = 16;
        state.cognitive.context_pressure = 0.25;

        let snap = probe.snapshot(&state);
        let l0 = snap.l0.unwrap();
        // drop_rate = error_streak / (errors + successes) = 2/20 = 0.1
        assert!((l0.request_drop_rate - 0.1).abs() < 1e-9);
        // provider_health = 1 - 0.25 = 0.75
        assert!((l0.provider_health - 0.75).abs() < 1e-6);
        assert!((l0.tool_latency_residual - 0.075).abs() < 1e-9);
    }

    /// End-to-end v0.1-L0 closed loop:
    ///   inject ToolLatencyJitter → run sim ticks → revert → keep ticking
    ///   → fit λ̂ on the recovery half → assert λ̂_0 > 0 + sane R².
    #[tokio::test]
    async fn end_to_end_lambda_hat_zero_is_positive() {
        let state = Arc::new(L0LatencyInjectionState::default());
        let injector = L0ProviderInjector::new(Arc::clone(&state));
        let v0 = V0Plant {
            w_latency: 1.0,
            w_drop: 0.0,
            w_health: 0.0,
        };
        let mut sim = L0SimRuntime::new(Arc::clone(&state), v0).set_decay_per_s(0.8);

        // Spike: inject a 250 ± 50 ms latency.
        let p = Perturbation::new(
            PerturbationKind::ToolLatencyJitter {
                mean_ms: 250,
                jitter_ms: 50,
                duration: Duration::from_secs(3),
            },
            Duration::from_secs(3),
        );
        let handle = injector.inject(&p).await.expect("inject");

        // Drive V_0 to its quasi-steady spike level.
        for _ in 0..30 {
            let _ = sim.tick(Duration::from_millis(100));
        }

        // Revert and now sample the recovery curve.
        injector.revert(handle).await.expect("revert");
        let mut est = LambdaEstimator::new(Level::L0, p.id);
        for _ in 0..40 {
            est.push(sim.tick(Duration::from_millis(100)));
        }

        let fit = est.fit_recovery().expect("fit recovery curve");
        assert!(
            fit.lambda_hat > 0.0,
            "λ̂_0 must be positive on a real recovery: got {}",
            fit.lambda_hat
        );
        assert!(
            fit.r_squared > 0.5,
            "R² should be reasonable on this clean signal: got {}",
            fit.r_squared
        );
        assert!(fit.n_samples >= 10);
    }

    #[test]
    fn sim_runtime_records_observations_into_margin_estimator() {
        let state = Arc::new(L0LatencyInjectionState::default());
        let v0 = V0Plant::default();
        let mut sim = L0SimRuntime::new(Arc::clone(&state), v0);
        for _ in 0..5 {
            let _ = sim.tick(Duration::from_millis(100));
        }
        // Each tick observes one synthetic event.
        assert_eq!(sim.margin_estimator().event_count(), 5);
    }
}
