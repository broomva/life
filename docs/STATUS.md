---
tags:
  - broomva
  - life
  - roadmap
type: operations
status: active
area: system
created: 2026-03-17
---

# Broomva Life: Implementation Status

**Date**: 2026-03-04**Version**: 0.2.0 (canonical baseline)**Rust**: edition 2024, MSRV 1.85+ (Spaces backend: edition 2021)**Tests**: 1042 passing (+5 ignored) across 30 crates + Spaces (32 crates total)

This document is the canonical implementation-state record for `/Users/broomva/broomva.tech/life`.If another status document conflicts with this one, treat this file as source of truth.

## Current State

The baseline unification is active and enforced in production paths:

- `aios-protocol` is the cross-project contract.
- `aios-runtime` is the runtime engine.
- Lago is the persistence backend through canonical port adapters.
- Arcan hosts the canonical runtime and provides integration adapters.
- Public runtime API surface is the canonical session API family.
- 2026-04-08: Reasoning observability Phase 1 landed in shared contracts.
  `aios-protocol` now defines typed `KnowledgeSearched`,
  `KnowledgeRetrieved`, and `KnowledgeEvaluated` events, and `nous-core`
  `EvalContext` now carries optional knowledge metrics for later middleware
  population and evaluator correlation.
- 2026-04-09: Reasoning observability Phase 2 landed on the active runtime path.
  Arcan now emits typed knowledge events from two production seams:
  wake-up knowledge bootstrap (`KnowledgeRetrieved`) and kernel turn
  middleware derived from canonical `ToolCallCompleted` events
  (`KnowledgeSearched`, `KnowledgeRetrieved`, `KnowledgeEvaluated`).
  Autonomic folds the typed knowledge variants directly, and
  `nous-middleware` now populates `EvalContext` with live knowledge
  coverage, freshness, retrieval count, relevance, and query metadata.
- 2026-04-09: Reasoning observability Phase 3 judge substrate landed in
  `nous-judge`. Async `ReasoningCoherence` and `KnowledgeUtilization`
  evaluators now exist, plus `registry_with_reasoning()` for the
  five-evaluator async judge set.
- 2026-04-09: Reasoning observability Phase 4 registry integration is now
  active on the canonical host path. `ToolHarnessObserver` run completion now
  flows through a typed `RunCompletionContext`, `arcand` reconstructs
  assistant output + executed tool summaries + knowledge evidence from the
  canonical event spine, and `NousToolObserver` now executes the shared
  `registry_with_reasoning()` async judge set instead of a hand-built trio.
  The async observer notification path is instrumented under
  `run_observer.notify`, preserving trace lineage for post-run evaluation and
  score publication.
- 2026-04-09: Reasoning observability trace completion is now active across the
  knowledge path. Vigil emits dedicated `knowledge.context_build`,
  `knowledge.search`, and `knowledge.lint` spans; derived `Knowledge*` events
  inherit the source event trace/span IDs; `nous-lago` publishes eval events
  with the current trace context serialized into Lago metadata; and
  `arcan-lago` has an integration test proving wake-up retrieval, search,
  eval, and lint events can be reconstructed as one reasoning trace by
  `trace_id`.
- 2026-04-09: EGRI calibration Phase 5 substrate landed in `lago-knowledge`.
  The crate now exposes a typed `KnowledgeThresholdArtifact` with hard bounds,
  parameterized BM25/search config so threshold mutation affects the live plant,
  and a benchmark schema/runner for Recall@1 and Recall@5 across dev/holdout
  splits. A 50-question seed benchmark file now lives under
  `crates/lago/lago-knowledge/benchmarks/knowledge-benchmark.json`; because the
  entity-page corpus referenced by the approved design was not present in the
  workspace, that file is a bootstrap seed that should be regenerated from the
  canonical entity corpus once mounted.
- 2026-04-10: EGRI calibration Phase 6 proposer substrate is active in
  `lago-knowledge`. `KnowledgeThresholdProposer` now emits deterministic,
  bounded `KnowledgeThresholdProposal`s over the threshold artifact, supports
  single-parameter and correlated mutations, expands after five non-improving
  trials, and filters repeated failed regions plus inherited cross-run
  insights before handing candidates to the future executor/evaluator loop.
- 2026-04-10: EGRI calibration evaluator substrate is active in
  `lago-knowledge`. `KnowledgeQualityEvaluator` now computes the approved
  weighted composite score over dev recall, holdout recall, reasoning
  coherence, knowledge health, token efficiency, and speed; emits
  outcome-compatible metadata; and enforces hard safety plus holdout
  anti-gaming constraints before future trial execution can promote threshold
  candidates.
