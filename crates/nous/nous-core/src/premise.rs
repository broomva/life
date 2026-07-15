//! Premise-staleness classifier — the second Nous observation lens.
//!
//! Nous historically shipped a single classifier: **control-divergence**
//! ("did execution match the plan?", realized by `nous-judge::PlanAdherence`).
//! That lens answers *is the loop wrong?* It cannot answer the orthogonal
//! question *was the plan still valid given the current world?* — and without
//! that question, every premise failure is misread as a loop failure, energy is
//! spent re-engineering the loop, the premise stays stale, and the cycle
//! compounds. See `research/entities/pattern/shadow-competence.md` (9/9).
//!
//! This module adds the missing lens: **premise-staleness**. It is a stateful,
//! session-scoped, deterministic detector — *no LLM in the hot path* — plus the
//! dual-classifier schema that routes a diagnosis to the layer that can fix it.
//!
//! # The dual-classifier schema
//!
//! Every diagnosis is tagged along two independent axes and routed accordingly:
//!
//! | [`FailureClass`] | Meaning | Route ([`DiagnosisRoute`]) |
//! |------------------|---------|----------------------------|
//! | [`Epistemic`](FailureClass::Epistemic) | the premise was/became wrong for the task | premise update |
//! | [`Control`](FailureClass::Control) | execution diverged from a still-valid plan | monitoring tweak |
//! | [`Both`](FailureClass::Both) | premise stale *and* execution diverged | architectural rework |
//! | [`Neither`](FailureClass::Neither) | false alarm | false-alarm corpus |
//!
//! A note on terminology: the source framing labels "a premise that *drifted*" a
//! *control* failure because the monitoring loop should have caught the drift.
//! We collapse "wrong from the start" and "drifted" onto the **epistemic** axis,
//! because the *remedy* for both is the same — **update the premise**. The
//! control axis is reserved for the pre-existing control-divergence lens
//! (execution vs. plan). This keeps routing 1:1 with the layer that acts on it.
//!
//! # The detector
//!
//! [`PremiseValidityWatcher`] accumulates three orthogonal staleness signals
//! over the life of a session:
//!
//! 1. **Output divergence** — cosine distance from a new output's feature vector
//!    to the mean of the rolling `N`-back window. ami_ai_'s concrete proposal:
//!    *"this output looks structurally different from the last five iterations."*
//! 2. **Plant divergence** — normalized gap between a predicted and an observed
//!    plant reading. The world moved out from under the plan.
//! 3. **No-op accumulation** — reverse-burden signal: consecutive actions that
//!    made no progress. Normally the burden is on the watcher to prove staleness;
//!    once no-ops pile up past a threshold, the burden reverses — the premise is
//!    presumed stale until progress resumes.
//!
//! The watcher is **embedding-agnostic**: it consumes feature vectors, so a real
//! embedding model can be substituted freely. For a zero-dependency default it
//! ships [`lexical_features`], a deterministic hashed-token bag that needs no
//! model and no I/O — resolving the "embedding model choice" open question by
//! not forcing one.

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

/// One of the two orthogonal failure axes, or their combination / absence.
///
/// Produced by [`Diagnosis::classify`] from a premise-staleness signal (the
/// epistemic axis) and a control-divergence signal (the control axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// The premise was wrong, or drifted out of validity. Fix the premise.
    Epistemic,
    /// Execution diverged from a still-valid plan. Fix the monitoring loop.
    Control,
    /// Premise stale *and* execution diverged. Rework the architecture.
    Both,
    /// Neither axis fired — a false alarm worth recording for calibration.
    Neither,
}

impl FailureClass {
    /// Build a class from the two independent axis signals.
    pub fn from_signals(epistemic: bool, control: bool) -> Self {
        match (epistemic, control) {
            (true, false) => Self::Epistemic,
            (false, true) => Self::Control,
            (true, true) => Self::Both,
            (false, false) => Self::Neither,
        }
    }

    /// The layer that can act on this class of failure.
    pub fn route(&self) -> DiagnosisRoute {
        match self {
            Self::Epistemic => DiagnosisRoute::PremiseUpdate,
            Self::Control => DiagnosisRoute::MonitoringTweak,
            Self::Both => DiagnosisRoute::ArchitecturalRework,
            Self::Neither => DiagnosisRoute::FalseAlarmCorpus,
        }
    }

