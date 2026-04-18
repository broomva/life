---
title: "Pneuma — Rust Trait Surface Specification"
tags:
  - spec
  - rust
  - pneuma
  - rcs
  - aios-protocol
  - trait-surface
created: "2026-04-18"
updated: "2026-04-18"
status: draft
related:
  - "[[pneuma-plexus-architecture]]"
  - "[[concept/pneuma]]"
  - "[[rcs]]"
  - "[[trait-not-rename]]"
---

# Pneuma — Rust Trait Surface Specification

This document is the canonical Rust specification for the `aios-protocol::pneuma` module. It defines the substrate-invariant trait family for inter-boundary observation and control flow. Every symbol in this spec is intended to be ported verbatim into `core/life/crates/aios/aios-protocol/src/pneuma.rs` (and a corresponding `pub mod pneuma;` + re-exports in `src/lib.rs`).

**Scope:** this spec defines the *trait surface only* — no transports, no concrete crates, no wire formats. Horizontal impls (`life-plexus`) and vertical retrofits (`lago-journal`, `autonomic-controller`, `egri`, `bstack-policy`) live in their own crates and will depend on this surface.

**Non-scope:** the spec deliberately omits:
- Async methods (open question — see end of doc).
- Backpressure / flow control primitives (open question).
- Cross-instance wire formats (P6 deliverable).
- Per-substrate impls (Phase 4 in architecture spec).

**Design invariants:**
1. **Trait, not rename.** Existing types stay. Pneuma adds a thin trait surface on top.
2. **Substrate-invariant.** Same trait shape works for silicon, neuromorphic, quantum, biological.
3. **Associated types over generics.** A Pneuma impl crosses *one* boundary; its Signal/Aggregate/Directive are implementation-determined, not caller-selected.
4. **Zero-sized markers.** All `Boundary` impls and `Axis` impls are ZSTs, used only for type-level dispatch.
5. **`Send + Sync + 'static`** on all payload types (enables cross-thread transport).

---

## 0. File header & module docstring

```rust
//! Pneuma — substrate-invariant inter-boundary observation and control flow.
//!
//! Pneuma (πνεῦμα, "breath") is the abstract pattern by which aggregate
//! observations cross a boundary upward and directives cross the same boundary
//! downward. It operates on two orthogonal recursion axes:
//!
//! - **Vertical** — L0→L1→L2→L3 within a single Life instance (formalized in
//!   [`crate::rcs::RecursiveControlledSystem`]).
//! - **Horizontal** — depth-k→depth-(k+1) between instances (a population of
//!   depth-k Life instances becomes the L0 plant of a depth-(k+1) instance).
//!
//! # Design
//!
//! `Pneuma` is a trait family, not a concrete implementation. Existing crates
//! retain their payload types (`EventKind`, `GatingProfile`, `HomeostaticState`,
//! etc.) and gain Pneuma impls. No renames.
//!
//! Each Pneuma impl crosses exactly one [`Boundary`]. The boundary is a
//! zero-sized type marker used to prevent cross-boundary miswiring at compile
//! time (e.g., an `L1→L2` impl cannot be passed where an `L0→L1` impl is
//! required).
//!
//! # Hierarchy
//!
//! ```text
//! trait Axis: Send + Sync + 'static
//!   ├── struct Vertical
//!   └── struct Horizontal
//!
//! trait Boundary: Send + Sync + 'static
//!   ├── (vertical) L0ToL1, L1ToL2, L2ToL3, L3ToExternal
//!   └── (horizontal) D0ToD1, D1ToD2, D2ToD3
//!
//! trait Pneuma: Send + Sync
//!   type B: Boundary
//!   type Signal, Aggregate, Directive: Send + Sync + 'static
//!   fn emit, aggregate, receive, substrate
//! ```
//!
//! # References
//!
//! - Architecture spec: `core/life/docs/specs/pneuma-plexus-architecture.md`
//! - Concept page: `research/entities/concept/pneuma.md`
//! - Horizontal stability theorem (planned): `research/rcs/papers/p6-horizontal-composition/`
//! - Substrate warp factors (planned): `research/rcs/papers/p7-thermodynamic-limits/`

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
```

---

## 1. Axis markers

Two axes of recursion — vertical (within one instance) and horizontal (between instances). Both are zero-sized marker types implementing the `Axis` trait. They exist so callers can statically assert which recursion they are operating on when composing multiple Pneuma impls.

```rust
// ---------------------------------------------------------------------------
// Axis markers
// ---------------------------------------------------------------------------

/// Marker trait for a recursion axis.
///
/// An axis is the direction of recursion in the Life Agent OS:
/// [`Vertical`] (levels within one instance) or [`Horizontal`] (depths between
/// instances). Implementations are zero-sized types used purely for type-level
/// dispatch; there is no runtime representation.
pub trait Axis: Send + Sync + 'static + fmt::Debug {
    /// Human-readable name of the axis (e.g., `"vertical"`).
    fn name() -> &'static str;
}

/// Vertical axis — recursion across levels within a single Life instance
/// (L0 → L1 → L2 → L3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Vertical;

/// Horizontal axis — recursion across depths between Life instances
/// (depth-k → depth-(k+1)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Horizontal;

