---
tags:
  - broomva
  - life
  - architecture
type: architecture
status: active
area: system
created: 2026-03-17
---

# Broomva Life: Canonical Architecture

**Date**: 2026-03-03**Baseline**: Canonical runtime unification active

This document describes the active architecture in this repository (`/life`).

## 1) System Overview

Life is a contract-first architecture for building artificial life from computational primitives. Six AOS primitives — cognition, execution, persistence, temporality, security, and homeostasis — map to biological systems:

| Primitive | Biological Analog | Active Project | Status |
| --- | --- | --- | --- |
| Cognition + Execution | Central nervous system | Arcan | ACTIVE |
| Tool Execution | Motor cortex / effectors | Praxis | ACTIVE |
| Persistence | Long-term memory formation | Lago | ACTIVE |
| Networking | Social/swarm behavior | Spaces | ACTIVE |
| Contract / DNA | Genome | aiOS | ACTIVE |
| Homeostasis | Autonomic nervous system | Autonomic | ACTIVE |
| Observability | Sensory feedback / proprioception | Vigil | ACTIVE |
| Temporality | Circadian rhythm | Chronos | PLANNED |
| Security | Immune system | Aegis | PLANNED |
| World Model | Prefrontal cortex | Nous | PLANNED |
| Knowledge | Hippocampus | Mnemo | PLANNED |

### Active Projects

- **aiOS**: canonical contract + runtime engine
- **Arcan**: daemon host + adapters + clients
- **Praxis**: canonical tool execution and sandbox engine
- **Lago**: durable event-sourced persistence substrate
- **Spaces**: distributed agent networking engine (SpacetimeDB 2.0)
- **Autonomic**: three-pillar homeostasis controller (operational, cognitive, economic)
- **Vigil**: OpenTelemetry-native observability (tracing, GenAI metrics, contract-derived spans)

### Planned Projects

- **Chronos**: temporal scheduler and time-awareness engine
- **Aegis**: OS-level sandbox, capability attestation, secret management
- **Nous**: world model and causal reasoning engine
- **Mnemo**: vector-indexed knowledge store and RAG pipeline

### Active Baseline Spine

1. `aios-protocol` defines canonical runtime contract and boundary types.
2. `aios-runtime` executes runtime behavior through protocol ports.
3. Lago persistence is consumed through canonical adapter implementation.
4. Arcan hosts the runtime and provides adapter implementations for provider/harness/policy/approval/memory.
5. Runtime API is the canonical session API family.
6. Reasoning observability hangs off the canonical event spine:
   knowledge bootstrap and knowledge tool completions emit typed
   `Knowledge*` events, Nous consumes knowledge-aware `EvalContext`, and
   Autonomic folds the same typed events into cognitive regulation.

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

`aios-runtime` depends only on these ports and canonical protocol types.

## Dependency Invariants

1. aiOS core crates must not depend on Arcan/Lago implementation crates.
2. Lago core crates must not depend on Arcan crates.
3. Runtime path data exchange must remain canonical protocol types.
4. Architecture dependency edges are validated by audit gate scripts.

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

## External Integration Adapters

- `symphony-arcan`: Symphony dispatches via Arcan HTTP instead of CLI subprocess.
- `autoany-aios`: EGRI execution via Arcan sessions.
- `autoany-lago`: EGRI trials persisted to Lago via `EventKind::Custom` with `"egri."` prefix.


## 6) Runtime Data Flow

Canonical run flow:

1. Client requests session creation or run execution.
2. Host ensures canonical session state is available.
3. `aios-runtime` executes run loop through port interfaces.
4. Events are appended/read through canonical event-store port implementation.
5. State and lifecycle updates are emitted as canonical events.
6. Clients consume event replay or event stream through canonical endpoints.

### Reasoning Observability Spine

The reasoning/knowledge path now follows the same canonical event route as the rest of the runtime:

1. Consciousness bootstrap assembles wake-up knowledge context and emits `KnowledgeRetrieved`.
2. `wiki_search` / `wiki_lint` finish as ordinary `ToolCallCompleted` events.
3. Arcan turn middleware derives typed `KnowledgeSearched`, `KnowledgeRetrieved`, and `KnowledgeEvaluated` events from those canonical tool results.
4. Autonomic folds the typed knowledge events into cognitive regulation state.
5. `arcand` reconstructs run-finished reasoning inputs from canonical session events into a typed `RunCompletionContext`:
   final answer, assistant messages, executed tool summary, and the latest knowledge evidence from `wiki_search`.