    /// Stable snake_case label for events and logs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Epistemic => "epistemic",
            Self::Control => "control",
            Self::Both => "both",
            Self::Neither => "neither",
        }
    }
}

impl std::fmt::Display for FailureClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Where a diagnosis should be routed — one destination per [`FailureClass`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisRoute {
    /// Epistemic failure → revise the stale/wrong premise.
    PremiseUpdate,
    /// Control failure → adjust the monitoring loop / control law.
    MonitoringTweak,
    /// Both → deeper architectural rework spanning premise and loop.
    ArchitecturalRework,
    /// Neither → append to the false-alarm corpus for threshold calibration.
    FalseAlarmCorpus,
}

impl DiagnosisRoute {
    /// Stable snake_case label for events and logs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::PremiseUpdate => "premise_update",
            Self::MonitoringTweak => "monitoring_tweak",
            Self::ArchitecturalRework => "architectural_rework",
            Self::FalseAlarmCorpus => "false_alarm_corpus",
        }
    }
}

impl std::fmt::Display for DiagnosisRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// A tagged diagnosis: a failure class, its derived route, and a rationale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnosis {
    /// Session the diagnosis belongs to.
    pub session_id: String,
    /// Which failure axis (or combination) fired.
    pub class: FailureClass,
    /// Where the diagnosis routes — always `class.route()`.
    pub route: DiagnosisRoute,
    /// Confidence in `[0.0, 1.0]`.
    pub confidence: f64,
    /// Optional human-readable rationale.
    pub rationale: Option<String>,
}

impl Diagnosis {
    /// Build a diagnosis from an explicit class; the route is derived.
    pub fn new(session_id: impl Into<String>, class: FailureClass, confidence: f64) -> Self {
        Self {
            session_id: session_id.into(),
            class,
            route: class.route(),
            confidence: confidence.clamp(0.0, 1.0),
            rationale: None,
        }
    }

    /// Classify from the two independent axis signals.
    ///
    /// `premise_stale` is the epistemic axis (this module's detector);
    /// `control_diverged` is the control axis (the existing control-divergence
    /// classifier, e.g. `nous-judge::PlanAdherence` scoring low).
    pub fn classify(
        session_id: impl Into<String>,
        premise_stale: bool,
        control_diverged: bool,
        confidence: f64,
    ) -> Self {
        Self::new(
            session_id,
            FailureClass::from_signals(premise_stale, control_diverged),
            confidence,
        )
    }

    /// Attach a rationale.
    pub fn with_rationale(mut self, rationale: impl Into<String>) -> Self {
        self.rationale = Some(rationale.into());
        self
    }

    /// Whether this diagnosis is a false alarm (`Neither`).
    pub fn is_false_alarm(&self) -> bool {
        self.class == FailureClass::Neither
    }
}

/// Which of the three orthogonal staleness detectors produced a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StalenessSignalKind {
    /// A new output diverged structurally from the rolling window.
    OutputDivergence,
    /// A predicted plant reading diverged from the observed one.
    PlantDivergence,
    /// Consecutive no-op actions accumulated past the reverse-burden threshold.
    NoOpAccumulation,
}

impl StalenessSignalKind {
    /// Stable snake_case label for events and logs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::OutputDivergence => "output_divergence",
            Self::PlantDivergence => "plant_divergence",
            Self::NoOpAccumulation => "noop_accumulation",
        }
    }
}

impl std::fmt::Display for StalenessSignalKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// A fired premise-staleness alarm. Feeds the epistemic axis of [`Diagnosis`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PremiseStalenessSignal {
    /// Session the signal belongs to.
    pub session_id: String,
    /// Which detector fired.
    pub kind: StalenessSignalKind,
    /// Signal strength: cosine distance, normalized plant gap, or no-op run
    /// length (as `f64`), depending on `kind`.
    pub magnitude: f64,
    /// The watcher's observation index at which the signal fired.
    pub observation: u64,
    /// Optional detail string.
    pub detail: Option<String>,
}

impl PremiseStalenessSignal {
    /// Build a [`Diagnosis`] treating this signal as the epistemic axis,
    /// combined with an externally-supplied control-divergence flag.
    pub fn diagnose(&self, control_diverged: bool) -> Diagnosis {
        Diagnosis::classify(
            self.session_id.clone(),
            true,
            control_diverged,
            self.magnitude.clamp(0.0, 1.0),
        )
        .with_rationale(format!(
            "premise-staleness [{}] magnitude {:.3} at observation {}",
            self.kind, self.magnitude, self.observation
        ))
    }
}

