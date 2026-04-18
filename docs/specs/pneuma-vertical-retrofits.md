---
title: "Pneuma Vertical Retrofits — Per-Crate Implementation Plans"
tags:
  - spec
  - architecture
  - pneuma
  - rcs
  - retrofit
  - life-os
created: "2026-04-18"
updated: "2026-04-18"
status: draft
related:
  - "[[pneuma-plexus-architecture]]"
  - "[[pneuma]]"
  - "[[trait-not-rename]]"
  - "[[recursive-controlled-system]]"
---

# Pneuma Vertical Retrofits — Per-Crate Implementation Plans

## Overview

This document specifies **per-crate retrofit plans** for adding `Pneuma` trait implementations to the four vertical-boundary crates in the Life Agent OS monorepo. Each retrofit crosses one RCS hierarchy boundary:

| Crate | Boundary | Status |
|---|---|---|
| `lago-journal` | L0 → L1 | Exists. First retrofit (canonical validator). |
| `autonomic-controller` | L1 → L2 | Exists. Contains `fold()` reducer — the load-bearing mechanism. |
| `autoany-core` (EGRI) | L2 → L3 | Exists as standalone workspace (not in `core/life`). |
| `bstack-policy` | L3 → External | **Does not exist as a Rust crate** — currently lives as `policy.yaml` + Python in `skills/bookkeeping`. Must be scaffolded. |

**Non-goals (invariant):** no renames. `EventKind`, `GatingProfile`, `HomeostaticState`, `TrialRecord`, `Action` all keep their current names and API surface. Pneuma impls sit on top.

**Prerequisite:** the `aios-protocol::pneuma` module (trait family + boundary markers + `SubstrateProfile`) must land first. See Phase 1 in `pneuma-plexus-architecture.md`. This document assumes that landing; impls here reference types defined in that module.

---

## Crate 1 — `lago-journal` (L0 → L1 boundary)

### Current state

- **Location:** `/Users/broomva/broomva/core/life/crates/lago/lago-journal/`
- **Workspace parent:** `core/life/crates/lago/` (the Lago persistence project)
- **Public API (`src/lib.rs:1-27`):**
  - `redb_journal::RedbJournal` — primary type, implements `lago_core::Journal`
  - `redb_journal::EventNotification` — broadcast payload
  - `snapshot::{SNAPSHOT_THRESHOLD, create_snapshot, load_snapshot, should_snapshot}`
  - `stream::EventTailStream`
  - `wal::Wal`
- **Core type:** `RedbJournal` (`src/redb_journal.rs:33-36`):
  ```rust
  pub struct RedbJournal {
      db: Arc<Database>,
      notify_tx: broadcast::Sender<EventNotification>,
  }
  ```
- **What it does:** append-only event journal backed by `redb` v2. Stores `EventEnvelope` records (compound key: session+branch+seq). Exposes `Journal` trait methods (`append`, `append_batch`, `read(EventQuery)`, `get_event`, `head_seq`, `stream`, session CRUD). Publishes `EventNotification` on every append via `tokio::sync::broadcast`.
- **Boundary served:** **L0 → L1** — observations flowing up from the external plant (L0: conversation, codebase, tool execution) into the agent's internal state (L1: homeostatic regulation via Autonomic, which subscribes to the journal).
- **Payload type alignment:** `lago_core::EventEnvelope.payload: EventPayload` where `EventPayload = aios_protocol::EventKind` via type alias (`lago-core/src/event.rs:17`). So `EventKind` is **already the canonical L0→L1 Signal**. Good: the Pneuma associated type lands cleanly.

### Mapping to Pneuma

| Pneuma slot | Existing type | Location | Notes |
|---|---|---|---|
| `Signal` | `aios_protocol::EventKind` | `aios-protocol/src/event.rs:206` | Already the canonical event payload. No change. |
| `Aggregate` | `EventSlice` (NEW — thin newtype) | `lago-journal/src/pneuma_impl.rs` | Wraps `Vec<EventEnvelope>` + the query that produced it. Non-destructive — does not replace `Vec<EventEnvelope>` as `read()` return. |
| `Directive` | `ReplayRequest` (NEW) | `lago-journal/src/pneuma_impl.rs` | Typed replay intent — declarative query for re-reading events. |
| `B` (Boundary) | `aios_protocol::pneuma::L0ToL1` | `aios-protocol/src/pneuma.rs` | Zero-sized marker. |

**Design note:** `Aggregate` cannot just be `Vec<EventEnvelope>` directly because the spec mandates a readout type distinct from the raw event list (callers at L1 want a slice + provenance). A thin newtype preserves the trait shape without breaking callers of `Journal::read`.

### Concrete impl block

```rust
// core/life/crates/lago/lago-journal/src/pneuma_impl.rs (NEW)

use std::sync::Arc;
use std::sync::Mutex;

use aios_protocol::EventKind;
use aios_protocol::pneuma::{
    CoordinationScaling, L0ToL1, Pneuma, PneumaError, ResourceCeiling, SubstrateKind,
    SubstrateProfile, WarpFactors,
};
use lago_core::{BranchId, EventEnvelope, EventQuery, SessionId};

use crate::redb_journal::RedbJournal;

/// Aggregate observation at the L0→L1 boundary: the slice of events
/// most recently appended, plus the query used to obtain it.
///
/// This is a thin newtype around `Vec<EventEnvelope>` — it deliberately
/// does NOT replace the `Journal::read` return type. The existing API is
/// preserved verbatim; `EventSlice` is an L1-facing readout.
#[derive(Debug, Clone)]
pub struct EventSlice {
    pub query: EventQuery,
    pub events: Vec<EventEnvelope>,
    /// Head sequence number for (session, branch) at time of read.
    pub head_seq: u64,
}

/// Directive from L1 down to the journal — a typed replay intent.
///
/// The L1 controller (Autonomic) may request replay of a specific
/// session/branch range, e.g. to rebuild a projection after a
/// snapshot restore. This is the canonical "push to L0" directive.
#[derive(Debug, Clone)]
pub struct ReplayRequest {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub from_seq: u64,
    pub to_seq: Option<u64>,
    /// Opaque correlation for tracing.
    pub request_id: String,
}

/// Pending replay queue — bounded, FIFO. Held inline on the journal
/// so `receive()` can non-blockingly pop the next request.
#[derive(Default)]
struct ReplayQueue {
    pending: Vec<ReplayRequest>,
}

/// Extension wrapper holding Pneuma state alongside a journal.
///
/// This keeps `RedbJournal` itself unchanged (no new fields on the
/// primary type). Pneuma state is layered in a sibling struct.
#[derive(Clone)]
pub struct LagoJournalPneuma {
    journal: RedbJournal,
    replays: Arc<Mutex<ReplayQueue>>,
    /// The last slice we computed (cached for `aggregate()`).
    last_slice: Arc<Mutex<Option<EventSlice>>>,
}

impl LagoJournalPneuma {
    pub fn wrap(journal: RedbJournal) -> Self {
        Self {
            journal,
            replays: Arc::new(Mutex::new(ReplayQueue::default())),
            last_slice: Arc::new(Mutex::new(None)),
        }
    }

    pub fn inner(&self) -> &RedbJournal {
        &self.journal
    }

    /// Queue a replay request from L1. Returns Ok(()) even when queue is empty.
    pub fn request_replay(&self, req: ReplayRequest) -> Result<(), PneumaError> {
        self.replays
            .lock()
            .map_err(|e| PneumaError::Transport(format!("replay queue poisoned: {e}")))?
            .pending
            .push(req);
        Ok(())
    }

    /// Record a slice after a successful read. Allows `aggregate()` to
    /// return the most recent readout without re-querying redb.
    pub fn record_slice(&self, slice: EventSlice) {
        if let Ok(mut cell) = self.last_slice.lock() {
            *cell = Some(slice);
        }
    }
}

impl Pneuma for LagoJournalPneuma {
    type B = L0ToL1;
    type Signal = EventKind;
    type Aggregate = EventSlice;
    type Directive = ReplayRequest;

    /// Emit an event into the journal.
    ///
    /// The Pneuma API is synchronous and fire-and-forget — we use the
    /// tokio blocking bridge to materialize an `EventEnvelope` and append.
    /// Callers that need the assigned seq should keep using `Journal::append`
    /// directly; `emit` returns `Ok(())` when the append succeeds.
    fn emit(&self, signal: EventKind) -> Result<(), PneumaError> {
        // Build a minimal envelope with the canonical payload.
        // The journal assigns seq on append; caller-provided seq is ignored.
        let envelope = EventEnvelope {
            event_id: lago_core::EventId::new(),
            session_id: SessionId::from_string("pneuma-emit"),
            branch_id: BranchId::from_string("main"),
            run_id: None,
            seq: 0,
            timestamp: EventEnvelope::now_micros(),
            parent_id: None,
            payload: signal,
            metadata: std::collections::HashMap::new(),
            schema_version: 1,
        };

        // Bridge the sync Pneuma contract to the async Journal API.
        // Use a blocking current-thread runtime (bounded, no tokio context).
        let journal = self.journal.clone();
        let result = futures::executor::block_on(async move {
            use lago_core::Journal;
            journal.append(envelope).await
        });

        result
            .map(|_| ())
            .map_err(|e| PneumaError::Transport(e.to_string()))
    }

    fn aggregate(&self) -> EventSlice {
        self.last_slice
            .lock()
            .ok()
            .and_then(|c| c.clone())
            .unwrap_or_else(|| EventSlice {
                query: EventQuery::new(),
                events: Vec::new(),
                head_seq: 0,
            })
    }

    fn receive(&self) -> Option<ReplayRequest> {
        let mut q = self.replays.lock().ok()?;
        if q.pending.is_empty() {
            None
        } else {
            Some(q.pending.remove(0))
        }
    }

    fn substrate(&self) -> SubstrateProfile {
        SubstrateProfile {
            kind: SubstrateKind::ClassicalSilicon,
            warp_factors: WarpFactors {
                time: 1.0,
                energy: 1.0,
                coordination: CoordinationScaling::Linear,
                memory: 1.0,
                branching: None,
            },
            ceiling: ResourceCeiling::Thermodynamic { max_watts: 500.0 },
        }
    }
}
```

