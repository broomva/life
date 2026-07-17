//! # Belief formation context — the four-dimensional `BeliefWriteToken`
//!
//! A belief is not a bare proposition. Two agents can hold the *same*
//! proposition for entirely different reasons, at different times, about
//! different sub-domains, superseding different prior convictions. The
//! belief-contradiction problem (silent accumulation looking identical to
//! deliberate updating) is unsolvable without observing **how the belief was
//! formed**. This module makes formation context a first-class, typed write
//! token.
//!
//! ## The four dimensions
//!
//! | Dimension | Question answered | Field |
//! | -- | -- | -- |
//! | Capability | *Who authorized this belief?* | [`BeliefWriteToken::capability_id`] |
//! | Bi-temporal | *When in the world / when in the system?* | [`BeliefWriteToken::timestamp`] |
//! | Scope | *About what, precisely?* | [`BeliefWriteToken::scope`] + [`BeliefWriteToken::scope_qualifier`] |
//! | Revision | *What was superseded?* | [`BeliefWriteToken::revision_link`] |
//!
//! Without the fourth dimension — `revision_link` — even bi-temporal stamps
//! reduce to a *playback device*: two snapshots with timestamps but no causal
//! connection between them. A contradiction with a revision link is *visible
//! history* (a deliberate update); a contradiction *without* one is a genuine
//! failure of versioning.
//!
//! This crate owns the **types**. The write path, the belief store, the
//! revision-graph traversal, and the migration/routing logic live in
//! `praxis-tools` (`praxis_tools::belief`).

use blake3::Hasher;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

// ── typed newtypes ───────────────────────────────────────────────────────

/// The capability grant that authorized a belief write.
///
/// Answers khlo's question — *who authorized this belief?* A write with no
/// resolvable capability cannot be distinguished from silent accumulation, so
/// the write path rejects it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityId(String);