/// Tuning knobs for [`PremiseValidityWatcher`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PremiseWatchConfig {
    /// Rolling `N`-back window size for output-divergence detection.
    pub window: usize,
    /// Cosine-distance threshold `k`; a new output beyond it fires.
    pub distance_threshold: f64,
    /// Consecutive no-ops before the reverse burden trips.
    pub noop_threshold: usize,
    /// Normalized plant-gap threshold for plant-divergence detection.
    pub plant_divergence_threshold: f64,
}

impl Default for PremiseWatchConfig {
    fn default() -> Self {
        // window=5 mirrors ami_ai_'s "last five iterations"; the thresholds are
        // conservative starting points pending empirical calibration (the
        // false-alarm corpus, `DiagnosisRoute::FalseAlarmCorpus`, is the data
        // source for that calibration).
        Self {
            window: 5,
            distance_threshold: 0.35,
            noop_threshold: 3,
            plant_divergence_threshold: 0.5,
        }
    }
}

/// Session-scoped, deterministic premise-staleness detector.
///
/// Construct via [`watch_premise_validity`] (defaults) or
/// [`PremiseValidityWatcher::new`] (explicit config). Feed it observations;
/// each `observe_*` call returns `Some(signal)` when its detector trips.
#[derive(Debug, Clone)]
pub struct PremiseValidityWatcher {
    session_id: String,
    config: PremiseWatchConfig,
    window: VecDeque<Vec<f64>>,
    observations: u64,
    consecutive_noops: usize,
}

impl PremiseValidityWatcher {
    /// Create a watcher with an explicit config.
    pub fn new(session_id: impl Into<String>, config: PremiseWatchConfig) -> Self {
        Self {
            session_id: session_id.into(),
            config,
            window: VecDeque::new(),
            observations: 0,
            consecutive_noops: 0,
        }
    }

    /// The session this watcher observes.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Total number of output observations fed so far.
    pub fn observations(&self) -> u64 {
        self.observations
    }

    /// Current consecutive no-op run length.
    pub fn consecutive_noops(&self) -> usize {
        self.consecutive_noops
    }

    /// Observe a new output as a feature vector.
    ///
    /// Once the rolling window holds a full baseline (`config.window` prior
    /// vectors), the cosine distance from `features` to the window mean is
    /// compared against `config.distance_threshold`. `features` is always
    /// appended to the window (evicting the oldest) regardless of whether it
    /// fired, so the baseline adapts.
    pub fn observe_output(&mut self, features: &[f64]) -> Option<PremiseStalenessSignal> {
        self.observations += 1;
        let signal = if self.config.window > 0 && self.window.len() >= self.config.window {
            let mean = window_mean(&self.window);
            let distance = cosine_distance(features, &mean);
            (distance > self.config.distance_threshold).then(|| PremiseStalenessSignal {
                session_id: self.session_id.clone(),
                kind: StalenessSignalKind::OutputDivergence,
                magnitude: distance,
                observation: self.observations,
                detail: Some(format!(
                    "cosine distance {:.3} > k={:.3} over {}-back window",
                    distance, self.config.distance_threshold, self.config.window
                )),
            })
        } else {
            None
        };

        self.window.push_back(features.to_vec());
        while self.window.len() > self.config.window {
            self.window.pop_front();
        }
        signal
    }

    /// Observe a new output as raw text, vectorized via [`lexical_features`].
    ///
    /// Convenience wrapper for the zero-dependency default embedding.
    pub fn observe_output_text(&mut self, text: &str) -> Option<PremiseStalenessSignal> {
        let features = lexical_features(text);
        self.observe_output(&features)
    }

