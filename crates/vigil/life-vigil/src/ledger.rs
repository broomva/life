//! Intervention ledger — forks as first-class recorded events (BRO-1880).
//!
//! Deterministic replay gives Vigil a *control group*, not a *diagnosis*.
//! Causal questions ("which context fragment caused this write?") need
//! **interventions**: re-run with exactly one thing changed. But "same seeds"
//! is theatre unless scheduler order, tool latency, retrieved context, and
//! model version are all pinned — otherwise a fork changes three variables
//! while pretending to change one, and any causal claim derived from it is
//! unauditable testimony.
//!
//! This module makes a fork a **first-class recorded event** carrying its
//! *validity tuple*:
//!
//! * **frozen** — variables pinned to the original run (seeds, scheduler
//!   slots, tool payloads, retrieved context, model version…)
//! * **manipulated** — the single variable changed (the `do()` target)
//! * **free** — variables declared unpinnable for this fork
//! * **N** — number of fork executions
//! * **outcomes** — the outcome distribution over the N runs
//!
//! Three semantics are enforced at the schema level so a fork's causal
//! standing is machine-checkable, not asserted:
//!
//! 1. **Single-fork attribution requires total pinning.** If `free ≠ ∅` and
//!    `N == 1` the event is [`Attribution::NonAttributive`] — a single run
//!    cannot average out free variation.
//! 2. **Distributional attribution requires exogeneity.** For `N > 1` the free
//!    variables must be *independent* of the manipulated one (latency jitter
//!    must not correlate with whether the suspect fragment was dropped). The
//!    harness emits an [`ExogeneityCheck`] result onto the event; without a
//!    passing check the distributional claim is non-attributive.
//! 3. **Model-version change is a plant swap, not an intervention.** A fork
//!    whose `manipulated` target is the model version is rejected — it must be
//!    recorded as a [`VersionProbeEvent`], which answers "is this behaviour
//!    version-stable?" rather than masquerading as causal attribution.
//!
//! **Replayer independence (BRO-1037).** The engine that re-executes the forks
//! and records this ledger must not be the runtime that produced the original
//! log — an observer sharing the observed process's reference frame has
//! correlated blind spots. Every event carries both identities and a
//! [`ReplayerIndependence`] check; a self-recorded ledger is flagged.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::semconv;

// ─── Variables ──────────────────────────────────────────────────────────────

/// The class of a fork variable — the axes neo_konsi named as the ones that
/// must be pinned before "same seeds" means anything.
///
/// [`VariableKind::ModelVersion`] is special: it is a *plant swap*, not a
/// controllable input, so it may only ever appear in the `frozen` set of a
/// [`ForkEvent`]. Manipulating it belongs to a [`VersionProbeEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariableKind {
    /// RNG / sampling seed.
    Seed,
    /// Task or tool scheduling order.
    SchedulerOrder,
    /// Tool-call latency / timing.
    ToolLatency,
    /// Tool return payload.
    ToolPayload,
    /// A retrieved context fragment.
    RetrievedContext,
    /// Model version / weights (a plant swap — see type docs).
    ModelVersion,
    /// Any other declared variable.
    Other,
}

/// A named variable that participates in a fork's validity tuple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkVariable {
    /// Stable identifier (e.g. `"context_fragment_7"`, `"tool:web_search.latency"`).
    pub name: String,
    /// The class of variable.
    pub kind: VariableKind,
}

impl ForkVariable {
    /// Construct a fork variable.
    pub fn new(name: impl Into<String>, kind: VariableKind) -> Self {
        Self {
            name: name.into(),
            kind,
        }
    }
}

// ─── Runtime identity & replayer independence ────────────────────────────────

/// Identity of a runtime that either produced the original log or replayed it.
///
/// The `instance` distinguishes distinct processes/executions of the same
/// named runtime; replayer independence turns on it (see
/// [`ReplayerIndependence`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeIdentity {
    /// Runtime name (e.g. `"arcan@prod"`, `"vigil-replayer"`).
    pub name: String,
    /// Unique instance / process identifier.
    pub instance: String,
}

impl RuntimeIdentity {
    /// Construct a runtime identity.
    pub fn new(name: impl Into<String>, instance: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            instance: instance.into(),
        }
    }
}

/// Whether the replayer that recorded the ledger is independent of the runtime
/// that produced the original log (BRO-1037 external-instrumentation
/// discipline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayerIndependence {
    /// Replayer runs in a distinct process from the observed runtime.
    Independent,
    /// Replayer *is* the observed runtime — the ledger narrates itself, so its
    /// causal claims share the runtime's blind spots.
    SelfRecorded,
}

impl ReplayerIndependence {
    /// `true` iff the reference frames are distinct.
    pub fn is_independent(&self) -> bool {
        matches!(self, ReplayerIndependence::Independent)
    }
}

/// Classify replayer independence from the two identities.
///
/// Independence requires a distinct process — the `instance` must differ. A
/// distinct `name` alone is not enough (two forks of the same crashed process
/// share its frame), and an identical instance is self-recording regardless of
/// name.
fn classify_independence(
    original: &RuntimeIdentity,
    replayer: &RuntimeIdentity,
) -> ReplayerIndependence {
    if original.instance == replayer.instance {
        ReplayerIndependence::SelfRecorded
    } else {
        ReplayerIndependence::Independent
    }
}