- 2026-04-10: EGRI calibration trial execution substrate is active in
  `lago-knowledge`. `KnowledgeTrialExecutor` now applies a
  `KnowledgeThresholdArtifact` to the benchmark/search plant, emits
  evaluator-compatible JSON metrics, carries explicit Arcan/Nous runtime signal
  inputs, and returns immutable `KnowledgeQualityOutcome`s for proposer
  feedback and future promotion decisions.
- 2026-04-10: EGRI calibration promotion persistence is active in
  `lago-knowledge`. `promote_to_lago_toml()` validates approved threshold
  artifacts, writes the promoted parameters into the `lago.toml` `[knowledge]`
  section with version/rollback metadata, preserves unrelated TOML sections,
  uses a path-scoped writer lock plus atomic rename, tolerates unversioned
  hand-authored knowledge baselines, and produces an
  `egri.knowledge.promoted` Lago event payload for audit and future Autonomic
  regression monitoring. Local `cargo test -p lago-knowledge` passes with 139
  tests.
- 2026-04-10: EGRI calibration rollback monitoring is active in Autonomic.
  The projection reducer now folds `egri.knowledge.promoted` into typed
  promotion state, counts consecutive post-promotion knowledge-health
  regressions against the promoted `health_threshold`, and folds
  `autonomic.RollbackRequested` acknowledgements. `KnowledgeRegressionRule`
  requests rollback after more than three consecutive regressions by attaching
  a structured advisory event to the gating profile; `autonomic-api` persists
  that advisory event to Lago when a journal is configured, returns the fresh
  published watermark in the gating response, and updates the in-memory
  projection to prevent duplicate rollback requests. Embedded Arcan gating
  acknowledges the same advisory events locally so rollback requests remain
  once-only even without the standalone Autonomic API.
- 2026-04-10: EGRI calibration campaign integration is active in
  `lago-knowledge`. `KnowledgeCalibrationCampaign` now runs the bounded
  proposer → trial runner → evaluator feedback loop, tracks incumbent score
  progression, writes the best qualifying artifact through the existing
  `lago.toml` promotion seam, and exposes a `KnowledgeTrialRunner` trait so
  future Arcan-backed trials and deterministic mock trials share the same
  contract. A cross-crate Autonomic integration test now proves the real
  `egri.knowledge.promoted` payload generated by Lago promotion folds into
  regression counting and emits the expected rollback advisory after sustained
  post-promotion health regression.
- 2026-04-10: Vigil LLM cost envelope integration is active on the Arcan
  provider boundary. `life-vigil` now exposes a richer typed
  `LlmRequestEnvelope` covering request identity, provider routing, token/cost
  economics, reliability, and governance metadata under `vigil.llm.*`
  semantic conventions. `arcan-aios-adapters` builds the envelope for each
  provider call, records it on the GenAI chat span, writes optional JSONL
  artifacts through `VIGIL_JSONL_PATH`, estimates response costs from the
  local pricing snapshot, and returns the serialized call record through
  `ModelCompletion`. `aios-runtime` persists that record as a
  `vigil.llm_call` custom event, so Lago replay can correlate provider
  economics with the same canonical session event spine used for reasoning
  observability and Autonomic regulation.
- 2026-04-10: `memory_graph` v1 is active in Arcan shell. The tool builds a
  derived Lago knowledge index over `.arcan/memory`, resolves starts by exact
  path/name/wikilink target, performs bounded wikilink traversal with cycle
  protection, and returns compact nodes plus `references` edges with
  provenance. The graph remains a derived retrieval layer; no new authoritative
  graph store or mandatory Lago route was introduced.

## Health Summary

| Area | aiOS | Arcan | Lago | Autonomic | Praxis | Vigil | Spaces |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Build | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| Tests | PASS (96) | PASS (474+16 w/ spacetimedb) | PASS (336) | PASS (219 targeted) | PASS (90) | PASS (26+2 ignored) | N/A (0 tests) |
| Clippy (-D warnings) | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| Canonical Port Usage | ACTIVE | CONSUMED | CONSUMED | CONSUMED | CONSUMED | CROSS-CUTTING | BRIDGED (arcan-spaces) |
| Production Runtime Path | CANONICAL | CANONICAL HOST | CANONICAL STORE | ADVISORY | TOOL ENGINE | OBSERVABILITY | NETWORKING |

Validation gates currently pass:

- `/Users/broomva/broomva.tech/life/aiOS`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/arcan`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/lago`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/autonomic`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/praxis`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/vigil`: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`
- `/Users/broomva/broomva.tech/life/spaces`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo check` (WASM module: `cargo check --target wasm32-unknown-unknown --manifest-path spacetimedb/Cargo.toml`)
- `/Users/broomva/broomva.tech/life`: `make audit`, `./scripts/architecture/verify_dependencies.sh`, `./conformance/run.sh`

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
- Reasoning observability is active on the canonical host path:
  knowledge bootstrap emits typed retrieval events at session spawn, and
  a kernel turn middleware derives typed knowledge observability events from
  `wiki_search` / `wiki_lint` tool completions without coupling persistence
  into the tool trait itself. Run completion now also moves through a typed
  observer payload so post-run evaluators consume canonical assistant/tool/
  knowledge context instead of re-deriving ad hoc metadata.

### Runtime Surface

- Active `arcand` module surface is canonical-only.
- Canonical API integration tests cover:
  - session lifecycle
  - named-session run auto-create behavior
  - streaming replay framing (including Vercel AI SDK v6 data envelope/header path)

### Spaces Bridge

- `arcan-spaces` provides port-based abstraction (`SpacesPort` trait) for Spaces networking.
- 6 tool definitions: list_channels, send_message, read_messages, send_dm, create_channel, list_members.
- Middleware for agent event logging to Spaces channels.
- Mock hub for testing (18 tests).
- **SpacetimeDB HTTP adapter** (`spacetimedb` feature): Concrete `SpacetimeDbClient` implementing `SpacesPort` via SpacetimeDB REST API (SQL reads + reducer calls). 16 new tests (+2 ignored live integration). Backend selection via `--spaces-backend spacetimedb` or `ARCAN_SPACES_BACKEND` env var.

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

### Context Engine (2026-03-19)

- 12 crates total (was 10): added `lago-knowledge` (143 tests) and `lago-auth` (5 tests).
- `lago-knowledge`: YAML frontmatter parsing, `[[wikilink]]` extraction, in-memory knowledge index, scored search (+2 name, +1 body, +1 tag), BFS graph traversal.
- `lago-knowledge`: also now includes EGRI calibration substrate —
  typed benchmark schema/runner, a seed benchmark corpus, parameterized BM25
  tuning surface, `KnowledgeThresholdArtifact` bounds/validation, and a
  deterministic `KnowledgeThresholdProposer`, `KnowledgeQualityEvaluator`,
  `KnowledgeTrialExecutor`, and `KnowledgeCalibrationCampaign` for bounded
  calibration candidates, immutable composite scoring, evaluator-ready trial
  execution, full campaign orchestration, and governed promotion to the
  `lago.toml` `[knowledge]` section with versioned rollback metadata plus
  `egri.knowledge.promoted` audit events.
- `lago-knowledge` traversal resolution now accepts exact paths, relative
  paths, path stems, and wikilink syntax for graph starts. `arcan-lago` shapes
  those traversal primitives into `MemoryGraphResponse`, and Arcan shell
  exposes the read-only `memory_graph` tool beside the existing agent-driven
  memory retrieval tools.
- Autonomic: EGRI rollback monitoring folds promoted knowledge threshold
  versions and regression counters, emits durable `autonomic.RollbackRequested`
  advisories after sustained post-promotion health regression, and marks the
  active promotion as handled once the rollback request is folded.
- `lago-auth`: JWT validation (HS256 shared secret), axum auth middleware, user→session mapping (`vault:{user_id}`).
- `lago-api`: Auth-protected `/v1/memory/*` routes (manifest, file CRUD, search, traverse, note resolution).
- `lagod`: `LAGO_JWT_SECRET` env var or `[auth]` TOML section. Session map rebuilt on startup. Backward-compatible when no secret set.
- `lago-cli`: 7 `lago memory` subcommands (status, ls, search, read, store, ingest, delete). Token from `BROOMVA_API_TOKEN` env or `~/.broomva/config.json`.
- Full workspace: 371+ tests passing, 0 clippy warnings.

## Governance and Dependency Control

Architecture dependency gate is active:

- Script: `/Users/broomva/broomva.tech/life/scripts/architecture/verify_dependencies.sh`
- Integrated in: `make audit`
- Audit enforcement path:
  - `/Users/broomva/broomva.tech/life/Makefile.control`
  - `/Users/broomva/broomva.tech/life/scripts/audit_control.sh`

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

## Autonomic

### Homeostasis Controller

- Three-pillar regulation: operational, cognitive, economic homeostasis.
- 5 crates: `autonomic-core` (51 tests), `autonomic-controller` (140 tests), `autonomic-lago` (8 tests), `autonomic-api` (18 tests), `autonomicd` (2 tests).
- Pure rule engine with deterministic projection fold over events.
- Economic modes: Sovereign, Conserving, Hustle, Hibernate — with hysteresis-gated transitions.
- Dual-mode advisory architecture:
  - **Embedded** (default): In-process `autonomic-controller` fold+rules with microsecond-latency gating; no network required.
  - **Remote** (opt-in via `--autonomic-url`): Consults standalone daemon via HTTP GET `/gating/{session_id}`; failures are non-fatal.
- Economic gate handle wired to provider layer: Hibernate blocks model calls, Hustle caps tokens.
- Token usage flows through RunFinished events → event mapping → Autonomic fold.
- Typed knowledge observability now flows through the same fold:
  `KnowledgeSearched` increments search volume,
  `KnowledgeRetrieved` accounts for injected context-token cost, and
  `KnowledgeEvaluated` updates knowledge health and indexed-note count.
- Lago journal integration via `--lago-data-dir` flag; on-demand session bootstrapping.

### Integration Points

- Depends on `aios-protocol` (canonical contract) and `lago-core`/`lago-journal` (persistence).
- Events use `EventKind::Custom` with `"autonomic."` prefix for forward-compatible Lago persistence.
- `arcan-aios-adapters` depends on `autonomic-core` and `autonomic-controller` for embedded mode.
- Does not depend on Arcan crates — standalone advisory service.

### Known Gaps

- Not yet consulted by Arcan agent loop (R5 Phase 1 COMPLETE — `AutonomicPolicyAdapter` decorator wired in Arcan).
- Feedback loop open (Autonomic projection always at default) (R5 Phase 2 COMPLETE — embedded controller, economic gating, token usage flow).
- No observability (metrics/traces) yet.
- Identity system is placeholder.

## Praxis

### Canonical Tool Execution Engine

- Standalone tool execution and sandbox engine extracted from `arcan-harness`.
- 4 crates: `praxis-core` (21 tests), `praxis-tools` (24 tests), `praxis-skills` (11 tests), `praxis-mcp` (34 tests).
- **90 tests total** across all crates.
- Depends only on `aios-protocol` — no dependency on Arcan, Lago, or Autonomic.
- Implements canonical `Tool` trait from `aios-protocol::tool`.

### Components

- **praxis-core**: Sandbox policy enforcement, workspace boundary checks (FsPolicy), FsPort abstraction (pluggable filesystem), command runner abstraction.
- **praxis-tools**: ReadFile, WriteFile, ListDir, Glob, Grep, EditFile (hashline/Blake3), Bash, ReadMemory, WriteMemory.
- **praxis-skills**: SKILL.md frontmatter parser, skill registry with discovery and activation.
- **praxis-mcp**: Full MCP server + client bridge via rmcp 0.15.
  - **Server**: `PraxisMcpServer` (`ServerHandler`) exposes any `ToolRegistry` as an MCP server.
  - **Transports**: stdio (Claude Desktop/CLI) and Streamable HTTP (axum) with session management.
  - **Client**: `connect_mcp_stdio()` connects to external MCP servers via subprocess.
  - **Bridge**: `McpTool` wraps external MCP tools as canonical `Tool` trait implementations.
  - **Conversions**: Bidirectional canonical ↔ MCP type mapping (definitions, results, annotations, content).
  - **Tests**: 24 unit + 9 integration + 1 doctest, including full MCP protocol roundtrip via duplex transport.

### Integration Points

- Depends on `aios-protocol` (canonical tool contract).
- Consumed by Arcan via `arcan-harness` bridge (Praxis is the canonical tool backend).
- Architecture dependency audit enforces isolation from Arcan/Lago/Autonomic.

### Known Gaps

- Not yet wired into Arcan (arcan-harness now bridges to Praxis tools).
- No integration tests with live external MCP servers (roundtrip tests use in-process duplex transport).

## Vigil

### Observability Foundation

- OpenTelemetry-native tracing and GenAI metrics for the Agent OS.
- 1 crate: `vigil` (56 tests + 2 ignored).
- Depends only on `aios-protocol` — no dependency on Arcan, Lago, Autonomic, or Praxis.
- Implements contract-derived spans (EventKind → OTel spans), GenAI semantic conventions (`gen_ai.*` attributes), and dual-write architecture (OTel spans + EventEnvelope trace context).

### Components

- **config**: `VigConfig` with env var overrides (`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`, `VIGIL_LOG_FORMAT`, `VIGIL_CAPTURE_CONTENT`, `VIGIL_SAMPLING_RATIO`).
- **semconv**: GenAI semantic conventions (`gen_ai.*`), Life attributes (`life.*`), Autonomic attributes (`autonomic.*`), Lago attributes (`lago.*`).
- **envelope/jsonl/pricing**: typed LLM call envelope, best-effort JSONL dual-write, and local model-pricing snapshot for response-side cost estimation.
- **spans**: Contract-derived span builders (`agent_span`, `phase_span`, `chat_span`, `tool_span`), knowledge-operation spans (`knowledge.context_build`, `knowledge.search`, `knowledge.lint`), typed LLM envelope attributes, and trace context helpers (`current_trace_context`, `write_trace_context`, `extract_trace_context`).
- **metrics**: `GenAiMetrics` — OTel instruments for token usage, operation duration, LLM request outcomes, estimated cost, tool executions, budget gauges, and mode transitions.

### Integration Points

- Depends on `aios-protocol` (canonical contract — `EventEnvelope`, `LoopPhase`, `TokenUsage`).
- Consumed by Arcan provider adapters for chat-span enrichment, aggregate GenAI/Vigil metrics, local JSONL LLM call artifacts, and serialized cost-envelope persistence through `ModelCompletion`.
- Graceful degradation: structured logging via `tracing-subscriber` when no OTLP endpoint is configured.

### Known Gaps

- No OTLP smoke test in CI.
- OpenAI-compatible and Anthropic provider paths populate finish reason, and populate time-to-first-token where streaming exposes a first content/tool delta. OpenAI-compatible non-streaming calls also populate retry counts from the provider retry loop. Fallback and circuit-breaker fields remain present but defaulted until a routing/circuit subsystem owns those decisions. PII and redaction fields remain schema-ready but are not yet populated by a sanitizer.

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
- arcan-spaces bridge uses mock hub only — concrete SpacetimeDB SDK adapter not yet implemented. (SpacetimeDB HTTP adapter COMPLETE — `SpacetimeDbClient` via REST API with backend selection).

## Architecture Scorecard

- Agent loop: 9/10 | Persistence: 10/10 | Tool harness: 9/10
- Memory: 8/10 | Context quality: 9/10 | Self-learning: 2/10 — EGRI substrate wired (autoany-aios + autoany-lago adapters), cross-run inheritance available. No live self-improvement loop yet.
- Observability: 2/10 | Security: 4/10 | Operational tooling: 8/10

---

## Remaining Work (Post-Baseline)

The baseline runtime architecture is in place and validated. Remaining work is additive:

1. Cross-project golden fixture expansion for replay determinism breadth (R1, COMPLETE — branch-merge, forward-compat-evolution, stream cursor/replay invariants).
2. Observability depth expansion (metrics/traces across runtime and adapters) (R2, FOUNDATION COMPLETE — Vigil crate with OTel tracing, GenAI metrics, contract-derived spans; integration into runtime projects pending).
3. Security hardening beyond current software-level sandbox controls (R3, PLANNED).
4. Memory and learning depth (R4, PLANNED).
5. Controller plane / Autonomic integration — Phase 0 COMPLETE (5 crates, 69 tests, Lago wired, hysteresis active); Phase 1 COMPLETE: Arcan advisory client wired (`AutonomicPolicyAdapter` decorator, 6 tests); **Phase 2 COMPLETE**: Embedded controller (dual-mode adapter, economic gate handle wired to provider, token usage flow, 24 new tests — R5 DONE).

### Infrastructure (2026-03-01)

- [x] Root PLANS.md created for execution tracking.
- [x] docs/control/ARCHITECTURE.md expanded (was stub).
- [x] docs/control/OBSERVABILITY.md expanded (was stub).
- [x] Recovery script (scripts/control/recover.sh) upgraded with diagnostics.
- [x] CLI E2E tests wired (scripts/control/cli_e2e.sh exercises lago-cli, lagod, arcan).
- [x] Web E2E tests wired (scripts/control/web_e2e.sh exercises arcand HTTP API).
- [x] CI workflows updated for CLI and Web E2E pipelines.
- [x] MemoryPort removed from canonical port list (was removed from aios-protocol 2026-02-28).

## Baseline Completion Checklist

- [x] Single canonical contract (`aios-protocol`) across projects.
- [x] Single canonical runtime engine (`aios-runtime`) in production host path.
- [x] Lago-backed canonical persistence adapter in active runtime path.
- [x] Canonical session API routed by `arcand` and hosted by `arcan`.
- [x] Architecture dependency gate integrated in audit flow.
- [x] Workspace build/lint/test gates green.
- [x] Conformance harness green.