impl Axis for Vertical {
    fn name() -> &'static str {
        "vertical"
    }
}

impl Axis for Horizontal {
    fn name() -> &'static str {
        "horizontal"
    }
}
```

---

## 2. Boundary markers

A `Boundary` names a specific interface (e.g., `L0→L1`, `D0→D1`) and has an associated axis. Boundaries are zero-sized markers used as associated types on `Pneuma` impls to prevent a caller from wiring, say, a plexus (`D0→D1`) where a journal (`L0→L1`) is expected.

```rust
// ---------------------------------------------------------------------------
// Boundary markers
// ---------------------------------------------------------------------------

/// Marker trait for a specific boundary crossed by a [`Pneuma`] impl.
///
/// A boundary is the pair `(axis, interface)` where `interface` names which
/// levels or depths the implementation connects. Like [`Axis`], every impl is a
/// zero-sized type — boundaries are compile-time tags, not runtime objects.
///
/// The `A` associated type pins each boundary to exactly one axis, preventing
/// nonsensical combinations (e.g., a "vertical depth-0 to depth-1" boundary).
pub trait Boundary: Send + Sync + 'static + fmt::Debug {
    /// The axis this boundary lives on ([`Vertical`] or [`Horizontal`]).
    type A: Axis;

    /// Name of the axis (e.g., `"vertical"`). Delegates to `Self::A::name()`.
    fn axis_name() -> &'static str {
        Self::A::name()
    }

    /// Name of the boundary (e.g., `"L0→L1"`, `"depth-0 → depth-1"`).
    fn boundary_name() -> &'static str;
}

// --- Vertical boundaries ---------------------------------------------------

/// Vertical boundary between L0 (external plant) and L1 (agent internal).
///
/// Typical impl: `lago-journal::LagoJournal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct L0ToL1;

/// Vertical boundary between L1 (agent internal) and L2 (meta-control / EGRI).
///
/// Typical impl: `autonomic-controller::AutonomicProjection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct L1ToL2;

/// Vertical boundary between L2 (EGRI) and L3 (governance).
///
/// Typical impl: `autoany-core::EgriLoop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct L2ToL3;

/// Vertical boundary between L3 (governance) and the external world.
///
/// Typical impl: `bstack-policy::BstackPolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct L3ToExternal;

impl Boundary for L0ToL1 {
    type A = Vertical;
    fn boundary_name() -> &'static str {
        "L0→L1"
    }
}

impl Boundary for L1ToL2 {
    type A = Vertical;
    fn boundary_name() -> &'static str {
        "L1→L2"
    }
}

impl Boundary for L2ToL3 {
    type A = Vertical;
    fn boundary_name() -> &'static str {
        "L2→L3"
    }
}

impl Boundary for L3ToExternal {
    type A = Vertical;
    fn boundary_name() -> &'static str {
        "L3→External"
    }
}

// --- Horizontal boundaries -------------------------------------------------

/// Horizontal boundary between depth-0 (individual agent) and depth-1
/// (collective / swarm).
///
/// Typical impl: `life-plexus::AgentFieldPlexus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct D0ToD1;

/// Horizontal boundary between depth-1 (collective) and depth-2 (federation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct D1ToD2;

/// Horizontal boundary between depth-2 (federation) and depth-3 (civilization).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct D2ToD3;

impl Boundary for D0ToD1 {
    type A = Horizontal;
    fn boundary_name() -> &'static str {
        "depth-0 → depth-1"
    }
}

impl Boundary for D1ToD2 {
    type A = Horizontal;
    fn boundary_name() -> &'static str {
        "depth-1 → depth-2"
    }
}

impl Boundary for D2ToD3 {
    type A = Horizontal;
    fn boundary_name() -> &'static str {
        "depth-2 → depth-3"
    }
}
```

---

## 3. Substrate metadata

`SubstrateProfile` is the runtime description of the physical substrate a Pneuma impl executes on. It exists so depth-(k+1) planners can reason about scaling laws (linear vs quadratic coordination, thermodynamic vs coherence-limited, etc.) without knowing the substrate statically.

All types in this section are network-transportable and therefore implement `Serialize + Deserialize`. They are `#[non_exhaustive]` where plausible growth is expected (new substrate kinds, new coordination regimes, new ceiling types).

```rust
// ---------------------------------------------------------------------------
// Substrate metadata
// ---------------------------------------------------------------------------

/// Runtime description of the physical substrate a [`Pneuma`] impl uses.
///
/// Consumed by depth-(k+1) planners to select boundaries by cost profile
/// (e.g., prefer a neuromorphic plexus for energy-bounded workloads, a
/// quantum plexus for branching/superposition workloads).
///
/// # Example
///
/// ```rust,ignore
/// use aios_protocol::pneuma::*;
///
/// let profile = SubstrateProfile {
///     kind: SubstrateKind::ClassicalSilicon,
///     warp_factors: WarpFactors::classical_baseline(),
///     ceiling: ResourceCeiling::Thermodynamic { max_watts: 500.0 },
/// };
/// assert_eq!(profile.kind.family(), "classical");
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubstrateProfile {
    /// Substrate family (classical silicon, neuromorphic, quantum, biological,
    /// hybrid).
    pub kind: SubstrateKind,

    /// Per-dimension warp factors relative to the classical baseline
    /// (time, energy, coordination, memory, branching).
    pub warp_factors: WarpFactors,

    /// The binding resource constraint on this substrate (thermodynamic,
    /// coherence, propagation).
    pub ceiling: ResourceCeiling,
}