    /// Observe a predicted-vs-observed plant reading.
    ///
    /// The gap is normalized by the larger magnitude of the two readings, so the
    /// threshold is scale-free. Returns a signal when the normalized gap exceeds
    /// `config.plant_divergence_threshold`.
    pub fn observe_plant(
        &mut self,
        predicted: f64,
        observed: f64,
    ) -> Option<PremiseStalenessSignal> {
        let scale = predicted.abs().max(observed.abs()).max(f64::EPSILON);
        let normalized = (predicted - observed).abs() / scale;
        (normalized > self.config.plant_divergence_threshold).then(|| PremiseStalenessSignal {
            session_id: self.session_id.clone(),
            kind: StalenessSignalKind::PlantDivergence,
            magnitude: normalized.min(1.0),
            observation: self.observations,
            detail: Some(format!(
                "predicted {predicted:.3} vs observed {observed:.3}, normalized gap {normalized:.3}"
            )),
        })
    }

    /// Observe whether an action made progress (reverse-burden no-op counter).
    ///
    /// Progress resets the counter. A no-op increments it; once the run reaches
    /// `config.noop_threshold` the burden reverses and each further no-op fires
    /// a signal whose magnitude is the run length.
    pub fn observe_action(&mut self, made_progress: bool) -> Option<PremiseStalenessSignal> {
        if made_progress {
            self.consecutive_noops = 0;
            return None;
        }
        self.consecutive_noops += 1;
        if self.config.noop_threshold > 0 && self.consecutive_noops >= self.config.noop_threshold {
            Some(PremiseStalenessSignal {
                session_id: self.session_id.clone(),
                kind: StalenessSignalKind::NoOpAccumulation,
                magnitude: self.consecutive_noops as f64,
                observation: self.observations,
                detail: Some(format!(
                    "{} consecutive no-ops >= threshold {}",
                    self.consecutive_noops, self.config.noop_threshold
                )),
            })
        } else {
            None
        }
    }
}

/// Start watching a session's premise validity with default tuning.
///
/// This is the `nous::watch_premise_validity(session)` entry point named in the
/// design: a session-scoped, stateful, deterministic premise-staleness detector.
pub fn watch_premise_validity(session_id: impl Into<String>) -> PremiseValidityWatcher {
    PremiseValidityWatcher::new(session_id, PremiseWatchConfig::default())
}

/// Dimensionality of the [`lexical_features`] vector.
pub const LEXICAL_DIM: usize = 64;

/// Deterministic, dependency-free lexical feature vector for a text output.
///
/// Tokens (maximal alphanumeric runs, lowercased) are hashed into a fixed
/// [`LEXICAL_DIM`]-bucket bag-of-words count vector using `DefaultHasher` (fixed
/// SipHash keys → stable across runs and machines). This is a cheap stand-in for
/// a real sentence embedding: it captures *structural* shifts in an output's
/// token distribution — exactly the "looks different from the last five
/// iterations" signal — with no model and no I/O. Swap in a real embedding by
/// calling [`PremiseValidityWatcher::observe_output`] with your own vector.
pub fn lexical_features(text: &str) -> Vec<f64> {
    let mut v = vec![0.0f64; LEXICAL_DIM];
    for token in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
    {
        let lower = token.to_ascii_lowercase();
        let mut hasher = DefaultHasher::new();
        lower.hash(&mut hasher);
        let bucket = (hasher.finish() as usize) % LEXICAL_DIM;
        v[bucket] += 1.0;
    }
    v
}

/// Element-wise mean of a window of feature vectors (zero-padded to the widest).
fn window_mean(window: &VecDeque<Vec<f64>>) -> Vec<f64> {
    let dim = window.iter().map(Vec::len).max().unwrap_or(0);
    let mut mean = vec![0.0f64; dim];
    if window.is_empty() {
        return mean;
    }
    for vec in window {
        for (i, &x) in vec.iter().enumerate() {
            mean[i] += x;
        }
    }
    let n = window.len() as f64;
    for m in &mut mean {
        *m /= n;
    }
    mean
}