// ─── Outcome distribution ────────────────────────────────────────────────────

/// The outcome distribution observed over the N fork executions.
///
/// A [`BTreeMap`] keyed by an outcome label (deterministic serialization),
/// counting how many of the N runs produced each outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeDistribution {
    /// Outcome label → number of runs that produced it.
    pub counts: BTreeMap<String, u32>,
}

impl OutcomeDistribution {
    /// Empty distribution (zero executions).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build a distribution from `(label, count)` pairs.
    pub fn from_pairs<I, S>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (S, u32)>,
        S: Into<String>,
    {
        let mut counts = BTreeMap::new();
        for (label, count) in pairs {
            *counts.entry(label.into()).or_insert(0) += count;
        }
        Self { counts }
    }

    /// Record one run producing `label`.
    pub fn observe(&mut self, label: impl Into<String>) {
        *self.counts.entry(label.into()).or_insert(0) += 1;
    }

    /// Total number of runs across all outcomes.
    pub fn total(&self) -> u32 {
        self.counts.values().copied().sum()
    }

    /// Number of distinct outcomes observed.
    pub fn distinct(&self) -> usize {
        self.counts.len()
    }

    /// Proportion of runs that produced `label` (0.0 if absent, 0.0 if empty).
    pub fn proportion(&self, label: &str) -> f64 {
        let total = self.total();
        if total == 0 {
            return 0.0;
        }
        f64::from(self.counts.get(label).copied().unwrap_or(0)) / f64::from(total)
    }

    /// Total-variation distance to another distribution (0.0 = identical,
    /// 1.0 = disjoint support). Used to judge version stability.
    pub fn total_variation(&self, other: &OutcomeDistribution) -> f64 {
        let mut labels: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        labels.extend(self.counts.keys().map(String::as_str));
        labels.extend(other.counts.keys().map(String::as_str));
        let mut sum = 0.0;
        for label in labels {
            sum += (self.proportion(label) - other.proportion(label)).abs();
        }
        0.5 * sum
    }
}

// ─── Exogeneity check ────────────────────────────────────────────────────────

/// One run's measured values, used by the exogeneity check.
///
/// `manipulated_value` is the `do()` target's realized value for the run
/// (e.g. `0.0`/`1.0` for whether the suspect fragment was dropped);
/// `free_values` are the realized values of the declared free variables
/// (e.g. tool latency jitter). Exogeneity holds when the free values do not
/// covary with the manipulated one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForkSample {
    /// Realized value of the manipulated variable for this run.
    pub manipulated_value: f64,
    /// Realized values of each declared free variable, keyed by name.
    pub free_values: BTreeMap<String, f64>,
}

impl ForkSample {
    /// Construct a sample.
    pub fn new(manipulated_value: f64, free_values: BTreeMap<String, f64>) -> Self {
        Self {
            manipulated_value,
            free_values,
        }
    }
}

/// Result of the exogeneity check: are the free variables independent of the
/// manipulated one?
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExogeneityCheck {
    /// No check has been run (the default). Treated as unverified.
    NotChecked,
    /// Free variables are independent of the manipulated one; carries the
    /// largest absolute correlation observed (below threshold).
    Independent {
        /// Largest |correlation| across free variables (≤ threshold).
        max_abs_correlation: f64,
    },
    /// A free variable covaries with the manipulated one — the fork changed
    /// more than one thing.
    Confounded {
        /// The offending free variable's name.
        variable: String,
        /// Its correlation with the manipulated variable.
        correlation: f64,
    },
    /// The check could not be decided (too few samples, missing values, or a
    /// constant manipulated series). Treated as unverified.
    Indeterminate {
        /// Human-readable reason.
        reason: String,
    },
}

impl ExogeneityCheck {
    /// `true` iff the check established independence.
    pub fn is_independent(&self) -> bool {
        matches!(self, ExogeneityCheck::Independent { .. })
    }

    /// Low-cardinality label for span/metric attributes.
    pub fn label(&self) -> &'static str {
        match self {
            ExogeneityCheck::NotChecked => "not_checked",
            ExogeneityCheck::Independent { .. } => "independent",
            ExogeneityCheck::Confounded { .. } => "confounded",
            ExogeneityCheck::Indeterminate { .. } => "indeterminate",
        }
    }
}

/// A pluggable exogeneity check the harness supplies.
///
/// The default [`PearsonExogeneityHook`] correlates each free variable against
/// the manipulated one; a harness with richer statistics (mutual information,
/// conditional independence tests) can implement its own.
pub trait ExogeneityHook {
    /// Check whether `free` variables are independent of `manipulated`, given
    /// the per-run `samples`.
    fn check(
        &self,
        manipulated: &ForkVariable,
        free: &[ForkVariable],
        samples: &[ForkSample],
    ) -> ExogeneityCheck;
}

