//! # Belief write path, store, and revision-graph traversal
//!
//! This module is the runtime for the four-dimensional [`BeliefWriteToken`]
//! defined in `praxis_core::belief`. It provides:
//!
//! - [`BeliefStore`] — an in-memory reference store of belief records with a
//!   capability-grant registry (mirroring the `praxis-skills` /
//!   `nous-tools::lineage` in-memory-reference convention). A lago-backed
//!   store lands separately.
//! - [`BeliefStore::write_belief`] — the write path enforcing the six checks,
//!   including the new [`BeliefWriteError::MissingRevisionLink`] on overlapping
//!   scope.
//! - [`BeliefStore::traverse_revisions`] — the revision-graph traversal API
//!   returning the chain of supersession (immediate-predecessor links,
//!   reconstructed transitively by walking).
//! - [`route_write`] / [`BeliefStore::record_operational`] — the migration:
//!   token-less writes route to the Vigil-observed *tested-operational* class;
//!   token writes take the Praxis *untested-normative* path.
//! - [`BeliefStore::recent_supersessions`] — the substrate read-model the Nous
//!   L2 metacognitive surface projects as *"what did I supersede recently and
//!   why"*.
//! - [`revision_masks_contradiction`] — the bookkeeping gate: a contradiction
//!   covered by a revision link is *visible history*, not a contradiction.

use chrono::{DateTime, Utc};
use praxis_core::belief::{
    AnimaDid, BeliefClaim, BeliefClass, BeliefRevisionAcknowledgment, BeliefScope,
    BeliefWriteToken, ContentAddressedRef, RevisionLink, ScopeQualifier, content_hash,
};
use std::collections::{BTreeSet, HashMap};
use thiserror::Error;

/// A capability grant — which coarse [`BeliefScope`]s a capability authorizes a
/// principal to write beliefs about.
///
/// The write path resolves [`BeliefWriteToken::capability_id`] against the
/// store's registry and checks scope containment.
#[derive(Debug, Clone)]
pub struct CapabilityGrant {
    /// The capability id (matches [`BeliefWriteToken::capability_id`]).
    pub id: praxis_core::belief::CapabilityId,
    /// The scopes this capability authorizes belief writes about.
    pub granted_scopes: BTreeSet<BeliefScope>,
}

impl CapabilityGrant {
    /// A grant for a single scope.
    pub fn new(id: impl Into<String>, scopes: impl IntoIterator<Item = BeliefScope>) -> Self {
        Self {
            id: praxis_core::belief::CapabilityId::new(id),
            granted_scopes: scopes.into_iter().collect(),
        }
    }

    /// Whether this grant authorizes writes about `scope`.
    pub fn grants(&self, scope: &BeliefScope) -> bool {
        self.granted_scopes.contains(scope)
    }
}

/// A stored belief record — the claim, its four-dimensional token, and derived
/// bookkeeping fields.
#[derive(Debug, Clone)]
pub struct BeliefRecord {
    /// Store-assigned identifier (`blf-00000001`).
    pub id: String,
    /// Blake3 content hash of the belief's semantic identity.
    pub content_hash: String,
    /// The claim the belief asserts.
    pub claim: BeliefClaim,
    /// The four-dimensional formation token.
    pub token: BeliefWriteToken,
    /// Belief class — normative (Praxis) or operational (Vigil).
    pub class: BeliefClass,
    /// If a later belief has superseded this one, its id.
    pub superseded_by: Option<String>,
}

impl BeliefRecord {
    /// A content-addressed reference to *this* belief, for another belief to
    /// supersede it.
    pub fn as_ref(&self) -> ContentAddressedRef {
        ContentAddressedRef::new(self.content_hash.clone(), self.token.timestamp.valid_from)
    }

    /// True while no later belief supersedes this one.
    pub fn is_live(&self) -> bool {
        self.superseded_by.is_none()
    }
}

