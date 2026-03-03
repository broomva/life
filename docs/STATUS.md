# Broomva Life: Implementation Status

**Date**: 2026-03-03
**Version**: 0.2.0 (canonical baseline)
**Rust**: edition 2024, MSRV 1.85+ (Spaces backend: edition 2021)
**Tests**: 657 passing (+1 ignored) across 19 crates + Spaces (21 crates total)

This document is the canonical implementation-state record for `/Users/broomva/broomva.tech/life`.
If another status document conflicts with this one, treat this file as source of truth.

---

## Current State

The baseline unification is active and enforced in production paths:

- `aios-protocol` is the cross-project contract.
- `aios-runtime` is the runtime engine.
- Lago is the persistence backend through canonical port adapters.
- Arcan hosts the canonical runtime and provides integration adapters.
- Public runtime API surface is the canonical session API family.

## Health Summary

| Area | aiOS | Arcan | Lago | Spaces |
|---|---|---|---|---|
| Build | PASS | PASS | PASS | PASS |
| Tests | PASS | PASS | PASS | N/A (0 tests) |
| Clippy (`-D warnings`) | PASS | PASS | PASS | PASS |
| Canonical Port Usage | ACTIVE | CONSUMED | CONSUMED | STANDALONE |
| Production Runtime Path | CANONICAL | CANONICAL HOST | CANONICAL STORE | NETWORKING |

Validation gates currently pass:

- `/Users/broomva/broomva.tech/life/aiOS`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/arcan`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/lago`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/spaces`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo check` (WASM module: `cargo check --target wasm32-unknown-unknown --manifest-path spacetimedb/Cargo.toml`)
- `/Users/broomva/broomva.tech/life`: `make audit`, `./scripts/architecture/verify_dependencies.sh`, `./conformance/run.sh`

---

## Canonical Architecture

### Hard Invariants

1. `aiOS` core crates do not depend on Arcan or Lago implementation crates.
2. Lago core crates do not depend on Arcan crates.
3. Runtime boundary data uses canonical protocol types (`EventRecord`, `EventKind`, protocol IDs, canonical state).
4. Persistence writes go through canonical event-store port implementations.
5. Canonical session API is the public runtime API family.

### Canonical Session API

- `POST /sessions`
- `POST /sessions/{session_id}/runs`
- `GET /sessions/{session_id}/state`
- `GET /sessions/{session_id}/events`
- `GET /sessions/{session_id}/events/stream`
- `POST /sessions/{session_id}/branches`
- `GET /sessions/{session_id}/branches`
- `POST /sessions/{session_id}/branches/{branch_id}/merge`
- `POST /sessions/{session_id}/approvals/{approval_id}`

---

## Project Status

## aiOS

### Canonical Contract

- `aios-protocol` exports canonical runtime ports:
  - `EventStorePort`
  - `ModelProviderPort`
  - `ToolHarnessPort`
  - `PolicyGatePort`
  - `ApprovalPort`

### Runtime

- `aios-runtime` is port-driven and decoupled from concrete Arcan/Lago internals.
- Supports branch-aware event sequences, run lifecycle events, policy/approval flow, and state/homeostasis update emission.
- Supports explicit session creation and named-session bootstrapping used by canonical hosts.

### Composition

- `aios-kernel` composes runtime + ports.
- `aios-events`, `aios-policy`, `aios-memory`, `aios-tools` align to canonical port interfaces.

## Arcan

### Host + Adapters

- `arcan` binary hosts `aios-runtime` as production runtime path.
- `arcan-aios-adapters` implements canonical provider/tool/policy/approval/memory ports.
- `arcand` serves the canonical session API router.

### Runtime Surface

- Active `arcand` module surface is canonical-only.
- Canonical API integration tests cover:
  - session lifecycle
  - named-session run auto-create behavior
  - streaming replay framing (including Vercel AI SDK v6 data envelope/header path)

### Client Alignment

- `arcan-tui` uses canonical session + approval endpoints.
- Stream parsing supports canonical event records and canonical Vercel AI SDK v6 wrapper payloads.

## Lago

### Canonical Persistence

