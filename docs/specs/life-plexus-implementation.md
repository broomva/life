---
title: "life-plexus Implementation Specification"
tags:
  - spec
  - implementation
  - rust
  - pneuma
  - plexus
  - life-os
  - rcs
created: "2026-04-18"
updated: "2026-04-18"
status: draft
axis: horizontal
boundary: D0→D1
related:
  - "[[pneuma-plexus-architecture]]"
  - "[[p6-horizontal-composition]]"
  - "[[p7-thermodynamic-limits]]"
  - "[[plexus]]"
  - "[[pneuma]]"
---

# life-plexus Implementation Specification

> **Scope.** This spec tells a contributor exactly how to land the `life-plexus` crate
> in the Life Agent OS monorepo. It is the implementation layer under the architecture
> spec at `core/life/docs/specs/pneuma-plexus-architecture.md` and is gated on
> Paper 6 (`research/rcs/papers/p6-horizontal-composition/`), whose four stability
> conditions C1–C4 this spec must satisfy.
>
> `life-plexus` is the first **horizontal** Pneuma impl: the d0→d1 boundary between
> individual Life agent instances and a depth-1 collective controller.
> The vertical impls (`Pneuma<L0→L1>`, `Pneuma<L1→L2>`, …) are already planned as
> retrofits of existing crates; this crate is net-new.

## 0. Prerequisites

This spec assumes the following have landed **before** `life-plexus` starts:

1. `aios-protocol::pneuma` module with `Axis`, `Boundary`, `Pneuma`, `SubstrateProfile`,
   `PneumaError` (Phase 1 of the architecture spec).
2. Nous-as-inline-L3 (L3 compression). Without it, the depth-0 L3 cadence is too slow
   for any useful d1 control cadence (C1 fails trivially).
3. Canonical parameters (`data/parameters.toml`) extended with a `[horizontal]`
   section carrying default δ, τ_p, α, N caps, σ that satisfy C1–C4 for a reference
   10-agent configuration.

If any of the above is missing, land it first; do **not** stub.

## 1. Crate layout

### 1.1 Directory

```
core/life/crates/plexus/
└── life-plexus/
    ├── Cargo.toml
    ├── CHANGELOG.md
    ├── README.md
    └── src/
        ├── lib.rs               # public API + `register_plexus_tools`
        ├── error.rs             # PlexusError (wraps PneumaError)
        ├── signal.rs            # PlexusSignal enum + serialization
        ├── locus.rs             # AgentLocus, Posture, FieldCoord, CapabilityId
        ├── formation.rs         # Formation, FormationTopology, FormationLifecycle, FormationRole
        ├── field.rs             # PopulationState, GradientField, QuorumReading, Trace, Pheromone
        ├── directive.rs         # CollectiveDirective
        ├── physics/
        │   ├── mod.rs           # module root
        │   ├── decay.rs         # exponential decay, half-life bookkeeping
        │   ├── propagation.rs   # k-NN via rstar R*-tree over FieldCoord
        │   ├── gradient.rs      # local gradient computation per agent
        │   ├── stigmergy.rs     # append-only trace store + TTL cleanup
        │   └── quorum.rs        # quorum detection within sense radius
        ├── transport/
        │   ├── mod.rs           # PlexusTransport trait + selector
        │   ├── in_process.rs    # tokio::broadcast + Arc<RwLock<_>> (default feature)
        │   ├── nats.rs          # async-nats JetStream backend (feature = "nats")
        │   └── mock.rs          # deterministic, configurable (feature = "mock")
        ├── plexus.rs            # AgentFieldPlexus struct + Pneuma impl
        ├── tools.rs             # arcan Tool wrappers (sense/emit/join/leave/mark/quorum)
        ├── trace_store.rs       # redb-backed pheromone persistence (feature = "persist")
        ├── config.rs            # PlexusConfig (population cap, decay defaults, TTLs)
        └── budget.rs            # horizontal stability budget helpers (λ_H)
```

File-based modules (`name.rs`) per `.claude/rules/code-style.md`, no `mod.rs` outside
the `physics/` and `transport/` subdirectories (where a `mod.rs` root is the
cleanest Rust-2024 approach for submodules).

### 1.2 Cargo.toml

```toml
[package]
name = "life-plexus"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
homepage.workspace = true
keywords = ["agent", "swarm", "plexus", "pneuma", "rcs"]
categories = ["asynchronous", "network-programming"]
description = "Plexus — horizontal d0→d1 Pneuma impl: field physics for inter-agent coordination"

[lints]
workspace = true

[features]
default = ["in-process"]
in-process = []               # tokio::broadcast + in-memory state
nats = ["dep:async-nats", "dep:futures-util"]
mock = []                     # deterministic test backend
persist = ["dep:redb"]        # redb-backed pheromone trace store (optional)

[dependencies]
# Internal
aios-protocol = { workspace = true }
arcan-core    = { workspace = true }          # for Tool/ToolRegistry surface (optional gate?)

# Core
serde         = { workspace = true, features = ["derive"] }
serde_json    = { workspace = true }
thiserror     = { workspace = true }
tracing       = { workspace = true }
tokio         = { workspace = true, features = ["sync", "time", "rt"] }
parking_lot   = { workspace = true }
chrono        = { workspace = true, features = ["serde"] }
ulid          = { workspace = true, features = ["serde"] }

# Spatial index for capability-space k-NN (already in workspace for Opsis)
rstar         = { workspace = true }

# Optional feature deps
async-nats    = { version = "0.39", optional = true }
futures-util  = { workspace = true, optional = true }
redb          = { workspace = true, optional = true }

[dev-dependencies]
tokio         = { workspace = true, features = ["macros", "rt-multi-thread", "test-util"] }
proptest      = "1"
tempfile      = { workspace = true }
serde_json    = { workspace = true }
```

**Notes on deps:**
- `async-nats` is new to the workspace; add it to `[workspace.dependencies]` in the
  same PR that lands `life-plexus`.
- `rstar` is already a workspace dep (used by Opsis).
- `arcan-core` is imported only so `register_plexus_tools` can implement the `Tool`
  trait; it is **not** a forward dep of Arcan — Arcan depends on `life-plexus`, not
  the other way around. We import `arcan-core` to preserve the `arcan-spaces`
  precedent. If this creates a dep-cycle concern, move the tool surface into a
  separate `arcan-plexus` bridge crate (symmetric to `arcan-spaces`).

