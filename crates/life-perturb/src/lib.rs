//! # life-perturb — Controlled perturbation injection for RCS λ̂ validation
//!
//! `life-perturb` is the Rust-native instrumentation layer that closes the
//! 3-orders-of-magnitude construct gap between the RCS paper's analytic
//! stability margins (λ_0=1.45, λ_1=0.41, λ_2=0.07, λ_3=0.006) and our
//! empirical estimates (~0 from stationary-trace regression).
//!
//! The paper's λ_i describe **exponential decay rates of perturbations** in
//! a Lyapunov function. Stationary aggregates (microRCS, microgrid) measure
//! a different quantity. To produce paper-magnitude λ̂ we must inject
//! controlled perturbations into the live Life runtime and fit
//!
//! ```text
//! V_k(t) = V_k(0) · exp(−λ̂_recovery · (t − t_inject))
//! ```
//!
//! to the recorded recovery curve.
//!
//! ## Status
//!
//! This is the v0.0 scaffold — design + traits + types only. No injectors
//! actually wire into the daemons yet. See
//! [`docs/superpowers/specs/2026-05-04-life-perturb-design.md`][spec] for
//! the full design and phasing plan. Linear ticket: [BRO-947].
//!
//! ## Module layout
//!
//! - [`perturbation`] — taxonomy of perturbations (`Level`, `PerturbationKind`,
//!   `Perturbation`, `Severity`).
//! - [`injector`] — `Injector` trait + per-level injector stubs.
//! - [`lyapunov`] — `LyapunovFn` trait + per-level V_k stubs.
//! - [`estimator`] — `LambdaEstimator` and `RecoveryFit` for fitting
//!   `exp(−λt)` to recovery curves.
//! - [`error`] — crate-level error type.
//!
//! ## See also
//!
//! - `crates/autonomic/autonomic-core/src/rcs_budget.rs::MarginEstimator` —
//!   estimates parameters of the budget formula (complementary).
//! - `crates/arcan/arcand/src/rcs_observer.rs` — runtime state observer
//!   (life#804).
//! - `crates/autonomic/autonomic-core/data/rcs-parameters.toml` — canonical
//!   paper values mirrored from `research/rcs/data/parameters.toml`.
//!
//! [spec]: ../../docs/superpowers/specs/2026-05-04-life-perturb-design.md
//! [BRO-947]: https://linear.app/broomva/issue/BRO-947

pub mod error;
pub mod estimator;
pub mod injector;
pub mod lyapunov;
pub mod perturbation;

pub use error::{PerturbError, PerturbResult};
pub use estimator::{LambdaEstimator, RecoveryFit};
pub use injector::{Injector, PerturbationHandle};
pub use lyapunov::{LyapunovFn, LyapunovSample, SystemSnapshot};
pub use perturbation::{Level, Perturbation, PerturbationId, PerturbationKind, Severity};