/// The physical family a substrate belongs to.
///
/// New variants may be added as new substrate classes are integrated. Consumers
/// should handle unknown kinds gracefully; see `#[non_exhaustive]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum SubstrateKind {
    /// Classical silicon (CPU/GPU). The baseline against which warp factors
    /// are measured.
    ClassicalSilicon,

    /// Spiking neural hardware (Loihi, SpiNNaker, Akida) — event-driven,
    /// low-power, coordination is O(N).
    Neuromorphic,

    /// Quantum processor. `qubits` is the usable qubit count; `coherence_us`
    /// is the nominal decoherence window in microseconds.
    Quantum {
        /// Number of usable qubits.
        qubits: u32,
        /// Nominal coherence window in microseconds.
        coherence_us: u32,
    },

    /// Biological substrate (wetware, organoid intelligence, DishBrain-style
    /// biological neural networks). Warp factors are highly speculative.
    Biological,

    /// Heterogeneous stack (e.g., classical + neuromorphic co-processor).
    /// The order expresses preference: earlier substrates are tried first.
    Hybrid(Vec<SubstrateKind>),
}

impl SubstrateKind {
    /// Returns a short family label (`"classical"`, `"neuromorphic"`,
    /// `"quantum"`, `"biological"`, `"hybrid"`).
    pub fn family(&self) -> &'static str {
        match self {
            Self::ClassicalSilicon => "classical",
            Self::Neuromorphic => "neuromorphic",
            Self::Quantum { .. } => "quantum",
            Self::Biological => "biological",
            Self::Hybrid(_) => "hybrid",
        }
    }
}

/// Multiplicative cost/capability factors relative to classical silicon.
///
/// All multipliers use the convention **"classical baseline = 1.0"**. Values
/// less than 1.0 indicate the substrate is *cheaper* on that dimension; greater
/// than 1.0 indicate the substrate is *more expensive*.
///
/// # Example
///
/// A neuromorphic substrate might have `energy: 0.01` (100× cheaper) and
/// `time: 2.0` (2× slower) relative to classical. A quantum substrate might
/// have `branching: Some(1e6)` (a million-way superposition) where classical
/// has `None`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WarpFactors {
    /// Tempo multiplier — `0.1` means 10× faster, `10.0` means 10× slower.
    pub time: f64,

    /// Energy-per-op multiplier — `0.01` means 100× cheaper.
    pub energy: f64,

    /// How coordination cost scales with population size N.
    pub coordination: CoordinationScaling,

    /// Memory-per-op multiplier.
    pub memory: f64,

    /// Branching factor — `Some(k)` means k-way parallel branches are cheap
    /// (superposition, speculation). `None` means no native branching
    /// (classical case).
    pub branching: Option<f64>,
}

impl WarpFactors {
    /// The reference baseline: classical silicon with all factors at 1.0,
    /// linear coordination, and no native branching.
    pub fn classical_baseline() -> Self {
        Self {
            time: 1.0,
            energy: 1.0,
            coordination: CoordinationScaling::Linear,
            memory: 1.0,
            branching: None,
        }
    }
}

/// How the cost of coordinating N participants scales with N.
///
/// This directly bounds the maximum stable population size for a horizontal
/// Pneuma impl (see P6 horizontal composition theorem).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum CoordinationScaling {
    /// O(N) — gossip, pheromones, gradient fields. Scales to large
    /// populations.
    Linear,

    /// O(N log N) — hierarchical aggregation, tree overlays.
    NLogN,

    /// O(N²) — full pairwise messaging. Hard upper bound on population size.
    Quadratic,

    /// Entangled / superposed coordination with no classical complexity
    /// analog (quantum substrates).
    Entangled,
}

/// The binding resource constraint on a substrate.
///
/// Exactly one resource is the bottleneck at steady state; the other
/// dimensions have slack. This selects which physical limit the planner must
/// respect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "ceiling", rename_all = "snake_case")]
pub enum ResourceCeiling {
    /// Thermodynamic ceiling — max sustainable power draw in watts.
    /// Classical silicon, especially at scale.
    Thermodynamic {
        /// Maximum sustained power draw, in watts.
        max_watts: f64,
    },

    /// Coherence ceiling — maximum operation duration before decoherence.
    /// Quantum substrates.
    Coherence {
        /// Maximum coherent-operation duration, in microseconds.
        max_duration_us: u32,
    },

    /// Propagation ceiling — maximum radius within which signals stay useful.
    /// Biological substrates, also cellular-scale neuromorphic meshes.
    Propagation {
        /// Maximum useful signal radius, in meters.
        max_radius_m: f64,
    },
}
```

---

## 4. Error type

A single error type for all Pneuma operations. Uses `thiserror::Error` to match the style of `KernelError` in this crate. `#[non_exhaustive]` so new error classes can be added without breaking downstream matches.