- `lago-aios-eventstore-adapter` implements canonical `EventStorePort` over `lago_core::Journal`.
- Canonical conversion path uses `lago_core::protocol_bridge`.
- Branch-local monotonic sequencing remains enforced by journal semantics.

### Substrate

- Journal, blob store, policy engine, API, and file/manifest subsystems are operational and tested.

---

## Governance and Dependency Control

Architecture dependency gate is active:

- Script: `/Users/broomva/broomva.tech/life/scripts/architecture/verify_dependencies.sh`
- Integrated in: `make audit`
- Audit enforcement path:
  - `/Users/broomva/broomva.tech/life/Makefile.control`
  - `/Users/broomva/broomva.tech/life/scripts/audit_control.sh`

---

## Conformance Coverage

Conformance harness entrypoint:

- `/Users/broomva/broomva.tech/life/conformance/run.sh`

Current suite validates:

1. Protocol contract checks (35 tests).
2. Arcand canonical session API behavior (9 tests: lifecycle, auto-create, streaming, cursor invariants, branch isolation, merge round-trip).
3. Arcan-Lago replay/bridge behavior (3 tests).
4. Lago journal sequence assignment semantics (1 test).
5. Lago API session/SSE behavior (8 tests).
6. Lago-aiOS eventstore adapter bridge checks (11 tests).
7. Lago journal golden replay tests (14 tests: simple-chat, tool-round-trip, branch-fork, branch-merge, forward-compat, forward-compat-evolution).

## Spaces

### Distributed Agent Networking

- SpacetimeDB 2.0 WASM module providing real-time distributed communication for agents.
- 11 tables, 20+ reducers, 5-tier RBAC (Owner/Admin/Moderator/Member/Agent).
- Channel types: Text, Voice, Announcement, AgentLog.
- Message types: Text, System, Join, Leave, AgentEvent.
- Rust CLI client with 26 commands using `spacetimedb-sdk`.
- Auto-generated client bindings (44 files) via `spacetime generate`.

### Integration Points

- Standalone project — does not depend on aiOS/Arcan/Lago crates.
- Arcan agents will connect as SDK clients for distributed coordination.
- AgentLog channels and AgentEvent messages provide agent-specific communication primitives.

### Known Gaps

- No unit tests (reducer tests, integration tests planned).
- No DM/private messaging.
- No Arcan integration bridge yet.

---

## Remaining Work (Post-Baseline)

The baseline runtime architecture is in place and validated. Remaining work is additive:

1. ~~Cross-project golden fixture expansion for replay determinism breadth~~ (R1, COMPLETE — branch-merge, forward-compat-evolution, stream cursor/replay invariants).
2. Observability depth expansion (metrics/traces across runtime and adapters) (R2, PLANNED).
3. Security hardening beyond current software-level sandbox controls (R3, PLANNED).
4. Memory and learning depth (R4, PLANNED).
5. Controller plane / Autonomic integration (R5, PLANNED).

### Infrastructure (2026-03-01)

- [x] Root PLANS.md created for execution tracking.
- [x] docs/control/ARCHITECTURE.md expanded (was stub).
- [x] docs/control/OBSERVABILITY.md expanded (was stub).
- [x] Recovery script (scripts/control/recover.sh) upgraded with diagnostics.
- [x] CLI E2E tests wired (scripts/control/cli_e2e.sh exercises lago-cli, lagod, arcan).
- [x] Web E2E tests wired (scripts/control/web_e2e.sh exercises arcand HTTP API).
- [x] CI workflows updated for CLI and Web E2E pipelines.
- [x] MemoryPort removed from canonical port list (was removed from aios-protocol 2026-02-28).

---

## Baseline Completion Checklist

- [x] Single canonical contract (`aios-protocol`) across projects.
- [x] Single canonical runtime engine (`aios-runtime`) in production host path.
- [x] Lago-backed canonical persistence adapter in active runtime path.
- [x] Canonical session API routed by `arcand` and hosted by `arcan`.
- [x] Architecture dependency gate integrated in audit flow.
- [x] Workspace build/lint/test gates green.
- [x] Conformance harness green.

