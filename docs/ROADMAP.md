# Agent OS: Forward Roadmap (Post-Baseline)

**Date**: 2026-03-03
**Baseline**: Canonical runtime unification complete

This roadmap starts from the active canonical baseline and lists forward-only execution phases.

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

### Scope

1. Define minimal controller integration contract using canonical protocol boundaries.
2. Implement controller outputs as policy/gating inputs into canonical runtime path.
3. Ensure controller actions are event-auditable and replay-safe.

### Acceptance

- Controller integration does not bypass canonical runtime/event paths.
- Controller-induced decisions are visible and deterministic in replay.

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

1. `/Users/broomva/broomva.tech/live/aiOS`
   - `cargo fmt`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test --workspace`
2. `/Users/broomva/broomva.tech/live/arcan`
   - `cargo fmt`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test --workspace`
3. `/Users/broomva/broomva.tech/live/lago`
   - `cargo fmt`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test --workspace`
4. `/Users/broomva/broomva.tech/live/spaces`
   - `cargo fmt`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo check`
5. `/Users/broomva/broomva.tech/live`
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
- R5 Controller plane: `PLANNED`
- Spaces networking: `ACTIVE` (v0.1.0 — integration with Arcan pending)

---

## 9) Planned Feature Track: Economic Actuation (Conway-Compatible)

Status: `PLANNED`

Reference:
- `docs/FEATURE_CONWAY_ACTUATION.md`

Intent:
- Add contract-safe economic/payment and external provisioning primitives (payment events, resource lease lifecycle, capability gates, spend-aware homeostasis) while preserving canonical replay and policy guarantees.

Placement in roadmap:
- Cross-cuts R2/R3/R5.
- Must not bypass canonical runtime or event paths.