/// Errors returned by the belief write path.
#[derive(Debug, Clone, Error, PartialEq)]
pub enum BeliefWriteError {
    /// Check 1 — no capability provided.
    #[error("belief write rejected: no capability provided")]
    MissingCapability,

    /// Check 1 — capability provided but not registered.
    #[error("belief write rejected: capability '{capability}' is not registered")]
    UnknownCapability {
        /// The unresolved capability id.
        capability: String,
    },

    /// Check 2 — the capability does not grant the belief's scope.
    #[error("belief write rejected: capability '{capability}' does not grant scope '{scope}'")]
    ScopeMismatch {
        /// The capability id.
        capability: String,
        /// The scope that was not granted.
        scope: String,
    },

    /// Check 3 — a normative belief must carry a scope qualifier.
    #[error("belief write rejected: a normative belief requires a scope qualifier")]
    MissingScopeQualifier,

    /// Check 4 — a normative belief must cite evidence.
    #[error("belief write rejected: a normative belief must cite at least one evidence reference")]
    MissingEvidence,

    /// Check 5 — an overlapping belief exists but no revision link was provided.
    #[error(
        "belief write rejected: a belief with overlapping scope (jaccard {jaccard:.2}) already \
         exists for this principal (id {existing_id}); either revise the existing belief or \
         specify a non-overlapping scope"
    )]
    MissingRevisionLink {
        /// The id of the live overlapping belief the write must revise.
        existing_id: String,
        /// The measured qualifier overlap.
        jaccard: f64,
    },

    /// Check 5 — a revision link was provided but its target does not exist.
    #[error("belief write rejected: revision link supersedes {hash} but no such belief exists")]
    RevisionTargetNotFound {
        /// The dangling superseded content hash.
        hash: String,
    },

    /// Check 6 — the bi-temporal stamp is incomplete.
    #[error(
        "belief write rejected: bi-temporal stamp is incomplete (valid_from and recorded_at must \
         both be set)"
    )]
    IncompleteBiTemporalStamp,
}

/// Where a belief write is routed — the two-class migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeliefWriteRoute {
    /// Token-bearing write → Praxis principal, untested-normative class, all
    /// four formation dimensions enforced.
    PraxisNormative,
    /// Token-less write → Vigil-observed, tested-operational class, survives on
    /// functional aliveness rather than formation context.
    VigilOperational,
}

/// The migration rule: a write carrying a formation token takes the Praxis
/// normative path; a legacy token-less write routes to the Vigil operational
/// surface.
pub fn route_write(has_token: bool) -> BeliefWriteRoute {
    if has_token {
        BeliefWriteRoute::PraxisNormative
    } else {
        BeliefWriteRoute::VigilOperational
    }
}

/// Whether a would-be contradiction between a newer token and an older belief
/// is *masked* — i.e. the newer belief carries a revision link that supersedes
/// exactly that older belief. When true, bookkeeping contradiction detection
/// must treat the pair as **visible history**, not a contradiction.
pub fn revision_masks_contradiction(newer: &BeliefWriteToken, older: &ContentAddressedRef) -> bool {
    matches!(&newer.revision_link, Some(link) if &link.superseded == older)
}

/// One entry in a revision chain returned by [`BeliefStore::traverse_revisions`].
#[derive(Debug, Clone)]
pub struct RevisionChainEntry {
    /// The belief record at this link.
    pub record: BeliefRecord,
    /// The acknowledgment that links this record to its predecessor, if this
    /// record supersedes an earlier one.
    pub via: Option<BeliefRevisionAcknowledgment>,
}

/// The substrate read-model behind the Nous L2 metacognitive surface —
/// *"what did I supersede recently and why"*.
#[derive(Debug, Clone)]
pub struct SupersessionView {
    /// The superseding belief's id.
    pub superseding_id: String,
    /// The superseding belief's claim.
    pub claim: BeliefClaim,
    /// The coarse scope of the supersession.
    pub scope: BeliefScope,
    /// The prior belief that was superseded.
    pub superseded: ContentAddressedRef,
    /// The structured + free-form acknowledgment of the change.
    pub acknowledgment: BeliefRevisionAcknowledgment,
    /// When the supersession was recorded.
    pub recorded_at: DateTime<Utc>,
}