Then in `lago-journal/src/lib.rs`, add:

```rust
pub mod pneuma_impl;

pub use pneuma_impl::{EventSlice, LagoJournalPneuma, ReplayRequest};
```

### Required changes to existing code

- **`lago-journal/Cargo.toml`:** add `aios-protocol.workspace = true` dependency and `futures = { workspace = true }` (likely already present — confirmed via `redb_journal.rs:1-9` using `futures`).
- **`lago-journal/src/lib.rs`:** add `pub mod pneuma_impl;` + re-exports. Nothing else changes.
- **`RedbJournal`:** no changes. The impl sits on a wrapper (`LagoJournalPneuma`) that holds `RedbJournal` by value (`Clone`-able, since `RedbJournal` is already `Clone`).
- **No visibility changes needed.** The existing `Journal` trait keeps its entire surface; Pneuma adds an orthogonal interface.
- **No method refactors.** `append`, `read`, `get_event`, `head_seq`, `stream`, session CRUD all keep signatures.

### Risks

| Risk | Impact | Mitigation |
|---|---|---|
| `EventSlice` name collision with a future aiOS type | Medium | Namespace it under `lago_journal::pneuma_impl::EventSlice`. Keep the name local and un-exported from the crate root unless explicitly imported. |
| `futures::executor::block_on` inside `emit()` deadlocks when called from a tokio runtime | **High** | Document: Pneuma `emit` is for synchronous contexts only. Async callers should use `Journal::append` directly. Consider an `AsyncPneuma` trait extension in a later iteration (captured in open questions). |
| `EventNotification` broadcast channel pressure if replay queue grows unbounded | Low | Cap `ReplayQueue.pending` at 1024; drop oldest (log + metric) on overflow. |
| Breaking the invariant "journal assigns seq" | Low | `emit()` sets `seq: 0` and relies on `append_batch_blocking` assigning the real seq (line 127 in `redb_journal.rs`). This is already the established pattern — the existing test `append_ignores_caller_provided_seq` (line 858) proves it. |
| Regression in `redb` transaction semantics | Critical | No change to transactional code paths; Pneuma only adds a wrapper. Existing 20+ journal tests must still pass. |

### Test plan

**Unit tests** (`lago-journal/src/pneuma_impl.rs` + inline `#[cfg(test)]`):

1. `pneuma_emit_appends_event` — call `emit(EventKind::UserMessage { content: "hi" })` → inspect journal → event present.
2. `pneuma_aggregate_returns_cached_slice` — after `record_slice`, `aggregate()` returns the same slice.
3. `pneuma_aggregate_empty_default` — without prior slice, returns empty default.
4. `pneuma_receive_fifo_pop` — push 3 `ReplayRequest`s via `request_replay`, drain via `receive()` → arrives in FIFO order.
5. `pneuma_receive_none_when_empty` — initial `receive()` returns `None`.
6. `pneuma_substrate_classical_baseline` — `substrate()` returns `ClassicalSilicon` with linear coordination scaling.
7. `pneuma_emit_sync_inside_tokio_runtime_fails_gracefully` — verify documented failure mode; may be `#[ignore]` if harness incompatibility.

**Integration tests** (`lago-journal/tests/pneuma_roundtrip.rs` — NEW):

1. `pneuma_emit_then_read_via_journal` — spawn `RedbJournal` on tempdir, wrap in `LagoJournalPneuma`, `emit()` 5 events, then use `Journal::read` directly and assert 5 events present with the emitted `EventKind` values.
2. `pneuma_receive_after_request_replay` — queue a `ReplayRequest`, assert `receive()` returns it.

**Existing tests must pass unchanged.** The full `cargo test -p lago-journal` suite (currently 20+ tests) must continue passing. This is the canonical regression check for "the trait doesn't break existing behavior."

### Rollout checklist

1. [ ] Land `aios-protocol::pneuma` module first (separate PR).
2. [ ] Create `lago-journal/src/pneuma_impl.rs` with the types above.
3. [ ] Add `pub mod pneuma_impl;` + re-exports to `lago-journal/src/lib.rs`.
4. [ ] Update `lago-journal/Cargo.toml` to add `aios-protocol` dependency.
5. [ ] Run `cargo fmt && cargo clippy --workspace -- -D warnings` inside `core/life/`.
6. [ ] Run `cargo test -p lago-journal` — all existing + new tests must pass.
7. [ ] Run full Lago verify: `cd core/life && cargo test --workspace`.
8. [ ] Add integration test `lago-journal/tests/pneuma_roundtrip.rs`.
9. [ ] Update `core/life/docs/STATUS.md` with new test count for lago-journal.
10. [ ] Commit with message `feat(lago-journal): Pneuma<L0ToL1> impl` — NO rename churn.
11. [ ] Open PR; CI must pass before merging.

---

## Crate 2 — `autonomic-controller` (L1 → L2 boundary)

### Current state

- **Location:** `/Users/broomva/broomva/core/life/crates/autonomic/autonomic-controller/`
- **Public API (`src/lib.rs:1-30`):** pure rule engine + projection reducer. No I/O.
  - `projection::fold` — the canonical reducer (`HomeostaticState → EventKind → HomeostaticState`)
  - `engine::evaluate` — rules → `AutonomicGatingProfile`
  - Rule modules: `belief_rules`, `cognitive_rules`, `economic_rules`, `eval_rules`, `knowledge_rules`, `operational_rules`, `strategy_rules`
  - `trust_scoring::compute_trust_score`
- **Core function (`src/projection.rs:23-158`):**
  ```rust
  pub fn fold(
      mut state: HomeostaticState,
      kind: &EventKind,
      seq: u64,
      ts_ms: u64,
  ) -> HomeostaticState { ... }
  ```
- **Core engine (`src/engine.rs:17-29`):**
  ```rust
  pub fn evaluate(state: &HomeostaticState, rules: &RuleSet) -> AutonomicGatingProfile { ... }
  ```
- **Output types** (in `autonomic-core`, not `autonomic-controller`):
  - `HomeostaticState` (`autonomic-core/src/gating.rs:271`) — folded state, 7 pillars
  - `AutonomicGatingProfile` (`autonomic-core/src/gating.rs:46`) — contains canonical `GatingProfile` + economic gates + rationale + advisory events
- **Boundary served:** **L1 → L2** — folded homeostatic observations flowing up to the meta-controller; gating decisions flowing back down to the agent loop.

### Assumption correction

The architecture spec says the existing `AutonomicProjection` type becomes the Pneuma impl host. **That type does not exist.** `autonomic-controller` is a library of pure functions — there is no stateful struct that owns both a `HomeostaticState` and the ability to produce `AutonomicGatingProfile`.

**Proposed fix:** introduce a new **non-breaking** struct `AutonomicProjection` in `autonomic-controller/src/projection.rs` that holds `HomeostaticState` + `Arc<RuleSet>`. This struct is **additive** — existing callers (`autonomic-api::state::AppState`, `autonomic-lago::subscriber`) keep using the raw `HomeostaticState` + standalone `fold()`/`evaluate()` functions; new Pneuma users construct the struct.

Also **important:** `HomeostaticDelta` **does not exist** in the codebase. The reducer consumes `EventKind` directly. The spec's claim that "HomeostaticDelta — events that drive the fold() reducer" is aspirational naming. **Decision: use `EventKind` as the Signal type, not invent `HomeostaticDelta`.** Rationale: honoring the trait-not-rename discipline, and there is no existing domain type named `HomeostaticDelta` to preserve.