### 1.3 Workspace wiring

Add to `core/life/Cargo.toml`:

```toml
# in [workspace.members] — after the "# Spaces A2A" block
"crates/plexus/life-plexus",

# in [workspace.dependencies]
life-plexus = { path = "crates/plexus/life-plexus", version = "0.3.0" }
async-nats  = "0.39"     # new
```

No facade crate yet (parity with `arcan-spaces` — promote to a facade only once
a second horizontal Pneuma impl exists).

## 2. Core types

All types below live in the modules indicated in §1.1. They all derive
`Clone + Debug + Serialize + Deserialize` unless noted.

### 2.1 Signals — `src/signal.rs`

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::locus::{CapabilityId, DomainId, GoalId};
use crate::formation::FormationId;

/// Micro-credit unit (1 credit = 1e-6 USDC). Re-exported from haima-core in prod;
/// declared here as a newtype so life-plexus does not depend on haima.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Micro(pub i64);

/// Stigmergic trace discriminator. Separates pheromone lanes so that unrelated
/// agent activities (e.g. "I explored this capability" vs "I failed this domain")
/// do not share the same gradient field.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceType {
    Explored,
    Succeeded,
    Failed,
    Recruited,
    Abandoned,
    Custom(u16),     // reserved for downstream projects
}

/// How far a signal is allowed to propagate through capability-space.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    Local,       // k nearest in capability-space (default: 8)
    Regional,    // cluster that includes this emitter
    Global,      // the entire population (rate-limited, expensive)
}

/// The field-physics primitives that cross the d0→d1 boundary.
///
/// Each variant carries its own decay parameters — the plexus does **not**
/// enforce a default half-life; defaults live in `PlexusConfig`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlexusSignal {
    /// A belief assertion, decays toward uncertainty.
    /// Typical emitter: an L1-healthy agent reporting a Nous-evaluated belief.
    /// Typical sensor : any agent whose goal overlaps the belief domain.
    BeliefClaim {
        claim: String,
        confidence: f32,         // [0, 1]
        half_life_ms: u64,
    },

    /// Offer of a capability for recruitment.
    /// Emitter: a lightly-loaded agent that owns `capability`.
    /// Sensor : any agent in the Recruiting phase of a formation.
    CapabilityOffer {
        capability: CapabilityId,
        cost: Option<Micro>,
        half_life_ms: u64,
    },

    /// Intent to pursue `goal` with `urgency`.
    /// Emitter: any agent that has just planned a goal.
    /// Sensor : nearby agents checking for coordination opportunities.
    IntentDeclaration {
        goal: GoalId,
        urgency: f32,            // [0, 1]; amplifies gradient
        half_life_ms: u64,
    },

    /// Raw observation injected into the shared field.
    /// Emitter: an agent that has just produced a high-signal output.
    /// Sensor : aggregators and the d1 controller's `aggregate()`.
    Observation {
        domain: DomainId,
        datum: Value,
        half_life_ms: u64,
    },

    /// Active recruitment request for members of a formation.
    /// Emitter: the would-be hub (for star/hierarchy) or any member (for mesh).
    /// Sensor : any agent whose capability set intersects `required`.
    Recruit {
        required: Vec<CapabilityId>,
        formation: FormationId,
        half_life_ms: u64,
    },

    /// Stigmergic marking — persists past the emitter's lifetime.
    /// Emitter: any agent completing a notable action (exploration, failure).
    /// Sensor : future agents navigating the same capability-space neighborhood.
    Pheromone {
        trace_type: TraceType,
        intensity: f32,          // initial strength; decays exponentially
        decay_ms: u64,           // different from other half_life_ms: pheromones
                                 //   persist long after the emitter drops
    },
}

impl PlexusSignal {
    /// The decay half-life (ms) of this signal, regardless of variant.
    pub fn half_life_ms(&self) -> u64 {
        match self {
            Self::BeliefClaim       { half_life_ms, .. } => *half_life_ms,
            Self::CapabilityOffer   { half_life_ms, .. } => *half_life_ms,
            Self::IntentDeclaration { half_life_ms, .. } => *half_life_ms,
            Self::Observation       { half_life_ms, .. } => *half_life_ms,
            Self::Recruit           { half_life_ms, .. } => *half_life_ms,
            Self::Pheromone         { decay_ms,     .. } => *decay_ms,
        }
    }

    pub fn default_reach(&self) -> Reach {
        match self {
            Self::BeliefClaim       { .. } => Reach::Regional,
            Self::CapabilityOffer   { .. } => Reach::Local,
            Self::IntentDeclaration { .. } => Reach::Local,
            Self::Observation       { .. } => Reach::Regional,
            Self::Recruit           { .. } => Reach::Regional,
            Self::Pheromone         { .. } => Reach::Local,
        }
    }
}
```

**Serialization format.** All signals serialize to JSON via serde; NATS payloads use
JSON by default (MessagePack optional behind `feature = "msgpack"` later). Every
signal is wrapped in an `EmittedSignal` envelope when it enters the field:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmittedSignal {
    pub id: ulid::Ulid,          // content-addressed would be an anti-pattern here
                                 //   because two identical signals from different
                                 //   agents are meaningfully distinct events
    pub emitted_by: AgentId,
    pub emitted_at_ms: i64,      // unix millis
    pub emitter_pos: FieldCoord, // snapshot at emit time
    pub reach: Reach,
    pub payload: PlexusSignal,
}
```

**Propagation rule per variant:**

| Variant             | Default Reach | Who emits                            | Who senses                                |
|---------------------|---------------|--------------------------------------|-------------------------------------------|
| BeliefClaim         | Regional      | Nous-validated agents                | Agents with goal overlap                  |
| CapabilityOffer     | Local (k=8)   | Lightly loaded agents                | Recruiting formations                     |
| IntentDeclaration   | Local (k=8)   | Any agent post-plan                  | Nearby agents (coordination)              |
| Observation         | Regional      | High-signal producers                | Aggregators, d1 controller                |
| Recruit             | Regional      | Formation hubs / mesh members        | Agents with matching capability           |
| Pheromone           | Local (k=16)  | Any agent on action boundary         | Future navigators of same neighborhood    |

**Decay function** (shared by all variants, applied at read-time in §3.1):
`strength(t) = initial · 0.5^((t − emit_t) / half_life_ms)` with `initial = 1.0`
for signals and `initial = pheromone.intensity` for pheromones.