/// Default exogeneity hook: Pearson correlation with an absolute-value
/// threshold. A free variable whose |correlation| with the manipulated series
/// meets or exceeds the threshold is flagged confounded.
#[derive(Debug, Clone, Copy)]
pub struct PearsonExogeneityHook {
    /// |correlation| at or above which a free variable is confounded.
    pub threshold: f64,
}

impl PearsonExogeneityHook {
    /// Construct with an explicit threshold.
    pub fn new(threshold: f64) -> Self {
        Self { threshold }
    }
}

impl Default for PearsonExogeneityHook {
    /// A conservative default threshold of 0.2.
    fn default() -> Self {
        Self { threshold: 0.2 }
    }
}

impl ExogeneityHook for PearsonExogeneityHook {
    fn check(
        &self,
        _manipulated: &ForkVariable,
        free: &[ForkVariable],
        samples: &[ForkSample],
    ) -> ExogeneityCheck {
        if samples.len() < 2 {
            return ExogeneityCheck::Indeterminate {
                reason: format!("need ≥2 samples for correlation, got {}", samples.len()),
            };
        }
        let manip: Vec<f64> = samples.iter().map(|s| s.manipulated_value).collect();
        if variance_is_zero(&manip) {
            return ExogeneityCheck::Indeterminate {
                reason: "manipulated variable is constant across runs".to_string(),
            };
        }

        let mut max_abs = 0.0_f64;
        for var in free {
            let mut column = Vec::with_capacity(samples.len());
            for s in samples {
                match s.free_values.get(&var.name) {
                    Some(v) => column.push(*v),
                    None => {
                        return ExogeneityCheck::Indeterminate {
                            reason: format!("free variable '{}' missing from a sample", var.name),
                        };
                    }
                }
            }
            match pearson_correlation(&manip, &column) {
                Some(corr) => {
                    if corr.abs() >= self.threshold {
                        return ExogeneityCheck::Confounded {
                            variable: var.name.clone(),
                            correlation: corr,
                        };
                    }
                    max_abs = max_abs.max(corr.abs());
                }
                None => {
                    // Zero variance in the free column ⇒ it cannot covary with
                    // anything; correlation is 0 by convention.
                    max_abs = max_abs.max(0.0);
                }
            }
        }
        ExogeneityCheck::Independent {
            max_abs_correlation: max_abs,
        }
    }
}

/// Pearson correlation of two equal-length series. Returns `None` when either
/// series has zero variance (correlation undefined).
pub fn pearson_correlation(xs: &[f64], ys: &[f64]) -> Option<f64> {
    let n = xs.len();
    if n == 0 || n != ys.len() {
        return None;
    }
    let nf = n as f64;
    let mean_x = xs.iter().sum::<f64>() / nf;
    let mean_y = ys.iter().sum::<f64>() / nf;
    let mut cov = 0.0;
    let mut var_x = 0.0;
    let mut var_y = 0.0;
    for i in 0..n {
        let dx = xs[i] - mean_x;
        let dy = ys[i] - mean_y;
        cov += dx * dy;
        var_x += dx * dx;
        var_y += dy * dy;
    }
    if var_x <= f64::EPSILON || var_y <= f64::EPSILON {
        return None;
    }
    Some(cov / (var_x.sqrt() * var_y.sqrt()))
}

fn variance_is_zero(xs: &[f64]) -> bool {
    if xs.is_empty() {
        return true;
    }
    let mean = xs.iter().sum::<f64>() / xs.len() as f64;
    xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() <= f64::EPSILON
}

// ─── Attribution ─────────────────────────────────────────────────────────────

/// The causal standing of a fork event, derived from its validity tuple.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribution {
    /// Total pinning, single run (`free == ∅ ∧ N == 1`): clean `do()`
    /// attribution.
    SingleFork,
    /// `N > 1` with verified exogeneity (or full pinning): the outcome
    /// distribution attributes to the manipulated variable.
    Distributional,
    /// The fork cannot support a causal claim; carries the reason.
    NonAttributive {
        /// Why the fork is non-attributive.
        reason: NonAttributiveReason,
    },
}

impl Attribution {
    /// `true` iff the fork supports a causal attribution.
    pub fn is_attributive(&self) -> bool {
        !matches!(self, Attribution::NonAttributive { .. })
    }

    /// Low-cardinality label for span/metric attributes.
    pub fn label(&self) -> &'static str {
        match self {
            Attribution::SingleFork => "single_fork",
            Attribution::Distributional => "distributional",
            Attribution::NonAttributive { .. } => "non_attributive",
        }
    }
}