/// Cosine distance `1 - cos_sim` between two vectors (zero-padded to the widest).
///
/// A zero-norm vector on either side yields distance `1.0` (maximal), since
/// similarity is undefined. Range is `[0.0, 2.0]`; for non-negative feature
/// vectors it stays within `[0.0, 1.0]`.
fn cosine_distance(a: &[f64], b: &[f64]) -> f64 {
    let dim = a.len().max(b.len());
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for i in 0..dim {
        let x = a.get(i).copied().unwrap_or(0.0);
        let y = b.get(i).copied().unwrap_or(0.0);
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a <= f64::EPSILON || norm_b <= f64::EPSILON {
        return 1.0;
    }
    let sim = dot / (norm_a.sqrt() * norm_b.sqrt());
    1.0 - sim
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_class_from_signals() {
        assert_eq!(
            FailureClass::from_signals(true, false),
            FailureClass::Epistemic
        );
        assert_eq!(
            FailureClass::from_signals(false, true),
            FailureClass::Control
        );
        assert_eq!(FailureClass::from_signals(true, true), FailureClass::Both);
        assert_eq!(
            FailureClass::from_signals(false, false),
            FailureClass::Neither
        );
    }

    #[test]
    fn failure_class_routes_one_to_one() {
        assert_eq!(
            FailureClass::Epistemic.route(),
            DiagnosisRoute::PremiseUpdate
        );
        assert_eq!(
            FailureClass::Control.route(),
            DiagnosisRoute::MonitoringTweak
        );
        assert_eq!(
            FailureClass::Both.route(),
            DiagnosisRoute::ArchitecturalRework
        );
        assert_eq!(
            FailureClass::Neither.route(),
            DiagnosisRoute::FalseAlarmCorpus
        );
    }

    #[test]
    fn failure_class_serde_roundtrip() {
        for class in [
            FailureClass::Epistemic,
            FailureClass::Control,
            FailureClass::Both,
            FailureClass::Neither,
        ] {
            let json = serde_json::to_string(&class).unwrap();
            let back: FailureClass = serde_json::from_str(&json).unwrap();
            assert_eq!(back, class);
        }
        assert_eq!(
            serde_json::to_string(&FailureClass::Epistemic).unwrap(),
            "\"epistemic\""
        );
    }

    #[test]
    fn diagnosis_derives_route_and_clamps_confidence() {
        let d = Diagnosis::new("s", FailureClass::Both, 1.7);
        assert_eq!(d.route, DiagnosisRoute::ArchitecturalRework);
        assert!((d.confidence - 1.0).abs() < f64::EPSILON);

        let d = Diagnosis::new("s", FailureClass::Neither, -0.5);
        assert!((d.confidence).abs() < f64::EPSILON);
        assert!(d.is_false_alarm());
    }

    #[test]
    fn diagnosis_classify_maps_axes() {
        assert_eq!(
            Diagnosis::classify("s", true, false, 0.9).class,
            FailureClass::Epistemic
        );
        assert_eq!(
            Diagnosis::classify("s", false, true, 0.9).class,
            FailureClass::Control
        );
        assert_eq!(
            Diagnosis::classify("s", true, true, 0.9).class,
            FailureClass::Both
        );
        let neither = Diagnosis::classify("s", false, false, 0.9);
        assert_eq!(neither.class, FailureClass::Neither);
        assert_eq!(neither.route, DiagnosisRoute::FalseAlarmCorpus);
    }

    #[test]
    fn cosine_distance_identical_is_zero() {
        let a = vec![1.0, 2.0, 3.0];
        assert!(cosine_distance(&a, &a).abs() < 1e-9);
    }

    #[test]
    fn cosine_distance_orthogonal_is_one() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((cosine_distance(&a, &b) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn cosine_distance_zero_norm_is_max() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 1.0];
        assert!((cosine_distance(&a, &b) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn cosine_distance_handles_unequal_lengths() {
        let a = vec![1.0, 1.0];
        let b = vec![1.0, 1.0, 0.0, 0.0];
        assert!(cosine_distance(&a, &b).abs() < 1e-9);
    }

    #[test]
    fn window_warmup_returns_none() {
        let mut w = PremiseValidityWatcher::new("s", PremiseWatchConfig::default());
        // window=5 default; first 5 observations are warm-up.
        for _ in 0..5 {
            assert!(w.observe_output(&[1.0, 0.0, 0.0]).is_none());
        }
        assert_eq!(w.observations(), 5);
    }

    #[test]
    fn output_divergence_fires_on_structural_shift() {
        let cfg = PremiseWatchConfig {
            window: 3,
            distance_threshold: 0.3,
            ..Default::default()
        };
        let mut w = PremiseValidityWatcher::new("s", cfg);
        // Establish a stable baseline pointing along axis 0.
        for _ in 0..3 {
            assert!(w.observe_output(&[1.0, 0.0, 0.0]).is_none());
        }
        // A consistent output does not fire.
        assert!(w.observe_output(&[1.0, 0.0, 0.0]).is_none());
        // A structurally different output (orthogonal) fires.
        let sig = w.observe_output(&[0.0, 1.0, 0.0]).expect("should fire");
        assert_eq!(sig.kind, StalenessSignalKind::OutputDivergence);
        assert!(sig.magnitude > 0.3);
        assert_eq!(sig.session_id, "s");
    }

    #[test]
    fn output_divergence_via_text() {
        let cfg = PremiseWatchConfig {
            window: 3,
            distance_threshold: 0.5,
            ..Default::default()
        };
        let mut w = PremiseValidityWatcher::new("s", cfg);
        for _ in 0..3 {
            assert!(
                w.observe_output_text("reading the config file and applying the plan")
                    .is_none()
            );
        }
        // Same topic — should stay quiet.
        assert!(
            w.observe_output_text("reading the config file and applying the plan step")
                .is_none()
        );
        // Totally different vocabulary — should fire.
        let sig = w.observe_output_text("quantum entanglement violin sonata pancake");
        assert!(sig.is_some());
        assert_eq!(sig.unwrap().kind, StalenessSignalKind::OutputDivergence);
    }

    #[test]
    fn plant_divergence_normalizes() {
        let mut w = watch_premise_validity("s");
        // Small gap relative to magnitude — no fire.
        assert!(w.observe_plant(100.0, 105.0).is_none());
        // Large relative gap — fires.
        let sig = w.observe_plant(100.0, 20.0).expect("should fire");
        assert_eq!(sig.kind, StalenessSignalKind::PlantDivergence);
        assert!(sig.magnitude > 0.5);
        assert!(sig.magnitude <= 1.0);
    }

    #[test]
    fn noop_accumulation_reverse_burden() {
        let cfg = PremiseWatchConfig {
            noop_threshold: 3,
            ..Default::default()
        };
        let mut w = PremiseValidityWatcher::new("s", cfg);
        assert!(w.observe_action(false).is_none()); // 1
        assert!(w.observe_action(false).is_none()); // 2
        let sig = w.observe_action(false).expect("should fire at 3"); // 3
        assert_eq!(sig.kind, StalenessSignalKind::NoOpAccumulation);
        assert!((sig.magnitude - 3.0).abs() < f64::EPSILON);
        // Progress resets the counter.
        assert!(w.observe_action(true).is_none());
        assert_eq!(w.consecutive_noops(), 0);
        assert!(w.observe_action(false).is_none()); // 1 again
    }

    #[test]
    fn signal_diagnoses_epistemic_or_both() {
        let mut w = watch_premise_validity("s");
        let sig = w.observe_plant(1.0, 0.0).expect("should fire");
        // Premise stale, control ok → epistemic → premise update.
        let d = sig.diagnose(false);
        assert_eq!(d.class, FailureClass::Epistemic);
        assert_eq!(d.route, DiagnosisRoute::PremiseUpdate);
        // Premise stale AND control diverged → both → architectural rework.
        let d = sig.diagnose(true);
        assert_eq!(d.class, FailureClass::Both);
        assert_eq!(d.route, DiagnosisRoute::ArchitecturalRework);
        assert!(d.rationale.is_some());
    }

    #[test]
    fn lexical_features_is_deterministic() {
        let a = lexical_features("the quick brown fox");
        let b = lexical_features("the quick brown fox");
        assert_eq!(a, b);
        assert_eq!(a.len(), LEXICAL_DIM);
        // Non-empty text produces a non-zero vector.
        assert!(a.iter().any(|&x| x > 0.0));
        // Empty / punctuation-only text produces a zero vector.
        assert!(lexical_features("   ... !!! ").iter().all(|&x| x == 0.0));
    }

    #[test]
    fn zero_window_disables_output_divergence() {
        let cfg = PremiseWatchConfig {
            window: 0,
            ..Default::default()
        };
        let mut w = PremiseValidityWatcher::new("s", cfg);
        for _ in 0..10 {
            assert!(w.observe_output(&[1.0, 0.0]).is_none());
        }
    }

    #[test]
    fn watcher_is_clone_and_session_scoped() {
        let w = watch_premise_validity("sess-42");
        assert_eq!(w.session_id(), "sess-42");
        let w2 = w.clone();
        assert_eq!(w2.session_id(), "sess-42");
    }
}
