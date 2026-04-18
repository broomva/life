---
title: "Pneuma/Plexus Architecture Specification"
tags:
  - spec
  - architecture
  - pneuma
  - plexus
  - rcs
  - life-os
created: "2026-04-18"
updated: "2026-04-18"
status: draft
related:
  - "[[2026-04-18-pneuma-plexus-recursion-synthesis]]"
  - "[[p6-horizontal-composition]]"
  - "[[p7-thermodynamic-limits]]"
  - "[[LAGO_ARCHITECTURE]]"
  - "[[METALAYER]]"
---

# Pneuma/Plexus Architecture Specification

## Overview

This specification defines the **Pneuma trait family** — a substrate-invariant abstraction for inter-boundary observation and control flow in the Life Agent OS — and **`life-plexus`**, its first horizontal (inter-agent) implementation.

Pneuma formalizes what already exists: the mechanisms by which Life's existing crates carry observations up and directives down between levels of the RCS hierarchy. Plexus extends this pattern to a new boundary: between depth-0 Life instances (individual agents) and a depth-1 controller stack (collective swarm intelligence).

**Design principle:** formalize, don't rename. Existing types (`EventKind`, `GatingProfile`, `HomeostaticState`) become associated-type slots of Pneuma impls. The existing architecture is made explicit, not replaced.

## Motivation

The Life Agent OS has two orthogonal recursion axes:

- **Vertical** — L0 → L1 → L2 → L3 within one instance. Formalized in `aios-protocol::rcs::RecursiveControlledSystem<L>`. Stability budget saturated: ω = 0.006.
- **Horizontal** — depth-k → depth-(k+1) between instances. A population of depth-k Life instances becomes the L0 plant of a depth-(k+1) Life instance.

Both axes are instances of the same pattern: **aggregate observations cross a boundary upward, directives cross the same boundary downward**. The Pneuma trait family names this pattern. Specific impls handle specific boundaries.

## Terminology

- **Pneuma** (πνεῦμα, breath): the abstract inter-boundary substrate. A trait family.
- **Plexus** (Latin, braid): one specific anatomical realization of Pneuma at the depth-0→depth-1 horizontal boundary. A Rust crate (`life-plexus`).
- **Axis**: Vertical (within an instance) or Horizontal (between instances at different depths).
- **Boundary**: the specific interface (L_n → L_{n+1} or Depth_k → Depth_{k+1}).
- **Signal**: a typed, decaying observation or intent that crosses a boundary.
- **Aggregate**: the observation-side readout of a boundary (what the higher side sees).
- **Directive**: the control-side input crossing a boundary downward.

## Architecture

### 1. Core trait family (in `aios-protocol`)

```rust
// aios-protocol/src/pneuma.rs (NEW)

/// Marker for which recursion axis a Pneuma impl operates on.
pub trait Axis: Send + Sync + 'static {}

pub struct Vertical;    impl Axis for Vertical {}
pub struct Horizontal;  impl Axis for Horizontal {}

/// Marker for the specific boundary a Pneuma impl crosses.
pub trait Boundary: Send + Sync + 'static {
    fn axis_name() -> &'static str;
    fn boundary_name() -> &'static str;
}

/// Vertical boundaries within a single instance.
pub struct L0ToL1;
pub struct L1ToL2;
pub struct L2ToL3;
pub struct L3ToExternal;

impl Boundary for L0ToL1 {
    fn axis_name() -> &'static str { "vertical" }
    fn boundary_name() -> &'static str { "L0→L1" }
}
// ...same for L1ToL2, L2ToL3, L3ToExternal

/// Horizontal boundaries between instances.
pub struct D0ToD1;
pub struct D1ToD2;
pub struct D2ToD3;

impl Boundary for D0ToD1 {
    fn axis_name() -> &'static str { "horizontal" }
    fn boundary_name() -> &'static str { "depth-0 → depth-1" }
}
// ...same for D1ToD2, D2ToD3

/// The core trait: an implementation carries signals across one boundary.
pub trait Pneuma: Send + Sync {
    type B: Boundary;

    /// Typed signal crossing the boundary (payload).
    type Signal: Send + Sync + 'static;

    /// What the upward side observes (h output).
    type Aggregate: Send + Sync + 'static;

    /// What the upward side sends downward (U input).
    type Directive: Send + Sync + 'static;

    /// Emit a signal into the substrate.
    fn emit(&self, signal: Self::Signal) -> Result<(), PneumaError>;

    /// Read the current aggregate observation.
    fn aggregate(&self) -> Self::Aggregate;

    /// Check for and consume pending directive (non-blocking).
    fn receive(&self) -> Option<Self::Directive>;

    /// Substrate metadata — enables depth-(k+1) planners to reason
    /// about which resources are cheap here.
    fn substrate(&self) -> SubstrateProfile;
}

/// Substrate characteristics that affect scaling laws.
#[derive(Clone, Debug)]
pub struct SubstrateProfile {
    pub kind: SubstrateKind,
    pub warp_factors: WarpFactors,
    pub ceiling: ResourceCeiling,
}

#[derive(Clone, Debug)]
pub enum SubstrateKind {
    ClassicalSilicon,
    Neuromorphic,
    Quantum { qubits: u32, coherence_us: u32 },
    Biological,
    Hybrid(Vec<SubstrateKind>),
}

#[derive(Clone, Debug)]
pub struct WarpFactors {
    pub time: f64,          // tempo multiplier vs classical baseline
    pub energy: f64,        // energy/op multiplier
    pub coordination: CoordinationScaling,
    pub memory: f64,
    pub branching: Option<f64>,  // None for classical (impossible)
}

#[derive(Clone, Debug)]
pub enum CoordinationScaling {
    Linear,         // O(N)
    NLogN,          // O(N log N)
    Quadratic,      // O(N²)
    Entangled,      // quantum — no classical analog
}

#[derive(Clone, Debug)]
pub enum ResourceCeiling {
    Thermodynamic { max_watts: f64 },
    Coherence { max_duration_us: u32 },
    Propagation { max_radius_m: f64 },
}

#[derive(thiserror::Error, Debug)]
pub enum PneumaError {
    #[error("substrate unavailable: {0}")]
    SubstrateUnavailable(String),
    #[error("signal rejected by shield: {0}")]
    ShieldRejection(String),
    #[error("transport error: {0}")]
    Transport(String),
}
```

