# Agent OS: Canonical Architecture

**Date**: 2026-02-22  
**Baseline**: Canonical runtime unification active

This document describes the active architecture in `/Users/broomva/broomva.tech/live`.

---

## 1) System Overview

The system is a contract-first architecture across three active projects (plus one planned controller project):

- **aiOS**: canonical contract + runtime engine
- **Arcan**: daemon host + adapters + clients
- **Lago**: durable event-sourced persistence substrate
- **Autonomic** (planned): controller for advanced homeostasis/maintenance policies

### Active Baseline Spine

1. `aios-protocol` defines canonical runtime contract and boundary types.
2. `aios-runtime` executes runtime behavior through protocol ports.
3. Lago persistence is consumed through canonical adapter implementation.
4. Arcan hosts the runtime and provides adapter implementations for provider/harness/policy/approval/memory.
5. Runtime API is the canonical session API family.

---

## 2) Canonical Boundaries

## Contract Boundary

- Boundary crate: `aios-protocol`
- Canonical boundary types include:
  - `EventRecord`, `EventKind`
  - protocol IDs (`SessionId`, `BranchId`, `RunId`, etc.)
  - canonical state structures

## Runtime Ports

`aios-protocol` runtime ports:

- `EventStorePort`
- `ModelProviderPort`
- `ToolHarnessPort`
- `PolicyGatePort`
- `ApprovalPort`
- `MemoryPort`

`aios-runtime` depends only on these ports and canonical protocol types.

## Dependency Invariants

1. aiOS core crates must not depend on Arcan/Lago implementation crates.
2. Lago core crates must not depend on Arcan crates.
3. Runtime path data exchange must remain canonical protocol types.
4. Architecture dependency edges are validated by audit gate scripts.

---

## 3) Runtime Host Topology

## Canonical Runtime Host (Arcan)

`arcan` daemon composes:

- `aios-runtime::KernelRuntime`
- Lago-backed canonical event store adapter
- Arcan adapter implementations for provider/tools/policy/approval/memory
- `arcand::canonical` router

## Canonical API Surface

- `POST /sessions`
- `POST /sessions/{session_id}/runs`
- `GET /sessions/{session_id}/state`
- `GET /sessions/{session_id}/events`
- `GET /sessions/{session_id}/events/stream`
- `POST /sessions/{session_id}/branches`
- `GET /sessions/{session_id}/branches`
- `POST /sessions/{session_id}/branches/{branch_id}/merge`
- `POST /sessions/{session_id}/approvals/{approval_id}`

No alternate production runtime route family is part of the baseline.

---

## 4) Persistence Topology (Lago)

## Event Journal

Lago provides append-only journal semantics with branch-local monotonic sequencing.

Key properties in active runtime path:

- append/read/head semantics through canonical adapter implementation
- branch-aware sequence assignment
- replay-compatible event access

## Blob + Store + Policy

Lago substrate provides:

- content-addressed blob storage
- filesystem/manifest capabilities
- policy engine support
- API and stream formatting utilities used by integration layers

---

## 5) Adapter Architecture

## Lago Adapter

- Crate: `lago-aios-eventstore-adapter`
- Responsibility: implement `EventStorePort` over `lago_core::Journal`
- Conversion path: canonical bridge conversions via Lago core protocol bridge

## Arcan Adapters

- Crate: `arcan-aios-adapters`
- Responsibilities:
  - model provider adapter
  - tool harness adapter
  - policy gate adapter
  - approval adapter
  - memory adapter

Adapters isolate implementation details from canonical runtime contract.

---

## 6) Runtime Data Flow

Canonical run flow:

1. Client requests session creation or run execution.
2. Host ensures canonical session state is available.
3. `aios-runtime` executes run loop through port interfaces.
4. Events are appended/read through canonical event-store port implementation.
5. State and lifecycle updates are emitted as canonical events.
6. Clients consume event replay or event stream through canonical endpoints.

Branch flow:

- branch create/list/merge operations are handled through canonical runtime APIs and persisted through canonical event storage path.

Approval flow:

- approval resolution uses canonical approval endpoint and canonical runtime approval port.

---

## 7) Streaming Model

Primary stream endpoint:

- `GET /sessions/{session_id}/events/stream`

Supported behavior:

- canonical event streaming for replay/live consumption
- optional Vercel AI SDK v6 envelope path through format handling in canonical stream route

---

## 8) Governance and Enforcement

Architecture enforcement is integrated into control audit:

- `/Users/broomva/broomva.tech/live/scripts/architecture/verify_dependencies.sh`
- `/Users/broomva/broomva.tech/live/Makefile.control`
- `/Users/broomva/broomva.tech/live/scripts/audit_control.sh`

Conformance and integration gates are exercised by:

- `/Users/broomva/broomva.tech/live/conformance/run.sh`

---

## 9) Crate Role Map (Active)

## aiOS

- `aios-protocol`: canonical contract and runtime ports
- `aios-runtime`: runtime engine
- `aios-kernel`: composition layer
- `aios-events` / `aios-policy` / `aios-memory` / `aios-tools`: canonical port-aligned components

## Arcan

- `arcan`: daemon host binary
- `arcand`: canonical session API router
- `arcan-aios-adapters`: canonical port adapter implementations
- `arcan-core`, `arcan-harness`, `arcan-provider`, `arcan-store`, `arcan-lago`, `arcan-tui`: supporting runtime/client layers

## Lago

- `lago-aios-eventstore-adapter`: canonical event-store adapter
- `lago-core`, `lago-journal`, `lago-store`, `lago-fs`, `lago-policy`, `lago-api`, `lago-ingest`, `lagod`, `lago-cli`: persistence substrate stack

---

## 10) Current Constraints

1. Baseline emphasizes canonical runtime/persistence integration, not full observability maturity.
2. OS-level sandbox hardening remains an active follow-up area.
3. Cross-project golden fixture breadth can still be expanded.
4. Additional controller-plane capabilities (Autonomic) remain planned.

---

## 11) Definition of Architectural Baseline

The baseline is complete when all of the following hold (currently true):

- Canonical contract is the sole integration contract.
- Canonical runtime engine is the production runtime path.
- Lago is active persistence backend for runtime events through canonical adapter path.
- Canonical session API is the production runtime API family.
- Architecture dependency audit and conformance gates pass.