```rust
// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors returned by [`Pneuma`] operations.
///
/// The variants are intentionally coarse-grained; implementations may attach
/// richer context by constructing via `PneumaError::transport(e)` style helpers
/// or by using the inner `String` payloads.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PneumaError {
    /// The underlying substrate is unavailable (down, disconnected,
    /// not initialized). Typically retriable.
    #[error("substrate unavailable: {0}")]
    SubstrateUnavailable(String),

    /// A safety shield (S in the RCS 7-tuple) rejected the signal or
    /// directive. Non-retriable at this boundary without modification.
    #[error("rejected by shield: {0}")]
    ShieldRejection(String),

    /// Transport-layer failure (NATS, SpacetimeDB, local channel, etc.).
    /// Retriability is transport-dependent; the string contains details.
    #[error("transport error: {0}")]
    Transport(String),

    /// The signal was syntactically or semantically invalid for this boundary
    /// (e.g., wrong payload type, missing required field, out-of-range value).
    /// Non-retriable without changing the signal.
    #[error("invalid signal: {0}")]
    InvalidSignal(String),
}

impl PneumaError {
    /// Convenience constructor for [`PneumaError::Transport`].
    pub fn transport(msg: impl Into<String>) -> Self {
        Self::Transport(msg.into())
    }

    /// Convenience constructor for [`PneumaError::SubstrateUnavailable`].
    pub fn substrate_unavailable(msg: impl Into<String>) -> Self {
        Self::SubstrateUnavailable(msg.into())
    }

    /// Convenience constructor for [`PneumaError::ShieldRejection`].
    pub fn shield_rejection(msg: impl Into<String>) -> Self {
        Self::ShieldRejection(msg.into())
    }

    /// Convenience constructor for [`PneumaError::InvalidSignal`].
    pub fn invalid_signal(msg: impl Into<String>) -> Self {
        Self::InvalidSignal(msg.into())
    }
}
```

---

## 5. The `Pneuma` trait

The canonical trait. Four methods; four associated types. A Pneuma impl is an object that knows how to carry signals across *exactly one* boundary.

**Why associated types (not generics):** an impl targets one specific boundary — a `LagoJournal` does not choose at call site whether to be L0→L1 or L1→L2. The signal, aggregate, and directive types are fixed for that impl. Associated types express "each impl has one choice" better than generics, which express "each call has one choice".

**Why no `self: &mut`:** most impls hold their state behind an `Arc<Mutex<_>>` or SpacetimeDB connection, so `&self` is sufficient. An impl that genuinely needs exclusive access can wrap itself. This keeps `Arc<dyn Pneuma<...>>` workable.

**Why `Option<Directive>` for `receive`:** non-blocking poll semantics. Async variants are an open question — see end of doc.

```rust
// ---------------------------------------------------------------------------
// Core Pneuma trait
// ---------------------------------------------------------------------------

/// Substrate-invariant inter-boundary substrate.
///
/// A `Pneuma` impl carries typed [`Signal`](Pneuma::Signal)s across a single
/// [`Boundary`], exposes the current [`Aggregate`](Pneuma::Aggregate) observation
/// to the upward side, and delivers [`Directive`](Pneuma::Directive)s downward.
///
/// # Associated types
///
/// - `B: Boundary` — which boundary this impl crosses. Compile-time tag.
/// - `Signal` — the upward-flowing payload (events, observations, beliefs).
/// - `Aggregate` — the upward-readable state (projection, population state).
/// - `Directive` — the downward-flowing control input.
///
/// All three payload types must be `Send + Sync + 'static` so they can traverse
/// thread and task boundaries.
///
/// # Example
///
/// A minimal in-memory impl for unit testing:
///
/// ```rust,ignore
/// use aios_protocol::pneuma::*;
///
/// // See MockPneuma later in this spec.
/// let p = MockPneuma::<L0ToL1, String, Vec<String>, ()>::new();
/// p.emit("hello".to_string()).unwrap();
/// assert_eq!(p.aggregate(), vec!["hello".to_string()]);
/// ```
pub trait Pneuma: Send + Sync {
    /// The boundary this impl crosses (zero-sized compile-time tag).
    type B: Boundary;

    /// Upward-flowing typed payload crossing the boundary.
    ///
    /// For `L0ToL1`, this is typically `crate::event::EventKind`. For
    /// `D0ToD1`, this is typically a `PlexusSignal` variant.
    type Signal: Send + Sync + 'static;

    /// What the upward side observes (the `h` output in the RCS 7-tuple).
    ///
    /// For `L1ToL2`, this is typically `crate::state::AgentStateVector` or
    /// autonomic's `HomeostaticState`.
    type Aggregate: Send + Sync + 'static;

    /// What the upward side sends downward (the `U` input in the RCS 7-tuple).
    ///
    /// For `L1ToL2`, this is typically `crate::mode::GatingProfile`.
    type Directive: Send + Sync + 'static;

    /// Emit a signal upward through the substrate.
    ///
    /// Implementations SHOULD apply decay, shields, and shaping before
    /// accepting the signal. Rejections are returned as [`PneumaError`].
    fn emit(&self, signal: Self::Signal) -> Result<(), PneumaError>;