### Mapping to Pneuma

| Pneuma slot | Existing type | Location | Notes |
|---|---|---|---|
| `Signal` | `aios_protocol::EventKind` | `aios-protocol/src/event.rs:206` | The reducer's input. No new type. |
| `Aggregate` | `autonomic_core::gating::HomeostaticState` | `autonomic-core/src/gating.rs:271` | Unchanged. 7-pillar state snapshot. |
| `Directive` | `autonomic_core::gating::AutonomicGatingProfile` | `autonomic-core/src/gating.rs:46` | Unchanged. Embeds canonical `GatingProfile`. |
| `B` (Boundary) | `aios_protocol::pneuma::L1ToL2` | `aios-protocol/src/pneuma.rs` | — |

### Concrete impl block

```rust
// core/life/crates/autonomic/autonomic-controller/src/pneuma_impl.rs (NEW)

use std::sync::{Arc, Mutex};

use aios_protocol::EventKind;
use aios_protocol::pneuma::{
    CoordinationScaling, L1ToL2, Pneuma, PneumaError, ResourceCeiling, SubstrateKind,
    SubstrateProfile, WarpFactors,
};
use autonomic_core::gating::{AutonomicGatingProfile, HomeostaticState};
use autonomic_core::rules::RuleSet;

use crate::engine;
use crate::projection;

/// Stateful L1→L2 Pneuma host. Wraps a `HomeostaticState` and a `RuleSet`.
///
/// This struct is ADDITIVE — existing callers that use `fold()` + `evaluate()`
/// directly continue to work. New callers that need Pneuma semantics
/// construct this struct and interact through the `Pneuma` trait.
pub struct AutonomicProjection {
    state: Arc<Mutex<HomeostaticState>>,
    rules: Arc<RuleSet>,
    /// Sequence counter for events seen via `emit()`. Monotonic.
    seq_counter: Arc<Mutex<u64>>,
    /// Cached latest gating profile from the last `evaluate()` pass.
    cached_gate: Arc<Mutex<Option<AutonomicGatingProfile>>>,
}

impl AutonomicProjection {
    pub fn new(agent_id: impl Into<String>, rules: RuleSet) -> Self {
        Self {
            state: Arc::new(Mutex::new(HomeostaticState::for_agent(agent_id))),
            rules: Arc::new(rules),
            seq_counter: Arc::new(Mutex::new(0)),
            cached_gate: Arc::new(Mutex::new(None)),
        }
    }

    pub fn from_existing(state: HomeostaticState, rules: Arc<RuleSet>) -> Self {
        let seq = state.last_event_seq;
        Self {
            state: Arc::new(Mutex::new(state)),
            rules,
            seq_counter: Arc::new(Mutex::new(seq)),
            cached_gate: Arc::new(Mutex::new(None)),
        }
    }

    /// Apply an event manually (convenience; the `Pneuma::emit` path
    /// eventually calls this). Returns the new state.
    pub fn apply(&self, kind: &EventKind, ts_ms: u64) -> HomeostaticState {
        let mut state = self.state.lock().expect("state mutex poisoned");
        let mut seq = self.seq_counter.lock().expect("seq mutex poisoned");
        *seq = seq.saturating_add(1);
        *state = projection::fold(state.clone(), kind, *seq, ts_ms);
        state.clone()
    }

    /// Force a rules evaluation now; caches the resulting profile so
    /// `receive()` will return it on next call.
    pub fn evaluate_now(&self) -> AutonomicGatingProfile {
        let state = self.state.lock().expect("state mutex poisoned").clone();
        let profile = engine::evaluate(&state, &self.rules);
        *self.cached_gate.lock().expect("gate mutex poisoned") = Some(profile.clone());
        profile
    }
}

impl Pneuma for AutonomicProjection {
    type B = L1ToL2;
    type Signal = EventKind;
    type Aggregate = HomeostaticState;
    type Directive = AutonomicGatingProfile;

    /// Fold a signal into the projected state.
    ///
    /// This is a pure update of the Mutex-protected `HomeostaticState`.
    /// Does not evaluate rules — call `evaluate_now()` or wait for a
    /// scheduled tick to convert state into a directive.
    fn emit(&self, signal: EventKind) -> Result<(), PneumaError> {
        let ts_ms = current_ms();
        let _new_state = self.apply(&signal, ts_ms);
        Ok(())
    }

    fn aggregate(&self) -> HomeostaticState {
        self.state
            .lock()
            .expect("state mutex poisoned")
            .clone()
    }

    /// Non-blocking — returns the cached gate if one is ready.
    ///
    /// The convention is: the operator of L2 (or a scheduler) calls
    /// `evaluate_now()` periodically; consumers of the directive stream
    /// call `receive()` to pop the latest profile.
    ///
    /// Each `receive()` consumes the cached gate (take semantics).
    fn receive(&self) -> Option<AutonomicGatingProfile> {
        self.cached_gate
            .lock()
            .ok()
            .and_then(|mut g| g.take())
    }

    fn substrate(&self) -> SubstrateProfile {
        SubstrateProfile {
            kind: SubstrateKind::ClassicalSilicon,
            warp_factors: WarpFactors {
                time: 1.0,
                energy: 1.0,
                coordination: CoordinationScaling::Linear,
                memory: 1.0,
                branching: None,
            },
            // L1 is memory-cheap (in-process folds); expose a coherence-style
            // ceiling reflecting the projection cache lifetime.
            ceiling: ResourceCeiling::Thermodynamic { max_watts: 200.0 },
        }
    }
}

fn current_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
```

Then in `autonomic-controller/src/lib.rs`, add:

```rust
pub mod pneuma_impl;

pub use pneuma_impl::AutonomicProjection;
```

### Required changes to existing code

- **`autonomic-controller/Cargo.toml`:** `aios-protocol` is already a workspace dep (`Cargo.toml:13`). No change needed except to make sure `aios-protocol::pneuma` is reachable after Phase 1 lands.
- **`autonomic-controller/src/lib.rs`:** add `pub mod pneuma_impl;` + re-export of `AutonomicProjection`. Other re-exports (`fold`, `evaluate`, rules) are untouched.
- **`projection::fold`:** **no signature change, no refactor.** The impl calls it directly.
- **`engine::evaluate`:** **no signature change, no refactor.**
- **`HomeostaticState` / `AutonomicGatingProfile`:** **zero changes.**
- **`autonomic-api::AppState`:** no immediate change required. Once the integration lands, `AppState.projections` can migrate from `HashMap<String, HomeostaticState>` to `HashMap<String, Arc<AutonomicProjection>>`, but this is a **separate follow-up PR**, not part of the Pneuma retrofit.
- **Visibility:** `AutonomicProjection` is public. Its internal `seq_counter` and `cached_gate` remain private.

### Risks

| Risk | Impact | Mitigation |
|---|---|---|
| `Mutex` lock contention if many producers emit concurrently | Medium | Use `parking_lot::Mutex` (already a workspace dep in Life) OR switch to `tokio::sync::Mutex` and require async context. First pass: `std::sync::Mutex` is fine (L2 controllers emit per-tick, not per-event). |
| `receive()` "take semantics" surprises a caller expecting latest-read | Medium | Document in rustdoc: `receive()` is a drain op. If peek is needed, callers use `aggregate()` + `evaluate_now()`. |
| `emit()` swallows the seq returned by `fold()` | Low | Tolerable. The projection's own counter is monotonic; the journal seq is already preserved upstream at L0. |
| `AutonomicProjection` name collides with future type | Low | Fully qualified usage: `autonomic_controller::AutonomicProjection`. |
| `apply()` panics on poisoned mutex | Low | Use `.lock().map_err(...)` and propagate as `PneumaError::Transport`. The shown impl uses `.expect()` for brevity — production code should map errors. |
| Existing projection callers find the new struct confusing | Low | Clear rustdoc on both `fold` (pure function, reducer) and `AutonomicProjection` (stateful owner for Pneuma). Document that they coexist. |

### Test plan

**Unit tests** (`autonomic-controller/src/pneuma_impl.rs` inline `#[cfg(test)]`):

1. `projection_new_has_default_state` — `AutonomicProjection::new("a", RuleSet::new())` starts with `HomeostaticState::for_agent("a")`.
2. `emit_updates_aggregate` — `emit(RunFinished { usage: Some(...) })` → `aggregate().cognitive.total_tokens_used` non-zero.
3. `emit_is_deterministic` — emit the same `EventKind` sequence from two clones → both yield the same `HomeostaticState` (matching the `fold_sequence_produces_deterministic_state` test in `projection.rs:648`).
4. `receive_returns_none_before_evaluate` — fresh projection, `receive()` == `None`.
5. `evaluate_now_caches_profile` — after `evaluate_now()`, `receive()` returns `Some(profile)`.
6. `receive_drains_cache` — call `receive()` twice; second returns `None`.
7. `substrate_is_classical` — `substrate()` returns `SubstrateKind::ClassicalSilicon`.
8. `apply_increments_seq_counter` — apply 3 events → internal seq_counter == 3.