6. `NousToolObserver` executes `registry_with_reasoning()` against that typed payload, populating `EvalContext` with tool summary + knowledge metadata for async judge evaluation.
7. Vigil instruments wake-up assembly plus `wiki_search` / `wiki_lint` with dedicated knowledge-operation spans, so the trace captures both retrieval and health evaluation at the operation seam.
8. The async observer handoff runs under `run_observer.notify`, and both derived `Knowledge*` events plus `nous-lago` eval publications preserve the active trace context, so post-run judge scores and EGRI outcome events stay attached to the originating trace.

This keeps knowledge observability aligned with the contract-first architecture: tools stay pure, the kernel event spine remains authoritative, and downstream regulation/evaluation consume the same typed substrate.

Branch flow:

- branch create/list/merge operations are handled through canonical runtime APIs and persisted through canonical event storage path.

Approval flow:

- approval resolution uses canonical approval endpoint and canonical runtime approval port.

## 7) Streaming Model

Primary stream endpoint:

- `GET /sessions/{session_id}/events/stream`

Supported behavior:

- canonical event streaming for replay/live consumption
- optional Vercel AI SDK v6 envelope path through format handling in canonical stream route

## 8) Governance and Enforcement

Architecture enforcement is integrated into control audit:

- `scripts/architecture/verify_dependencies.sh`
- `Makefile.control`
- `scripts/audit_control.sh`

Conformance and integration gates are exercised by:

- `conformance/run.sh`

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
- `arcan-spaces`: Spaces networking bridge (port-based abstraction, tools, middleware)
- `arcan-core`, `arcan-harness`, `arcan-provider`, `arcan-store`, `arcan-lago`, `arcan-tui`: supporting runtime/client layers

## Lago

- `lago-aios-eventstore-adapter`: canonical event-store adapter
- `lago-core`, `lago-journal`, `lago-store`, `lago-fs`, `lago-policy`, `lago-api`, `lago-ingest`, `lagod`, `lago-cli`: persistence substrate stack

## Praxis

- `praxis-core`: sandbox policy enforcement, workspace boundary checks (FsPolicy), command runner
- `praxis-tools`: canonical tool implementations (ReadFile, WriteFile, ListDir, Glob, Grep, EditFile, Bash, ReadMemory, WriteMemory)
- `praxis-skills`: SKILL.md frontmatter parser, skill registry with discovery and activation
- `praxis-mcp`: MCP server connection management, McpTool bridge (rmcp 0.15)

## Autonomic

- `autonomic-core`: types, traits, errors (economic modes, gating profiles, hysteresis gates, rules)
- `autonomic-controller`: pure rule engine — projection reducer + rule evaluation (no I/O)
- `autonomic-lago`: Lago bridge — event subscription + publishing
- `autonomic-api`: axum HTTP server (/gating, /projection, /health endpoints)
- `autonomicd`: daemon binary with config, signal handling, optional Lago journal

## Vigil

- `vigil`: OpenTelemetry-native observability (config, semconv, spans, metrics)
- Cross-cutting: depends on `aios-protocol`, consumed by Arcan/Lago/Autonomic/Praxis
- Contract-derived spans map EventKind → OTel spans with GenAI semantic conventions
- Dual-write: trace context written into EventEnvelope for persisted event correlation

## Spaces

- `spaces`: CLI client using `spacetimedb-sdk` (Rust 2024 edition)
- `spaces/spacetimedb`: WASM module using `spacetimedb` 2.0.2 (Rust 2021 edition, `cdylib`)
- 11 tables, 20+ reducers, 5-tier RBAC, real-time pub/sub via SpacetimeDB
- Connected to Arcan via `arcan-spaces` bridge (port-based abstraction, mock-backed, concrete SDK adapter pending)

## 10) Current Constraints

1. Vigil provides the observability foundation (tracing, metrics, GenAI conventions); integration into runtime projects is the next step.
2. OS-level sandbox hardening remains an active follow-up area.
3. Cross-project golden fixture breadth can still be expanded.
4. Autonomic is active but advisory-only — Arcan does not yet query it during agent runs.

## 11) Definition of Architectural Baseline

The baseline is complete when all of the following hold (currently true):

- Canonical contract is the sole integration contract.
- Canonical runtime engine is the production runtime path.
- Lago is active persistence backend for runtime events through canonical adapter path.
- Canonical session API is the production runtime API family.
- Architecture dependency audit and conformance gates pass.
