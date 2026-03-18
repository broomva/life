---
tags:
  - broomva
  - life
  - roadmap
type: roadmap
status: active
area: system
created: 2026-03-17
---

# Broomva Life: Forward Roadmap (Post-Baseline)

**Date**: 2026-03-03
**Baseline**: Canonical runtime unification complete

This roadmap starts from the active canonical baseline and lists forward-only execution phases. Each phase advances the "Life" vision — building artificial life from the six AOS primitives (cognition, execution, persistence, temporality, security, homeostasis).

---

## 0) Baseline Snapshot

Completed baseline outcomes:

- `aios-protocol` is active contract boundary.
- `aios-runtime` is active runtime engine in production host path.
- Lago-backed canonical event-store adapter is active.
- Arcan adapter layer is active.
- Canonical session API family is active and tested.
- Dependency architecture audit is integrated in control audit.
- Conformance harness is green.

Baseline is now the required foundation for all new work.

---

## 1) Phase R1 — Conformance Expansion

**Goal**: Raise confidence from baseline correctness to stronger determinism guarantees across projects.

### Scope

1. Expand cross-project golden fixtures for event replay determinism.
2. Add protocol forward-compat fixtures for unknown/custom event evolution.
3. Add stronger stream cursor/replay invariants in canonical API tests.
4. Add branch-heavy replay/merge deterministic fixtures.

### Acceptance

- Golden fixture suite covers at least canonical run lifecycle, branch lifecycle, approval flow, and replay parity across adapters.
- Determinism checks pass in CI/conformance without flaky tolerance.

---

## 2) Phase R2 — Observability Maturity

**Goal**: Make runtime and adapter behavior operationally transparent.

### Scope

1. Expand structured telemetry across canonical runtime phases and adapter boundaries.
2. Add clear metrics for run throughput, failure classes, approval latency, and stream health.
3. Improve auditability of canonical event write/read paths.
4. Add operator-facing diagnostics for branch/replay anomalies.

### Acceptance

- Key runtime and adapter paths emit structured telemetry with stable field shapes.
- Alertable signals exist for failure hotspots and stream/persistence health regressions.

---

## 3) Phase R3 — Security Hardening

**Goal**: Move from baseline-safe behavior to hardened runtime controls.

### Scope

1. Strengthen sandbox isolation beyond current process-level controls.
2. Tighten capability and approval policy defaults where appropriate.
3. Expand secret-handling and redaction coverage in logs/events.
4. Add explicit tests for high-risk capability gating paths.

### Acceptance

- Hardened sandbox policy profile is available and verified in test gates.
- Security-sensitive path tests pass in workspace gates.

---

## 4) Phase R4 — Memory and Learning Depth

**Goal**: Evolve memory/learning features on top of canonical baseline without violating contract boundaries.

> **Partially unblocked**: autoany-aios + autoany-lago adapters wired, autoany_core has dead-end tracking, stagnation detection, strategy distillation, cross-run inheritance.

### Scope

1. Expand canonical memory workflows (proposal/commit/query quality and policies).
2. Improve event-derived memory projection/query fidelity.
3. Add learning-oriented fixture and regression suites for memory lifecycle events.
4. Keep all enhancements contract-first through `aios-protocol`.

### Acceptance

- Memory lifecycle semantics remain replay-safe and deterministic.
- Memory behavior remains adapter-agnostic at runtime boundary.

---

## 5) Phase R5 — Controller Plane (Autonomic)

**Goal**: Introduce controller-driven stability behaviors without creating a second runtime model.

### Phase 0 — COMPLETE (2026-03-03)

- [x] Core types, traits, errors (economic modes, gating profiles, hysteresis gates, rules)
- [x] Pure rule engine with 6 rules + deterministic projection fold
- [x] Lago bridge (publisher/subscriber) with 8 tests
- [x] HTTP API (/gating, /projection, /health)
- [x] Daemon binary with CLI args, TOML config, optional Lago journal
- [x] On-demand session bootstrapping from Lago journal
- [x] Hysteresis gates wired into fold (time-based cooldown)
- [x] Registered as git submodule, architecture dependency audit passing