/// Why a fork is [`Attribution::NonAttributive`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonAttributiveReason {
    /// `N == 0` — no executions, nothing to attribute.
    NoExecutions,
    /// `free ≠ ∅ ∧ N == 1` — a single run cannot average out free variation
    /// (semantic rule 1).
    FreeVarsWithSingleRun,
    /// `N > 1` but exogeneity was never established (semantic rule 2).
    ExogeneityUnverified,
    /// `N > 1` and a free variable is confounded with the manipulated one
    /// (semantic rule 2).
    Confounded {
        /// The confounded free variable's name.
        variable: String,
    },
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors constructing a [`ForkEvent`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ForkError {
    /// The manipulated target is the model version — that is a plant swap and
    /// must be recorded as a [`VersionProbeEvent`] (semantic rule 3).
    #[error(
        "manipulating the model version is a plant swap, not an intervention; record a VersionProbeEvent instead"
    )]
    ModelVersionRequiresProbe,

    /// The model version appears outside the frozen set — a cross-version
    /// comparison masquerading as a fork (semantic rule 3).
    #[error("model version must be frozen in a fork; found it in the {set} set")]
    ModelVersionMustBeFrozen {
        /// Which set the model-version variable was found in.
        set: &'static str,
    },

    /// The manipulated variable also appears in the frozen or free set — a
    /// variable cannot be both pinned/free and the `do()` target.
    #[error("manipulated variable '{name}' also appears in the {set} set")]
    ManipulatedOverlap {
        /// The overlapping variable name.
        name: String,
        /// Which set it overlaps with.
        set: &'static str,
    },

    /// The same variable appears in both the frozen and free sets.
    #[error("variable '{name}' appears in both the frozen and free sets")]
    FrozenFreeOverlap {
        /// The overlapping variable name.
        name: String,
    },

    /// The outcome distribution's total does not equal `N`.
    #[error("outcome total {total} does not equal N ({n})")]
    OutcomeCountMismatch {
        /// Sum of outcome counts.
        total: u32,
        /// Declared number of executions.
        n: u32,
    },
}

// ─── Fork event ──────────────────────────────────────────────────────────────

/// A fork recorded as a first-class Vigil event, carrying its validity tuple.
///
/// Construct via [`ForkEvent::new`], which enforces the schema-level invariants
/// (rules 1–3 and replayer independence are then queryable). Run the harness's
/// exogeneity check via [`ForkEvent::apply_exogeneity`] before reading
/// [`ForkEvent::attribution`] for an `N > 1` fork with free variables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForkEvent {
    /// Stable identifier for this fork.
    pub fork_id: String,
    /// The runtime that produced the original log being forked from.
    pub original_runtime: RuntimeIdentity,
    /// The engine that re-executed the forks and recorded this ledger.
    pub replayer: RuntimeIdentity,
    /// Variables pinned to the original run.
    pub frozen: Vec<ForkVariable>,
    /// The single variable changed — the `do()` target.
    pub manipulated: ForkVariable,
    /// Variables declared unpinnable for this fork.
    pub free: Vec<ForkVariable>,
    /// Number of fork executions.
    pub n: u32,
    /// Outcome distribution over the N runs.
    pub outcomes: OutcomeDistribution,
    /// Result of the harness exogeneity check (defaults to
    /// [`ExogeneityCheck::NotChecked`]).
    pub exogeneity: ExogeneityCheck,
}