### 2.2 Agent identity & position — `src/locus.rs`

```rust
use serde::{Deserialize, Serialize};
use aios_protocol::ids::AgentId;

/// Strongly-typed capability identifier. Mirrors the bstack capability
/// taxonomy; we use a string newtype to avoid locking in the bstack enum.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityId(pub String);

/// Semantic domain (e.g. "rust-codegen", "rocket-sim", "eDNA-metabarcoding").
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DomainId(pub String);

/// A goal identifier — usually mirrors a Linear ticket or skill invocation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GoalId(pub String);

/// Agent posture — roughly homologous to Autonomic's OperatingMode but
/// describes the agent's *relationship to the field* rather than its
/// internal operating point.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Posture {
    /// Reading signals, not yet emitting. Default start-up state.
    Receptive,
    /// Concentrated on one goal; sensing is narrowed to that capability neighborhood.
    Focused,
    /// Actively emitting (e.g. publishing an observation stream).
    Broadcasting,
    /// Paused — senses nothing, emits nothing. Used for budget recovery.
    Dormant,
    /// In an active formation; directives come from the formation's coordinator.
    Swarming(crate::formation::FormationId),
}

/// Position in capability-space. Not geographic — a learned or declared
/// embedding of the agent's capability vector. 32-dim is a reasonable default;
/// see §10 open question on dimensionality choice.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldCoord {
    pub dims: [f32; FIELD_DIM],
}

pub const FIELD_DIM: usize = 32;

impl FieldCoord {
    pub fn zero() -> Self { Self { dims: [0.0; FIELD_DIM] } }

    /// L2 distance to another coordinate.
    pub fn distance(&self, other: &Self) -> f32 {
        self.dims.iter()
            .zip(other.dims.iter())
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            .sqrt()
    }
}

/// R*-tree point adapter. Implemented in `src/physics/propagation.rs`.
impl rstar::Point for FieldCoord {
    type Scalar = f32;
    const DIMENSIONS: usize = FIELD_DIM;
    fn generate(mut gen: impl FnMut(usize) -> Self::Scalar) -> Self {
        let mut dims = [0.0; FIELD_DIM];
        for (i, slot) in dims.iter_mut().enumerate() { *slot = gen(i); }
        Self { dims }
    }
    fn nth(&self, idx: usize) -> Self::Scalar { self.dims[idx] }
    fn nth_mut(&mut self, idx: usize) -> &mut Self::Scalar { &mut self.dims[idx] }
}

/// An agent's full presence in the field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentLocus {
    pub id: AgentId,
    pub capabilities: Vec<CapabilityId>,
    pub posture: Posture,
    /// Normalized load \in [0, 1]; 1.0 == saturated.
    pub load: f32,
    /// Ephemeral budget for sensing/emitting this cycle (micro-credits or
    /// arbitrary scalar; see §10 open question on budget enforcement).
    pub attention_budget: u32,
    pub field_position: FieldCoord,
}
```

### 2.3 Formations — `src/formation.rs`

```rust
use serde::{Deserialize, Serialize};
use aios_protocol::ids::AgentId;
use crate::locus::{CapabilityId, GoalId};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FormationId(pub ulid::Ulid);

impl FormationId { pub fn new() -> Self { Self(ulid::Ulid::new()) } }

/// Topology of the formation's internal coordination graph.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FormationTopology {
    Mesh,                                     // all-to-all, at most ~8 members
    Star   { hub: AgentId },                  // hub aggregates, workers report
    Chain,                                    // ordered pipeline
    Ring,                                     // ordered cyclic
    Hierarchy { depth: u8 },                  // tree of coordinators
}

/// Role within a formation. One agent may hold multiple roles across formations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormationRole {
    Hub,
    Worker,
    Observer,     // passive presence — counts for quorum but does not execute
    Coordinator,  // subset-leader in Hierarchy topology
}

/// Lifecycle state. Always monotonic: Recruiting → Active → Dispersing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormationLifecycle {
    Recruiting,
    Active,
    Dispersing,
}

/// A transient swarm structure. Formations die on TTL expiry unless renewed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Formation {
    pub id: FormationId,
    pub purpose: GoalId,
    pub topology: FormationTopology,
    pub lifecycle: FormationLifecycle,
    /// Minimum members required to transition Recruiting → Active.
    pub quorum_min: u8,
    /// Time-to-live from most recent keepalive.
    pub ttl: Duration,
    /// (agent, role) pairs. Vec not HashMap because order matters for Chain/Ring.
    pub members: Vec<(AgentId, FormationRole)>,
    /// Capabilities the formation still needs (filled as members join).
    pub required_capabilities: Vec<CapabilityId>,
    /// Unix-millis of most recent keepalive; TTL is relative to this.
    pub last_keepalive_ms: i64,
}

impl Formation {
    pub fn is_expired(&self, now_ms: i64) -> bool {
        let elapsed_ms = now_ms.saturating_sub(self.last_keepalive_ms) as u128;
        elapsed_ms > self.ttl.as_millis()
    }
}
```

### 2.4 Aggregate — `src/field.rs`

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use aios_protocol::ids::AgentId;
use crate::locus::{AgentLocus, CapabilityId, FieldCoord};
use crate::formation::Formation;
use crate::signal::{TraceType, Reach};

/// What the depth-1 controller reads when it calls `aggregate()`.
///
/// Snapshot type — the aggregate operator (`AgentFieldPlexus::aggregate()`)
/// returns a fresh copy every call; this is never mutated in place.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PopulationState {
    pub members: Vec<AgentLocus>,
    pub active_formations: Vec<Formation>,
    pub gradient_fields: HashMap<CapabilityId, GradientField>,
    pub pheromone_map: HashMap<TraceType, Vec<Trace>>,
    pub quorum_readings: HashMap<CapabilityId, QuorumReading>,
    /// Wall-clock time of this snapshot, for drift detection at d1.
    pub sampled_at_ms: i64,
    /// Which reach bucket(s) the aggregator covered. A partial aggregate
    /// labeled `Local` is still a valid response — the d1 controller uses
    /// this to decide whether to resample.
    pub coverage: Reach,
}