### 2. Vertical implementations (existing crates gain Pneuma impls)

No renames. Existing types stay. Each crate adds a Pneuma impl for its boundary:

```rust
// crates/lago/lago-journal/src/pneuma_impl.rs (NEW)
use aios_protocol::pneuma::*;

impl Pneuma for LagoJournal {
    type B = L0ToL1;
    type Signal = EventKind;           // unchanged — already the canonical L0→L1 payload
    type Aggregate = EventSlice;       // existing query type
    type Directive = ReplayRequest;    // existing replay API

    fn emit(&self, e: EventKind) -> Result<(), PneumaError> {
        self.append(e).map_err(|e| PneumaError::Transport(e.to_string()))
    }
    fn aggregate(&self) -> EventSlice { self.latest_slice() }
    fn receive(&self) -> Option<ReplayRequest> { self.pending_replays().pop() }
    fn substrate(&self) -> SubstrateProfile {
        SubstrateProfile {
            kind: SubstrateKind::ClassicalSilicon,
            warp_factors: WarpFactors::classical_baseline(),
            ceiling: ResourceCeiling::Thermodynamic { max_watts: 500.0 },
        }
    }
}
```

```rust
// crates/autonomic/autonomic-controller/src/pneuma_impl.rs (NEW)
impl Pneuma for AutonomicProjection {
    type B = L1ToL2;
    type Signal = HomeostaticDelta;    // the events fold() consumes
    type Aggregate = HomeostaticState; // unchanged — the fold output
    type Directive = GatingProfile;    // unchanged — what engine.rs emits

    fn emit(&self, d: HomeostaticDelta) -> Result<(), PneumaError> { /* ... */ }
    fn aggregate(&self) -> HomeostaticState { self.current_state() }
    fn receive(&self) -> Option<GatingProfile> { self.latest_gate() }
    fn substrate(&self) -> SubstrateProfile { /* classical baseline */ }
}
```

Same pattern for:
- `egri` (from autoany): `impl Pneuma<B = L2ToL3>`
- `bstack-policy`: `impl Pneuma<B = L3ToExternal>`

### 3. Horizontal implementation: `life-plexus` (new crate)