/// In-memory reference store of belief records + capability grants.
///
/// Append-only: records are never mutated except to stamp `superseded_by` when
/// a later belief supersedes them, preserving full history for traversal.
#[derive(Debug, Default)]
pub struct BeliefStore {
    records: Vec<BeliefRecord>,
    by_id: HashMap<String, usize>,
    capabilities: HashMap<praxis_core::belief::CapabilityId, CapabilityGrant>,
    next_seq: u64,
}

impl BeliefStore {
    /// A fresh, empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a capability grant so writes citing it can be authorized.
    pub fn register_capability(&mut self, grant: CapabilityGrant) {
        self.capabilities.insert(grant.id.clone(), grant);
    }

    /// All records, in recorded order.
    pub fn records(&self) -> &[BeliefRecord] {
        &self.records
    }

    /// Look up a record by its store-assigned id.
    pub fn get(&self, id: &str) -> Option<&BeliefRecord> {
        self.by_id.get(id).map(|&i| &self.records[i])
    }

    /// Resolve a content-addressed reference to a stored record (matching both
    /// content hash and `valid_from`).
    pub fn resolve(&self, r: &ContentAddressedRef) -> Option<&BeliefRecord> {
        self.records.iter().find(|rec| {
            rec.content_hash == r.content_hash && rec.token.timestamp.valid_from == r.valid_from
        })
    }

    fn resolve_index(&self, r: &ContentAddressedRef) -> Option<usize> {
        self.records.iter().position(|rec| {
            rec.content_hash == r.content_hash && rec.token.timestamp.valid_from == r.valid_from
        })
    }