### Remaining Scope

1. Wire Arcan agent loop to query Autonomic `/gating/{session_id}` before runs.
2. Ensure controller actions are event-auditable and replay-safe.
3. Add observability (metrics/traces) to Autonomic API.
4. E2E integration test across Lago + Autonomic.

### Acceptance

- Controller integration does not bypass canonical runtime/event paths.
- Controller-induced decisions are visible and deterministic in replay.
- Arcan continues normally if Autonomic is unavailable (advisory architecture).

---

## 6) Ongoing Engineering Rules

These rules apply to every roadmap phase:

1. Contract-first changes: boundary updates land in `aios-protocol` first.
2. No alternate production runtime path.
3. No cross-layer concrete dependency violations.
4. Every phase must pass build/lint/test/conformance/audit gates.
5. Documentation must be updated in the same change set as architecture behavior changes.

---

## 7) Required Gates Per Milestone

A milestone is complete only when all pass:

1. `/Users/broomva/broomva.tech/life/aiOS`
   - `cargo fmt`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test --workspace`
2. `/Users/broomva/broomva.tech/life/arcan`
   - `cargo fmt`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test --workspace`
3. `/Users/broomva/broomva.tech/life/lago`
   - `cargo fmt`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test --workspace`
4. `/Users/broomva/broomva.tech/life/autonomic`
   - `cargo fmt`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test --workspace`
5. `/Users/broomva/broomva.tech/life/spaces`
   - `cargo fmt`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo check`
6. `/Users/broomva/broomva.tech/life`
   - `make audit`
   - `./conformance/run.sh`

---

## 8) Tracking Model

Status labels used in this roadmap:

- `COMPLETE`: implemented and gate-validated.
- `ACTIVE`: currently in execution.
- `PLANNED`: approved but not started.
- `BLOCKED`: cannot proceed due to unmet dependency/gate.

Current labels:

- Baseline unification: `COMPLETE`
- R1 Conformance expansion: `COMPLETE`
- R2 Observability maturity: `PLANNED`
- R3 Security hardening: `PLANNED`
- R4 Memory/learning depth: `PLANNED`
- R5 Controller plane: `ACTIVE` (Phase 0 complete — 5 crates, 69 tests, Lago wired, hysteresis active; Arcan integration pending)
- Spaces networking: `ACTIVE` (v0.1.0 — arcan-spaces bridge implemented, concrete SDK adapter pending)
- R6 Temporality (Chronos): `PLANNED`
- R7 Security enforcement (Aegis): `PLANNED`
- R8 World model (Nous): `PLANNED`
- R9 Knowledge store (Mnemo): `PLANNED`

---

## 9) Phase R6–R9 — AOS Primitive Expansion

These phases introduce the remaining AOS primitives as standalone projects integrated through the canonical `aios-protocol` contract.

### R6 — Temporality (Chronos)

- Temporal scheduler with heartbeat, deadline, and time-boxed execution
- Circadian-style activity cycles for agent energy management
- Contract-first: temporal events flow through canonical event store

### R7 — Security Enforcement (Aegis)

- OS-level sandbox isolation (beyond current soft sandbox)
- Capability attestation and secret management
- Network isolation enforcement
- Integrates as a policy gate through `aios-protocol`

### R8 — World Model (Nous)

- Agent's persistent understanding of its environment
- Causal reasoning and state prediction
- Observation-driven model updates through canonical event streams

### R9 — Knowledge Store (Mnemo)

- Vector-indexed persistent knowledge
- RAG pipeline for retrieval-augmented generation
- Memory lifecycle management (consolidation, forgetting, retrieval)
- Integrates with Lago for durable storage

---

## 10) Planned Feature Track: Economic Actuation (Conway-Compatible)

Status: `PLANNED`

Reference:
- `docs/FEATURE_CONWAY_ACTUATION.md`

Intent:
- Add contract-safe economic/payment and external provisioning primitives (payment events, resource lease lifecycle, capability gates, spend-aware homeostasis) while preserving canonical replay and policy guarantees.

Placement in roadmap:
- Cross-cuts R2/R3/R5.
- Must not bypass canonical runtime or event paths.

