//! λ̂ estimator — fits `V_k(t) = V_k(0) · exp(−λ̂ · (t − t_inject))` to a
//! recovery curve.
//!
//! v0.0 scaffold: trait + struct surface + a placeholder OLS log-linear fit
//! that returns a default `RecoveryFit` when there are too few samples.
//! Real bootstrap CI + naturalness-window exclusion land in v0.1.
//!
//! See spec §5 (`estimator.rs` block) for the full API plan and §8 Q1 for
//! the open question on OLS vs MLE.

use serde::{Deserialize, Serialize};

use crate::error::{PerturbError, PerturbResult};
use crate::lyapunov::LyapunovSample;
use crate::perturbation::{Level, PerturbationId};

/// Result of fitting an exponential decay to a recovery curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RecoveryFit {
    /// Estimated decay rate λ̂ (1/seconds). Compare to paper's λ_i.
    pub lambda_hat: f64,
    /// Coefficient of determination on the log-linear regression.
    pub r_squared: f64,
    /// Bootstrap 95% CI for λ̂. v0.0 scaffold returns `(NaN, NaN)`.
    pub bootstrap_ci_95: (f64, f64),
    /// Number of samples used in the fit (post-windowing).
    pub n_samples: usize,
    /// Fit window (ms since epoch): inclusive `[start, end]`.
    pub fit_window_ms: (u64, u64),
}

impl Default for RecoveryFit {
    /// Produce a "no-fit" sentinel so doctests / scaffolded callers can
    /// move samples through the API without aborting.
    fn default() -> Self {
        Self {
            lambda_hat: 0.0,
            r_squared: 0.0,
            bootstrap_ci_95: (f64::NAN, f64::NAN),
            n_samples: 0,
            fit_window_ms: (0, 0),
        }
    }
}

/// Stateful aggregator: feed samples in, call [`Self::fit_recovery`] at the
/// end of the integration window.
#[derive(Debug, Clone)]
pub struct LambdaEstimator {
    /// Level this estimator is fitting (L0..L3).
    pub level: Level,
    /// Perturbation under measurement — threaded through to the fit
    /// result for telemetry attribution.
    pub perturbation: PerturbationId,
    /// Collected samples in chronological order.
    pub samples: Vec<LyapunovSample>,
}

impl LambdaEstimator {
    /// Construct an empty estimator for a given perturbation.
    pub fn new(level: Level, perturbation: PerturbationId) -> Self {
        Self {
            level,
            perturbation,
            samples: Vec::new(),
        }
    }

    /// Append a sample to the estimator. Out-of-order samples are
    /// permitted; the fit step handles ordering.
    pub fn push(&mut self, sample: LyapunovSample) {
        self.samples.push(sample);
    }

    /// Fit `V_k(t) = V_k(0) · exp(−λ̂ · t)` via OLS on
    /// `ln V_k(t) ≈ ln V_k(0) − λ̂ · t`. v0.0 scaffold: returns
    /// `RecoveryFit::default()` when fewer than 3 positive samples are
    /// present, otherwise computes a basic OLS slope and returns it.
    pub fn fit_recovery(&self) -> PerturbResult<RecoveryFit> {
        if self.samples.len() < 3 {
            return Err(PerturbError::Fit(format!(
                "need >= 3 samples, got {}",
                self.samples.len()
            )));
        }

        // Sort by time and filter strictly-positive V (log-domain).
        let mut sorted: Vec<_> = self.samples.to_vec();
        sorted.sort_by_key(|s| s.t_ms);
        let positives: Vec<_> = sorted.iter().copied().filter(|s| s.v > 0.0).collect();
        if positives.len() < 3 {
            return Err(PerturbError::Fit(format!(
                "need >= 3 positive samples for log-linear fit, got {}",
                positives.len()
            )));
        }

        // OLS: ln(v_i) = a − λ * (t_i - t0); slope = -λ̂.
        let t0_ms = positives[0].t_ms;
        let n = positives.len() as f64;
        let xs: Vec<f64> = positives
            .iter()
            .map(|s| (s.t_ms.saturating_sub(t0_ms)) as f64 / 1000.0) // seconds
            .collect();
        let ys: Vec<f64> = positives.iter().map(|s| s.v.ln()).collect();
        let mean_x = xs.iter().sum::<f64>() / n;
        let mean_y = ys.iter().sum::<f64>() / n;
        let mut sxx = 0.0;
        let mut sxy = 0.0;
        let mut syy = 0.0;
        for i in 0..positives.len() {
            let dx = xs[i] - mean_x;
            let dy = ys[i] - mean_y;
            sxx += dx * dx;
            sxy += dx * dy;
            syy += dy * dy;
        }
        if sxx == 0.0 {
            return Err(PerturbError::Fit("zero variance in t".to_string()));
        }
        let slope = sxy / sxx;
        let r_squared = if syy == 0.0 {
            1.0
        } else {
            (sxy * sxy) / (sxx * syy)
        };

        let lambda_hat = -slope;
        let last = positives.last().unwrap();
        Ok(RecoveryFit {
            lambda_hat,
            r_squared,
            // v0.0 scaffold: real bootstrap CI lands in v0.1.
            bootstrap_ci_95: (f64::NAN, f64::NAN),
            n_samples: positives.len(),
            fit_window_ms: (t0_ms, last.t_ms),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_recovery_recovers_known_lambda() {
        // V(t) = exp(-0.5 * t), sampled every 100 ms for 2 s.
        let id = PerturbationId::new();
        let mut est = LambdaEstimator::new(Level::L0, id);
        for k in 0..21u64 {
            let t = (k * 100) as f64 / 1000.0;
            let v = (-0.5_f64 * t).exp();
            est.push(LyapunovSample::new(k * 100, v));
        }
        let fit = est.fit_recovery().expect("fit succeeds");
        // Should recover λ ≈ 0.5 within numerical tolerance.
        assert!(
            (fit.lambda_hat - 0.5).abs() < 1e-6,
            "fit returned λ={}",
            fit.lambda_hat
        );
        assert!(fit.r_squared > 0.999);
        assert_eq!(fit.n_samples, 21);
    }

    #[test]
    fn fit_recovery_fails_with_too_few_samples() {
        let id = PerturbationId::new();
        let mut est = LambdaEstimator::new(Level::L0, id);
        est.push(LyapunovSample::new(0, 1.0));
        est.push(LyapunovSample::new(100, 0.5));
        let err = est.fit_recovery().expect_err("rejected");
        match err {
            PerturbError::Fit(msg) => assert!(msg.contains("need >= 3")),
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn default_recovery_fit_is_zero_lambda() {
        let f = RecoveryFit::default();
        assert_eq!(f.lambda_hat, 0.0);
        assert_eq!(f.n_samples, 0);
    }
}
