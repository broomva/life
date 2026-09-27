//! Reconcile discrepancy taxonomy — Nous's contribution to `bstack reconcile`.
//!
//! Reconcile (BRO-1039) cross-references narrative claims against external
//! evidence. It historically catches two forward discrepancies:
//!
//! - [`ClaimedNotDone`](DiscrepancyClass::ClaimedNotDone) — the narrative claims
//!   an action; no external evidence backs it (deflection).
//! - [`ReportedNoAction`](DiscrepancyClass::ReportedNoAction) — report-mass
//!   without corresponding action-mass.
//!
//! The premise-staleness work adds two more classes, both driven by the
//! dual-classifier surface in [`crate::premise`]:
//!
//! - [`PremiseStale`](DiscrepancyClass::PremiseStale) — the **third** class
//!   (ticket item 4): the plan is internally valid but its premise drifted, so
//!   narrative and evidence agree with each other yet both diverge from the
//!   *current world*. Routes to a premise update, not a monitoring tweak.
//! - [`ShadowCompetence`](DiscrepancyClass::ShadowCompetence) — the **inverse**
//!   class (ticket item 5): *done-but-not-claimed*. A Vigil span shows an action
//!   the bridge log never planned — a successful adaptation the architecture
//!   never authorized. This is the dual of `ClaimedNotDone` and is flagged as a
//!   **promote candidate**, not a failure. See
//!   `research/entities/pattern/shadow-competence.md`.

use serde::{Deserialize, Serialize};

use crate::premise::FailureClass;

/// A class of discrepancy surfaced by reconcile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscrepancyClass {
    /// Narrative claims an action; no external evidence backs it.
    ClaimedNotDone,
    /// Report-mass without corresponding action-mass.
    ReportedNoAction,
    /// Plan valid, but its premise drifted out from under it (third class).
    PremiseStale,
    /// Evidence of an action with no matching plan — done-but-not-claimed.
    ShadowCompetence,
}

impl DiscrepancyClass {
    /// Stable snake_case label for events and logs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::ClaimedNotDone => "claimed_not_done",
            Self::ReportedNoAction => "reported_no_action",
            Self::PremiseStale => "premise_stale",
            Self::ShadowCompetence => "shadow_competence",
        }
    }

    /// Whether this discrepancy is a promotion candidate rather than a failure.
    ///
    /// Only shadow competence qualifies: it is a *successful* adaptation the
    /// architecture did not authorize, worth crystallizing into official
    /// structure (subject to the architecture's judgment — detection is not
    /// endorsement).
    pub fn is_promote_candidate(&self) -> bool {
        matches!(self, Self::ShadowCompetence)
    }

    /// The dual-classifier failure axis this discrepancy maps to, if any.
    ///
    /// `PremiseStale` maps to [`FailureClass::Epistemic`] (fix the premise).
    /// The narrative-vs-evidence classes and shadow competence are reconcile's
    /// own concerns and do not route through the failure classifier.
    pub fn failure_class(&self) -> Option<FailureClass> {
        match self {
            Self::PremiseStale => Some(FailureClass::Epistemic),
            _ => None,
        }
    }
}

impl std::fmt::Display for DiscrepancyClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// A single reconcile finding: a class, its session, and an optional detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconcileDiscrepancy {
    /// Session the discrepancy belongs to.
    pub session_id: String,
    /// Which class of discrepancy.
    pub class: DiscrepancyClass,
    /// Optional human-readable detail.
    pub detail: Option<String>,
}

impl ReconcileDiscrepancy {
    /// Construct a discrepancy of the given class.
    pub fn new(session_id: impl Into<String>, class: DiscrepancyClass) -> Self {
        Self {
            session_id: session_id.into(),
            class,
            detail: None,
        }
    }

    /// Attach a detail string.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Detect a shadow-competence trail: action evidence without a plan.
    ///
    /// The done-but-not-claimed detector (ticket item 5): a Vigil span shows an
    /// action (`has_action_span`) but the bridge log has no plan
    /// (`has_plan_record`). When those hold, the action is a shadow trail and a
    /// promote candidate; otherwise there is nothing to flag.
    pub fn from_shadow_trail(
        session_id: impl Into<String>,
        has_action_span: bool,
        has_plan_record: bool,
    ) -> Option<Self> {
        (has_action_span && !has_plan_record).then(|| {
            Self::new(session_id, DiscrepancyClass::ShadowCompetence).with_detail(
                "action span present with no matching plan record (done-but-not-claimed)",
            )
        })
    }

    /// Whether this finding should be routed as a promote candidate.
    pub fn is_promote_candidate(&self) -> bool {
        self.class.is_promote_candidate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_stable() {
        assert_eq!(DiscrepancyClass::ClaimedNotDone.label(), "claimed_not_done");
        assert_eq!(
            DiscrepancyClass::ReportedNoAction.label(),
            "reported_no_action"
        );
        assert_eq!(DiscrepancyClass::PremiseStale.label(), "premise_stale");
        assert_eq!(
            DiscrepancyClass::ShadowCompetence.label(),
            "shadow_competence"
        );
    }

    #[test]
    fn only_shadow_competence_is_promote_candidate() {
        assert!(!DiscrepancyClass::ClaimedNotDone.is_promote_candidate());
        assert!(!DiscrepancyClass::ReportedNoAction.is_promote_candidate());
        assert!(!DiscrepancyClass::PremiseStale.is_promote_candidate());
        assert!(DiscrepancyClass::ShadowCompetence.is_promote_candidate());
    }

    #[test]
    fn premise_stale_maps_to_epistemic() {
        assert_eq!(
            DiscrepancyClass::PremiseStale.failure_class(),
            Some(FailureClass::Epistemic)
        );
        assert!(DiscrepancyClass::ClaimedNotDone.failure_class().is_none());
        assert!(DiscrepancyClass::ShadowCompetence.failure_class().is_none());
    }

    #[test]
    fn shadow_trail_detects_action_without_plan() {
        // Action span, no plan → shadow trail (promote candidate).
        let d = ReconcileDiscrepancy::from_shadow_trail("s", true, false)
            .expect("should detect shadow trail");
        assert_eq!(d.class, DiscrepancyClass::ShadowCompetence);
        assert!(d.is_promote_candidate());
        assert!(d.detail.is_some());

        // Action span with a plan → authorized, nothing to flag.
        assert!(ReconcileDiscrepancy::from_shadow_trail("s", true, true).is_none());
        // Plan but no action → not this detector's concern (forward case).
        assert!(ReconcileDiscrepancy::from_shadow_trail("s", false, true).is_none());
        // Neither → nothing.
        assert!(ReconcileDiscrepancy::from_shadow_trail("s", false, false).is_none());
    }

    #[test]
    fn discrepancy_serde_roundtrip() {
        let d = ReconcileDiscrepancy::new("sess-1", DiscrepancyClass::PremiseStale)
            .with_detail("premise drifted");
        let json = serde_json::to_string(&d).unwrap();
        let back: ReconcileDiscrepancy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
        assert_eq!(back.class, DiscrepancyClass::PremiseStale);
    }

    #[test]
    fn discrepancy_class_serde_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&DiscrepancyClass::ShadowCompetence).unwrap(),
            "\"shadow_competence\""
        );
    }
}