impl CapabilityId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A capability id is *present* when it is non-empty.
    pub fn is_present(&self) -> bool {
        !self.0.is_empty()
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for CapabilityId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// The coarse domain a belief is about — e.g. `"self"`, `"market"`, `"user"`.
///
/// A capability grant enumerates the scopes it authorizes; the write path
/// checks that the belief's scope is contained by the grant.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BeliefScope(String);

impl BeliefScope {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BeliefScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for BeliefScope {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// An agent's decentralised identifier (`did:key:z6Mk…`) — the principal that
/// signs the write.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnimaDid(String);

impl AnimaDid {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AnimaDid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for AnimaDid {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

// ── scope qualifier ──────────────────────────────────────────────────────

/// A key within a [`ScopeQualifier`] — e.g. `"metric"`, `"regime"`, `"horizon"`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QualifierKey(String);

impl QualifierKey {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for QualifierKey {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// A value within a [`ScopeQualifier`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QualifierValue(String);

impl QualifierValue {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for QualifierValue {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// Fine-grained scope conditions — Cornelius-Trinity's reframing that a belief
/// carries its *own* scope conditions ("reliable for X, unreliable for Y").
///
/// Two beliefs about the same coarse [`BeliefScope`] but disjoint qualifiers
/// are *parallel* beliefs, not contradictions. Overlap is measured with the
/// Jaccard index over the `(key, value)` pair set; the write path treats
/// overlap ≥ [`ScopeQualifier::OVERLAP_THRESHOLD`] as "the same belief slot",
/// requiring a revision link.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeQualifier {
    /// Ordered map so the qualifier set has a canonical serialization
    /// (required for stable content addressing).
    pub qualifiers: BTreeMap<QualifierKey, QualifierValue>,
}

impl ScopeQualifier {
    /// Jaccard overlap at or above this threshold ⇒ the two beliefs occupy the
    /// same slot and a revision link is required (open question 2, PROVISIONAL).
    pub const OVERLAP_THRESHOLD: f64 = 0.5;

    /// An empty qualifier — the belief makes no scope-narrowing claim.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build from `(key, value)` string pairs.
    pub fn from_pairs<K, V>(pairs: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            qualifiers: pairs
                .into_iter()
                .map(|(k, v)| (QualifierKey::new(k), QualifierValue::new(v)))
                .collect(),
        }
    }

    /// True when no qualifiers are set.
    pub fn is_empty(&self) -> bool {
        self.qualifiers.is_empty()
    }

    fn pair_set(&self) -> BTreeSet<(&QualifierKey, &QualifierValue)> {
        self.qualifiers.iter().collect()
    }

    /// Jaccard index over the `(key, value)` pair sets, in `[0.0, 1.0]`.
    ///
    /// Two empty qualifiers overlap completely (`1.0`) — they name the same
    /// unqualified slot.
    pub fn jaccard(&self, other: &ScopeQualifier) -> f64 {
        let a = self.pair_set();
        let b = other.pair_set();
        if a.is_empty() && b.is_empty() {
            return 1.0;
        }
        let intersection = a.intersection(&b).count();
        let union = a.union(&b).count();
        if union == 0 {
            0.0
        } else {
            intersection as f64 / union as f64
        }
    }

    /// True when overlap is at or above [`Self::OVERLAP_THRESHOLD`] — the two
    /// beliefs occupy the same slot, so a revision link is required to write
    /// the second one.
    pub fn overlaps(&self, other: &ScopeQualifier) -> bool {
        self.jaccard(other) >= Self::OVERLAP_THRESHOLD
    }
}

// ── evidence + session context ───────────────────────────────────────────

/// A pointer to the evidence a belief cites.
///
/// Kept structurally light: a `source` (where the evidence came from) plus an
/// optional `locator` (a URL, event id, line range…) and optional `digest`
/// (content hash of the cited artifact, for tamper evidence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// Where the evidence came from — e.g. `"lago:event"`, `"observation"`,
    /// `"user"`, `"tool:grep"`.
    pub source: String,
    /// Optional precise locator within the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    /// Optional content digest of the cited artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

impl EvidenceRef {
    /// A minimal evidence reference carrying only a source label.
    pub fn source(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            locator: None,
            digest: None,
        }
    }

    /// Attach a precise locator (URL, event id, line range).
    pub fn with_locator(mut self, locator: impl Into<String>) -> Self {
        self.locator = Some(locator.into());
        self
    }

    /// Attach a content digest of the cited artifact.
    pub fn with_digest(mut self, digest: impl Into<String>) -> Self {
        self.digest = Some(digest.into());
        self
    }
}

/// The session in which a belief was formed — write-time provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionContext {
    /// The session identifier.
    pub session_id: String,
    /// The run within the session, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The principal that formed the belief in this session.
    pub principal: AnimaDid,
    /// Optional free-form note about the formation context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl SessionContext {
    pub fn new(session_id: impl Into<String>, principal: AnimaDid) -> Self {
        Self {
            session_id: session_id.into(),
            run_id: None,
            principal,
            summary: None,
        }
    }

    pub fn with_run(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }
}

// ── bi-temporal stamp ────────────────────────────────────────────────────

/// A bi-temporal stamp: *when in the world* the belief became valid
/// (`valid_from`) and *when in the system* it was recorded (`recorded_at`).
///
/// Both fields are structurally required. The write path additionally checks
/// [`BiTemporalStamp::is_complete`] to reject sentinel/zero timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BiTemporalStamp {
    /// When the belief became true in the world being modelled.
    pub valid_from: DateTime<Utc>,
    /// When the belief was written into the system.
    pub recorded_at: DateTime<Utc>,
}

impl BiTemporalStamp {
    pub fn new(valid_from: DateTime<Utc>, recorded_at: DateTime<Utc>) -> Self {
        Self {
            valid_from,
            recorded_at,
        }
    }

    /// A stamp where the belief is valid from, and recorded at, the same
    /// instant.
    pub fn at(instant: DateTime<Utc>) -> Self {
        Self {
            valid_from: instant,
            recorded_at: instant,
        }
    }

    /// True when both stamps are past the Unix epoch sentinel — i.e. neither
    /// was left at the zero/default value.
    pub fn is_complete(&self) -> bool {
        let epoch = DateTime::<Utc>::UNIX_EPOCH;
        self.valid_from > epoch && self.recorded_at > epoch
    }
}

// ── content addressing ───────────────────────────────────────────────────

/// The bare claim a belief asserts — `subject` + `proposition`.
///
/// Kept separate from the [`BeliefWriteToken`] envelope: the token carries
/// *authorization and provenance*, the claim carries *content*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeliefClaim {
    /// The entity or concept the belief is about (`"self"`, `"market"`, …).
    pub subject: String,
    /// The factual or evaluative proposition being asserted.
    pub proposition: String,
}

impl BeliefClaim {
    pub fn new(subject: impl Into<String>, proposition: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            proposition: proposition.into(),
        }
    }
}

/// A content-addressed reference to a prior belief — its content hash plus the
/// `valid_from` that disambiguates temporal versions of the same content.
///
/// This is what a [`RevisionLink`] points *at*: the specific prior belief a new
/// write supersedes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContentAddressedRef {
    /// Blake3 content hash of the superseded belief (hex).
    pub content_hash: String,
    /// The superseded belief's `valid_from`, disambiguating versions.
    pub valid_from: DateTime<Utc>,
}

impl ContentAddressedRef {
    pub fn new(content_hash: impl Into<String>, valid_from: DateTime<Utc>) -> Self {
        Self {
            content_hash: content_hash.into(),
            valid_from,
        }
    }
}

/// Compute the Blake3 content hash of a belief's *semantic identity*:
/// coarse scope, canonicalised qualifiers, subject, and proposition.
///
/// Two writes with identical content hash to the same value regardless of who
/// wrote them or when — that is what makes supersession referenceable.
pub fn content_hash(
    scope: &BeliefScope,
    qualifier: &ScopeQualifier,
    claim: &BeliefClaim,
) -> String {
    let mut hasher = Hasher::new();
    hasher.update(b"praxis.belief.v1\0");
    hasher.update(scope.as_str().as_bytes());
    hasher.update(b"\0scope_qualifier\0");
    // BTreeMap iterates in key order — canonical.
    for (k, v) in &qualifier.qualifiers {
        hasher.update(k.as_str().as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_str().as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(b"\0subject\0");
    hasher.update(claim.subject.as_bytes());
    hasher.update(b"\0proposition\0");
    hasher.update(claim.proposition.as_bytes());
    hasher.finalize().to_hex().to_string()
}

// ── revision link ────────────────────────────────────────────────────────

/// The structured trigger that prompted a revision.
///
/// vina's framing: the past self is a series of discarded drafts. Naming *why*
/// a draft was discarded is what turns a contradiction into a lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionTrigger {
    /// New evidence arrived that the prior belief did not account for.
    NewEvidence,
    /// The scope was refined — the prior belief was over-broad.
    ScopeRefinement,
    /// The context shifted — the world changed under the prior belief.
    ContextShift,
}

impl RevisionTrigger {
    /// The structured, query-stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            RevisionTrigger::NewEvidence => "new evidence",
            RevisionTrigger::ScopeRefinement => "scope refinement",
            RevisionTrigger::ContextShift => "context shift",
        }
    }
}