/// Sampled scalar field over capability-space, keyed by capability.
///
/// We store a sparse set of (coord, scalar) samples rather than a dense grid —
/// capability-space is high-dim (FIELD_DIM=32) so a dense grid is intractable.
/// Interpolation/smoothing is a d1-controller responsibility; the plexus
/// returns raw samples and lets the consumer pick the reducer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GradientField {
    pub capability: CapabilityId,
    pub samples: Vec<(FieldCoord, f32)>,
}

/// Density reading for one capability, centered at the sampling agent.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct QuorumReading {
    pub capability_digest: u64,     // FNV-64 of CapabilityId for compact logs
    pub density: f32,               // #matching agents / sense-radius volume
    pub neighbors_with_capability: u32,
    pub sense_radius: f32,          // field-space units
}

/// Stigmergic marking. Differs from a PlexusSignal::Pheromone in that a Trace
/// is already *in the field* — this is the stored form after ingest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trace {
    pub id: ulid::Ulid,
    pub trace_type: TraceType,
    pub position: FieldCoord,
    pub initial_intensity: f32,
    pub emitted_at_ms: i64,
    pub decay_ms: u64,
    /// Original emitter, retained for audit only — Traces outlive their emitters.
    pub emitted_by: AgentId,
}

impl Trace {
    /// Current intensity at time `now_ms`, applying exponential decay.
    pub fn intensity(&self, now_ms: i64) -> f32 {
        let dt = (now_ms.saturating_sub(self.emitted_at_ms)) as f64;
        let hl = self.decay_ms.max(1) as f64;
        (self.initial_intensity as f64 * 0.5_f64.powf(dt / hl)) as f32
    }

    /// True when intensity would be below the log-noise floor.
    pub fn is_expired(&self, now_ms: i64, floor: f32) -> bool {
        self.intensity(now_ms) < floor
    }
}
```

### 2.5 Directives — `src/directive.rs`

```rust
use serde::{Deserialize, Serialize};
use aios_protocol::ids::AgentId;
use crate::formation::{FormationId, FormationTopology};
use crate::locus::{GoalId, Posture};
use crate::signal::{Micro, Reach};

/// What the depth-1 controller sends down through the plexus.
///
/// A CollectiveDirective is **advisory** by default and subject to the
/// receiving agent's L0 safety shield. This is the implementation of P6's C2
/// (bounded directive authority): a CollectiveDirective cannot override a
/// depth-0 agent's local shields.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CollectiveDirective {
    AdjustPosture     { target: AgentId, posture: Posture },
    RequestFormation  { purpose: GoalId, topology: FormationTopology,
                        members: Vec<AgentId> },
    DissolveFormation { id: FormationId, reason: String },
    AllocateBudget    { target: AgentId, delta: Micro },
    BroadcastNarrative{ narrative: String, reach: Reach },
}
```

## 3. Physics implementation

Every physics primitive below lives in `src/physics/*.rs` and is tested in isolation
(see §8.1).

### 3.1 Signal decay — `physics/decay.rs`

```rust
/// Exponential decay with half-life.
///
/// strength(t) = initial · 0.5^((t − emit_t_ms) / half_life_ms)
///
/// Half-life, not time-constant, because it is easier for humans to reason about
/// ("this signal is irrelevant in ~3 minutes") than τ.
#[inline]
pub fn decayed(initial: f32, emit_t_ms: i64, now_ms: i64, half_life_ms: u64) -> f32 {
    if half_life_ms == 0 { return 0.0; }
    let dt = now_ms.saturating_sub(emit_t_ms) as f64;
    if dt <= 0.0 { return initial; }
    let exponent = dt / half_life_ms as f64;
    (initial as f64 * 0.5_f64.powf(exponent)) as f32
}
```

Applied at read-time, never at write-time — that way there is only ever one
copy of `initial` to reason about and no distributed clock-correction issues.

### 3.2 Propagation — `physics/propagation.rs`

Signals have a `Reach` bound; propagation selects *which agents* actually see a
signal. This uses capability-space distance, not wall-clock distance.

**Algorithmic approach.** An `rstar::RTree<FieldCoord>` indexed by agent.
- `Reach::Local` — `tree.nearest_neighbors(&emitter_pos).take(k_local)`
  (default `k_local = 8`).
- `Reach::Regional` — expanding-radius sphere query
  (`tree.locate_within_distance(pos, r)`) until we have `k_regional = 32` or the
  radius hits the `PlexusConfig::regional_radius` cap.
- `Reach::Global` — return all agents; rate-limited in `PlexusConfig` to at
  most `global_per_sec`, default 1/sec.

Tree is rebuilt **lazily** on a tick cadence (`PlexusConfig::rebuild_ms`,
default 250ms); individual updates are appended to a pending-insertion buffer
between rebuilds.

### 3.3 Gradient computation — `physics/gradient.rs`

Each agent computes its own local gradient from the signals within its reach:

```rust
/// Weighted sum of sensed signals, projected onto one capability axis.
///
/// For each signal s sensed by this agent:
///   w_s = decayed_strength(s)
///       · relevance(s, capability)
///       · (1 / (1 + distance(s.emitter, self_pos)))
///
/// `relevance` is a cheap scalar: 1.0 if the signal is explicitly tagged with
/// `capability`, 0.5 if tagged with a capability sharing a prefix, 0.0 otherwise.
pub fn local_gradient(
    me: &AgentLocus,
    capability: &CapabilityId,
    sensed: &[EmittedSignal],
    now_ms: i64,
) -> Vec<(FieldCoord, f32)> { /* implementation */ }
```

Aggregation at d1 is the union of these local samples — explicitly sparse, per
§2.4 rationale. A d1 controller that wants a dense field runs its own
interpolation on the returned `Vec<(FieldCoord, f32)>`.

### 3.4 Stigmergy — `physics/stigmergy.rs`

Append-only `Vec<Trace>` per TraceType, behind a `parking_lot::RwLock`.
Two background tasks:

1. **TTL sweep**. Every `trace_sweep_ms` (default 5000), drop traces whose
   current intensity is below a noise floor (`trace_noise_floor`, default 0.01).
2. **Compaction**. If the trace count exceeds `trace_cap_per_type` (default
   4096), drop the oldest `N / 4` regardless of intensity. This is lossy by
   design — stigmergic systems tolerate pruning.

Persistence (`feature = "persist"`) shadows the in-memory store into a redb
table `plexus_traces` keyed by `(TraceType, Ulid)`. Intentionally **not**
lago-journal-backed — traces are ephemeral field state, not ground-truth events.
A fresh restart with `feature = "persist"` loads the last compacted snapshot;
without the feature, restart means an empty field, consistent with §9 scope.

### 3.5 Quorum detection — `physics/quorum.rs`

```rust
/// Count neighbors (within `radius`) that possess `capability`.
///
/// The R*-tree returns candidates by spatial proximity; we then filter by
/// capability list. O(k + m) where k = spatial neighbors, m = capabilities
/// per agent.
pub fn quorum_reading(
    tree: &rstar::RTree<AgentIndex>,
    center: FieldCoord,
    radius: f32,
    capability: &CapabilityId,
    loci: &HashMap<AgentId, AgentLocus>,
) -> QuorumReading;
```

`density` = `neighbors_with_capability / ((4/3)π·radius³)` with the caveat that
FIELD_DIM > 3, so we use the analogous n-ball volume formula. In practice the
absolute density is uncalibrated; what matters is *relative* quorum across
capabilities and time.

## 4. Pneuma trait impl — `src/plexus.rs`

```rust
use std::sync::Arc;
use parking_lot::RwLock;
use aios_protocol::pneuma::{
    Pneuma, PneumaError, SubstrateProfile, SubstrateKind, WarpFactors,
    CoordinationScaling, ResourceCeiling, D0ToD1,
};
use crate::config::PlexusConfig;
use crate::directive::CollectiveDirective;
use crate::field::PopulationState;
use crate::signal::{EmittedSignal, PlexusSignal, Reach};
use crate::transport::PlexusTransport;