impl ForkEvent {
    /// Construct and validate a fork event.
    ///
    /// Enforces:
    /// * the manipulated target is not the model version (rule 3),
    /// * the model version, if present, is in `frozen` only (rule 3),
    /// * the manipulated variable does not overlap `frozen`/`free`,
    /// * `frozen` and `free` are disjoint,
    /// * `outcomes.total() == n`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        fork_id: impl Into<String>,
        original_runtime: RuntimeIdentity,
        replayer: RuntimeIdentity,
        frozen: Vec<ForkVariable>,
        manipulated: ForkVariable,
        free: Vec<ForkVariable>,
        n: u32,
        outcomes: OutcomeDistribution,
    ) -> Result<Self, ForkError> {
        // Rule 3: model version is a plant swap.
        if manipulated.kind == VariableKind::ModelVersion {
            return Err(ForkError::ModelVersionRequiresProbe);
        }
        if free.iter().any(|v| v.kind == VariableKind::ModelVersion) {
            return Err(ForkError::ModelVersionMustBeFrozen { set: "free" });
        }

        // Manipulated must not also be pinned or free.
        if frozen.iter().any(|v| v.name == manipulated.name) {
            return Err(ForkError::ManipulatedOverlap {
                name: manipulated.name.clone(),
                set: "frozen",
            });
        }
        if free.iter().any(|v| v.name == manipulated.name) {
            return Err(ForkError::ManipulatedOverlap {
                name: manipulated.name.clone(),
                set: "free",
            });
        }

        // Frozen and free must be disjoint.
        if let Some(name) = frozen
            .iter()
            .find(|f| free.iter().any(|g| g.name == f.name))
            .map(|v| v.name.clone())
        {
            return Err(ForkError::FrozenFreeOverlap { name });
        }

        // Outcome total must equal N.
        let total = outcomes.total();
        if total != n {
            return Err(ForkError::OutcomeCountMismatch { total, n });
        }

        Ok(Self {
            fork_id: fork_id.into(),
            original_runtime,
            replayer,
            frozen,
            manipulated,
            free,
            n,
            outcomes,
            exogeneity: ExogeneityCheck::NotChecked,
        })
    }

    /// The ledger event-type discriminant for this event (always
    /// [`LedgerEventType::Fork`]).
    pub fn event_type(&self) -> LedgerEventType {
        LedgerEventType::Fork
    }

    /// Run the supplied exogeneity hook over `samples` and store the result on
    /// the event. Returns the computed [`ExogeneityCheck`].
    pub fn apply_exogeneity(
        &mut self,
        hook: &dyn ExogeneityHook,
        samples: &[ForkSample],
    ) -> &ExogeneityCheck {
        self.exogeneity = hook.check(&self.manipulated, &self.free, samples);
        &self.exogeneity
    }

    /// Whether the replayer is independent of the original runtime (BRO-1037).
    pub fn replayer_independence(&self) -> ReplayerIndependence {
        classify_independence(&self.original_runtime, &self.replayer)
    }

    /// The causal standing of this fork, derived from the validity tuple and
    /// the stored exogeneity result.
    pub fn attribution(&self) -> Attribution {
        if self.n == 0 {
            return Attribution::NonAttributive {
                reason: NonAttributiveReason::NoExecutions,
            };
        }
        if self.free.is_empty() {
            // Total pinning: single run is a clean do(); N>1 is a (possibly
            // redundant) distributional record, still attributive.
            return if self.n == 1 {
                Attribution::SingleFork
            } else {
                Attribution::Distributional
            };
        }
        // free ≠ ∅
        if self.n == 1 {
            // Rule 1: a single run cannot average out free variation.
            return Attribution::NonAttributive {
                reason: NonAttributiveReason::FreeVarsWithSingleRun,
            };
        }
        // Rule 2: distributional attribution requires exogeneity.
        match &self.exogeneity {
            ExogeneityCheck::Independent { .. } => Attribution::Distributional,
            ExogeneityCheck::Confounded { variable, .. } => Attribution::NonAttributive {
                reason: NonAttributiveReason::Confounded {
                    variable: variable.clone(),
                },
            },
            ExogeneityCheck::NotChecked | ExogeneityCheck::Indeterminate { .. } => {
                Attribution::NonAttributive {
                    reason: NonAttributiveReason::ExogeneityUnverified,
                }
            }
        }
    }

    /// Whether this fork's causal claim is *both* attributive and recorded by
    /// an independent replayer — the two conditions that make it evidence
    /// rather than testimony.
    pub fn is_valid_evidence(&self) -> bool {
        self.attribution().is_attributive() && self.replayer_independence().is_independent()
    }

    /// Emit this fork's key fields as OTel span attributes.
    pub fn record_on_span(&self, span: &tracing::Span) {
        span.record(
            semconv::VIGIL_LEDGER_EVENT_TYPE,
            LedgerEventType::Fork.label(),
        );
        span.record(semconv::VIGIL_LEDGER_FORK_ID, self.fork_id.as_str());
        span.record(
            semconv::VIGIL_LEDGER_MANIPULATED,
            self.manipulated.name.as_str(),
        );
        span.record(
            semconv::VIGIL_LEDGER_MANIPULATED_KIND,
            variable_kind_label(self.manipulated.kind),
        );
        span.record(semconv::VIGIL_LEDGER_FROZEN_COUNT, self.frozen.len() as u64);
        span.record(semconv::VIGIL_LEDGER_FREE_COUNT, self.free.len() as u64);
        span.record(semconv::VIGIL_LEDGER_N, u64::from(self.n));
        span.record(
            semconv::VIGIL_LEDGER_ATTRIBUTION,
            self.attribution().label(),
        );
        span.record(semconv::VIGIL_LEDGER_EXOGENEITY, self.exogeneity.label());
        span.record(
            semconv::VIGIL_LEDGER_REPLAYER_INDEPENDENT,
            self.replayer_independence().is_independent(),
        );
    }
}

// ─── Version probe ───────────────────────────────────────────────────────────

/// Whether a behaviour is stable across a model-version change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionStability {
    /// The two versions produce (within tolerance) the same outcome
    /// distribution.
    Stable,
    /// The versions diverge — behaviour is version-dependent.
    Divergent,
}

impl VersionStability {
    /// Low-cardinality label / boolean for span attributes.
    pub fn is_stable(&self) -> bool {
        matches!(self, VersionStability::Stable)
    }
}

/// A cross-version probe recorded as a distinct event type.
///
/// A model-version change is a *plant swap*, not a `do()` intervention: it
/// cannot answer "which fragment caused this write?" but it *can* answer "is
/// this behaviour version-stable?". Keeping it a separate type prevents a
/// cross-version comparison from masquerading as causal attribution (rule 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionProbeEvent {
    /// Stable identifier for this probe.
    pub probe_id: String,
    /// The runtime that produced the original log.
    pub original_runtime: RuntimeIdentity,
    /// The engine that re-executed the probe and recorded this ledger.
    pub replayer: RuntimeIdentity,
    /// Everything else pinned across the two versions.
    pub frozen: Vec<ForkVariable>,
    /// Baseline model version.
    pub from_version: String,
    /// Probed model version.
    pub to_version: String,
    /// Number of executions per version.
    pub n: u32,
    /// Outcome distribution under the baseline version.
    pub baseline_outcomes: OutcomeDistribution,
    /// Outcome distribution under the probed version.
    pub probe_outcomes: OutcomeDistribution,
}