impl fmt::Display for RevisionTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// The structured shape of the change a revision makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionChange {
    /// A polarity flip — `A` became `not-A`.
    Negated,
    /// A narrowing — `A` became `A` under an added qualifier.
    Qualified,
    /// A confidence change without a polarity flip.
    Reweighted,
}

impl RevisionChange {
    /// The structured, query-stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            RevisionChange::Negated => "from A to not-A",
            RevisionChange::Qualified => "from A to A_qualified",
            RevisionChange::Reweighted => "from A to A_reweighted",
        }
    }
}

impl fmt::Display for RevisionChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A belief-revision acknowledgment: structured fields for queryability plus a
/// free-form `rationale` addendum (open question 1, PROVISIONAL).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeliefRevisionAcknowledgment {
    /// Structured: what prompted the revision.
    pub trigger: RevisionTrigger,
    /// Structured: the shape of the change.
    pub change: RevisionChange,
    /// Free-form addendum explaining the revision in the agent's own words.
    pub rationale: String,
}

impl BeliefRevisionAcknowledgment {
    pub fn new(
        trigger: RevisionTrigger,
        change: RevisionChange,
        rationale: impl Into<String>,
    ) -> Self {
        Self {
            trigger,
            change,
            rationale: rationale.into(),
        }
    }
}