    /// Read the current aggregate observation.
    ///
    /// This is a pure read — it MUST NOT mutate substrate state.
    /// Implementations SHOULD return a snapshot, not a live view.
    fn aggregate(&self) -> Self::Aggregate;

    /// Poll for a pending directive, if any.
    ///
    /// Non-blocking. Returns `None` if no directive is currently pending.
    /// Calling `receive` MAY consume the directive from an internal queue;
    /// callers that need idempotent reads should wrap the impl.
    fn receive(&self) -> Option<Self::Directive>;

    /// Describe the substrate this impl runs on.
    ///
    /// Used by planners and schedulers to reason about scaling laws and
    /// resource ceilings. The profile SHOULD be approximately constant over
    /// the lifetime of the impl (slow-changing runtime introspection is
    /// acceptable; per-call variation is not).
    fn substrate(&self) -> SubstrateProfile;
}
```

---

## 6. Example impl — `MockPneuma`

A minimal, configurable, in-memory implementation used across unit tests. It:
- Buffers emitted signals in a `Mutex<Vec<_>>`.
- Takes configurable functions for `aggregate` and `receive`.
- Returns a caller-supplied `SubstrateProfile`.

Because `MockPneuma` must be generic over `B: Boundary`, `S`, `A`, `D`, and carry function pointers, the impl is slightly busier than production code — but it covers all trait surfaces without pulling in real transport.

```rust
// ---------------------------------------------------------------------------
// MockPneuma — testing substrate
// ---------------------------------------------------------------------------

/// A configurable in-memory [`Pneuma`] impl for unit tests.
///
/// - `emit` pushes onto an internal `Vec<S>`.
/// - `aggregate` returns a caller-supplied snapshot (or a clone of the
///   emitted buffer if the snapshot fn is `None`).
/// - `receive` pops from a caller-populated directive queue.
/// - `substrate` returns a caller-supplied profile.
///
/// # Example
///
/// ```rust,ignore
/// use aios_protocol::pneuma::*;
///
/// let p: MockPneuma<L0ToL1, i32, Vec<i32>, ()> = MockPneuma::new(
///     SubstrateProfile {
///         kind: SubstrateKind::ClassicalSilicon,
///         warp_factors: WarpFactors::classical_baseline(),
///         ceiling: ResourceCeiling::Thermodynamic { max_watts: 10.0 },
///     },
/// );
/// p.emit(1).unwrap();
/// p.emit(2).unwrap();
/// assert_eq!(p.aggregate(), vec![1, 2]);
/// ```
pub struct MockPneuma<B, S, A, D>
where
    B: Boundary,
    S: Send + Sync + 'static + Clone,
    A: Send + Sync + 'static,
    D: Send + Sync + 'static,
{
    buffer: std::sync::Mutex<Vec<S>>,
    directives: std::sync::Mutex<std::collections::VecDeque<D>>,
    aggregate_fn: std::sync::Mutex<
        Option<Box<dyn Fn(&[S]) -> A + Send + Sync + 'static>>,
    >,
    default_aggregate: std::sync::Mutex<Option<A>>,
    substrate: SubstrateProfile,
    _boundary: std::marker::PhantomData<fn() -> B>,
}

impl<B, S, A, D> MockPneuma<B, S, A, D>
where
    B: Boundary,
    S: Send + Sync + 'static + Clone,
    A: Send + Sync + 'static,
    D: Send + Sync + 'static,
{
    /// Create a new `MockPneuma` with the given substrate profile, no
    /// aggregator function, and no pending directives.
    pub fn new(substrate: SubstrateProfile) -> Self {
        Self {
            buffer: std::sync::Mutex::new(Vec::new()),
            directives: std::sync::Mutex::new(std::collections::VecDeque::new()),
            aggregate_fn: std::sync::Mutex::new(None),
            default_aggregate: std::sync::Mutex::new(None),
            substrate,
            _boundary: std::marker::PhantomData,
        }
    }

    /// Install an aggregator function that maps the emitted buffer to an
    /// `A`. Takes precedence over `set_aggregate`.
    pub fn with_aggregator<F>(self, f: F) -> Self
    where
        F: Fn(&[S]) -> A + Send + Sync + 'static,
    {
        *self.aggregate_fn.lock().unwrap() = Some(Box::new(f));
        self
    }

    /// Set a fixed aggregate value to return when no aggregator is installed.
    pub fn set_aggregate(&self, value: A) {
        *self.default_aggregate.lock().unwrap() = Some(value);
    }

    /// Push a directive onto the outbound queue. Consumed by `receive`.
    pub fn push_directive(&self, directive: D) {
        self.directives.lock().unwrap().push_back(directive);
    }

    /// Number of emitted signals currently buffered.
    pub fn emitted_count(&self) -> usize {
        self.buffer.lock().unwrap().len()
    }

    /// Snapshot of the emitted buffer (cloned).
    pub fn emitted(&self) -> Vec<S> {
        self.buffer.lock().unwrap().clone()
    }
}