impl VersionProbeEvent {
    /// Construct a version probe.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        probe_id: impl Into<String>,
        original_runtime: RuntimeIdentity,
        replayer: RuntimeIdentity,
        frozen: Vec<ForkVariable>,
        from_version: impl Into<String>,
        to_version: impl Into<String>,
        n: u32,
        baseline_outcomes: OutcomeDistribution,
        probe_outcomes: OutcomeDistribution,
    ) -> Self {
        Self {
            probe_id: probe_id.into(),
            original_runtime,
            replayer,
            frozen,
            from_version: from_version.into(),
            to_version: to_version.into(),
            n,
            baseline_outcomes,
            probe_outcomes,
        }
    }

    /// The ledger event-type discriminant for this event (always
    /// [`LedgerEventType::VersionProbe`]).
    pub fn event_type(&self) -> LedgerEventType {
        LedgerEventType::VersionProbe
    }

    /// Whether the replayer is independent of the original runtime (BRO-1037).
    pub fn replayer_independence(&self) -> ReplayerIndependence {
        classify_independence(&self.original_runtime, &self.replayer)
    }

    /// Judge version stability by total-variation distance between the two
    /// outcome distributions. `tolerance` is the maximum TV distance still
    /// considered stable (e.g. `0.05`).
    pub fn stability(&self, tolerance: f64) -> VersionStability {
        if self.baseline_outcomes.total_variation(&self.probe_outcomes) <= tolerance {
            VersionStability::Stable
        } else {
            VersionStability::Divergent
        }
    }

    /// Emit this probe's key fields as OTel span attributes.
    pub fn record_on_span(&self, span: &tracing::Span, tolerance: f64) {
        span.record(
            semconv::VIGIL_LEDGER_EVENT_TYPE,
            LedgerEventType::VersionProbe.label(),
        );
        span.record(semconv::VIGIL_LEDGER_PROBE_ID, self.probe_id.as_str());
        span.record(
            semconv::VIGIL_LEDGER_FROM_VERSION,
            self.from_version.as_str(),
        );
        span.record(semconv::VIGIL_LEDGER_TO_VERSION, self.to_version.as_str());
        span.record(semconv::VIGIL_LEDGER_N, u64::from(self.n));
        span.record(
            semconv::VIGIL_LEDGER_VERSION_STABLE,
            self.stability(tolerance).is_stable(),
        );
        span.record(
            semconv::VIGIL_LEDGER_REPLAYER_INDEPENDENT,
            self.replayer_independence().is_independent(),
        );
    }
}

// ─── Ledger event discriminant ───────────────────────────────────────────────

/// The event-type discriminant carried by every ledger event, so mixed
/// streams (and the trace schema) self-describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerEventType {
    /// A single-variable `do()` intervention ([`ForkEvent`]).
    Fork,
    /// A cross-version plant swap ([`VersionProbeEvent`]).
    VersionProbe,
}

impl LedgerEventType {
    /// Low-cardinality label for span/metric attributes.
    pub fn label(&self) -> &'static str {
        match self {
            LedgerEventType::Fork => "fork",
            LedgerEventType::VersionProbe => "version_probe",
        }
    }
}

/// A ledger event — either a fork or a version probe. Serializes with a
/// `event_type` tag so a heterogeneous ledger stream round-trips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum LedgerEvent {
    /// A single-variable intervention.
    Fork(ForkEvent),
    /// A cross-version plant swap.
    VersionProbe(VersionProbeEvent),
}