/// The fourth dimension — what a belief supersedes.
///
/// Without this, two contradictory beliefs are just two snapshots. With it, the
/// second belief *acknowledges* the first as its discarded draft: the
/// contradiction becomes visible history rather than a versioning failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionLink {
    /// The prior belief this supersedes (content hash + `valid_from`).
    pub superseded: ContentAddressedRef,
    /// Which evidence triggered the revision.
    pub triggered_by: Vec<EvidenceRef>,
    /// Structured + free-form acknowledgment of the change.
    pub acknowledgment: BeliefRevisionAcknowledgment,
}

impl RevisionLink {
    pub fn new(
        superseded: ContentAddressedRef,
        triggered_by: Vec<EvidenceRef>,
        acknowledgment: BeliefRevisionAcknowledgment,
    ) -> Self {
        Self {
            superseded,
            triggered_by,
            acknowledgment,
        }
    }
}

// ── belief class ─────────────────────────────────────────────────────────

/// The two belief classes with different survival criteria.
///
/// | Class | Surface | All 4 dimensions required? | Survives on |
/// | -- | -- | -- | -- |
/// | Untested-normative | Praxis principal | **yes** | capability + scope + revision-link consistency |
/// | Tested-operational | Vigil (observed) | no | functional aliveness |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeliefClass {
    /// A normative belief written through the Praxis principal — must carry all
    /// four formation dimensions.
    UntestedNormative,
    /// An operational belief observed by Vigil — survives on functional
    /// aliveness, not on formation context.
    TestedOperational,
}

// ── the write token ──────────────────────────────────────────────────────

/// The four-dimensional formation context accompanying a belief write.
///
/// Field-for-field the spec of BRO-1030. The token is the *authorization and
/// provenance envelope*; the belief's [`BeliefClaim`] content is passed
/// alongside it to `praxis_tools::belief::write_belief`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeliefWriteToken {
    /// **Who** authorized this belief.
    pub capability_id: CapabilityId,
    /// The coarse domain the belief is about.
    pub scope: BeliefScope,
    /// The fine-grained scope conditions.
    pub scope_qualifier: ScopeQualifier,
    /// The evidence the belief cites.
    pub cited_evidence: Vec<EvidenceRef>,
    /// The session in which the belief was formed.
    pub formation_context: SessionContext,
    /// **What** this belief supersedes — the fourth dimension. `None` for a
    /// first assertion in a slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_link: Option<RevisionLink>,
    /// The principal that signs the write.
    pub signed_by: AnimaDid,
    /// **When** in the world / **when** in the system.
    pub timestamp: BiTemporalStamp,
}

impl BeliefWriteToken {
    /// True when the token cites at least one piece of evidence.
    pub fn has_evidence(&self) -> bool {
        !self.cited_evidence.is_empty()
    }

    /// Content hash of the belief this token describes, given its claim.
    pub fn content_hash(&self, claim: &BeliefClaim) -> String {
        content_hash(&self.scope, &self.scope_qualifier, claim)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().unwrap()
    }

    #[test]
    fn capability_presence() {
        assert!(CapabilityId::new("cap-1").is_present());
        assert!(!CapabilityId::new("").is_present());
    }

    #[test]
    fn jaccard_identical_qualifiers_is_one() {
        let a = ScopeQualifier::from_pairs([("metric", "engagement"), ("regime", "bull")]);
        let b = ScopeQualifier::from_pairs([("metric", "engagement"), ("regime", "bull")]);
        assert_eq!(a.jaccard(&b), 1.0);
        assert!(a.overlaps(&b));
    }