```rust
// crates/plexus/life-plexus/src/lib.rs (NEW)

/// Plexus signals — the typed field physics units.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PlexusSignal {
    BeliefClaim { claim: String, confidence: f32, half_life_ms: u64 },
    CapabilityOffer { capability: CapabilityId, cost: Option<Micro> },
    IntentDeclaration { goal: GoalId, urgency: f32 },
    Observation { domain: DomainId, datum: Value },
    Recruit { required: Vec<CapabilityId>, formation: FormationId },
    Pheromone { trace_type: TraceType, intensity: f32, decay_ms: u64 },
}

/// Population-level aggregate — what the depth-1 controller sees.
#[derive(Clone, Debug)]
pub struct PopulationState {
    pub members: Vec<AgentLocus>,
    pub active_formations: Vec<Formation>,
    pub gradient_fields: HashMap<CapabilityId, GradientField>,
    pub pheromone_map: HashMap<TraceType, Vec<Trace>>,
    pub quorum_readings: HashMap<CapabilityId, QuorumReading>,
}

/// Depth-1 → depth-0 directives.
#[derive(Clone, Debug)]
pub enum CollectiveDirective {
    AdjustPosture { target: AgentId, posture: Posture },
    RequestFormation { purpose: GoalId, topology: FormationTopology, members: Vec<AgentId> },
    DissolveFormation { id: FormationId },
    AllocateBudget { target: AgentId, delta: Micro },
    BroadcastNarrative { narrative: String, reach: Reach },
}

pub struct AgentFieldPlexus { /* ... */ }

impl Pneuma for AgentFieldPlexus {
    type B = D0ToD1;
    type Signal = PlexusSignal;
    type Aggregate = PopulationState;
    type Directive = CollectiveDirective;

    fn emit(&self, s: PlexusSignal) -> Result<(), PneumaError> {
        // apply decay, propagate through field, respect reach
    }
    fn aggregate(&self) -> PopulationState {
        // compute gradient fields, quorum readings, formation states
    }
    fn receive(&self) -> Option<CollectiveDirective> { /* ... */ }
    fn substrate(&self) -> SubstrateProfile { /* classical + NATS transport */ }
}
```

**Design parameters constrained by P6:**

- Signal half-lives (δ) must satisfy `δ > 1/τ_p` (decay faster than propagation)
- Directive rate bounded by `1/τ₀^(d1) ≤ 1/(κ · τ₃^(d0))`
- Population size N bounded by coupling-strength reserve
- Aggregate operator choice (mean / max / gradient) affects λ_H directly

### 4. Integration with Arcan

Arcan doesn't import pneuma directly. It receives `Arc<dyn Pneuma<B = ...>>` at construction time. Tools wrap pneuma operations:

```rust
pub fn register_pneuma_tools<P: Pneuma + 'static>(
    registry: &mut ToolRegistry,
    pneuma: Arc<P>,
) {
    // For vertical (L0→L1): tools expose emit_event, read_events
    // For horizontal (d0→d1): tools expose emit_signal, sense_field, join_formation
}
```

This mirrors the existing `SpacesPort` pattern in `arcan-spaces` — proven precedent for substrate-abstract tool interfaces.

### 5. Payload type ownership

Each controller crate owns its payload types. Payloads flow through Pneuma:

| Crate | Payload types | Pneuma role |
|---|---|---|
| lago | EventKind, EventSlice | substrate (vertical) |
| autonomic | HomeostaticDelta, GatingProfile | substrate (vertical) |
| haima | HaimaDelta, TaskBilled | controller → contributes payloads |
| anima | AnimaClaim | controller → contributes payloads |
| nous | NousScore, Evaluation | controller → contributes payloads |
| vigil | ObservationTrace | controller → contributes payloads |
| plexus | PlexusSignal, PopulationState, CollectiveDirective | substrate (horizontal) |

## Sequencing

### Phase 0: Prerequisites
1. **Write P6** (`research/rcs/papers/p6-horizontal-composition/`). Horizontal stability theorem.
2. **Write P7** (`research/rcs/papers/p7-thermodynamic-limits/`). Substrate scaling + depth-Kardashev.
3. **L3 compression project**. Nous-as-L3-evaluator inline. Prerequisite for fast horizontal recursion.

### Phase 1: Land Pneuma trait
4. Add `aios-protocol::pneuma` module with traits and markers.
5. Unit tests for `SubstrateProfile`, `WarpFactors`.

### Phase 2: Retrofit vertical impls
6. `lago-journal` implements `Pneuma<B = L0ToL1>`. Validates the trait doesn't break existing behavior.
7. `autonomic-controller` implements `Pneuma<B = L1ToL2>`.
8. `egri` implements `Pneuma<B = L2ToL3>`.
9. `bstack-policy` implements `Pneuma<B = L3ToExternal>`.

### Phase 3: Build `life-plexus`
10. New crate `crates/plexus/life-plexus/` with field physics primitives.
11. `AgentFieldPlexus` implements `Pneuma<B = D0ToD1>`.
12. NATS-based transport (or in-process broadcast for single-machine).
13. Integration with Arcan tools via `register_pneuma_tools`.
14. Horizontal-composition integration test — two depth-0 Life instances coordinating through a depth-1 plexus.

### Phase 4: Substrate-aware planning
15. `bstack plan --depth=N` CLI querying `depth-cost-scale.toml` + `substrate-warp.toml`.
16. Per-substrate Pneuma impls as they become available (neuromorphic, quantum).

## Non-goals