impl<B, S, A, D> Pneuma for MockPneuma<B, S, A, D>
where
    B: Boundary,
    S: Send + Sync + 'static + Clone,
    A: Send + Sync + 'static + Clone,
    D: Send + Sync + 'static,
{
    type B = B;
    type Signal = S;
    type Aggregate = A;
    type Directive = D;

    fn emit(&self, signal: Self::Signal) -> Result<(), PneumaError> {
        self.buffer
            .lock()
            .map_err(|e| PneumaError::transport(format!("mock mutex poisoned: {e}")))?
            .push(signal);
        Ok(())
    }

    fn aggregate(&self) -> Self::Aggregate {
        // Preference order: aggregator fn > default value > panic with a
        // descriptive message (mock-only behavior; production impls must
        // always return a meaningful aggregate).
        let buf = self.buffer.lock().unwrap();
        let agg_fn = self.aggregate_fn.lock().unwrap();
        if let Some(f) = agg_fn.as_ref() {
            return f(&buf);
        }
        drop(agg_fn);
        let default = self.default_aggregate.lock().unwrap();
        if let Some(a) = default.as_ref() {
            return a.clone();
        }
        panic!(
            "MockPneuma::aggregate called with no aggregator and no default; \
             call with_aggregator() or set_aggregate() first"
        );
    }

    fn receive(&self) -> Option<Self::Directive> {
        self.directives.lock().unwrap().pop_front()
    }

    fn substrate(&self) -> SubstrateProfile {
        self.substrate.clone()
    }
}
```

---

## 7. Unit tests

Coverage targets:
1. Axis markers return the expected strings.
2. Boundary markers return the expected names and axis delegation works.
3. `SubstrateProfile` serializes and deserializes losslessly.
4. `WarpFactors::classical_baseline()` returns the documented identity values.
5. `MockPneuma` emit / aggregate / receive roundtrip.
6. `MockPneuma` aggregator function overrides default aggregate.
7. `PneumaError` variants format correctly via `Display`.
8. Zero-sized boundary markers are actually zero-sized.

```rust
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Axis markers ------------------------------------------------------

    #[test]
    fn axis_names() {
        assert_eq!(Vertical::name(), "vertical");
        assert_eq!(Horizontal::name(), "horizontal");
    }

    // --- Boundary markers --------------------------------------------------

    #[test]
    fn vertical_boundary_names() {
        assert_eq!(L0ToL1::boundary_name(), "L0→L1");
        assert_eq!(L1ToL2::boundary_name(), "L1→L2");
        assert_eq!(L2ToL3::boundary_name(), "L2→L3");
        assert_eq!(L3ToExternal::boundary_name(), "L3→External");
    }

    #[test]
    fn horizontal_boundary_names() {
        assert_eq!(D0ToD1::boundary_name(), "depth-0 → depth-1");
        assert_eq!(D1ToD2::boundary_name(), "depth-1 → depth-2");
        assert_eq!(D2ToD3::boundary_name(), "depth-2 → depth-3");
    }

    #[test]
    fn boundary_axis_delegation() {
        // Vertical boundaries report "vertical".
        assert_eq!(L0ToL1::axis_name(), "vertical");
        assert_eq!(L1ToL2::axis_name(), "vertical");
        assert_eq!(L2ToL3::axis_name(), "vertical");
        assert_eq!(L3ToExternal::axis_name(), "vertical");

        // Horizontal boundaries report "horizontal".
        assert_eq!(D0ToD1::axis_name(), "horizontal");
        assert_eq!(D1ToD2::axis_name(), "horizontal");
        assert_eq!(D2ToD3::axis_name(), "horizontal");
    }

    #[test]
    fn boundary_markers_are_zero_sized() {
        use std::mem::size_of;
        assert_eq!(size_of::<L0ToL1>(), 0);
        assert_eq!(size_of::<L1ToL2>(), 0);
        assert_eq!(size_of::<L2ToL3>(), 0);
        assert_eq!(size_of::<L3ToExternal>(), 0);
        assert_eq!(size_of::<D0ToD1>(), 0);
        assert_eq!(size_of::<D1ToD2>(), 0);
        assert_eq!(size_of::<D2ToD3>(), 0);
        assert_eq!(size_of::<Vertical>(), 0);
        assert_eq!(size_of::<Horizontal>(), 0);
    }

    #[test]
    fn boundary_markers_are_send_sync() {
        // Compile-time check — if a marker is not Send+Sync, this fails to
        // compile. The assertion itself is trivial.
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<L0ToL1>();
        assert_send_sync::<L1ToL2>();
        assert_send_sync::<L2ToL3>();
        assert_send_sync::<L3ToExternal>();
        assert_send_sync::<D0ToD1>();
        assert_send_sync::<D1ToD2>();
        assert_send_sync::<D2ToD3>();
    }

    // --- SubstrateProfile serde -------------------------------------------

    #[test]
    fn substrate_profile_serde_roundtrip() {
        let profile = SubstrateProfile {
            kind: SubstrateKind::Quantum {
                qubits: 127,
                coherence_us: 100,
            },
            warp_factors: WarpFactors {
                time: 0.5,
                energy: 2.0,
                coordination: CoordinationScaling::Entangled,
                memory: 0.25,
                branching: Some(1e6),
            },
            ceiling: ResourceCeiling::Coherence {
                max_duration_us: 100,
            },
        };

        let json = serde_json::to_string(&profile).unwrap();
        let back: SubstrateProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(profile, back);
    }

    #[test]
    fn substrate_kind_family_labels() {
        assert_eq!(SubstrateKind::ClassicalSilicon.family(), "classical");
        assert_eq!(SubstrateKind::Neuromorphic.family(), "neuromorphic");
        assert_eq!(
            SubstrateKind::Quantum {
                qubits: 2,
                coherence_us: 10
            }
            .family(),
            "quantum"
        );
        assert_eq!(SubstrateKind::Biological.family(), "biological");
        assert_eq!(
            SubstrateKind::Hybrid(vec![SubstrateKind::ClassicalSilicon]).family(),
            "hybrid"
        );
    }

    #[test]
    fn warp_factors_classical_baseline_is_identity() {
        let w = WarpFactors::classical_baseline();
        assert!((w.time - 1.0).abs() < 1e-12);
        assert!((w.energy - 1.0).abs() < 1e-12);
        assert!((w.memory - 1.0).abs() < 1e-12);
        assert_eq!(w.coordination, CoordinationScaling::Linear);
        assert!(w.branching.is_none());
    }

    // --- PneumaError -------------------------------------------------------

    #[test]
    fn pneuma_error_display() {
        let e = PneumaError::transport("nats down");
        assert_eq!(format!("{e}"), "transport error: nats down");

        let e = PneumaError::shield_rejection("budget exhausted");
        assert_eq!(format!("{e}"), "rejected by shield: budget exhausted");

        let e = PneumaError::substrate_unavailable("not initialized");
        assert_eq!(format!("{e}"), "substrate unavailable: not initialized");

        let e = PneumaError::invalid_signal("missing field 'domain'");
        assert_eq!(
            format!("{e}"),
            "invalid signal: missing field 'domain'"
        );
    }

    // --- MockPneuma --------------------------------------------------------

    fn classical_profile() -> SubstrateProfile {
        SubstrateProfile {
            kind: SubstrateKind::ClassicalSilicon,
            warp_factors: WarpFactors::classical_baseline(),
            ceiling: ResourceCeiling::Thermodynamic { max_watts: 10.0 },
        }
    }

    #[test]
    fn mock_pneuma_emit_aggregate_roundtrip() {
        let p: MockPneuma<L0ToL1, i32, Vec<i32>, ()> =
            MockPneuma::new(classical_profile())
                .with_aggregator(|buf: &[i32]| buf.to_vec());

        p.emit(1).unwrap();
        p.emit(2).unwrap();
        p.emit(3).unwrap();

        assert_eq!(p.emitted_count(), 3);
        assert_eq!(p.aggregate(), vec![1, 2, 3]);
    }

    #[test]
    fn mock_pneuma_default_aggregate() {
        let p: MockPneuma<L1ToL2, &'static str, i32, ()> =
            MockPneuma::new(classical_profile());
        p.set_aggregate(42);

        p.emit("x").unwrap();
        assert_eq!(p.aggregate(), 42);
    }

    #[test]
    fn mock_pneuma_receive_fifo() {
        let p: MockPneuma<L1ToL2, (), (), &'static str> =
            MockPneuma::new(classical_profile());
        p.set_aggregate(());

        p.push_directive("first");
        p.push_directive("second");

        assert_eq!(p.receive(), Some("first"));
        assert_eq!(p.receive(), Some("second"));
        assert_eq!(p.receive(), None);
    }

    #[test]
    fn mock_pneuma_substrate_passthrough() {
        let p: MockPneuma<D0ToD1, (), (), ()> =
            MockPneuma::new(classical_profile());
        let profile = p.substrate();
        assert_eq!(profile.kind.family(), "classical");
    }

    #[test]
    fn mock_pneuma_trait_object_is_usable() {
        // Verify Pneuma is dyn-compatible for its intended boundary types.
        let p: MockPneuma<L0ToL1, i32, i32, ()> =
            MockPneuma::new(classical_profile());
        p.set_aggregate(0);

        let dyn_p: &dyn Pneuma<
            B = L0ToL1,
            Signal = i32,
            Aggregate = i32,
            Directive = (),
        > = &p;

        dyn_p.emit(7).unwrap();
        assert_eq!(dyn_p.aggregate(), 0);
        assert!(dyn_p.receive().is_none());
    }

    #[test]
    fn mock_pneuma_aggregator_sees_emitted_buffer() {
        let p: MockPneuma<D0ToD1, u32, u32, ()> =
            MockPneuma::new(classical_profile())
                .with_aggregator(|buf: &[u32]| buf.iter().sum());
        for i in 1..=5 {
            p.emit(i).unwrap();
        }
        assert_eq!(p.aggregate(), 15);
    }
}
```

---

## 8. `lib.rs` integration

The following two edits land the module in the crate:

```rust
// In core/life/crates/aios/aios-protocol/src/lib.rs