    /// The live (not-yet-superseded) belief for `principal` in the same coarse
    /// `scope` whose qualifier overlaps `qualifier` at or above the Jaccard
    /// threshold, plus the measured overlap. `None` when the slot is free.
    fn find_overlapping(
        &self,
        principal: &AnimaDid,
        scope: &BeliefScope,
        qualifier: &ScopeQualifier,
    ) -> Option<(&BeliefRecord, f64)> {
        self.records
            .iter()
            .filter(|rec| rec.is_live())
            .filter(|rec| &rec.token.signed_by == principal)
            .filter(|rec| &rec.token.scope == scope)
            .map(|rec| (rec, rec.token.scope_qualifier.jaccard(qualifier)))
            .filter(|(_, j)| *j >= ScopeQualifier::OVERLAP_THRESHOLD)
            // Prefer the strongest overlap when several slots are close.
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// Write a normative belief through the four-dimensional token, enforcing
    /// the six write-path checks in order. On success the record is committed
    /// and any superseded predecessor is stamped.
    ///
    /// See [`BeliefWriteError`] for the failure taxonomy.
    pub fn write_belief(
        &mut self,
        claim: BeliefClaim,
        token: BeliefWriteToken,
    ) -> Result<BeliefRecord, BeliefWriteError> {
        // Check 1 — capability presence.
        if !token.capability_id.is_present() {
            return Err(BeliefWriteError::MissingCapability);
        }
        let grant = self.capabilities.get(&token.capability_id).ok_or_else(|| {
            BeliefWriteError::UnknownCapability {
                capability: token.capability_id.to_string(),
            }
        })?;

        // Check 2 — scope match.
        if !grant.grants(&token.scope) {
            return Err(BeliefWriteError::ScopeMismatch {
                capability: token.capability_id.to_string(),
                scope: token.scope.to_string(),
            });
        }

        // Check 6 — bi-temporal stamps both set. (Checked early: a malformed
        // stamp is invalid regardless of the other dimensions.)
        if !token.timestamp.is_complete() {
            return Err(BeliefWriteError::IncompleteBiTemporalStamp);
        }

        // Check 3 — scope qualifier presence (required for normative beliefs).
        if token.scope_qualifier.is_empty() {
            return Err(BeliefWriteError::MissingScopeQualifier);
        }

        // Check 4 — evidence trace (required for normative beliefs).
        if !token.has_evidence() {
            return Err(BeliefWriteError::MissingEvidence);
        }

        // Check 5 — revision link required when an overlapping live belief
        // already exists for this principal.
        let principal = token.signed_by.clone();
        let overlap = self
            .find_overlapping(&principal, &token.scope, &token.scope_qualifier)
            .map(|(rec, j)| (rec.id.clone(), j));

        let superseded_index = match (&overlap, &token.revision_link) {
            (Some((existing_id, jaccard)), None) => {
                return Err(BeliefWriteError::MissingRevisionLink {
                    existing_id: existing_id.clone(),
                    jaccard: *jaccard,
                });
            }
            (_, Some(link)) => {
                // A revision link — dangling or not — must point at a real
                // belief, whether or not an overlap was detected.
                let idx = self.resolve_index(&link.superseded).ok_or_else(|| {
                    BeliefWriteError::RevisionTargetNotFound {
                        hash: link.superseded.content_hash.clone(),
                    }
                })?;
                Some(idx)
            }
            (None, None) => None,
        };

        // Commit.
        let hash = content_hash(&token.scope, &token.scope_qualifier, &claim);
        self.next_seq += 1;
        let id = format!("blf-{:08}", self.next_seq);
        let record = BeliefRecord {
            id: id.clone(),
            content_hash: hash,
            claim,
            token,
            class: BeliefClass::UntestedNormative,
            superseded_by: None,
        };

        if let Some(idx) = superseded_index {
            self.records[idx].superseded_by = Some(id.clone());
        }

        let index = self.records.len();
        self.records.push(record.clone());
        self.by_id.insert(id, index);
        Ok(record)
    }

    /// Record a legacy, token-less belief as a *tested-operational* belief —
    /// the Vigil-observed class. No formation dimensions are enforced; the
    /// belief survives on functional aliveness, not on formation context.
    ///
    /// This is the migration target for pre-token writes (deliverable 3).
    pub fn record_operational(
        &mut self,
        claim: BeliefClaim,
        scope: BeliefScope,
        observed_at: DateTime<Utc>,
        observer: AnimaDid,
    ) -> BeliefRecord {
        let token = BeliefWriteToken {
            capability_id: praxis_core::belief::CapabilityId::new(""),
            scope: scope.clone(),
            scope_qualifier: ScopeQualifier::empty(),
            cited_evidence: Vec::new(),
            formation_context: praxis_core::belief::SessionContext::new(
                "vigil:observed",
                observer.clone(),
            ),
            revision_link: None,
            signed_by: observer,
            timestamp: praxis_core::belief::BiTemporalStamp::at(observed_at),
        };
        let hash = content_hash(&scope, &token.scope_qualifier, &claim);
        self.next_seq += 1;
        let id = format!("blf-{:08}", self.next_seq);
        let record = BeliefRecord {
            id: id.clone(),
            content_hash: hash,
            claim,
            token,
            class: BeliefClass::TestedOperational,
            superseded_by: None,
        };
        let index = self.records.len();
        self.records.push(record.clone());
        self.by_id.insert(id, index);
        record
    }

    /// Walk the revision graph from `belief_id`, following immediate-predecessor
    /// links up to `depth` supersessions. The returned chain starts with the
    /// belief itself and proceeds to older, superseded beliefs.
    ///
    /// A `depth` of 0 returns just the starting belief. The chain is
    /// reconstructed transitively — each belief links only to its immediate
    /// predecessor (open question 3, PROVISIONAL).
    pub fn traverse_revisions(&self, belief_id: &str, depth: usize) -> Vec<RevisionChainEntry> {
        let mut chain = Vec::new();
        let mut current = self.get(belief_id).cloned();
        let mut via: Option<BeliefRevisionAcknowledgment> = None;
        let mut steps = 0usize;

        while let Some(rec) = current {
            let next_link: Option<RevisionLink> = rec.token.revision_link.clone();
            chain.push(RevisionChainEntry {
                record: rec,
                via: via.take(),
            });
            if steps >= depth {
                break;
            }
            match next_link {
                Some(link) => {
                    via = Some(link.acknowledgment.clone());
                    current = self.resolve(&link.superseded).cloned();
                    steps += 1;
                }
                None => break,
            }
        }
        chain
    }

    /// The Nous L2 metacognitive read-model: the most recent supersessions by
    /// `principal`, newest first, capped at `limit`. Each view answers *what
    /// did I supersede, and why*.
    pub fn recent_supersessions(
        &self,
        principal: &AnimaDid,
        limit: usize,
    ) -> Vec<SupersessionView> {
        let mut views: Vec<SupersessionView> = self
            .records
            .iter()
            .filter(|rec| &rec.token.signed_by == principal)
            .filter_map(|rec| {
                rec.token
                    .revision_link
                    .as_ref()
                    .map(|link| SupersessionView {
                        superseding_id: rec.id.clone(),
                        claim: rec.claim.clone(),
                        scope: rec.token.scope.clone(),
                        superseded: link.superseded.clone(),
                        acknowledgment: link.acknowledgment.clone(),
                        recorded_at: rec.token.timestamp.recorded_at,
                    })
            })
            .collect();
        // Newest first; break ties by superseding id (which is seq-ordered).
        views.sort_by(|a, b| {
            b.recorded_at
                .cmp(&a.recorded_at)
                .then_with(|| b.superseding_id.cmp(&a.superseding_id))
        });
        views.truncate(limit);
        views
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use praxis_core::belief::{
        BiTemporalStamp, CapabilityId, EvidenceRef, RevisionChange, RevisionTrigger, SessionContext,
    };

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().unwrap()
    }

    fn alice() -> AnimaDid {
        AnimaDid::new("did:key:z6MkAlice")
    }

    fn store_with_cap() -> BeliefStore {
        let mut store = BeliefStore::new();
        store.register_capability(CapabilityGrant::new(
            "cap-belief-write",
            [BeliefScope::new("market"), BeliefScope::new("self")],
        ));
        store
    }

    /// Build a normative token for `market` with the given qualifier + optional
    /// revision link.
    fn token(
        qualifier: ScopeQualifier,
        revision: Option<RevisionLink>,
        valid_from: i64,
        recorded_at: i64,
    ) -> BeliefWriteToken {
        BeliefWriteToken {
            capability_id: CapabilityId::new("cap-belief-write"),
            scope: BeliefScope::new("market"),
            scope_qualifier: qualifier,
            cited_evidence: vec![EvidenceRef::source("observation")],
            formation_context: SessionContext::new("sess-1", alice()),
            revision_link: revision,
            signed_by: alice(),
            timestamp: BiTemporalStamp::new(ts(valid_from), ts(recorded_at)),
        }
    }

    #[test]
    fn write_fails_without_capability() {
        let mut store = store_with_cap();
        let mut t = token(
            ScopeQualifier::from_pairs([("metric", "engagement")]),
            None,
            1000,
            1000,
        );
        t.capability_id = CapabilityId::new("");
        let err = store
            .write_belief(BeliefClaim::new("market", "reliable"), t)
            .unwrap_err();
        assert_eq!(err, BeliefWriteError::MissingCapability);
    }

    #[test]
    fn write_fails_with_unregistered_capability() {
        let mut store = BeliefStore::new(); // no grants registered
        let t = token(
            ScopeQualifier::from_pairs([("metric", "engagement")]),
            None,
            1000,
            1000,
        );
        let err = store
            .write_belief(BeliefClaim::new("market", "reliable"), t)
            .unwrap_err();
        assert!(matches!(err, BeliefWriteError::UnknownCapability { .. }));
    }

    #[test]
    fn write_fails_on_scope_mismatch() {
        let mut store = store_with_cap();
        let mut t = token(
            ScopeQualifier::from_pairs([("metric", "engagement")]),
            None,
            1000,
            1000,
        );
        t.scope = BeliefScope::new("weather"); // not granted
        let err = store
            .write_belief(BeliefClaim::new("weather", "sunny"), t)
            .unwrap_err();
        assert!(matches!(err, BeliefWriteError::ScopeMismatch { .. }));
    }

    #[test]
    fn write_fails_without_scope_qualifier() {
        let mut store = store_with_cap();
        let t = token(ScopeQualifier::empty(), None, 1000, 1000);
        let err = store
            .write_belief(BeliefClaim::new("market", "reliable"), t)
            .unwrap_err();
        assert_eq!(err, BeliefWriteError::MissingScopeQualifier);
    }

    #[test]
    fn write_fails_without_evidence() {
        let mut store = store_with_cap();
        let mut t = token(
            ScopeQualifier::from_pairs([("metric", "engagement")]),
            None,
            1000,
            1000,
        );
        t.cited_evidence.clear();
        let err = store
            .write_belief(BeliefClaim::new("market", "reliable"), t)
            .unwrap_err();
        assert_eq!(err, BeliefWriteError::MissingEvidence);
    }

    #[test]
    fn write_fails_on_incomplete_bitemporal() {
        let mut store = store_with_cap();
        let mut t = token(
            ScopeQualifier::from_pairs([("metric", "engagement")]),
            None,
            1000,
            1000,
        );
        t.timestamp.valid_from = DateTime::<Utc>::UNIX_EPOCH;
        let err = store
            .write_belief(BeliefClaim::new("market", "reliable"), t)
            .unwrap_err();
        assert_eq!(err, BeliefWriteError::IncompleteBiTemporalStamp);
    }

    #[test]
    fn first_write_in_a_slot_succeeds() {
        let mut store = store_with_cap();
        let t = token(
            ScopeQualifier::from_pairs([("metric", "engagement")]),
            None,
            1000,
            1000,
        );
        let rec = store
            .write_belief(BeliefClaim::new("market", "reliable"), t)
            .unwrap();
        assert_eq!(rec.class, BeliefClass::UntestedNormative);
        assert!(rec.is_live());
        assert_eq!(store.records().len(), 1);
    }

    #[test]
    fn overlapping_write_without_revision_link_is_rejected() {
        let mut store = store_with_cap();
        // First belief.
        store
            .write_belief(
                BeliefClaim::new("market", "engagement is reliable"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    None,
                    1000,
                    1000,
                ),
            )
            .unwrap();
        // Contradicting belief, same slot, no revision link → rejected.
        let err = store
            .write_belief(
                BeliefClaim::new("market", "engagement is NOT reliable"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    None,
                    2000,
                    2000,
                ),
            )
            .unwrap_err();
        match err {
            BeliefWriteError::MissingRevisionLink {
                existing_id,
                jaccard,
            } => {
                assert_eq!(existing_id, "blf-00000001");
                assert_eq!(jaccard, 1.0);
            }
            other => panic!("expected MissingRevisionLink, got {other:?}"),
        }
    }

    #[test]
    fn disjoint_scope_qualifier_allows_parallel_beliefs() {
        let mut store = store_with_cap();
        store
            .write_belief(
                BeliefClaim::new("market", "engagement is reliable"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    None,
                    1000,
                    1000,
                ),
            )
            .unwrap();
        // Different metric → disjoint qualifier → parallel, no revision needed.
        let rec = store
            .write_belief(
                BeliefClaim::new("market", "revenue is reliable"),
                token(
                    ScopeQualifier::from_pairs([("metric", "revenue")]),
                    None,
                    2000,
                    2000,
                ),
            )
            .unwrap();
        assert!(rec.is_live());
        assert_eq!(store.records().iter().filter(|r| r.is_live()).count(), 2);
    }

    #[test]
    fn revision_link_supersedes_and_marks_predecessor() {
        let mut store = store_with_cap();
        let first = store
            .write_belief(
                BeliefClaim::new("market", "engagement is reliable"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    None,
                    1000,
                    1000,
                ),
            )
            .unwrap();
        let link = RevisionLink::new(
            first.as_ref(),
            vec![EvidenceRef::source("lago:event").with_locator("evt-99")],
            BeliefRevisionAcknowledgment::new(
                RevisionTrigger::NewEvidence,
                RevisionChange::Negated,
                "engagement turned out to be gameable under adversarial load",
            ),
        );
        let second = store
            .write_belief(
                BeliefClaim::new("market", "engagement is NOT reliable"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    Some(link),
                    2000,
                    2000,
                ),
            )
            .unwrap();
        // Predecessor now superseded; successor live.
        assert_eq!(
            store.get(&first.id).unwrap().superseded_by.as_deref(),
            Some(second.id.as_str())
        );
        assert!(store.get(&second.id).unwrap().is_live());
        // The slot's only live belief is the successor.
        assert_eq!(store.records().iter().filter(|r| r.is_live()).count(), 1);
    }

    #[test]
    fn dangling_revision_link_is_rejected() {
        let mut store = store_with_cap();
        let bogus = ContentAddressedRef::new("nonexistent-hash", ts(5));
        let link = RevisionLink::new(
            bogus,
            vec![EvidenceRef::source("x")],
            BeliefRevisionAcknowledgment::new(
                RevisionTrigger::NewEvidence,
                RevisionChange::Negated,
                "…",
            ),
        );
        let err = store
            .write_belief(
                BeliefClaim::new("market", "engagement is NOT reliable"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    Some(link),
                    2000,
                    2000,
                ),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            BeliefWriteError::RevisionTargetNotFound { .. }
        ));
    }