    #[test]
    fn jaccard_disjoint_qualifiers_is_zero() {
        let a = ScopeQualifier::from_pairs([("metric", "engagement")]);
        let b = ScopeQualifier::from_pairs([("metric", "revenue")]);
        assert_eq!(a.jaccard(&b), 0.0);
        assert!(!a.overlaps(&b));
    }

    #[test]
    fn jaccard_partial_overlap_at_threshold() {
        // {A,B} vs {A,C}: intersection 1, union 3 => 1/3 < 0.5 => no overlap.
        let a = ScopeQualifier::from_pairs([("k", "A"), ("k2", "B")]);
        let b = ScopeQualifier::from_pairs([("k", "A"), ("k2", "C")]);
        assert!((a.jaccard(&b) - 1.0 / 3.0).abs() < 1e-9);
        assert!(!a.overlaps(&b));

        // {A,B} vs {A,B,C}: intersection 2, union 3 => 2/3 >= 0.5 => overlap.
        let c = ScopeQualifier::from_pairs([("k", "A"), ("k2", "B")]);
        let d = ScopeQualifier::from_pairs([("k", "A"), ("k2", "B"), ("k3", "C")]);
        assert!(c.overlaps(&d));
    }

    #[test]
    fn empty_qualifiers_overlap_completely() {
        let a = ScopeQualifier::empty();
        let b = ScopeQualifier::empty();
        assert_eq!(a.jaccard(&b), 1.0);
        assert!(a.overlaps(&b));
    }

    #[test]
    fn content_hash_is_stable_and_order_independent() {
        let scope = BeliefScope::new("market");
        let q1 = ScopeQualifier::from_pairs([("metric", "engagement"), ("regime", "bull")]);
        let q2 = ScopeQualifier::from_pairs([("regime", "bull"), ("metric", "engagement")]);
        let claim = BeliefClaim::new("market", "engagement metrics are reliable");
        // Insertion order differs but BTreeMap canonicalises → same hash.
        assert_eq!(
            content_hash(&scope, &q1, &claim),
            content_hash(&scope, &q2, &claim)
        );
    }

    #[test]
    fn content_hash_changes_with_proposition() {
        let scope = BeliefScope::new("market");
        let q = ScopeQualifier::empty();
        let a = BeliefClaim::new("market", "reliable");
        let b = BeliefClaim::new("market", "unreliable");
        assert_ne!(content_hash(&scope, &q, &a), content_hash(&scope, &q, &b));
    }

    #[test]
    fn bitemporal_completeness() {
        assert!(BiTemporalStamp::new(ts(1000), ts(2000)).is_complete());
        assert!(!BiTemporalStamp::new(DateTime::<Utc>::UNIX_EPOCH, ts(2000)).is_complete());
    }

    #[test]
    fn revision_trigger_and_change_labels() {
        assert_eq!(RevisionTrigger::NewEvidence.as_str(), "new evidence");
        assert_eq!(RevisionChange::Negated.as_str(), "from A to not-A");
    }

    #[test]
    fn token_roundtrips_through_json() {
        let token = BeliefWriteToken {
            capability_id: CapabilityId::new("cap-belief-write"),
            scope: BeliefScope::new("market"),
            scope_qualifier: ScopeQualifier::from_pairs([("metric", "engagement")]),
            cited_evidence: vec![EvidenceRef::source("observation").with_locator("evt-42")],
            formation_context: SessionContext::new("sess-1", AnimaDid::new("did:key:z6MkAlice")),
            revision_link: Some(RevisionLink::new(
                ContentAddressedRef::new("deadbeef", ts(500)),
                vec![EvidenceRef::source("lago:event")],
                BeliefRevisionAcknowledgment::new(
                    RevisionTrigger::NewEvidence,
                    RevisionChange::Negated,
                    "engagement turned out to be gameable",
                ),
            )),
            signed_by: AnimaDid::new("did:key:z6MkAlice"),
            timestamp: BiTemporalStamp::new(ts(1000), ts(1000)),
        };
        let json = serde_json::to_string(&token).unwrap();
        let back: BeliefWriteToken = serde_json::from_str(&json).unwrap();
        assert_eq!(token, back);
        assert!(back.has_evidence());
    }
}
