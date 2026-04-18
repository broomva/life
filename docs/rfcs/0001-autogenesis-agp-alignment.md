# RFC 0001 — Autogenesis Protocol (AGP) Alignment

**Status:** Draft
**Created:** 2026-04-18
**Author:** broomva (carlosdavidescobar)
**Related paper:** [Autogenesis: A Self-Evolving Agent Protocol](https://arxiv.org/abs/2604.15034) (W. Zhang, NTU + Skywork AI, Apr 2026)
**Related spec:** `docs/specs/pneuma-trait-surface.md`
**Related synthesis:** `broomva/research/notes/2026-04-18-autogenesis-agp-rcs-life-alignment-synthesis.md`
**Supersedes / depends on:** none (new)

---

## Summary

Autogenesis Protocol (AGP) is the protocol-level formalization of an architectural pattern the self-evolving-agents field has been converging on for three years. It defines two layers: **RSPL** (five typed versioned resource entities — Prompt, Agent, Tool, Environment, Memory) and **SEPL** (five-operator algebra — Reflect, Select, Improve, Evaluate, Commit). AGP's near-isomorphic relationship to our existing Pneuma trait family and autoany EGRI loop means Life is already ~80% of the way there at the architectural level. This RFC proposes the remaining 20% as a four-phase sequenced implementation plan that minimizes coupling risk and respects the workspace's stability-budget discipline.

## Motivation

Three structural gaps in Life's current implementation map directly to concepts AGP defines cleanly:

1. **No first-class learnability mask.** Life has implicit mutability via Rust `&mut` and `Proposer::propose()` discretion in `autoany-core`. There is no compile-time marker saying "this resource participates in SEPL evolution." This blocks auditable self-modification and makes EGRI Law 3 (Immutable Evaluator) a runtime convention rather than a type-level invariant.
2. **EGRI loop lacks an explicit Reflect operator.** `autoany-core::loop_engine::EgriLoop` has propose → execute → evaluate → select → promote. There is no ρ (Reflect) step that consumes execution traces and produces hypotheses. `StrategyReport` exists as a design artifact but is not wired. Vigil is now shipped (v0.2.0) and emits contract-derived OTel spans — the trace substrate exists; the consumer does not.
3. **Pneuma has a trait-surface spec (`docs/specs/pneuma-trait-surface.md`) but no implementation.** The spec is complete and mechanical-port-ready. Without landing it, RSPL and Evolvable cannot anchor to the broader substrate-invariant abstraction, and we risk re-inventing the boundary/axis machinery inside `aios-protocol::rspl`.

AGP gives us the type signatures and operator algebra; we have the stability theorems (λᵢ > 0 per level), the homeostasis controller (Autonomic), and the event-sourced substrate (Lago) that AGP lacks. Closing the gap is small, crisp, and mutually reinforcing: AGP types + RCS rate bounds = a publishable protocol.

## Non-goals

- **Not a new evolution engine.** EGRI stays the execution model; SEPL is its type signature.
- **Not a rewrite of Pneuma.** The existing spec at `docs/specs/pneuma-trait-surface.md` is authoritative; this RFC adds the depth-0 horizontal-0 RSPL layer on top.
- **Not a benchmark chase.** We are not targeting GAIA numbers in this work. The value is protocol-level auditability and the RCS ↔ AGP reciprocal claim.
- **Not a breaking change.** No existing public API is modified; everything proposed is additive.

## Glossary

| Term | Meaning |
|---|---|
| RSPL | Resource Substrate Protocol Layer — AGP's five-type resource taxonomy |
| SEPL | Self-Evolution Protocol Layer — AGP's five-operator algebra |
| ρ σ ι ε κ | Reflect, Select, Improve, Evaluate, Commit (SEPL operators) |
| 𝒵 ℋ 𝒟 𝒢 𝒮 | Trace, Hypothesis, Modification, Objective, Evaluation state spaces |
| `g_v` | Learnability mask — bit indicating whether a resource participates in 𝒱_evo |
| Pneuma | Substrate-invariant trait family for inter-boundary flow (see `docs/specs/pneuma-trait-surface.md`) |
| EGRI | Evaluator-Governed Recursive Improvement (autoany Level 2 meta-controller) |

## Design

### Architectural framing

```
         ┌──────────────────────────────────────────────┐
         │   SEPL operator algebra (ρ σ ι ε κ)          │ ← autoany-core::loop_engine (+ reflect)
         └──────────────────────────────────────────────┘
                              │ operates on
                              ▼
         ┌──────────────────────────────────────────────┐
         │   Evolvable<T> trait + g_v learnability mask │ ← aios-protocol::rspl::evolvable
         └──────────────────────────────────────────────┘
                              │ refines
                              ▼
         ┌──────────────────────────────────────────────┐
         │   RSPL five entity types                     │
         │   Prompt | Agent | Tool | Env | Memory       │ ← aios-protocol::rspl
         └──────────────────────────────────────────────┘
                              │ impl
                              ▼
         ┌──────────────────────────────────────────────┐
         │   Pneuma trait family                        │
         │   Axis | Boundary | Pneuma                   │ ← aios-protocol::pneuma
         └──────────────────────────────────────────────┘
```

Four layers, bottom-up implementation order. Each layer is independently testable; each higher layer consumes the lower layer via typed interfaces; nothing below is breaking-change.

### Layer 1 — Pneuma trait surface (`aios-protocol::pneuma`)

**Status:** Spec complete at `docs/specs/pneuma-trait-surface.md` (1106 lines). Implementation not yet started.

**Scope:** Mechanical port of the spec into `core/life/crates/aios/aios-protocol/src/pneuma.rs` plus re-exports from `lib.rs`. Spec explicitly states "Every symbol in this spec is intended to be ported verbatim."

**Contents:** Axis markers (Vertical, Horizontal), Boundary markers (L0ToL1, L1ToL2, L2ToL3, L3ToExternal, D0ToD1, D1ToD2, D2ToD3), SubstrateProfile metadata (SubstrateKind, WarpFactors, ResourceCeiling, CoordinationRegime), `trait Pneuma` with associated types Signal/Aggregate/Directive, `trait PneumaError`, `trait SubstrateAware`.

**Dependencies:** `serde`, `thiserror`, `std::fmt` only. No new crate deps.

**Tests:** The spec lists 14 test scenarios (trait object safety, send/sync bounds, boundary-to-axis binding, substrate kind defaults, warp factor computation, etc.). All should land with the port.

**Sequencing:** Land this first as an independent PR before anything below.

### Layer 2 — RSPL five entity types (`aios-protocol::rspl`)

**Status:** Not yet designed in code. This RFC proposes the shape.

**Scope:** A new module `core/life/crates/aios/aios-protocol/src/rspl.rs` (or submodule directory) defining the five RSPL entity types from AGP §3.1.1 as Pneuma impls at depth 0.

**Contents:**

```rust
//! RSPL — depth-0 horizontal-0 instantiation of Pneuma.
//!
//! The five entity types defined in AGP (Autogenesis Protocol, arXiv:2604.15034)
//! are concrete Pneuma impls for an individual LLM-based agent. Each type
//! names a boundary crossed during agent operation.

use crate::pneuma::*;

/// Prompt — instruction or context descending from controller to plant.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    pub description: String,
    pub body: String,
    pub version: String,
    pub learnability: Learnability,   // g_v
    pub metadata: serde_json::Value,
}

// Mirror struct for Agent, Tool, Environment, Memory with per-type φ semantics.
```

Each of the five types `impl Pneuma` with the appropriate boundary marker (Prompt → L1ToL0, Memory → L0ToL1, etc.) and with a shared `impl Evolvable`.

**Dependencies:** `aios-protocol::pneuma` (Layer 1).

**Tests:** Round-trip serde, boundary type-safety (compile-fail tests showing L0→L1 impl cannot be wired where L1→L0 is expected), Pneuma trait object safety.

### Layer 3 — Evolvable trait + learnability mask (`aios-protocol::rspl::evolvable`)

**Status:** Not yet designed in code. Proposed here.

**Scope:** A trait marker + type encoding AGP's `g_v ∈ {0,1}` bit, plus a richer `MutationSchema` for declaring *what shape* of mutation is legal on each resource.

**Contents:**

```rust
/// Learnability annotation — whether a resource participates in the
/// SEPL evolvable variable set 𝒱_evo.
///
/// AGP defines `g_v ∈ {0,1}` per entity; we generalize to include a
/// mutation-shape schema for richer auditability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Learnability {
    /// Frozen — not in 𝒱_evo. Required for evaluators (EGRI Law 3).
    Frozen,
    /// Evolvable with an unrestricted mutation space.
    Open,
    /// Evolvable with a schema-constrained mutation space.
    Constrained(MutationSchemaId),
}

pub trait Evolvable {
    fn learnability(&self) -> Learnability;
    fn mutation_schema(&self) -> Option<MutationSchema> {
        None
    }
}

/// Compile-time enforcement of EGRI Law 3 (Immutable Evaluator).
pub trait ImmutableResource: Evolvable {
    // sealed: auto-implemented when Learnability::Frozen
}
```

**Dependencies:** `aios-protocol::rspl` (Layer 2).

**Tests:** Compile-fail test that an `impl Evolvable` returning `Frozen` cannot be passed to SEPL's `ι` (Improve); round-trip of `Learnability` through serde; interaction with the stability-budget cost accounting (a `Frozen` resource contributes zero to `L_d · η`).

### Layer 4 — SEPL operator algebra (`autoany-core::meta_controller`)

**Status:** Partially present as EGRI (`loop_engine.rs` has σ, ι, ε, κ equivalents). Missing: explicit ρ Reflect, and a typed `MetaController<L, R>` trait.

**Scope:** A new trait in `core/autoany/autoany-core/src/meta_controller.rs` that gives EGRI's existing propose/execute/evaluate/select/promote cycle a typed operator signature:

```rust
/// SEPL operator algebra as a typed trait.
///
/// Refines existing `EgriLoop` with explicit Reflect (ρ) and typed state spaces.
pub trait MetaController<L, R>
where
    L: aios_protocol::rcs::Level,
    R: aios_protocol::rspl::Evolvable,
{
    type Trace;         // 𝒵
    type Hypothesis;    // ℋ
    type Modification;  // 𝒟
    type Objective;     // 𝒢
    type Evaluation;    // 𝒮

    fn reflect(&self, trace: &Self::Trace, state: &R) -> Vec<Self::Hypothesis>;
    fn select(&self, state: &R, hypotheses: &[Self::Hypothesis]) -> Vec<Self::Modification>;
    fn improve(&self, state: &R, mods: &[Self::Modification]) -> R;
    fn evaluate(&self, candidate: &R, objective: &Self::Objective) -> Self::Evaluation;
    fn commit(&self, candidate: R, eval: &Self::Evaluation) -> CommitDecision<R>;
}
```

And a default `ReflectOperator` impl that consumes Vigil OTel span output (via a `Subscriber`) and produces natural-language hypotheses through an LLM call.

**Dependencies:** `aios-protocol::rspl` (Layer 2, for `Evolvable` + `Learnability`), Vigil (for trace stream), existing `autoany-core::loop_engine` (to be refactored to impl this trait).

**Tests:** EgriLoop can be type-viewed as a `MetaController<L2, _>`; Reflect consumes a canned trace + produces hypotheses; integration test that the existing loop is preserved behaviorally under the new signature.

### Commit gate and stability budget coupling

A key insight from the AGP ↔ RCS mapping (see synthesis note) is that AGP's κ (Commit) gate is heuristic, while RCS provides a quantitative stability budget `λᵢ > 0` per level. We should couple them:

```rust
pub enum CommitDecision<R> {
    Accepted(R, StabilityBudgetUpdate),
    RejectedUnsafe(UnsafeReason),
    RejectedBudget(BudgetViolation),  // new — λ_i would go ≤ 0
}
```

Every Commit consumes budget via `L_d · η` (adaptation cost); the Autonomic controller publishes the remaining margin; the MetaController must reject if commit would violate `λ_i > 0`. This is the control-theoretic grounding AGP lacks, contributed back from RCS. Formalization belongs in RCS Paper P2.

## Alternatives considered

**A. Do nothing — keep AGP as an external reference, don't add Pneuma/RSPL modules.**
Rejected because: (1) EGRI's missing Reflect operator is a real gap that Vigil's trace stream can now fill; (2) the learnability mask is a small, high-value addition that removes a runtime convention in favor of a type-level invariant; (3) the Pneuma trait surface spec already exists and is idle.

**B. Combined single PR (Pneuma + RSPL + Evolvable + MetaController) in one landing.**
Rejected because: the workspace has 37 crates, 1077 tests, and a strict quality bar. A 2000+ LOC PR touching aios-protocol (depended on by arcan, lago, autonomic, praxis) is an unreasonable review burden. Four layered PRs in dependency order is cleaner.

**C. Skip Pneuma, implement RSPL + Evolvable directly.**
Rejected because: the Pneuma spec is authored, tested, and ready. Skipping it would force RSPL to re-invent the Axis/Boundary/SubstrateProfile machinery, creating two parallel abstractions that drift. The spec explicitly says "RSPL is the depth-0 horizontal-0 Pneuma instantiation."

**D. Adopt AGP's operator algebra verbatim without the Evolvable mask.**
Rejected because: the `g_v` bit is the smallest, most type-safe way to encode EGRI Law 3 (Immutable Evaluator). Without it, Law 3 remains a runtime convention, and every new resource type risks violating it.

## Dependencies

### Architectural

```
Layer 1: Pneuma        (no dependency)
Layer 2: RSPL          (depends on Layer 1)
Layer 3: Evolvable     (depends on Layer 2)
Layer 4: MetaController (depends on Layer 3 + Vigil)
```

### Crate

- `aios-protocol` — primary (all four layers live here or re-export from here)
- `autoany-core` — Layer 4 implementation
- `vigil` — Layer 4 consumes OTel span output (already shipped v0.2.0)
- `autonomic-core` — Layer 4 coupling to stability budget (already shipped)

### Data / schema

None. All additions are type-level; no schema migrations.

### CI / deployment

- Pre-commit: `make smoke` continues to gate
- Pre-push: `make check` runs format + clippy + test; must stay green
- Each layer's PR adds its own tests; full `cargo test --workspace` must pass
- No release-train impact; Layer 4 can bump `aios-protocol` to 0.3.0 if any public API shift materializes

## Migration

Zero. Everything additive. Existing code paths unchanged.

## Security

No new attack surface. RSPL's five types encapsulate concepts that already exist in the codebase (tools, environments, memory). The Evolvable trait *reduces* attack surface by making immutability type-enforceable where it was runtime-convention.

The commit-gate coupling to stability budget (`RejectedBudget`) introduces a new failure mode for self-evolution — evolution can be blocked by budget exhaustion. This is **desirable** safety behavior; operators must understand it.

## Rollout plan

| Phase | Deliverable | PR size | Est. effort | Blocks next |
|---|---|---|---|---|
| P1 | Layer 1: Pneuma trait surface in aios-protocol | ~800 LOC + 300 LOC tests | 2-3 days | Yes |
| P2 | Layer 2: RSPL five types as Pneuma impls | ~400 LOC + 200 LOC tests | 1-2 days | Yes |
| P3 | Layer 3: Evolvable trait + Learnability enum | ~150 LOC + 150 LOC tests | 1 day | No (P4 depends) |
| P4 | Layer 4: MetaController trait + Reflect operator | ~600 LOC + 400 LOC tests | 3-5 days | — |

**Milestone**: `aios-protocol 0.3.0` release after P2 lands (Pneuma + RSPL both public).

**P5 research-track (parallel)**: RCS Paper P2 (EGRI meta-controller) cites AGP and AgentOrchestra v4 explicitly, positions RCS stability budget as the rate bound AGP's operator algebra lacks. No code dependency.

## Open questions

- **Async in Pneuma trait.** The spec deliberately omits async methods (flagged as "open question"). This blocks wiring Pneuma to arcan's async runtime cleanly. Resolution before P1 lands.
- **Backpressure / flow control.** Also flagged in Pneuma spec. Needs a concrete answer before P2 (RSPL emit/aggregate semantics).
- **Evolvable granularity.** Is `Learnability::Constrained(MutationSchemaId)` adequate, or do we need a full `MutationAlgebra` type? Decided in P3 design review.
- **Reflect operator default.** Should the LLM-based reflector live in `autoany-core` or move to a separate `autoany-reflect` crate? Decided in P4 design review.
- **Commit-gate stability coupling.** Exact integration point between `MetaController::commit` and `autonomic-core::StabilityBudget` — do we pass a budget handle, or query Autonomic HTTP? Decided in P4.
- **Naming.** `MetaController` vs `SeplController` vs `EgriController`. The term `MetaController` is used in RCS docs; `SeplController` makes the AGP lineage explicit; `EgriController` reflects autoany lineage. Pick one and cross-reference in docs.

## Success criteria

- `cargo build --workspace` + `cargo test --workspace` + `cargo clippy --workspace` all green after each phase
- No public API of existing crates (arcan, lago, autonomic, praxis) regresses
- Each of the four layers has ≥80% test coverage and doctest examples
- The synthesis note's proposed `MetaController<L, R>` trait is ported into the code
- An integration test demonstrates Pneuma → RSPL → Evolvable → MetaController composed end-to-end
- `research/entities/concept/autogenesis-agp.md`, `sepl-operator-algebra.md`, `evolvable-resource.md` remain linked/accurate
- RCS paper P2 draft cites AGP by the time P4 lands

## References

- Primary: [Autogenesis: A Self-Evolving Agent Protocol](https://arxiv.org/abs/2604.15034) (arXiv:2604.15034)
- Predecessor: [AgentOrchestra (TEA Protocol)](https://arxiv.org/abs/2506.12508) (arXiv:2506.12508)
- Reference impl: [SkyworkAI/DeepResearchAgent](https://github.com/SkyworkAI/DeepResearchAgent)
- Local spec: `docs/specs/pneuma-trait-surface.md`
- Local synthesis: `broomva/research/notes/2026-04-18-autogenesis-agp-rcs-life-alignment-synthesis.md`
- Entity pages: `research/entities/concept/{autogenesis-agp,sepl-operator-algebra,rspl-resource-substrate,evolvable-resource,tea-protocol}.md`

## Changelog

- 2026-04-18: Initial draft (broomva).