- **Not replacing Spaces.** Spaces stays as human-visible communication. Plexus is agent-native coordination. They coexist.
- **Not introducing depth as a type parameter on `RecursiveControlledSystem<L>`.** Depth is instantiation: `RCS<L0, State = Vec<D0Instance>>`. Trait stays clean.
- **Not renaming EventKind, GatingProfile, HomeostaticState, or any existing payload types.** Only adding Pneuma impls on top.

## Open questions

1. **Tool surface for horizontal pneuma.** What abstractions does Arcan expose? `emit_signal` is low-level; higher-level abstractions (`recruit`, `join_formation`, `leave_trace`) may be worth first-classing.
2. **Feature flags.** Should `Pneuma` be feature-gated in aios-protocol, or always present? Probably always present once stable.
3. **Serialization.** All Pneuma associated types must be `Serialize + Deserialize` for cross-instance transport. Constraint or freedom?
4. **Testing pattern.** A `MockPneuma` is obvious; what about a "physics sandbox" that simulates decay + propagation for unit tests without real transport?
5. **Failure semantics.** If `emit()` fails (substrate down, shield rejection), should the calling agent treat it as soft (log and continue) or hard (halt)? Per-boundary policy.

## Companion specifications (agent-team outputs, 2026-04-18)

This architecture overview is complemented by four deeper specs produced in parallel:

- **Trait surface (full Rust):** `core/life/docs/specs/pneuma-trait-surface.md` — 1,106-line complete `aios-protocol::pneuma` module spec, ready for direct port to Rust. Associated types over generics, zero-sized boundary markers with `Boundary::A: Axis` pinning, synchronous trait with dyn-compatibility verified, `MockPneuma` testing impl, 13 unit tests.
- **Vertical retrofits (per-crate plans):** `core/life/docs/specs/pneuma-vertical-retrofits.md` — detailed additive-only plans for four crates. **Surfaces important corrections from code audit** (see below).
- **Plexus implementation:** `core/life/docs/specs/life-plexus-implementation.md` — full crate spec with field physics primitives, three transport backends (NATS / in-process / mock), P6-compliance by construction.
- **P6 proof sketch:** `research/rcs/papers/p6-horizontal-composition/proof-sketch.md` — horizontal stability theorem with Lyapunov composition, four-step proof outline, worked numerical example showing floor inheritance.

## Corrections from retrofit audit (important)

The retrofit agent's code exploration surfaced three corrections to earlier naming assumptions in this spec:

1. **`HomeostaticDelta` does not exist as a Rust type.** The autonomic-controller has `fold()` + `evaluate()` as standalone functions operating on `HomeostaticState`. The spec's earlier sketch of `HomeostaticDelta` was aspirational naming. The retrofit plan uses `EventKind` directly as the Signal type for the L1→L2 boundary, consistent with trait-not-rename discipline.

2. **`AutonomicProjection` does not exist as a Rust type.** It was conceptual. The retrofit plan adds a new wrapper struct (`AutonomicProjection`) as an additive host for the Pneuma impl — without changing any existing function signature in autonomic-controller.

3. **`bstack-policy` does not exist as a Rust crate.** Governance currently lives as `.control/policy.yaml` + `skills/bookkeeping/scripts/bookkeeping.py`. The retrofit plan scaffolds a new Rust crate from scratch for this boundary.

4. **`autoany-core` lives in a separate workspace** (`core/autoany/`, published to crates.io). Adding an `aios-protocol` path dependency would break its publishing pipeline. **The retrofit uses an adapter crate pattern** — a new `autoany-aios-pneuma` crate inside `core/life/crates/autoany/`, mirroring the existing `autoany-aios` and `autoany-lago` adapter precedent.

These corrections don't change the architectural thesis (pneuma as substrate-invariant trait family) but they do tighten the implementation path. See `pneuma-vertical-retrofits.md` for the detailed per-crate plans.

## References

- Session synthesis: `research/notes/2026-04-18-pneuma-plexus-recursion-synthesis.md`
- Horizontal stability theorem: `research/rcs/papers/p6-horizontal-composition/README.md` + `proof-sketch.md`
- Thermodynamic limits: `research/rcs/papers/p7-thermodynamic-limits/README.md`
- Paper 5 SCOPE (dormant): `research/rcs/papers/p5-categorical-foundations/SCOPE.md`
- RCS foundations: `research/rcs/papers/p0-foundations/main.tex`
- SpacesPort precedent: `core/life/crates/arcan/arcan-spaces/src/port.rs`
- EGRI L2 implementation: `core/autoany/autoany-core/src/loop_engine.rs`
- Autonomic L1 implementation: `core/life/crates/autonomic/autonomic-controller/src/engine.rs`
