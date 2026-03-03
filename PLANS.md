# Execution Plans

**Last updated**: 2026-03-03

Active and recent execution plans for the `/live` monorepo.

---

## Completed: R1 Conformance Expansion

**Status**: COMPLETE
**Started**: 2026-02-22
**Completed**: 2026-03-01
**Scope**: Raise confidence from baseline correctness to stronger determinism guarantees.

### Tasks

- [x] Golden fixture suite for canonical run lifecycle
- [x] Adapter round-trip tests (lago-aios-eventstore-adapter)
- [x] Lago journal sequence assignment conformance
- [x] Lago API session/SSE behavior conformance
- [x] Protocol contract checks in conformance harness
- [x] Branch-heavy replay/merge deterministic fixtures (branch-merge.json + 3 golden tests)
- [x] Protocol forward-compat fixtures (forward-compat-evolution.json + 3 golden tests)
- [x] Stream cursor/replay invariants in canonical API tests (4 tests: cursor past head, cursor zero, branch isolation, merge round-trip)

### Results

- 657 tests passing (+1 ignored) — up from 596 at baseline
- Conformance suite: 81 tests across 7 suites
- Golden fixtures: 6 fixture files, 14 replay tests
- No regressions, all gates green

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

## Completed: Spaces Networking Engine (v0.1.0)

**Status**: COMPLETE
**Started**: 2026-03-02
**Completed**: 2026-03-03
**Scope**: Production-ready distributed communication engine for agent-to-agent and human-to-agent interaction.

### Tasks

- [x] SpacetimeDB 2.0 WASM module with 11 tables and 20+ reducers
- [x] 5-tier RBAC (Owner/Admin/Moderator/Member/Agent)
- [x] Server/channel/thread/message lifecycle with cascading deletes
- [x] Agent-specific primitives (AgentLog channels, AgentEvent messages, `register_agent` reducer)
- [x] Rust CLI client with 26 commands using `spacetimedb-sdk`
- [x] Auto-generated client bindings (44 files)
- [x] Harness and control scripts updated to include Spaces
- [x] Documentation integration (STATUS, ARCHITECTURE, NAVIGATION, ROADMAP, CLAUDE.md, AGENTS.md)

### Results

- 2 crates (client + WASM module), ~1,500 LOC
- Standalone project — no dependency on aiOS/Arcan/Lago
- Integration point: Arcan agents connect as SDK clients

### Known Gaps (follow-up)

- [ ] Reducer unit tests and integration tests
- [ ] Arcan integration bridge
- [ ] DM/private messaging

---

## Planned: R2 Observability Maturity

**Status**: PLANNED (R1 complete, unblocked)
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
- 657 tests passing (+1 ignored)