fn variable_kind_label(kind: VariableKind) -> &'static str {
    match kind {
        VariableKind::Seed => "seed",
        VariableKind::SchedulerOrder => "scheduler_order",
        VariableKind::ToolLatency => "tool_latency",
        VariableKind::ToolPayload => "tool_payload",
        VariableKind::RetrievedContext => "retrieved_context",
        VariableKind::ModelVersion => "model_version",
        VariableKind::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(name: &str) -> ForkVariable {
        ForkVariable::new(name, VariableKind::RetrievedContext)
    }

    fn runtimes() -> (RuntimeIdentity, RuntimeIdentity) {
        (
            RuntimeIdentity::new("arcan@prod", "run-original-1"),
            RuntimeIdentity::new("vigil-replayer", "replay-1"),
        )
    }

    #[test]
    fn single_fork_total_pinning_is_attributive() {
        let (orig, repl) = runtimes();
        let ev = ForkEvent::new(
            "fork-1",
            orig,
            repl,
            vec![
                ForkVariable::new("seed", VariableKind::Seed),
                ForkVariable::new("model", VariableKind::ModelVersion),
            ],
            ctx("fragment_7"),
            vec![], // empty free set ⇒ total pinning
            1,
            OutcomeDistribution::from_pairs([("wrote_file", 1)]),
        )
        .unwrap();

        assert_eq!(ev.attribution(), Attribution::SingleFork);
        assert!(ev.attribution().is_attributive());
        assert!(ev.replayer_independence().is_independent());
        assert!(ev.is_valid_evidence());
    }

    #[test]
    fn rule1_free_vars_with_single_run_is_non_attributive() {
        let (orig, repl) = runtimes();
        let ev = ForkEvent::new(
            "fork-2",
            orig,
            repl,
            vec![ForkVariable::new("seed", VariableKind::Seed)],
            ctx("fragment_7"),
            vec![ForkVariable::new("latency", VariableKind::ToolLatency)],
            1,
            OutcomeDistribution::from_pairs([("wrote_file", 1)]),
        )
        .unwrap();

        assert_eq!(
            ev.attribution(),
            Attribution::NonAttributive {
                reason: NonAttributiveReason::FreeVarsWithSingleRun
            }
        );
        assert!(!ev.is_valid_evidence());
    }

    #[test]
    fn rule2_distributional_requires_exogeneity() {
        let (orig, repl) = runtimes();
        let mut ev = ForkEvent::new(
            "fork-3",
            orig,
            repl,
            vec![ForkVariable::new("seed", VariableKind::Seed)],
            ctx("fragment_7"),
            vec![ForkVariable::new("latency", VariableKind::ToolLatency)],
            4,
            OutcomeDistribution::from_pairs([("wrote_file", 3), ("no_write", 1)]),
        )
        .unwrap();

        // Before the exogeneity check: unverified ⇒ non-attributive.
        assert_eq!(
            ev.attribution(),
            Attribution::NonAttributive {
                reason: NonAttributiveReason::ExogeneityUnverified
            }
        );

        // Latency (free) is independent of whether the fragment was dropped
        // (manipulated). The latency distribution is identical across the
        // manip=0 and manip=1 groups, so the correlation is exactly zero.
        let samples = vec![
            ForkSample::new(0.0, BTreeMap::from([("latency".to_string(), 12.0)])),
            ForkSample::new(1.0, BTreeMap::from([("latency".to_string(), 12.0)])),
            ForkSample::new(0.0, BTreeMap::from([("latency".to_string(), 13.0)])),
            ForkSample::new(1.0, BTreeMap::from([("latency".to_string(), 13.0)])),
        ];
        let hook = PearsonExogeneityHook::default();
        ev.apply_exogeneity(&hook, &samples);
        assert!(ev.exogeneity.is_independent());
        assert_eq!(ev.attribution(), Attribution::Distributional);
    }

    #[test]
    fn rule2_confounded_free_var_is_non_attributive() {
        let (orig, repl) = runtimes();
        let mut ev = ForkEvent::new(
            "fork-4",
            orig,
            repl,
            vec![ForkVariable::new("seed", VariableKind::Seed)],
            ctx("fragment_7"),
            vec![ForkVariable::new("latency", VariableKind::ToolLatency)],
            4,
            OutcomeDistribution::from_pairs([("wrote_file", 2), ("no_write", 2)]),
        )
        .unwrap();

        // Latency covaries perfectly with the manipulated indicator — the fork
        // changed two things at once.
        let samples = vec![
            ForkSample::new(0.0, BTreeMap::from([("latency".to_string(), 10.0)])),
            ForkSample::new(1.0, BTreeMap::from([("latency".to_string(), 20.0)])),
            ForkSample::new(0.0, BTreeMap::from([("latency".to_string(), 10.0)])),
            ForkSample::new(1.0, BTreeMap::from([("latency".to_string(), 20.0)])),
        ];
        let hook = PearsonExogeneityHook::default();
        ev.apply_exogeneity(&hook, &samples);
        assert!(matches!(ev.exogeneity, ExogeneityCheck::Confounded { .. }));
        assert_eq!(
            ev.attribution(),
            Attribution::NonAttributive {
                reason: NonAttributiveReason::Confounded {
                    variable: "latency".to_string()
                }
            }
        );
    }

    #[test]
    fn rule3_model_version_manipulation_is_rejected() {
        let (orig, repl) = runtimes();
        let err = ForkEvent::new(
            "fork-5",
            orig,
            repl,
            vec![],
            ForkVariable::new("model", VariableKind::ModelVersion),
            vec![],
            1,
            OutcomeDistribution::from_pairs([("x", 1)]),
        )
        .unwrap_err();
        assert_eq!(err, ForkError::ModelVersionRequiresProbe);
    }

    #[test]
    fn rule3_free_model_version_is_rejected() {
        let (orig, repl) = runtimes();
        let err = ForkEvent::new(
            "fork-6",
            orig,
            repl,
            vec![],
            ctx("fragment_7"),
            vec![ForkVariable::new("model", VariableKind::ModelVersion)],
            2,
            OutcomeDistribution::from_pairs([("x", 2)]),
        )
        .unwrap_err();
        assert_eq!(err, ForkError::ModelVersionMustBeFrozen { set: "free" });
    }

    #[test]
    fn manipulated_overlap_is_rejected() {
        let (orig, repl) = runtimes();
        let err = ForkEvent::new(
            "fork-7",
            orig,
            repl,
            vec![ctx("fragment_7")],
            ctx("fragment_7"),
            vec![],
            1,
            OutcomeDistribution::from_pairs([("x", 1)]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ForkError::ManipulatedOverlap {
                name: "fragment_7".to_string(),
                set: "frozen"
            }
        );
    }

    #[test]
    fn outcome_count_mismatch_is_rejected() {
        let (orig, repl) = runtimes();
        let err = ForkEvent::new(
            "fork-8",
            orig,
            repl,
            vec![],
            ctx("fragment_7"),
            vec![],
            3,
            OutcomeDistribution::from_pairs([("x", 2)]),
        )
        .unwrap_err();
        assert_eq!(err, ForkError::OutcomeCountMismatch { total: 2, n: 3 });
    }

    #[test]
    fn self_recorded_replayer_is_flagged() {
        let orig = RuntimeIdentity::new("arcan@prod", "shared-instance");
        let repl = RuntimeIdentity::new("arcan@prod", "shared-instance");
        let ev = ForkEvent::new(
            "fork-9",
            orig,
            repl,
            vec![],
            ctx("fragment_7"),
            vec![],
            1,
            OutcomeDistribution::from_pairs([("x", 1)]),
        )
        .unwrap();
        assert_eq!(
            ev.replayer_independence(),
            ReplayerIndependence::SelfRecorded
        );
        // Attributive, but not valid evidence: the ledger narrates itself.
        assert!(ev.attribution().is_attributive());
        assert!(!ev.is_valid_evidence());
    }

    #[test]
    fn no_executions_is_non_attributive() {
        let (orig, repl) = runtimes();
        let ev = ForkEvent::new(
            "fork-10",
            orig,
            repl,
            vec![],
            ctx("fragment_7"),
            vec![],
            0,
            OutcomeDistribution::empty(),
        )
        .unwrap();
        assert_eq!(
            ev.attribution(),
            Attribution::NonAttributive {
                reason: NonAttributiveReason::NoExecutions
            }
        );
    }

    #[test]
    fn exogeneity_indeterminate_with_too_few_samples() {
        let hook = PearsonExogeneityHook::default();
        let manip = ForkVariable::new("frag", VariableKind::RetrievedContext);
        let free = vec![ForkVariable::new("latency", VariableKind::ToolLatency)];
        let samples = vec![ForkSample::new(
            1.0,
            BTreeMap::from([("latency".to_string(), 1.0)]),
        )];
        let check = hook.check(&manip, &free, &samples);
        assert!(matches!(check, ExogeneityCheck::Indeterminate { .. }));
    }

    #[test]
    fn version_probe_stability() {
        let (orig, repl) = runtimes();
        let stable = VersionProbeEvent::new(
            "probe-1",
            orig.clone(),
            repl.clone(),
            vec![ForkVariable::new("seed", VariableKind::Seed)],
            "claude-sonnet-4-5",
            "claude-sonnet-4-6",
            10,
            OutcomeDistribution::from_pairs([("wrote_file", 8), ("no_write", 2)]),
            OutcomeDistribution::from_pairs([("wrote_file", 8), ("no_write", 2)]),
        );
        assert_eq!(stable.stability(0.05), VersionStability::Stable);

        let divergent = VersionProbeEvent::new(
            "probe-2",
            orig,
            repl,
            vec![],
            "claude-sonnet-4-5",
            "claude-opus-4-8",
            10,
            OutcomeDistribution::from_pairs([("wrote_file", 9), ("no_write", 1)]),
            OutcomeDistribution::from_pairs([("wrote_file", 2), ("no_write", 8)]),
        );
        assert_eq!(divergent.stability(0.05), VersionStability::Divergent);
    }

    #[test]
    fn ledger_event_round_trips_with_tag() {
        let (orig, repl) = runtimes();
        let fork = ForkEvent::new(
            "fork-rt",
            orig.clone(),
            repl.clone(),
            vec![],
            ctx("fragment_7"),
            vec![],
            1,
            OutcomeDistribution::from_pairs([("x", 1)]),
        )
        .unwrap();
        let ev = LedgerEvent::Fork(fork);
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"event_type\":\"fork\""));
        let back: LedgerEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);

        let probe = VersionProbeEvent::new(
            "probe-rt",
            orig,
            repl,
            vec![],
            "v1",
            "v2",
            2,
            OutcomeDistribution::from_pairs([("x", 2)]),
            OutcomeDistribution::from_pairs([("x", 2)]),
        );
        let ev = LedgerEvent::VersionProbe(probe);
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"event_type\":\"version_probe\""));
        let back: LedgerEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn pearson_correlation_basics() {
        // Perfect positive correlation.
        assert!(
            (pearson_correlation(&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0]).unwrap() - 1.0).abs() < 1e-9
        );
        // Perfect negative correlation.
        assert!(
            (pearson_correlation(&[1.0, 2.0, 3.0], &[6.0, 4.0, 2.0]).unwrap() + 1.0).abs() < 1e-9
        );
        // Zero variance ⇒ undefined.
        assert!(pearson_correlation(&[1.0, 1.0, 1.0], &[2.0, 4.0, 6.0]).is_none());
    }

    #[test]
    fn total_variation_distance() {
        let a = OutcomeDistribution::from_pairs([("x", 5), ("y", 5)]);
        let b = OutcomeDistribution::from_pairs([("x", 5), ("y", 5)]);
        assert!(a.total_variation(&b).abs() < 1e-9);
        let c = OutcomeDistribution::from_pairs([("x", 10)]);
        let d = OutcomeDistribution::from_pairs([("y", 10)]);
        assert!((c.total_variation(&d) - 1.0).abs() < 1e-9);
    }
}