/// The horizontal Pneuma impl. Owns the in-process field state; delegates
/// cross-process fanout to the [`PlexusTransport`] it was constructed with.
///
/// One `AgentFieldPlexus` represents the view from **one** participating
/// agent. A depth-1 controller holding N agent views is a separate concern
/// (see `PlexusFleet` in §9 future work).
pub struct AgentFieldPlexus {
    config: PlexusConfig,
    transport: Arc<dyn PlexusTransport>,
    state: Arc<RwLock<PlexusState>>,
    pending_directives: Arc<RwLock<Vec<CollectiveDirective>>>,
}

/// Internal state — tree, caches, timestamps.
struct PlexusState { /* locus table, RTree, trace store, etc. */ }

impl Pneuma for AgentFieldPlexus {
    type B         = D0ToD1;
    type Signal    = PlexusSignal;
    type Aggregate = PopulationState;
    type Directive = CollectiveDirective;

    fn emit(&self, signal: Self::Signal) -> Result<(), PneumaError> {
        // 1. Respect population cap (C4).
        let state = self.state.read();
        if state.loci_count() > self.config.population_cap {
            return Err(PneumaError::ShieldRejection(
                format!("population cap {} exceeded", self.config.population_cap)));
        }
        drop(state);

        // 2. Respect per-agent attention budget.
        //    Implementation reads `me.attention_budget`; if 0, reject.

        // 3. Wrap into EmittedSignal with emitter pos + reach.
        let emitted = EmittedSignal {
            id: ulid::Ulid::new(),
            emitted_by: self.config.agent_id.clone(),
            emitted_at_ms: now_ms(),
            emitter_pos: self.current_pos(),
            reach: signal.default_reach(),
            payload: signal,
        };

        // 4. Dispatch to transport — the only place where "real" propagation
        //    happens. Transport errors bubble up as PneumaError::Transport.
        self.transport.publish(&emitted)
            .map_err(|e| PneumaError::Transport(e.to_string()))?;

        // 5. Also ingest locally so this agent's own aggregate sees it.
        self.ingest_local(emitted);
        Ok(())
    }

    fn aggregate(&self) -> Self::Aggregate {
        let state = self.state.read();
        let now = now_ms();

        PopulationState {
            members:           state.loci.values().cloned().collect(),
            active_formations: state.formations.values().cloned().collect(),
            gradient_fields:   state.compute_gradient_fields(now, &self.config),
            pheromone_map:     state.live_pheromones(now, &self.config),
            quorum_readings:   state.compute_quorums(&self.config),
            sampled_at_ms:     now,
            coverage:          Reach::Regional, // default for full aggregate
        }
    }

    fn receive(&self) -> Option<Self::Directive> {
        self.pending_directives.write().pop()
    }

    fn substrate(&self) -> SubstrateProfile {
        SubstrateProfile {
            kind: SubstrateKind::ClassicalSilicon,
            warp_factors: WarpFactors {
                time: 1.0,
                energy: 1.0,
                coordination: CoordinationScaling::NLogN, // R*-tree k-NN
                memory: 1.0,
                branching: None,                          // classical
            },
            ceiling: ResourceCeiling::Propagation {
                max_radius_m: self.config.nats_max_cluster_diameter_m
                                    .unwrap_or(1_000.0),
            },
        }
    }
}
```

The `ingest_local` hook is also invoked by the transport's **subscribe** side
when a remote agent publishes a signal — that is how other agents' signals
land in this agent's view.

## 5. Transport layer — `src/transport/`

```rust
/// Abstracts "publish this emitted signal to peers" and "subscribe to their
/// emissions." Transport is explicit because the transport *is* the physics
/// boundary: propagation delay τ_p and decay δ are transport properties.
pub trait PlexusTransport: Send + Sync {
    /// Fire-and-forget publish. Errors are transient; retry is caller-side.
    fn publish(&self, emitted: &EmittedSignal) -> Result<(), TransportError>;

