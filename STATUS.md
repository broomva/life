# Agent OS: Implementation Status

**Date**: 2026-02-22  
**Version**: 0.2.0 (canonical baseline)  
**Rust**: edition 2024, MSRV 1.85+

This document is the canonical implementation-state record for `/Users/broomva/broomva.tech/live`.
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

| Area | aiOS | Arcan | Lago |
|---|---|---|---|
| Build | PASS | PASS | PASS |
| Tests | PASS | PASS | PASS |
| Clippy (`-D warnings`) | PASS | PASS | PASS |
| Canonical Port Usage | ACTIVE | CONSUMED | CONSUMED |
| Production Runtime Path | CANONICAL | CANONICAL HOST | CANONICAL STORE |

Validation gates currently pass:

- `/Users/broomva/broomva.tech/live/aiOS`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/live/arcan`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/live/lago`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/live`: `make audit`, `./scripts/architecture/verify_dependencies.sh`, `./conformance/run.sh`

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
  - `MemoryPort`

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

- Script: `/Users/broomva/broomva.tech/live/scripts/architecture/verify_dependencies.sh`
- Integrated in: `make audit`
- Audit enforcement path:
  - `/Users/broomva/broomva.tech/live/Makefile.control`
  - `/Users/broomva/broomva.tech/live/scripts/audit_control.sh`

---

## Conformance Coverage

Conformance harness entrypoint:

- `/Users/broomva/broomva.tech/live/conformance/run.sh`

Current suite validates:

1. Protocol contract checks.
2. Arcand canonical session API behavior.
3. Arcan-Lago replay/bridge behavior.
4. Lago journal sequence assignment semantics.
5. Lago API session/SSE behavior.

---

## Remaining Work (Post-Baseline)

The baseline runtime architecture is in place and validated. Remaining work is additive:

1. Cross-project golden fixture expansion for replay determinism breadth.
2. Observability depth expansion (metrics/traces across runtime and adapters).
3. Security hardening beyond current software-level sandbox controls.
4. Continued documentation depth upgrades in non-status reference documents.

---

## Baseline Completion Checklist

- [x] Single canonical contract (`aios-protocol`) across projects.
- [x] Single canonical runtime engine (`aios-runtime`) in production host path.
- [x] Lago-backed canonical persistence adapter in active runtime path.
- [x] Canonical session API routed by `arcand` and hosted by `arcan`.
- [x] Architecture dependency gate integrated in audit flow.
- [x] Workspace build/lint/test gates green.
- [x] Conformance harness green.