    #[test]
    fn traverse_revisions_walks_the_chain() {
        let mut store = store_with_cap();
        let a = store
            .write_belief(
                BeliefClaim::new("market", "A"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    None,
                    1000,
                    1000,
                ),
            )
            .unwrap();
        let b = store
            .write_belief(
                BeliefClaim::new("market", "B"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    Some(RevisionLink::new(
                        a.as_ref(),
                        vec![EvidenceRef::source("e1")],
                        BeliefRevisionAcknowledgment::new(
                            RevisionTrigger::NewEvidence,
                            RevisionChange::Negated,
                            "A→B",
                        ),
                    )),
                    2000,
                    2000,
                ),
            )
            .unwrap();
        let c = store
            .write_belief(
                BeliefClaim::new("market", "C"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    Some(RevisionLink::new(
                        b.as_ref(),
                        vec![EvidenceRef::source("e2")],
                        BeliefRevisionAcknowledgment::new(
                            RevisionTrigger::ContextShift,
                            RevisionChange::Qualified,
                            "B→C",
                        ),
                    )),
                    3000,
                    3000,
                ),
            )
            .unwrap();

        // Full chain from the head.
        let chain = store.traverse_revisions(&c.id, 10);
        let ids: Vec<&str> = chain.iter().map(|e| e.record.id.as_str()).collect();
        assert_eq!(ids, vec![c.id.as_str(), b.id.as_str(), a.id.as_str()]);
        // The head has no incoming ack; each older entry carries the ack that
        // linked its successor to it.
        assert!(chain[0].via.is_none());
        assert_eq!(chain[1].via.as_ref().unwrap().rationale, "B→C");
        assert_eq!(chain[2].via.as_ref().unwrap().rationale, "A→B");

        // Depth-bounded traversal.
        let shallow = store.traverse_revisions(&c.id, 1);
        assert_eq!(shallow.len(), 2);
        assert_eq!(shallow[0].record.id, c.id);
        assert_eq!(shallow[1].record.id, b.id);
    }

