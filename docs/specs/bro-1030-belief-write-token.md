# BRO-1030 — `BeliefWriteToken`: four-dimensional belief formation context

**Status:** implemented (praxis-core + praxis-tools) · **Crates:** `praxis-core::belief`, `praxis-tools::belief`

A belief is not a bare proposition. The belief-contradiction problem — *silent
accumulation looking identical to deliberate updating* — is unsolvable without
observing **how a belief was formed**. This spec makes formation context a
first-class, typed write token with four dimensions.

## The four dimensions

| Dimension | Question answered | Field | Sourced from |
| -- | -- | -- | -- |
| Capability | *Who authorized this belief?* | `capability_id` | khlo — formation-context-as-write-time |
| Bi-temporal | *When in the world / when in the system?* | `timestamp: BiTemporalStamp` | bi-temporal stamps |
| Scope | *About what, precisely?* | `scope` + `scope_qualifier` | Cornelius-Trinity — beliefs carry their own scope conditions |
| **Revision** | ***What was superseded?*** | `revision_link` | **vina — the past self is a series of discarded drafts** |

Without the fourth dimension, even bi-temporal stamps reduce to a *playback
device*: two snapshots with timestamps but no causal connection. A
contradiction **with** a revision link is *visible history* (a deliberate
update); a contradiction **without** one is a genuine failure of versioning.

## Schema (`praxis-core::belief`)

```rust
struct BeliefWriteToken {
    capability_id: CapabilityId,
    scope: BeliefScope,
    scope_qualifier: ScopeQualifier,          // BTreeMap<QualifierKey, QualifierValue>
    cited_evidence: Vec<EvidenceRef>,
    formation_context: SessionContext,        // session + run + principal
    revision_link: Option<RevisionLink>,      // the fourth dimension
    signed_by: AnimaDid,
    timestamp: BiTemporalStamp,               // { valid_from, recorded_at }
}

struct RevisionLink {
    superseded: ContentAddressedRef,          // blake3 content hash + valid_from
    triggered_by: Vec<EvidenceRef>,
    acknowledgment: BeliefRevisionAcknowledgment,
}

struct BeliefRevisionAcknowledgment {
    trigger: RevisionTrigger,   // structured: NewEvidence | ScopeRefinement | ContextShift
    change: RevisionChange,     // structured: Negated | Qualified | Reweighted
    rationale: String,          // free-form addendum
}
```

The claim itself (`BeliefClaim { subject, proposition }`) is passed alongside
the token to `write_belief` — the token carries *authorization and provenance*,
the claim carries *content*. Content addressing hashes
`(scope, canonicalised scope_qualifier, subject, proposition)` with blake3.

## Write-path checks (`praxis-tools::belief::BeliefStore::write_belief`)

Enforced in this order; each maps to a `BeliefWriteError` variant:

1. **Capability presence** — empty `capability_id` ⇒ `MissingCapability`; an
   unregistered id ⇒ `UnknownCapability`.
2. **Scope match** — the capability grant must authorize `token.scope` ⇒ else
   `ScopeMismatch`.
3. **Bi-temporal completeness** — both `valid_from` and `recorded_at` set (not
   the epoch sentinel) ⇒ else `IncompleteBiTemporalStamp`.
4. **Scope qualifier presence** — required for normative beliefs ⇒ else
   `MissingScopeQualifier`.
5. **Evidence trace** — ≥ 1 cited evidence for normative beliefs ⇒ else
   `MissingEvidence`.
6. **Revision link required when applicable** — if a **live** belief for the
   same principal + coarse scope has a scope-qualifier Jaccard overlap ≥ 0.5,
   the write **must** carry a `revision_link` ⇒ else
   `MissingRevisionLink { existing_id, jaccard }`. A revision link (dangling or
   not) must resolve to a real stored belief ⇒ else `RevisionTargetNotFound`.

On success the predecessor named by the revision link is stamped
`superseded_by`, so the slot has exactly one **live** head.

## Two belief classes

| Class | Surface | All 4 dimensions required? | Survives on |
| -- | -- | -- | -- |
| Untested-normative | Praxis principal (`write_belief`) | **yes** | capability + scope + revision-link consistency |
| Tested-operational | Vigil-observed (`record_operational`) | no | functional aliveness |

**Migration** (`route_write`): a write carrying a formation token → Praxis
normative path; a legacy token-less write → Vigil operational surface
(tested-operational, no formation checks). New writes without a `revision_link`
on an overlapping scope are **rejected**, not silently accepted.

## Revision-graph traversal

`BeliefStore::traverse_revisions(belief_id, depth)` walks immediate-predecessor
links up to `depth` supersessions, returning `Vec<RevisionChainEntry>` from the
live head back to older discarded drafts. Each entry carries the acknowledgment
that linked its successor to it. Multi-step chains are reconstructed by walking
(open question 3: each belief links only to its **immediate** predecessor).

## Bookkeeping integration

`revision_masks_contradiction(newer_token, older_ref)` is the gate: contradiction
detection fires **only** on Praxis writes *without* a revision link covering the
conflict. With a revision link pointing at the conflicting belief, the pair is
**visible history**, not a contradiction — bookkeeping treats it as a lineage
edge, not an alarm.

## L2 metacognitive surface (Nous)

`BeliefStore::recent_supersessions(principal, limit)` is the substrate
read-model behind the Nous L2 view *"what did I supersede recently, and why"*.
Each `SupersessionView` carries the superseding claim, the superseded reference,
the structured + free-form acknowledgment, and `recorded_at`. Praxis owns the
queryable substrate; the Nous daemon **projects** it as a metacognitive surface
(no praxis→nous dependency — Nous reads the read-model, consistent with how
autonomic/nous consult substrate through ports).

## Resolved open questions

1. **Acknowledgment granularity** — structured (`RevisionTrigger` +
   `RevisionChange` enums) for queryability + a free-form `rationale` field.
2. **Revision vs `scope_qualifier`** — revision required only when qualifier
   Jaccard overlap ≥ 0.5; disjoint qualifiers are *parallel* beliefs.
3. **Multi-step chains** — immediate predecessor only; chain reconstructed by
   `traverse_revisions`.
4. **Cross-agent revision graphs** — composes with BRO-1029 (out of scope here).
5. **How L2 EGRI / Nous reads the graph** — via `recent_supersessions` /
   `traverse_revisions` read-models (above).

See `docs/specs/bro-1030-belief-write-token.html` for the visual decision matrix
and worked examples.