    /// Install a subscriber closure. Called once per received EmittedSignal.
    /// The closure must be `Send + 'static` because NATS drives it from its
    /// own task.
    fn subscribe(&self, f: Box<dyn Fn(EmittedSignal) + Send + Sync + 'static>);

    /// Publish a directive — the d1 → d0 channel. Orthogonal to `publish`
    /// because directives are different subjects and carry different auth.
    fn publish_directive(&self, dir: &CollectiveDirective) -> Result<(), TransportError>;

    /// Subscribe to directives.
    fn subscribe_directives(&self, f: Box<dyn Fn(CollectiveDirective) + Send + Sync + 'static>);
}
```

Selection is by constructor:

```rust
impl AgentFieldPlexus {
    pub fn in_process(cfg: PlexusConfig) -> Self {
        let transport = Arc::new(transport::in_process::InProcessTransport::new(&cfg));
        Self::with_transport(cfg, transport)
    }

    #[cfg(feature = "nats")]
    pub async fn with_nats(cfg: PlexusConfig, url: &str) -> Result<Self, TransportError> {
        let transport = Arc::new(transport::nats::NatsTransport::connect(url, &cfg).await?);
        Ok(Self::with_transport(cfg, transport))
    }

    #[cfg(feature = "mock")]
    pub fn mock(cfg: PlexusConfig) -> (Self, Arc<transport::mock::MockTransport>) {
        let mock = Arc::new(transport::mock::MockTransport::new(&cfg));
        let this = Self::with_transport(cfg, mock.clone());
        (this, mock)   // return mock handle so tests can introspect
    }
}
```

### 5.1 NATS JetStream backend — `transport/nats.rs`

**Subject encoding:** `plexus.<deploy>.<field_cell>.<signal_kind>.<agent_id>`
where:
- `<deploy>` is from `PlexusConfig::deploy_id` (prod / staging / test-run-$run).
- `<field_cell>` is a coarse spatial hash of the emitter's FieldCoord (default
  8 bins/axis, so `2^8` cells per dim — but we hash the **sign-bit of each
  axis** only, giving `2^FIELD_DIM = 2^32` cells at most; the cell key is a
  hex string of the 32 sign bits, producing a subject like
  `plexus.prod.f0a12d3e.intent.AGT-abc`).
- `<signal_kind>` is `belief|offer|intent|obs|recruit|pheromone`.
- `<agent_id>` is the emitter; receivers subscribe with wildcards.

**Directive subject:** `plexus.<deploy>.d1-directive.<target_agent_id>`.

**TTL on messages:** JetStream stream configured with
`max_age = longest_half_life * 4` so that stale signals are rolled out of the
server even if a consumer is slow. Plexus still applies per-signal decay at
read; the server TTL is belt-and-suspenders.

**Shared state via NATS KV:** `plexus.<deploy>.loci` KV bucket — agents write
their own `AgentLocus` here with a 30s heartbeat. Absence of heartbeat ⇒ the
locus expires out of the KV. This is what enables peer discovery without a
registry service.

### 5.2 In-process backend — `transport/in_process.rs`

- `tokio::sync::broadcast::channel::<EmittedSignal>(cap)` with
  `cap = config.broadcast_buffer` (default 1024).
- Shared state: `Arc<RwLock<Vec<AgentLocus>>>`.
- Sole consumer of the `default` feature flag, so single-machine testing needs
  no NATS cluster.

### 5.3 Mock backend — `transport/mock.rs`

- In-memory `Vec<EmittedSignal>` append log; `publish` appends synchronously.
- Deterministic: emission timestamps come from a `MockClock` that advances
  only when `MockTransport::tick(Duration)` is called.
- Configurable failure modes: `fail_next_n`, `drop_directives`, `delay_ms`.

## 6. Constraints from P6

The four horizontal-stability conditions from `research/rcs/papers/p6-horizontal-composition/README.md`
map onto `life-plexus` as follows:

### C1 — Time-scale dilation: `τ₀^(d1) ≥ κ · max_i τ₃^(d0)`, `κ ≥ 10`

Implementation:
- `PlexusConfig::directive_min_interval_ms` enforced at
  `AgentFieldPlexus::receive_directive` — if a directive arrives less than
  this interval after the previous one for the same target agent, it is
  **buffered** and coalesced rather than delivered.
- Default `directive_min_interval_ms = 10 × depth_0_l3_cadence_ms`
  (pulled from canonical `parameters.toml`, e.g. 10× 30s Nous cadence = 5min).
- Health endpoint surfaces the actual observed `directive_rate_hz` so P6
  compliance can be verified at runtime by Vigil.

### C2 — Bounded directive authority: `α · L_d^(k) < min_i λ₀^(k, i)`

Implementation:
- Every `CollectiveDirective` traverses the receiving agent's L0 shield
  (`RecursiveControlledSystem::<L0>::shield`) **before** being applied.
  `life-plexus` ships a helper `apply_directive_with_shield()` that enforces
  this; agents that integrate plexus must use it (lint rule in review).
- Hard cap: directives that adjust budgets (`AllocateBudget`) are clamped to
  `PlexusConfig::max_budget_delta_per_directive` (default: 10% of the
  target's current budget).
- `BroadcastNarrative` directives with `Reach::Global` require the target
  set to opt in via `PlexusConfig::accept_global_narrative` (default false).

### C3 — Signal decay exceeds propagation: `δ > 1/τ_p`

Implementation:
- Default half-lives in `PlexusConfig` are sized **10× the measured
  propagation delay**. NATS clusters typically run τ_p ∈ [5, 50] ms at
  intra-region distances, so defaults are:
  - Pheromone: `60_000 ms` (60s) — slow decay
  - BeliefClaim: `30_000 ms` (30s)
  - Observation: `15_000 ms`
  - Recruit: `10_000 ms`
  - CapabilityOffer: `5_000 ms`
  - IntentDeclaration: `2_000 ms` — fastest
- The fastest default half-life (2s) is 40× the worst-case τ_p (50ms), so
  C3 is satisfied with ~4× margin.
- `PlexusConfig::min_half_life_ms` is a hard lower bound (default 500ms);
  signals emitted with shorter half-lives are rejected with
  `PneumaError::ShieldRejection`.

### C4 — Sub-critical coupling: `N · σ < reserve_budget(k+1)`

Implementation:
- `PlexusConfig::population_cap` (default: 50) is enforced in
  `AgentFieldPlexus::emit` — if the locus count exceeds it, new emissions are
  rejected (but reads continue to work, preserving observability).
- `σ` is controlled indirectly by `PlexusConfig::coupling_weights` — a
  per-signal-kind multiplier in `[0, 1]`. Defaults sum to < 1 so that the
  aggregate coupling strength across kinds remains bounded.
- Runtime metric `plexus.coupling_product = population * mean_coupling` is
  emitted to Vigil; d1 controllers observing it above a threshold should
  request `DissolveFormation` directives or raise `population_cap` gates.

All four of these are expressed as setpoints in
`core/life/crates/plexus/life-plexus/data/horizontal-parameters.toml`
(mirror of the authoritative `research/rcs/data/parameters.toml` under a new
`[horizontal]` section, synced via the existing
`life/scripts/sync-rcs-parameters.sh`).

## 7. Integration with Arcan — `src/tools.rs`

Following the `arcan-spaces` precedent exactly: one public
`register_plexus_tools(&mut ToolRegistry, Arc<AgentFieldPlexus>)` function,
one `Tool` impl per verb.

| Tool name              | Read/Write | Description                                              |
|------------------------|------------|----------------------------------------------------------|
| `plexus_sense_field`   | R          | Return recent signals within current attention budget    |
| `plexus_emit_signal`   | W          | Write a PlexusSignal to the field                        |
| `plexus_join_formation`| W          | Join a Recruiting formation as `FormationRole::Worker`   |
| `plexus_leave_formation`| W         | Exit a formation (idempotent)                            |
| `plexus_mark_pheromone`| W          | Stigmergic emit — shorthand for `emit_signal(Pheromone)` |
| `plexus_query_quorum`  | R          | Count nearby agents with a given capability              |

### 7.1 Example — `plexus_sense_field`

```rust
pub struct PlexusSenseFieldTool {
    plexus: Arc<AgentFieldPlexus>,
}

impl Tool for PlexusSenseFieldTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "plexus_sense_field".into(),
            description: "Return signals recently emitted into the plexus field \
                          within this agent's attention budget.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "max_signals": { "type": "integer", "default": 32, "maximum": 128 },
                    "kinds": {
                        "type": "array",
                        "items": { "type": "string",
                                   "enum": ["belief_claim","capability_offer",
                                            "intent_declaration","observation",
                                            "recruit","pheromone"] }
                    },
                    "since_ms": { "type": "integer",
                                  "description": "Unix millis lower bound" }
                }
            }),
            annotations: Some(ToolAnnotations {
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: true,
                requires_confirmation: false,
            }),
            category: Some("plexus".into()),
            tags: vec!["plexus".into(), "sensing".into()],
            timeout_secs: Some(5),
            title: None,
            output_schema: None,
        }
    }

    fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, CoreError> {
        // 1. Decrement attention budget, reject if 0.
        // 2. Query field for matching signals.
        // 3. Apply decay and return JSON array.
    }
}
```

Remaining five tools follow the same pattern and live in `src/tools.rs`.
They are defined here rather than in `arcan-plexus` to match the
`arcan-spaces` precedent — revisit this if a dep-cycle appears.

## 8. Test strategy

### 8.1 Unit tests (per primitive)

Each `physics/*.rs` has a `#[cfg(test)]` module with at least:
- **`decay`**: `decayed(1.0, 0, HL, HL) == 0.5`; `decayed(1.0, 0, 2·HL, HL) == 0.25`;
  monotonic decrease; symmetry across `half_life` scales.
- **`propagation`**: seed 100 random agents, emit from one, assert
  `Reach::Local` returns exactly `k_local` receivers sorted by distance.
- **`gradient`**: synthetic field with a known signal, assert gradient
  samples are proportional to `decayed_strength · relevance`.
- **`stigmergy`**: emit 4096 traces over 1 hour of simulated time, assert
  compaction never drops a trace whose current intensity > 0.5·floor.
- **`quorum`**: fixed 10-agent grid, assert `neighbors_with_capability` is
  exactly `k` for capabilities held by `k` neighbors.

All deterministic — no wall-clock dependencies; `now_ms` is injected.

### 8.2 Integration: two-agent chain formation (in-process)

```rust
#[tokio::test]
async fn two_agents_form_chain_and_propagate_signals() {
    let (a_plex, _a_mock) = AgentFieldPlexus::mock(cfg_for("agent-A"));
    let (b_plex, _b_mock) = AgentFieldPlexus::mock(cfg_for("agent-B"));
    // Wire the two mocks into a shared field.
    MockTransport::join(&a_plex, &b_plex);

    // A emits Recruit for a Chain formation.
    a_plex.emit(recruit_chain_signal()).unwrap();

    // B's aggregate should reflect the pending formation.
    let state = b_plex.aggregate();
    assert_eq!(state.active_formations.len(), 1);
    assert_eq!(state.active_formations[0].lifecycle, FormationLifecycle::Recruiting);

    // B joins via the plexus_join_formation tool path.
    b_plex.join_formation(state.active_formations[0].id).unwrap();

    // After the next tick, formation should transition to Active (quorum met).
    tokio::time::sleep(Duration::from_millis(100)).await;
    let state = b_plex.aggregate();
    assert_eq!(state.active_formations[0].lifecycle, FormationLifecycle::Active);
}
```

### 8.3 Physics sanity

- **Signal-decay curve fit**: emit 1000 signals at `t=0`, query at
  `t ∈ {HL/2, HL, 2HL, 4HL}`, fit an exponential, assert the recovered
  half-life is within 1% of the emit-time `half_life_ms`.
- **Propagation distance**: plot the receive-count histogram vs distance,
  assert it is flat within `Reach::Local.k` and zero beyond.
- **Formation dissolution on TTL**: create a formation with 500ms TTL, never
  keepalive, assert it transitions Recruiting → Dispersing → (gone) within
  2×TTL.

### 8.4 End-to-end: two Life instances on NATS

Gated by `feature = "nats"` and the presence of `NATS_URL` env var in CI.
The test spawns:
- `life-plexus` agent A, configured with `agent_id = "AGT-e2e-A"`.
- `life-plexus` agent B, configured with `agent_id = "AGT-e2e-B"`.
- A mock depth-1 controller that calls `aggregate()` every 500ms and, when
  the observed `IntentDeclaration` count exceeds 3, issues a
  `RequestFormation` directive targeting both.

Assertions:
- Both A and B observe the other's `IntentDeclaration` signals within
  τ_p ≤ 100ms.
- Both receive the directive within ≤ 500ms.
- After TTL, the formation dissolves without controller intervention.
- Total wall time of the test ≤ 5s.

## 9. Deferred / out of scope

1. **Persistence of signals across agent restarts.** Signals are ephemeral by
   design — C3 demands they decay faster than they propagate, and persisting
   them would create standing waves. The `feature = "persist"` flag only
   persists **compacted pheromone trace snapshots**, not signal history.
2. **Cross-deployment federation.** A depth-1 plexus is single-cluster
   initially. Multi-cluster (e.g. one NATS in us-east-1, another in eu-west-1)
   is a depth-2 concern and would live in a separate crate.
3. **Quantum or neuromorphic backends.** Classical silicon only. The
   `SubstrateProfile` API supports reporting alternative substrates, but
   `life-plexus` itself does not implement any.
4. **Learned capability-space embeddings.** `FieldCoord` is populated by the
   agent itself (declared) in v1. A learned embedding (capability2vec) is
   explicitly future work.
5. **Voting/consensus primitives.** No `Raft`, no quorum voting on directives.
   The plexus is stigmergic — agents act on gradients, not votes. A future
   `life-synod` crate can sit on top if voting semantics are required.
6. **Economic settlement.** `Micro` appears in `CapabilityOffer` and
   `AllocateBudget`, but actual payment rails live in `haima`. The plexus
   announces costs; `haima` executes them.
7. **Formation topology evolution.** A formation is born with one
   `FormationTopology` and keeps it until dispersed. No migration
   `Chain → Mesh` in v1.
8. **Automatic backpressure on d1 directives.** If directives arrive faster
   than C1 allows, we coalesce. We do not push-back on the d1 controller
   itself — that is a d1-side concern. A future version could emit a
   `PneumaError::DirectiveRateExceeded` that the d1 controller handles.
9. **Cryptographic attribution.** Signals carry `emitted_by: AgentId` but are
   not signed. A future `feature = "signed"` can require ed25519 signatures
   via `aios_protocol::identity::BasicIdentity`.

## 10. Open questions

1. **Attention budget enforcement** — should `attention_budget` be consumed
   per-signal-emitted, per-signal-sensed, or both? Current design says both
   (1 credit each), but sensing 32 signals costs 32× more than one, which
   might starve observers; alternative is a free-read, paid-write model.
2. **Capability-space dimensionality** — `FIELD_DIM = 32` is a guess. Too few
   and distinct capabilities collide; too many and R*-tree k-NN degrades.
   Empirical tuning on a 50-agent testbed is needed.
3. **Formation topology during lifecycle** — do we allow Recruiting
   formations to switch topology (e.g. start as Star, devolve to Mesh if the
   hub overloads)? Current design says no; may need revisiting.
4. **Gradient sampling vs full aggregation** — `aggregate()` currently
   returns sparse samples. Should there be a `dense_aggregate()` variant
   that does KDE interpolation server-side, or is that always a
   d1-controller concern?
5. **Relationship with Spaces** — when both are present:
   (a) fallback: plexus uses Spaces when NATS is unavailable,
   (b) mirror: plexus emits to both, or
   (c) disjoint: plexus is agent-native, Spaces is human-visible, no crosstalk.
   Current architecture spec says (c); this may not survive contact with
   operators who want a single pane of glass.
6. **Heterogeneous populations** — P6 Theorem 1 assumes homogeneous
   `λᵢ` across agents. If one agent in the population has `λ₀ < α·L_d`, does
   `life-plexus` refuse to admit it (shield), or degrade gracefully? Current
   design admits silently; may need a `peer_stability_gate` config.
7. **Reach inflation by hub formations** — a Star formation hub, by
   definition, sees everything its workers see. Does the hub's
   `attention_budget` need to be `workers·budget` or just `budget`? Current
   design says one budget per agent regardless of formation role.
8. **Directive replay under partial network** — if the transport drops a
   `DissolveFormation` directive, is the formation expected to dissolve
   anyway via TTL? Current design says yes (TTL is the source of truth);
   double-check this preserves C2 under packet loss.
9. **Cross-feature interaction** — `feature = "persist"` + `feature = "mock"`
   is a meaningful combination for test reproducibility but is not currently
   tested. Need a matrix test in CI.
10. **Nous evaluator access** — should the plexus call Nous on a
    `BeliefClaim` before admitting it, or trust the emitter's self-score?
    Trust-by-default is faster but allows belief-spam; Nous-gating is safer
    but creates a hot dependency. Likely answer: trust, but rate-limit per
    emitter via `coupling_weights[BeliefClaim]`.
11. **Budget units** — `AllocateBudget` uses `Micro` (1e-6 USDC). Is the
    plexus the right place to carry monetary units, or should this be
    `haima::BudgetDelta`? Import from haima creates a workspace cycle
    (haima → plexus → haima via Tool integration); defining locally is the
    current compromise.
12. **Naming of capability-space origin** — who decides `(0, 0, …, 0)`? If
    every agent picks its own `FieldCoord::zero()` as "itself", all agents
    claim the center and k-NN breaks. Current design requires every agent to
    receive a `FieldCoord` from a bstack-issued embedding at startup;
    unclear who owns that embedding service.

## Appendix A — Public API surface at v0.1

```rust
// re-exports from src/lib.rs
pub use aios_protocol::pneuma::{Pneuma, PneumaError, SubstrateProfile};

pub use crate::{
    config::PlexusConfig,
    directive::CollectiveDirective,
    error::PlexusError,
    field::{GradientField, PopulationState, QuorumReading, Trace},
    formation::{Formation, FormationId, FormationLifecycle,
                FormationRole, FormationTopology},
    locus::{AgentLocus, CapabilityId, DomainId, FieldCoord, GoalId, Posture},
    plexus::AgentFieldPlexus,
    signal::{EmittedSignal, Micro, PlexusSignal, Reach, TraceType},
    tools::register_plexus_tools,
    transport::{PlexusTransport, TransportError},
};
```

## Appendix B — Minimum viable contributor checklist

1. ☐ Scaffold `crates/plexus/life-plexus/` with `Cargo.toml`, `src/lib.rs`, README.
2. ☐ Add `"crates/plexus/life-plexus"` to workspace members.
3. ☐ Implement `aios-protocol::pneuma` module (prerequisite, may precede this PR).
4. ☐ Implement §2 types in their respective files; `cargo check` green.
5. ☐ Implement §3 physics primitives with unit tests (§8.1); `cargo test -p life-plexus` green.
6. ☐ Implement `InProcessTransport` (no feature flag); wire to `AgentFieldPlexus`.
7. ☐ Implement §8.2 in-process integration test.
8. ☐ Implement `MockTransport` under `feature = "mock"`.
9. ☐ Implement NATS transport under `feature = "nats"` with a manual README test.
10. ☐ Implement §7 tool wrappers; register in `arcan` via `register_plexus_tools`.
11. ☐ Add horizontal params to `research/rcs/data/parameters.toml`; run `make params`
     and `bash core/life/scripts/sync-rcs-parameters.sh`.
12. ☐ Wire §8.4 e2e test behind `feature = "nats"`.
13. ☐ Update `pneuma-plexus-architecture.md` "Status" from `draft` to `phase-3-shipping`.