// Add to the module overview:
// - [`pneuma`] — Substrate-invariant inter-boundary observation/control flow

pub mod pneuma; // add alongside `pub mod rcs;`

// Add to the re-export block:
pub use pneuma::{
    Axis, Boundary, CoordinationScaling, D0ToD1, D1ToD2, D2ToD3, Horizontal, L0ToL1, L1ToL2,
    L2ToL3, L3ToExternal, MockPneuma, Pneuma, PneumaError, ResourceCeiling, SubstrateKind,
    SubstrateProfile, Vertical, WarpFactors,
};
```

No `Cargo.toml` changes are required — `serde`, `serde_json`, and `thiserror` are already workspace dependencies.

---

## 9. Open design questions

These are review-gate questions. The spec above picks defaults; each question may reopen the spec after discussion.

1. **Should `Pneuma` methods be async?**
   Current: synchronous with non-blocking `receive`. Rationale: `lago-journal` and `autonomic-controller` already expose sync surfaces; forcing async propagates runtime coupling to `aios-protocol`, which is deliberately runtime-free. Alternative: add a parallel `AsyncPneuma` trait gated on a feature flag, deferring the choice to impl crates. **Proposed decision: keep sync for now; revisit in Phase 3 after horizontal transport is real.**

2. **How should backpressure be signaled?**
   Current: `emit` returns `Result<(), PneumaError>` with no explicit "try again later" variant. A busy substrate must either buffer silently or return `Transport("full")`. Should we add a dedicated `PneumaError::Backpressure { retry_after_ms }` variant, or a `try_emit` that returns `Result<EmitOutcome, _>` where `EmitOutcome` distinguishes `Accepted`, `Queued`, `Dropped`? **Open.**

3. **Should we require `Clone` on associated types?**
   Current: no. `Send + Sync + 'static` only. `Aggregate` is often expensive to clone (large state vectors). But unit-testing impls and multi-consumer fanout both want clone. Alternative: require `Clone` on `Signal` (cheap) and `Directive` (cheap), leave `Aggregate` unconstrained. **Proposed decision: require `Clone` on `Signal` and `Directive`; leave `Aggregate` unconstrained.** (Not yet reflected in the trait above; deferred for discussion.)