    #[test]
    fn recent_supersessions_read_model() {
        let mut store = store_with_cap();
        let a = store
            .write_belief(
                BeliefClaim::new("market", "A"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    None,
                    1000,
                    1000,
                ),
            )
            .unwrap();
        let b = store
            .write_belief(
                BeliefClaim::new("market", "B"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    Some(RevisionLink::new(
                        a.as_ref(),
                        vec![EvidenceRef::source("e1")],
                        BeliefRevisionAcknowledgment::new(
                            RevisionTrigger::NewEvidence,
                            RevisionChange::Negated,
                            "A→B",
                        ),
                    )),
                    2000,
                    2000,
                ),
            )
            .unwrap();

        let views = store.recent_supersessions(&alice(), 10);
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].superseding_id, b.id);
        assert_eq!(views[0].superseded, a.as_ref());
        assert_eq!(
            views[0].acknowledgment.trigger,
            RevisionTrigger::NewEvidence
        );
    }

    #[test]
    fn revision_masks_contradiction_gate() {
        let a_ref = ContentAddressedRef::new("hash-a", ts(1000));
        // Token with a revision link superseding a_ref.
        let mut t = token(
            ScopeQualifier::from_pairs([("metric", "engagement")]),
            Some(RevisionLink::new(
                a_ref.clone(),
                vec![EvidenceRef::source("e1")],
                BeliefRevisionAcknowledgment::new(
                    RevisionTrigger::NewEvidence,
                    RevisionChange::Negated,
                    "…",
                ),
            )),
            2000,
            2000,
        );
        assert!(revision_masks_contradiction(&t, &a_ref));
        // A different older ref is NOT masked.
        let other = ContentAddressedRef::new("hash-z", ts(1));
        assert!(!revision_masks_contradiction(&t, &other));
        // No revision link → never masks.
        t.revision_link = None;
        assert!(!revision_masks_contradiction(&t, &a_ref));
    }

    #[test]
    fn migration_routing() {
        assert_eq!(route_write(true), BeliefWriteRoute::PraxisNormative);
        assert_eq!(route_write(false), BeliefWriteRoute::VigilOperational);
    }

    #[test]
    fn operational_write_bypasses_formation_checks() {
        let mut store = store_with_cap();
        let rec = store.record_operational(
            BeliefClaim::new("latency", "p99 under 200ms"),
            BeliefScope::new("self"),
            ts(1000),
            alice(),
        );
        assert_eq!(rec.class, BeliefClass::TestedOperational);
        // Operational beliefs carry no capability and no evidence, yet persist.
        assert!(!rec.token.capability_id.is_present());
        assert!(rec.token.cited_evidence.is_empty());
    }

    #[test]
    fn third_write_must_revise_current_head_not_original() {
        let mut store = store_with_cap();
        let a = store
            .write_belief(
                BeliefClaim::new("market", "A"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    None,
                    1000,
                    1000,
                ),
            )
            .unwrap();
        let _b = store
            .write_belief(
                BeliefClaim::new("market", "B"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    Some(RevisionLink::new(
                        a.as_ref(),
                        vec![EvidenceRef::source("e1")],
                        BeliefRevisionAcknowledgment::new(
                            RevisionTrigger::NewEvidence,
                            RevisionChange::Negated,
                            "A→B",
                        ),
                    )),
                    2000,
                    2000,
                ),
            )
            .unwrap();
        // A third overlapping write with NO revision link is still rejected,
        // now pointing at B (the live head), not the superseded A.
        let err = store
            .write_belief(
                BeliefClaim::new("market", "C"),
                token(
                    ScopeQualifier::from_pairs([("metric", "engagement")]),
                    None,
                    3000,
                    3000,
                ),
            )
            .unwrap_err();
        match err {
            BeliefWriteError::MissingRevisionLink { existing_id, .. } => {
                assert_eq!(existing_id, "blf-00000002"); // B, the live head
            }
            other => panic!("expected MissingRevisionLink, got {other:?}"),
        }
    }
}
