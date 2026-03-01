# Execution Plans

**Last updated**: 2026-03-01

Active and recent execution plans for the `/live` monorepo.

---

## Active: R1 Conformance Expansion

**Status**: IN PROGRESS
**Started**: 2026-02-22
**Scope**: Raise confidence from baseline correctness to stronger determinism guarantees.

### Completed

- [x] Golden fixture suite for canonical run lifecycle
- [x] Adapter round-trip tests (lago-aios-eventstore-adapter)
- [x] Lago journal sequence assignment conformance
- [x] Lago API session/SSE behavior conformance
- [x] Protocol contract checks in conformance harness

### Remaining

- [ ] Branch-heavy replay/merge deterministic fixtures
- [ ] Protocol forward-compat fixtures (unknown/custom event evolution)
- [ ] Stream cursor/replay invariants in canonical API tests

### Acceptance Criteria

- Golden fixture suite covers: run lifecycle, branch lifecycle, approval flow, replay parity across adapters
- Determinism checks pass in CI/conformance without flaky tolerance

### Checkpoints

1. Each new fixture set lands with passing `conformance/run.sh`
2. No regressions in existing 596 tests
3. STATUS.md updated after each fixture batch

---

## Completed: Infrastructure Gap Closure

**Status**: COMPLETE
**Started**: 2026-03-01
**Scope**: Close documented-but-unimplemented gaps in control, harness, and docs.

### Tasks

- [x] Create root PLANS.md (this file)
- [x] Flesh out docs/control/ARCHITECTURE.md (was 318-byte stub → full system design)
- [x] Flesh out docs/control/OBSERVABILITY.md (was 220-byte stub → metrics, events, sensors, alerting)
- [x] Implement real recovery script (scripts/control/recover.sh — diagnose, auto-fmt, re-validate, escalate)
- [x] Wire CLI E2E tests (scripts/control/cli_e2e.sh builds + exercises lago-cli, lagod, arcan)
- [x] Wire Web E2E tests (scripts/control/web_e2e.sh starts arcand, exercises session API)
- [x] Update docs/STATUS.md to 2026-03-01 (fixed MemoryPort, added conformance suites, infra log)

### Acceptance Criteria

- `make audit` passes (baseline)
- `scripts/audit_control.sh . --strict` passes all file-existence checks
- All control docs have substantive content (no stubs)
- Recovery script performs diagnostic steps, not just re-run with `|| true`
- CLI E2E exercises lago-cli and arcan binaries against temp state
- Web E2E exercises arcand HTTP API canonical endpoints

---

## Planned: R2 Observability Maturity

**Status**: PLANNED (blocked on R1 completion)
**Scope**: Structured telemetry across runtime and adapter boundaries.

### Key Deliverables

- Expand structured telemetry across canonical runtime phases
- Metrics for run throughput, failure classes, approval latency, stream health
- Auditability of canonical event write/read paths
- Operator-facing diagnostics for branch/replay anomalies

---

## Completed Plans

### Baseline Canonical Unification (v0.2.0)

**Completed**: 2026-02-28

- Single canonical contract (`aios-protocol`) across all projects
- Single canonical runtime engine (`aios-runtime`) in production host path
- Lago-backed canonical persistence adapter active
- Canonical session API active and tested
- Architecture dependency gate + conformance harness green
- 596 tests passing (+1 ignored)