4. **Per-boundary feature flags?**
   Current: all boundaries are always compiled in. A `no_default_features` caller gets everything. Since boundary markers are ZSTs and the trait is trivial, there is ~zero cost. Alternative: gate horizontal boundaries behind `feature = "horizontal"` for crates that only ever do vertical work. **Proposed decision: no feature flags until we measure a real cost.**

5. **Substrate runtime introspection vs compile-time typing?**
   Current: `SubstrateProfile` is runtime data returned from `substrate()`. Alternative: `type Substrate: SubstrateTrait` as a fourth associated type so the planner has compile-time substrate specialization. Runtime wins for hybrid/heterogeneous deployments; compile-time wins for zero-cost dispatch. **Open — likely resolved by a `SubstrateKind` enum + runtime probe, as currently specified.**

6. **Should `receive` return `Vec<Directive>` or `Option<Directive>`?**
   Current: `Option`. Bulk receive (draining a queue) would be `while let Some(d) = p.receive() {}`. Alternative: `fn drain(&self) -> Vec<Self::Directive>` for batch processing. **Proposed decision: add `drain` as a provided method with a default `Option`-loop implementation in a later revision.**

7. **Should `emit` consume or borrow the signal?**
   Current: `fn emit(&self, signal: Self::Signal)` — consumes. This matches `EventEnvelope` emission in `lago-journal`. Alternative: `fn emit(&self, signal: &Self::Signal)` — borrows; impls that need ownership call `.clone()` internally. Consuming is more ergonomic for producers; borrowing is more flexible for fanout. **Proposed decision: keep consuming. Fanout is the impl's problem.**

8. **Should `Signal`, `Aggregate`, `Directive` require `Debug`?**
   Current: no. `Debug` is useful for logs and tracing. Alternative: require `Debug` (ergonomic for `tracing`, `format!`), or make it a separate `PneumaDebug` marker trait for impls that opt in. **Proposed decision: require `Debug` on all three associated types.** (Not yet reflected in the trait above.)

9. **Should `substrate()` be `fn substrate(&self) -> &SubstrateProfile` (borrow) instead?**
   Current: clones. Rationale: planners sometimes want to move the profile into a dedicated data structure. Alternative: return `Cow<'_, SubstrateProfile>` to let impls choose. **Proposed decision: keep `-> SubstrateProfile` and rely on cheap clone of small enum/struct.**

10. **Should there be a `PneumaExt` trait with combinators?**
    E.g., `map_signal`, `filter`, `compose`, `instrument`. These mirror `Stream` combinators and would reduce boilerplate at call sites. **Proposed decision: defer until three or more real impls exist; combinators should be discovered, not invented.**

---

## Appendix A — Full module file (copy-paste target)

For reviewers: the sections above, in order, form a single `pneuma.rs` file. The assembly order is:

1. Header docstring + imports (§0).
2. Axis markers (§1).
3. Boundary markers (§2).
4. Substrate metadata (§3).
5. `PneumaError` (§4).
6. `Pneuma` trait (§5).
7. `MockPneuma` (§6).
8. `#[cfg(test)] mod tests` (§7).

Once landed, proceed with the retrofit sequence documented in `pneuma-plexus-architecture.md` §Sequencing Phase 2 (lago-journal → autonomic-controller → egri → bstack-policy).