**Integration tests** (`autonomic-controller/tests/pneuma_integration.rs` — NEW):

1. `full_l1_to_l2_flow` — build projection with default `RuleSet`, emit `RunErrored` ×5 → error_streak rule fires → `evaluate_now()` → `receive()` returns an `AutonomicGatingProfile` with operational restrictions applied.
2. `pneuma_and_raw_fold_agree` — run the same `EventKind` sequence through `fold()` alone and through `AutonomicProjection::emit()`. Final `HomeostaticState`s must be identical (aside from the `last_event_seq` value — the projection's internal counter differs from a caller-supplied one; use `evaluate_now()` + compare structural fields only).

**Existing tests:** all 31 tests in `autonomic-controller` must continue to pass.

### Rollout checklist

1. [ ] Land lago-journal Pneuma first (landing validates the pattern).
2. [ ] Create `autonomic-controller/src/pneuma_impl.rs`.
3. [ ] Add `pub mod pneuma_impl;` + re-export to `autonomic-controller/src/lib.rs`.
4. [ ] Run `cd core/life && cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace`.
5. [ ] Add `autonomic-controller/tests/pneuma_integration.rs`.
6. [ ] Update `core/life/docs/STATUS.md` with new test count.
7. [ ] Commit: `feat(autonomic-controller): Pneuma<L1ToL2> impl via AutonomicProjection`.
8. [ ] Later follow-up (separate PR): migrate `autonomic-api::AppState` to hold `Arc<AutonomicProjection>` instead of raw `HomeostaticState` — ONLY if there is a concrete caller needing the trait.

---

## Crate 3 — `autoany_core` (L2 → L3 boundary)

### Current state

- **Location:** `/Users/broomva/broomva/core/autoany/autoany-core/`
- **Important:** this is a **separate workspace** from `core/life/`. The crate name on crates.io is `autoany_core` (underscore) per `Cargo.toml:2`. Adding `aios-protocol` as a dependency crosses a repository boundary — see Risks.
- **Core trait/engine (`src/loop_engine.rs:28-279`):**
  ```rust
  pub struct EgriLoop<A, P, X, E, S>
  where
      A: Clone,
      P: Proposer<Artifact = A>,
      X: Executor<Artifact = A>,
      E: Evaluator<Artifact = A>,
      S: Selector,
  { ... }
  ```
- **Key types (`src/types.rs`):**
  - `TrialRecord` (line 144) — full record per trial: mutation + outcome + decision + lineage
  - `Mutation` (line 80) — operator + description + diff + hypothesis
  - `Outcome` (line 103) — score + constraints_passed + violations
  - `Decision` (line 135) — action (`Promoted | Discarded | Branched | Escalated`) + reason + new state id
  - `Action` (line 115) — the decision taxonomy
- **Ledger (`src/ledger.rs`):** append-only `Vec<TrialRecord>`, optionally file-backed (JSONL).
- **Boundary served:** **L2 → L3** — trial outcomes flow up to the governance layer (which decides whether to accept new policy parameters, artifacts, or thresholds); promotion/rollback decisions flow down from governance.

### Mapping to Pneuma

| Pneuma slot | Existing type | Location | Notes |
|---|---|---|---|
| `Signal` | `TrialRecord` | `types.rs:144` | Each trial is a signal from L2 to L3: "here is evidence about a mutation." Mutation alone is not enough — governance wants outcome too. |
| `Aggregate` | `LedgerSummary` (NEW thin struct) | `pneuma_impl.rs` | Derived from `Ledger` — trial_count, baseline_score, best_score, promoted_count, recent_actions. Does not replace `LoopSummary` from `loop_engine.rs:48`, which is a per-loop reporting type. |
| `Directive` | `GovernanceDirective` (NEW) | `pneuma_impl.rs` | Wraps the enum of {Promote, Rollback, FreezeLoop, AdjustBudget, EscalateToHuman}. Governance at L3 pushes these down. |
| `B` (Boundary) | `aios_protocol::pneuma::L2ToL3` | `aios-protocol/src/pneuma.rs` | — |

**Why `TrialRecord` as Signal, not `Mutation`?** A mutation without outcome is a proposal, not an observation. The spec's "trial record or mutation proposal" option should resolve to trial record — that is what L3 actually consumes. Mutations alone are L2-internal.

### Concrete impl block

```rust
// core/autoany/autoany-core/src/pneuma_impl.rs (NEW, optional feature)

#[cfg(feature = "pneuma")]
use aios_protocol::pneuma::{
    CoordinationScaling, L2ToL3, Pneuma, PneumaError, ResourceCeiling, SubstrateKind,
    SubstrateProfile, WarpFactors,
};

use std::sync::{Arc, Mutex};

use crate::ledger::Ledger;
use crate::types::{Action, Score, TrialRecord};

/// L3-facing aggregate view of a running EGRI loop.
///
/// Derived entirely from the `Ledger`. Read-only; does not store copies
/// of trial records (they live in the ledger).
#[derive(Debug, Clone)]
pub struct LedgerSummary {
    pub trial_count: usize,
    pub promoted_count: usize,
    pub discarded_count: usize,
    pub escalated_count: usize,
    pub branched_count: usize,
    pub baseline_score: Option<Score>,
    pub best_score: Option<Score>,
    /// Last up to 8 actions, newest first.
    pub recent_actions: Vec<Action>,
    /// Consecutive non-improvements (stagnation signal).
    pub consecutive_non_improvements: usize,
}

impl LedgerSummary {
    pub fn from_ledger(ledger: &Ledger) -> Self {
        let records = ledger.records();
        let baseline_score = records.first().map(|r| r.outcome.score.clone());
        let best_score = ledger.last_promoted().map(|r| r.outcome.score.clone());
        let recent_actions: Vec<Action> = records
            .iter()
            .rev()
            .take(8)
            .map(|r| r.decision.action)
            .collect();

        Self {
            trial_count: ledger.trial_count(),
            promoted_count: ledger.by_action(Action::Promoted).len(),
            discarded_count: ledger.by_action(Action::Discarded).len(),
            escalated_count: ledger.by_action(Action::Escalated).len(),
            branched_count: ledger.by_action(Action::Branched).len(),
            baseline_score,
            best_score,
            recent_actions,
            consecutive_non_improvements: ledger.consecutive_non_improvements(),
        }
    }
}

/// Governance directive pushed from L3 down into the EGRI loop.
///
/// Not a mutation on the artifact — a control intent on the loop itself.
#[derive(Debug, Clone)]
pub enum GovernanceDirective {
    /// Accept a trial's candidate as the new promoted state.
    Promote { trial_id: String, reason: String },
    /// Roll back to the last promoted state.
    Rollback { reason: String },
    /// Freeze the loop; do not consume any more budget.
    FreezeLoop { reason: String },
    /// Adjust the remaining budget quota (signed delta).
    AdjustBudget { delta_trials: i32, reason: String },
    /// Escalate a trial decision to a human (pause until resolved).
    EscalateToHuman { trial_id: String, reason: String },
    /// Update a policy parameter (free-form key-value).
    UpdatePolicy {
        parameter: String,
        value: serde_json::Value,
        reason: String,
    },
}

/// L2→L3 Pneuma host. Wraps a `Ledger` and a directive queue.
///
/// Keeps the `Ledger` borrowable — the caller (a running `EgriLoop`)
/// retains primary ownership. `EgriLedgerPneuma` accepts signals
/// (trial records) via `emit()` and forwards to the wrapped ledger.
pub struct EgriLedgerPneuma {
    ledger: Arc<Mutex<Ledger>>,
    directives: Arc<Mutex<Vec<GovernanceDirective>>>,
}

impl EgriLedgerPneuma {
    pub fn new(ledger: Ledger) -> Self {
        Self {
            ledger: Arc::new(Mutex::new(ledger)),
            directives: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn ledger_arc(&self) -> Arc<Mutex<Ledger>> {
        Arc::clone(&self.ledger)
    }

    /// Push a governance directive from L3. Used by a governance actor.
    pub fn push_directive(&self, dir: GovernanceDirective) {
        self.directives
            .lock()
            .expect("directive mutex poisoned")
            .push(dir);
    }
}

#[cfg(feature = "pneuma")]
impl Pneuma for EgriLedgerPneuma {
    type B = L2ToL3;
    type Signal = TrialRecord;
    type Aggregate = LedgerSummary;
    type Directive = GovernanceDirective;

    fn emit(&self, record: TrialRecord) -> Result<(), PneumaError> {
        let mut ledger = self
            .ledger
            .lock()
            .map_err(|e| PneumaError::Transport(format!("ledger poisoned: {e}")))?;
        ledger
            .append(record)
            .map_err(|e| PneumaError::Transport(e.to_string()))?;
        Ok(())
    }

    fn aggregate(&self) -> LedgerSummary {
        let ledger = self.ledger.lock().expect("ledger mutex poisoned");
        LedgerSummary::from_ledger(&ledger)
    }

    fn receive(&self) -> Option<GovernanceDirective> {
        let mut q = self.directives.lock().ok()?;
        if q.is_empty() { None } else { Some(q.remove(0)) }
    }

    fn substrate(&self) -> SubstrateProfile {
        SubstrateProfile {
            kind: SubstrateKind::ClassicalSilicon,
            warp_factors: WarpFactors {
                time: 1.0,
                energy: 1.0,
                coordination: CoordinationScaling::Linear,
                memory: 1.0,
                branching: None,
            },
            // L2 thermodynamic budget is dominated by LLM inference for
            // proposer/evaluator — allow a higher watt ceiling.
            ceiling: ResourceCeiling::Thermodynamic { max_watts: 1500.0 },
        }
    }
}
```

Then in `autoany-core/src/lib.rs`, add (conditional):

```rust
#[cfg(feature = "pneuma")]
pub mod pneuma_impl;
#[cfg(feature = "pneuma")]
pub use pneuma_impl::{EgriLedgerPneuma, GovernanceDirective, LedgerSummary};
```

### Required changes to existing code

- **`autoany-core/Cargo.toml`:** add an optional feature `pneuma = ["aios-protocol"]` and a conditional dependency:
  ```toml
  [dependencies]
  # ...existing...
  aios-protocol = { version = "*", optional = true, path = "../../life/crates/aios/aios-protocol" }

  [features]
  default = []
  async = ["tokio"]
  pneuma = ["aios-protocol"]
  ```
  **Open question:** `aios-protocol` is inside `core/life/`, but `autoany_core` is published to crates.io and shouldn't carry a path dependency. Options:
    - Option A: publish `aios-protocol` to crates.io first; `autoany_core` then uses a versioned dep.
    - Option B: keep the Pneuma impl in a **separate adapter crate** (`autoany-aios-pneuma` — new, in `core/life/crates/autoany/`), mirroring how `autoany-aios` and `autoany-lago` already live outside the core workspace.
  - **Recommendation: Option B.** `autoany-core` stays free of Life workspace deps. The adapter crate `autoany-aios-pneuma` implements `Pneuma` for a wrapper type that owns a `Ledger`.

- **`autoany-core/src/lib.rs`:** no change required if we go with Option B (the Pneuma impl lives in the adapter crate).
- **`Ledger`:** no changes. Its existing methods (`append`, `records`, `by_action`, `last_promoted`, `trial_count`, `consecutive_non_improvements`) are all read-compatible with `LedgerSummary::from_ledger`.
- **`TrialRecord`, `Mutation`, `Outcome`, `Action`:** zero changes.
- **`EgriLoop`:** no changes. The impl sits alongside the loop — a governance actor consumes `aggregate()` and produces directives.

### Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Cross-workspace dependency pollution | **High** | Use Option B: adapter crate in `core/life/crates/autoany/`. `autoany-core` stays published independently. |
| `Mutex<Ledger>` locking blocks `EgriLoop::step()` | Medium | Step execution already holds the ledger exclusively during `append()` (single-threaded loop). If parallelism is introduced later, revisit. |
| `LedgerSummary::recent_actions` iteration cost if ledger grows to millions of records | Low | Bounded to 8 elements; `ledger.records().iter().rev().take(8)` is O(8). |
| `GovernanceDirective` variants drift from real governance use cases | Medium | Mark the enum `#[non_exhaustive]`. Start with the 6 variants shown; extend only when a real L3 caller demands. |
| Feature-gating adds complexity to CI | Low | Mitigated by Option B — the adapter crate builds unconditionally in Life's workspace. |

### Test plan

**Unit tests** (`autoany-aios-pneuma/src/lib.rs` — adapter crate, new):

1. `summary_empty_ledger` — empty ledger → `trial_count == 0`, `baseline_score == None`, `recent_actions.is_empty()`.
2. `summary_with_baseline` — baseline trial → `baseline_score == Some(Scalar(1.0))`.
3. `summary_counts_actions` — append mixed actions → counts correct.
4. `summary_recent_actions_reversed` — append 10 records → `recent_actions.len() == 8` and newest first.
5. `summary_consecutive_non_improvements` — tracks the same logic as `Ledger::consecutive_non_improvements`.
6. `emit_appends_to_ledger` — `emit(record)` → inner ledger contains record.
7. `push_then_receive_directive` — `push_directive(Promote{...})` → `receive()` returns `Some(Promote{...})`.
8. `receive_fifo_order` — push 3 directives → receive in order.
9. `substrate_reports_high_watts` — `substrate().ceiling` is `Thermodynamic { max_watts: 1500.0 }`.

**Integration tests** (`autoany-aios-pneuma/tests/egri_pneuma_flow.rs`):

1. `egri_loop_emits_trials_as_signals` — run a toy `EgriLoop` (using the example's `Proposer`/`Evaluator`) for 5 trials; feed each returned `TrialRecord` via `pneuma.emit`; assert `pneuma.aggregate().trial_count == 5`.
2. `governance_rollback_directive_received` — push `GovernanceDirective::Rollback`; consumer (a governance actor mock) receives it.

### Rollout checklist

1. [ ] Ensure `aios-protocol` is usable from `core/life/` workspace (already true — the L0→L1 retrofit gets there first).
2. [ ] Create `core/life/crates/autoany/autoany-aios-pneuma/` as a new crate (NOT in `core/autoany/`).
3. [ ] Add it to `core/life/Cargo.toml` workspace members if applicable.
4. [ ] Implement `EgriLedgerPneuma`, `LedgerSummary`, `GovernanceDirective` + `impl Pneuma`.
5. [ ] Add unit + integration tests.
6. [ ] Run `cd core/life && cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace`.
7. [ ] Cross-verify `cd core/autoany && make check && make test` — **autoany-core must remain untouched** (verified by running its tests with its own `Cargo.lock`).
8. [ ] Commit: `feat(autoany-aios-pneuma): Pneuma<L2ToL3> adapter crate`.

---

## Crate 4 — `bstack-policy` (L3 → External boundary)

### Current state

**`bstack-policy` does not exist as a Rust crate.** The governance layer is currently realized across:

- `.control/policy.yaml` — declarative policy (G1–G11 gates, profiles, setpoints) at `/Users/broomva/broomva/.control/policy.yaml`
- `skills/bookkeeping/scripts/bookkeeping.py` — Python pipeline that enforces knowledge-graph scoring and lint rules
- `skills/bookkeeping/SKILL.md` — authoritative spec for the 7-stage pipeline and scoring thresholds
- `skills/control-metalayer/` — metalayer skills (control-metalayer-loop, agent-consciousness, knowledge-graph-memory)
- Pre-commit hooks at `.githooks/` — gate enforcement
- `core/life/crates/lago/lago-policy/` — **different crate** — RBAC and tool-governance for the Lago runtime; **not** the L3 governance layer.

**Confirmed via search:** the string `bstack-policy` / `BstackPolicy` appears only in the Pneuma architecture spec itself (`core/life/docs/specs/pneuma-plexus-architecture.md`) and entity pages — no Rust code references it. This retrofit must **scaffold the crate first**, then add Pneuma.

### Decision: scaffold a new crate

Create `core/life/crates/bstack/bstack-policy/` (mirroring other Life crates layout). This crate will:

1. Parse `.control/policy.yaml` into typed Rust structs.
2. Expose a `PolicyState` aggregate that reflects current setpoint status, gate pass/fail counts, and profile (baseline/governed/autonomous).
3. Emit policy violation events, promotion acceptance events, audit findings.
4. Receive governance rule updates from the external human/process boundary.

### Mapping to Pneuma

| Pneuma slot | Proposed type | Location | Notes |
|---|---|---|---|
| `Signal` | `PolicyObservation` (NEW) | `bstack-policy/src/pneuma_impl.rs` | Union of {violation, audit-finding, setpoint-deviation, gate-blocked}. |
| `Aggregate` | `PolicyState` (NEW) | `bstack-policy/src/state.rs` | Current profile, setpoint values, violation counters, recent audit findings. |
| `Directive` | `GovernanceUpdate` (NEW) | `bstack-policy/src/pneuma_impl.rs` | Profile switch, setpoint adjust, gate add/remove, rule revision. |
| `B` (Boundary) | `aios_protocol::pneuma::L3ToExternal` | `aios-protocol/src/pneuma.rs` | — |

### Concrete impl block (scaffold + impl)

```rust
// core/life/crates/bstack/bstack-policy/Cargo.toml
[package]
name = "bstack-policy"
description = "L3 governance layer: policy parsing, setpoint tracking, gate enforcement"
edition.workspace = true
version.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
aios-protocol.workspace = true
serde.workspace = true
serde_yaml = "0.9"
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile = "3"
```

```rust
// core/life/crates/bstack/bstack-policy/src/lib.rs (NEW)
//! L3 governance layer — policy parsing, setpoint tracking, gate enforcement.
//!
//! This crate sits at the top of the RCS hierarchy. It is where the control
//! metalayer (CLAUDE.md + AGENTS.md + policy.yaml) crystallizes into Rust
//! runtime types. Pneuma<B = L3ToExternal> exposes it to the outside world.

pub mod config;
pub mod gates;
pub mod pneuma_impl;
pub mod setpoints;
pub mod state;

pub use config::PolicyDocument;
pub use gates::{Gate, GateEnforcement, HardGate, SoftGate};
pub use pneuma_impl::{BstackPolicy, GovernanceUpdate, PolicyObservation};
pub use setpoints::{Setpoint, SetpointSeverity};
pub use state::PolicyState;
```

```rust
// core/life/crates/bstack/bstack-policy/src/config.rs (NEW)
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level policy document — deserialized from `.control/policy.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDocument {
    pub version: String,
    pub profile: String, // baseline | governed | autonomous
    pub workspace: String,
    pub setpoints: Vec<crate::setpoints::Setpoint>,
    pub gates: GatesDoc,
    pub profiles: std::collections::HashMap<String, ProfileDoc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatesDoc {
    pub hard: Vec<crate::gates::HardGate>,
    #[serde(default)]
    pub soft: Vec<crate::gates::SoftGate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileDoc {
    pub description: String,
    #[serde(default)]
    pub gates: Vec<String>,
    #[serde(default)]
    pub egri: Option<String>,
}

impl PolicyDocument {
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }

    pub fn from_file(path: &Path) -> Result<Self, crate::pneuma_impl::BstackPolicyError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::pneuma_impl::BstackPolicyError::Io(e.to_string()))?;
        Self::from_yaml(&content)
            .map_err(|e| crate::pneuma_impl::BstackPolicyError::Parse(e.to_string()))
    }
}
```

```rust
// core/life/crates/bstack/bstack-policy/src/setpoints.rs (NEW)
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetpointSeverity {
    Blocking,
    Informational,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Setpoint {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub target: Option<serde_yaml::Value>,
    #[serde(default)]
    pub alert_below: Option<f64>,
    #[serde(default)]
    pub alert_above: Option<f64>,
    pub measurement: String,
    pub severity: SetpointSeverity,
}
```

```rust
// core/life/crates/bstack/bstack-policy/src/gates.rs (NEW)
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardGate {
    pub id: String,
    pub rule: String,
    #[serde(default)]
    pub pattern: Option<String>,
    pub measurement: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoftGate {
    pub id: String,
    pub rule: String,
    pub measurement: String,
}

#[derive(Debug, Clone)]
pub enum Gate {
    Hard(HardGate),
    Soft(SoftGate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateEnforcement {
    Passed,
    Blocked,
    Advisory,
}
```

```rust
// core/life/crates/bstack/bstack-policy/src/state.rs (NEW)
use std::collections::HashMap;

use crate::config::PolicyDocument;
use crate::pneuma_impl::PolicyObservation;

/// Compliance state — derived from the policy document + recent observations.
#[derive(Debug, Clone)]
pub struct PolicyState {
    pub profile: String,
    /// Setpoint id → latest measured value. Populated by observations.
    pub setpoint_values: HashMap<String, f64>,
    /// Gate id → trigger count (cumulative).
    pub gate_trigger_counts: HashMap<String, u64>,
    /// Recent violations (bounded, newest first).
    pub recent_violations: Vec<PolicyObservation>,
    /// Setpoint id → alert flag.
    pub setpoint_alerts: HashMap<String, bool>,
}

impl PolicyState {
    pub fn from_document(doc: &PolicyDocument) -> Self {
        Self {
            profile: doc.profile.clone(),
            setpoint_values: HashMap::new(),
            gate_trigger_counts: HashMap::new(),
            recent_violations: Vec::new(),
            setpoint_alerts: HashMap::new(),
        }
    }
}
```

```rust
// core/life/crates/bstack/bstack-policy/src/pneuma_impl.rs (NEW)

use std::sync::{Arc, Mutex};

use aios_protocol::pneuma::{
    CoordinationScaling, L3ToExternal, Pneuma, PneumaError, ResourceCeiling, SubstrateKind,
    SubstrateProfile, WarpFactors,
};
use serde::{Deserialize, Serialize};

use crate::config::PolicyDocument;
use crate::state::PolicyState;

#[derive(Debug, thiserror::Error)]
pub enum BstackPolicyError {
    #[error("io: {0}")]
    Io(String),
    #[error("parse: {0}")]
    Parse(String),
}

/// Typed L3 → External observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PolicyObservation {
    GateBlocked {
        gate_id: String,
        action_description: String,
        timestamp_ms: u64,
    },
    SetpointDeviation {
        setpoint_id: String,
        measured: f64,
        expected: Option<f64>,
        timestamp_ms: u64,
    },
    AuditFinding {
        check_id: String,
        severity: String,
        detail: String,
        timestamp_ms: u64,
    },
    PolicyViolation {
        rule: String,
        detail: String,
        timestamp_ms: u64,
    },
    PromotionAccepted {
        entity_slug: String,
        score: u32,
        timestamp_ms: u64,
    },
}

/// Typed External → L3 directive (governance update).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GovernanceUpdate {
    SwitchProfile {
        new_profile: String, // baseline | governed | autonomous
        reason: String,
    },
    AdjustSetpoint {
        setpoint_id: String,
        new_target: f64,
        reason: String,
    },
    AddHardGate {
        gate: crate::gates::HardGate,
        reason: String,
    },
    RemoveGate {
        gate_id: String,
        reason: String,
    },
    RevisePolicyDocument {
        new_document: PolicyDocument,
        reason: String,
    },
}

/// The L3 governance Pneuma host.
pub struct BstackPolicy {
    doc: Arc<Mutex<PolicyDocument>>,
    state: Arc<Mutex<PolicyState>>,
    directives: Arc<Mutex<Vec<GovernanceUpdate>>>,
    max_recent_violations: usize,
}

impl BstackPolicy {
    /// Load from a yaml file.
    pub fn from_yaml_file(path: &std::path::Path) -> Result<Self, BstackPolicyError> {
        let doc = PolicyDocument::from_file(path)?;
        let state = PolicyState::from_document(&doc);
        Ok(Self {
            doc: Arc::new(Mutex::new(doc)),
            state: Arc::new(Mutex::new(state)),
            directives: Arc::new(Mutex::new(Vec::new())),
            max_recent_violations: 64,
        })
    }

    pub fn push_directive(&self, u: GovernanceUpdate) {
        self.directives
            .lock()
            .expect("directive mutex poisoned")
            .push(u);
    }

    /// Apply a policy update — e.g. profile switch. Pure function on the doc.
    /// Returns true if the doc was modified.
    pub fn apply(&self, u: &GovernanceUpdate) -> bool {
        let mut doc = self.doc.lock().expect("doc mutex poisoned");
        let mut state = self.state.lock().expect("state mutex poisoned");
        match u {
            GovernanceUpdate::SwitchProfile { new_profile, .. } => {
                doc.profile = new_profile.clone();
                state.profile = new_profile.clone();
                true
            }
            GovernanceUpdate::AdjustSetpoint {
                setpoint_id,
                new_target,
                ..
            } => {
                if let Some(sp) = doc.setpoints.iter_mut().find(|s| &s.id == setpoint_id) {
                    sp.target = Some(serde_yaml::to_value(new_target).unwrap_or_default());
                    true
                } else {
                    false
                }
            }
            GovernanceUpdate::AddHardGate { gate, .. } => {
                doc.gates.hard.push(gate.clone());
                true
            }
            GovernanceUpdate::RemoveGate { gate_id, .. } => {
                let before = doc.gates.hard.len() + doc.gates.soft.len();
                doc.gates.hard.retain(|g| &g.id != gate_id);
                doc.gates.soft.retain(|g| &g.id != gate_id);
                let after = doc.gates.hard.len() + doc.gates.soft.len();
                before != after
            }
            GovernanceUpdate::RevisePolicyDocument { new_document, .. } => {
                *doc = new_document.clone();
                *state = PolicyState::from_document(&doc);
                true
            }
        }
    }
}

impl Pneuma for BstackPolicy {
    type B = L3ToExternal;
    type Signal = PolicyObservation;
    type Aggregate = PolicyState;
    type Directive = GovernanceUpdate;

    fn emit(&self, obs: PolicyObservation) -> Result<(), PneumaError> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| PneumaError::Transport(format!("state poisoned: {e}")))?;

        match &obs {
            PolicyObservation::GateBlocked { gate_id, .. } => {
                *state
                    .gate_trigger_counts
                    .entry(gate_id.clone())
                    .or_insert(0) += 1;
                state.recent_violations.insert(0, obs.clone());
            }
            PolicyObservation::SetpointDeviation {
                setpoint_id,
                measured,
                ..
            } => {
                state
                    .setpoint_values
                    .insert(setpoint_id.clone(), *measured);
                state.setpoint_alerts.insert(setpoint_id.clone(), true);
            }
            PolicyObservation::AuditFinding { .. }
            | PolicyObservation::PolicyViolation { .. } => {
                state.recent_violations.insert(0, obs.clone());
            }
            PolicyObservation::PromotionAccepted { .. } => {
                // informational, no state mutation
            }
        }

        // Bound recent_violations.
        if state.recent_violations.len() > self.max_recent_violations {
            state.recent_violations.truncate(self.max_recent_violations);
        }

        Ok(())
    }

    fn aggregate(&self) -> PolicyState {
        self.state.lock().expect("state mutex poisoned").clone()
    }

    fn receive(&self) -> Option<GovernanceUpdate> {
        let mut q = self.directives.lock().ok()?;
        if q.is_empty() { None } else { Some(q.remove(0)) }
    }

    fn substrate(&self) -> SubstrateProfile {
        SubstrateProfile {
            kind: SubstrateKind::ClassicalSilicon,
            warp_factors: WarpFactors {
                time: 1.0,
                energy: 0.1, // governance is cheap — few bits per decision
                coordination: CoordinationScaling::Linear,
                memory: 1.0,
                branching: None,
            },
            // L3 is rate-limited by human/process cycle time — use the
            // Propagation ceiling as a proxy for "governance radius."
            ceiling: ResourceCeiling::Propagation { max_radius_m: 1.0e7 },
        }
    }
}
```

### Required changes to existing code

- **Workspace membership:** add `core/life/crates/bstack/bstack-policy` to `core/life/Cargo.toml` `[workspace.members]`.
- **`.control/policy.yaml`:** **no changes.** The crate parses the existing YAML as-is. If fields the crate can't parse are added later, serde's `#[serde(default)]` handles missing fields.
- **`skills/bookkeeping/scripts/bookkeeping.py`:** no changes required immediately. Later, the Python pipeline may emit `PolicyObservation` events via the Rust crate — but that is a **follow-up** beyond the Pneuma retrofit.
- **`lago-policy`:** unaffected — different crate, different purpose (RBAC for tool governance).

### Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Crate doesn't exist yet — creating it is the retrofit | **High** | This is an integration-point risk. Start with a minimal skeleton; only claim Pneuma wiring works when the full test suite passes. |
| `.control/policy.yaml` schema drift over time breaking the parser | Medium | Use `#[serde(default)]` everywhere, `#[non_exhaustive]` on user-facing enums, reject-on-strict optional (`from_yaml_strict` variant). |
| Conflicting "policy" types with `lago-policy` | Low | Distinct crate names (`bstack-policy` vs `lago-policy`), distinct type names (`BstackPolicy` vs `PolicyEngine`). |
| `PolicyState.recent_violations` unbounded growth | Low | Bounded at 64 (configurable via `max_recent_violations`). |
| Governance directives crossing process boundaries with no transport | **High** | `BstackPolicy` is in-process today. External callers (CI systems, human governance actors) need an HTTP/IPC shim. Not part of this retrofit — follow-up. |
| `serde_yaml` is deprecated; yaml-rust alternatives | Medium | Use a maintained alternative if `serde_yaml` 0.9 is unmaintained at landing time. Check `cargo tree` before merging. |

### Test plan

**Unit tests** (inline `#[cfg(test)]` in each module):

1. `config::from_yaml_parses_real_policy_yaml` — parse the existing `.control/policy.yaml` verbatim; assert version/profile/setpoints loaded.
2. `state::from_document_initializes_empty_counters` — new state has no violations, no alerts.
3. `emit_gate_blocked_increments_counter` — emit 3 `GateBlocked{gate_id:"G1"}` → `gate_trigger_counts["G1"] == 3`.
4. `emit_setpoint_deviation_sets_alert` — emit → `setpoint_alerts[id] == true`.
5. `recent_violations_bounded` — emit 100 violations → `recent_violations.len() == max_recent_violations` (default 64).
6. `receive_drains_directives_fifo` — push 3 → receive in order.
7. `apply_switch_profile_changes_document` — `apply(SwitchProfile{"autonomous"})` → `doc.profile == "autonomous"`.
8. `apply_remove_gate_removes_from_doc` — add then remove by id → gate absent.
9. `substrate_reports_governance_propagation_ceiling` — ceiling is `Propagation`.

**Integration tests** (`bstack-policy/tests/policy_roundtrip.rs`):

1. `end_to_end_policy_update_flow` — load real `.control/policy.yaml`, push `SwitchProfile("autonomous")`, call `apply()`, re-read `aggregate()`, assert profile flipped.
2. `emit_observations_then_aggregate_reflects_state` — emit mixed observations, assert final `PolicyState` matches expected counts and alerts.

### Rollout checklist

1. [ ] Confirm decision to scaffold `bstack-policy` as a new Life crate (vs. keeping policy in Python only).
2. [ ] Create directory `core/life/crates/bstack/bstack-policy/` with `Cargo.toml`, `src/lib.rs`, `src/config.rs`, `src/setpoints.rs`, `src/gates.rs`, `src/state.rs`, `src/pneuma_impl.rs`.
3. [ ] Register in `core/life/Cargo.toml` workspace.
4. [ ] Copy `.control/policy.yaml` into `tests/fixtures/policy.yaml` for round-trip parse tests.
5. [ ] Implement the scaffold; run `cargo test -p bstack-policy`.
6. [ ] Implement `impl Pneuma for BstackPolicy`; add pneuma unit + integration tests.
7. [ ] Run full workspace verify: `cd core/life && cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace`.
8. [ ] Update `core/life/docs/STATUS.md` — new crate, new tests.
9. [ ] Update `core/life/CLAUDE.md` — add bstack crate family section under "Projects".
10. [ ] Commit: `feat(bstack-policy): scaffold + Pneuma<L3ToExternal> impl`.
11. [ ] Follow-up (separate PR): wire `bookkeeping.py` to emit `PolicyObservation::PromotionAccepted` over a thin Python→Rust IPC (FFI or stdout JSON).

---

## Dependency ordering

Retrofits should land in this sequence:

1. **Phase 0 prerequisite: `aios-protocol::pneuma` module.** This is the single biggest hazard — the trait family, boundary markers, and `SubstrateProfile` must exist before any impl can reference them. No retrofit is possible without this. Should be its own PR with unit tests on `SubstrateProfile`/`WarpFactors` construction.

2. **`lago-journal` (L0→L1).** Canonical validator. If the trait shape is wrong, lago-journal will reveal it first because:
   - `EventKind` is already the L0→L1 payload (lowest risk of semantic mismatch)
   - The wrapper approach (`LagoJournalPneuma`) is the pattern other crates will copy
   - 20+ existing tests provide a strong regression net
   - Failure here blocks everything else; success here proves the trait design

3. **`autonomic-controller` (L1→L2).** Builds on the validated pattern. `HomeostaticState` and `AutonomicGatingProfile` are the most mature existing types in Life, so this retrofit exercises the "preserve existing types" discipline.

4. **`autoany-aios-pneuma` (L2→L3 adapter).** Deferred because:
   - `autoany-core` lives in a separate workspace (`core/autoany/`) with its own `Cargo.lock` and publishing pipeline
   - The adapter crate approach means autoany-core itself stays unchanged
   - Allows L2 semantics to be tuned without destabilizing the published crate

5. **`bstack-policy` (L3→External).** Last because:
   - The crate doesn't exist yet — **scaffolding + retrofit in one step**
   - L3 has the narrowest stability margin (λ₃ ≈ 0.006) — must not destabilize governance cadence
   - Requires external-boundary thinking (CI, human actors) that is genuinely speculative

**Strict rule:** do NOT land any retrofit if the previous step's CI is red. Each retrofit must hold the invariant that all previous tests still pass.

---

## Migration strategy

### How to ship one at a time without breaking existing tests

The core discipline is **additive only**:

- Each retrofit adds code; no existing file's public API changes.
- New types live in NEW files (`pneuma_impl.rs`, `state.rs`, etc.), not by modifying existing files.
- `lib.rs` in each crate gets exactly one added `pub mod` declaration per retrofit.
- Zero changes to existing `struct` / `enum` definitions, including field layout, derives, and method signatures.
- New fields are never added to existing structs. If a Pneuma impl needs extra state, it lives on a wrapper struct.

### Per-retrofit commit hygiene

- One PR per crate. Each PR includes:
  - The new `pneuma_impl.rs` file
  - The `Cargo.toml` dependency addition (if any)
  - The `lib.rs` re-export
  - Unit tests inline + one integration test file
  - A STATUS.md update reflecting the new test count
- Commit message format: `feat({crate}): Pneuma<{boundary}> impl` — no other verbs.
- If any existing test fails in a retrofit PR, the retrofit is wrong. Debug, don't paper over.

### Fallback plan if a retrofit reveals a trait problem

If lago-journal's retrofit reveals that the `Pneuma` trait shape is wrong (e.g., `emit` should be async, `receive` should return `Vec<Directive>` not `Option`), the correct response is:

1. Land NO retrofits.
2. Revise `aios-protocol::pneuma` (it's a prerequisite PR, still revisable).
3. Re-attempt lago-journal.

Do not ship a broken trait and retrofit against it; that forces churn later across all four crates.

---

## CI verification

At each retrofit landing, the following must be green:

### For lago-journal (retrofit 1)
- `cd core/life && cargo fmt -- --check`
- `cd core/life && cargo clippy --workspace -- -D warnings`
- `cd core/life && cargo test -p lago-journal`
- `cd core/life && cargo test -p lago-api -p lago-cli -p lagod` (downstream consumers)
- `cd core/life && cargo build --workspace`

### For autonomic-controller (retrofit 2)
- All of the above, plus:
- `cd core/life && cargo test -p autonomic-controller`
- `cd core/life && cargo test -p autonomic-api -p autonomic-lago -p autonomicd`
- `cd core/life && cargo test --workspace` (full regression)

### For autoany-aios-pneuma (retrofit 3)
- `cd core/life && cargo test -p autoany-aios-pneuma` (new crate)
- `cd core/life && cargo test --workspace`
- Cross-check `cd core/autoany && make check && make test` — **autoany-core tests must be unchanged** (prove the adapter approach didn't leak into autoany-core)

### For bstack-policy (retrofit 4)
- `cd core/life && cargo test -p bstack-policy` (new crate)
- `cd core/life && cargo test --workspace`
- Ensure `.control/policy.yaml` parses via a test fixture
- `make control-audit` still green at the workspace root

### Cross-cutting
- Pre-commit hooks (smoke gate) pass
- `make bstack-check` (27-skill validation) still passes
- `python3 skills/bookkeeping/scripts/bookkeeping.py lint --all` passes

---

## Cross-cutting concerns

### Serialization

The architecture spec flags this as open question #3. Concrete answer for the four retrofits:

- **`EventKind`** is already `Serialize + Deserialize` (with a forward-compatible deserializer). No change.
- **`HomeostaticState`** is already `Serialize + Deserialize` (derived on `autonomic-core/src/gating.rs:271`).
- **`AutonomicGatingProfile`** is already `Serialize + Deserialize`.
- **`TrialRecord`** is already `Serialize + Deserialize` (derived on `autoany-core/src/types.rs:144`).
- **`EventSlice`, `ReplayRequest`**: derive `Serialize + Deserialize` in the new types to preserve horizontal-transport compatibility.
- **`LedgerSummary`, `GovernanceDirective`**: derive `Serialize + Deserialize`.
- **`PolicyObservation`, `GovernanceUpdate`, `PolicyState`**: derive `Serialize + Deserialize`.

Policy: **all Pneuma associated types must be `Serialize + Deserialize`** to prepare for the horizontal plexus layer. This is a hard constraint on new types; existing types that already satisfy it are unchanged.

### Feature flags

- `aios-protocol::pneuma` should be **always present** once stable. Feature-gating the trait family would create incompatible crate combinations — one consumer with `pneuma` enabled, another without, leads to subtle breakage.
- The **adapter crate approach** for autoany-core (retrofit 3) is the only place where feature gating is considered, and even there, Option B (separate adapter crate) removes the need for a feature flag on `autoany-core`.
- `bstack-policy` is unconditional inside `core/life/` — it's a Life workspace member.

### Backwards compatibility

**Invariants (all four retrofits must preserve):**

1. **No existing type is renamed.**
2. **No existing method signature changes.**
3. **No existing trait bound changes.**
4. **No existing public field visibility changes.**
5. **`cargo test --workspace` at `core/life/` must pass before and after each retrofit.**
6. **`cargo test --workspace` at `core/autoany/` must pass before and after retrofit 3** (the adapter crate is the mechanism that makes this true).
7. **Schema versions on `EventEnvelope` are not incremented** — no on-disk format change.

**Forward-compatibility:**

- `EventKind::Custom` remains the fallback for unknown payload types. Pneuma doesn't change this.
- New `PolicyObservation` variants should be added via `#[non_exhaustive]` to allow variant addition without breaking consumers.

### Observability

Existing tracing instrumentation (in `fold`, `RedbJournal::append`, etc.) is **not modified**. Pneuma methods should add their own spans:

```rust
#[instrument(skip(self), fields(pneuma.boundary = "L0->L1"))]
fn emit(&self, signal: EventKind) -> Result<(), PneumaError> { ... }
```

Spans attribute names: `pneuma.boundary`, `pneuma.op` (`emit | aggregate | receive`), `pneuma.substrate_kind`.

### Interaction with existing RCS types

The retrofits do NOT implement `RecursiveControlledSystem<L>` on the new Pneuma hosts. The RCS trait is about state/observation/control dynamics (`f`, `h`); the Pneuma trait is about substrate transport (`emit`, `aggregate`, `receive`). These are orthogonal:

- A Pneuma impl can optionally ALSO be an RCS impl (future — not required).
- A crate that implements RCS (like `arcand/src/rcs_observer.rs`) can consume a Pneuma impl for its transport needs (future).

Keep the two trait families loosely coupled; do not prematurely couple them in the first retrofits.

---

## Open Questions Specific to Retrofits

1. **Sync vs. async `emit`?** The Pneuma trait as specified in `pneuma-plexus-architecture.md` is synchronous. lago-journal is naturally async (redb + tokio). The lago retrofit uses `futures::executor::block_on`, which is brittle inside tokio. **Proposal:** add an `AsyncPneuma` trait extension later; keep sync for now to match the spec.

2. **Should `AutonomicProjection` own its `RuleSet`, or borrow it?** Chosen: owns `Arc<RuleSet>`. Rationale: `RuleSet` contains boxed trait objects (`Box<dyn HomeostaticRule>`), cloning is nontrivial; `Arc` sharing is the idiomatic path.

3. **Directive buffering depth?** lago-journal replays, autoany directives, bstack-policy updates all use unbounded `Vec`. **Proposal:** cap each at 1024 with "drop oldest + log" on overflow. Out of scope for the initial retrofit; add in a follow-up.

4. **Does `bstack-policy` need to be a Rust crate at all?** The current governance realization is 90% YAML + Python. Making it Rust gains: typed API, in-process callers (Arcan, Autonomic) can read policy without subprocess cost. Costs: duplicates Python logic; maintenance burden. **Proposal:** land a minimal Rust crate (parse-only + Pneuma) to establish the interface; keep Python enforcement until a Rust caller exists.

5. **Should retrofits emit `EventKind` variants for their own activity?** e.g., `AutonomicProjection` could emit `EventKind::Custom{event_type: "pneuma.l1_emit", ...}` on every `emit()`. **Proposal: No, not in the retrofits.** Observability lives in tracing spans (cheaper, structured). EventKind pollution is a separate discussion.

---

## References

- `/Users/broomva/broomva/core/life/docs/specs/pneuma-plexus-architecture.md` — parent architecture
- `/Users/broomva/broomva/research/entities/concept/pneuma.md` — conceptual entity
- `/Users/broomva/broomva/research/entities/pattern/trait-not-rename.md` — design discipline
- `/Users/broomva/broomva/research/entities/concept/recursive-controlled-system.md` — RCS framework
- `/Users/broomva/broomva/core/life/crates/aios/aios-protocol/src/rcs.rs` — existing RCS traits
- `/Users/broomva/broomva/core/life/crates/aios/aios-protocol/src/event.rs` — canonical `EventKind`
- `/Users/broomva/broomva/core/life/crates/lago/lago-journal/src/redb_journal.rs:33-36` — `RedbJournal` struct
- `/Users/broomva/broomva/core/life/crates/autonomic/autonomic-controller/src/projection.rs:23-158` — `fold()` reducer
- `/Users/broomva/broomva/core/life/crates/autonomic/autonomic-core/src/gating.rs:271-300` — `HomeostaticState`
- `/Users/broomva/broomva/core/autoany/autoany-core/src/loop_engine.rs:28-279` — `EgriLoop`
- `/Users/broomva/broomva/core/autoany/autoany-core/src/types.rs:144-156` — `TrialRecord`
- `/Users/broomva/broomva/.control/policy.yaml` — current L3 governance document
- `/Users/broomva/broomva/skills/bookkeeping/SKILL.md` — Python governance pipeline
