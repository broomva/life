# Agent OS: Development Roadmap

## Guiding Principles

1. **Progressive value delivery** — Each phase produces a usable, demonstrable artifact
2. **Test-first validation** — Every feature has tests before it's considered done
3. **Integration over isolation** — Prioritize connecting what exists over building new things
4. **Single-user first** — Prove the core loop before multi-tenancy or distribution
5. **Vertical slice** — End-to-end functionality beats horizontal layer completeness
6. **Memory is the unlock** — Without persistence and context, agents are stateless toys
7. **Contract-first unification** — One canonical event model across all projects (aiOS defines, Arcan/Lago implement)
8. **Homeostasis is a kernel service** — Agent stability is not optional; it's infrastructure

---

## Current State (v0.2.0)

**What works**: The core agent loop is fully functional. A user can start the Arcan daemon, send a chat message, have it processed by an LLM (Anthropic or mock), execute 9 tools through the sandbox, persist all events to Lago's redb journal (ACID), and stream responses via multi-format SSE. Sessions are replayable from the event journal. 386 tests pass across 16 crates (~16.5K lines of Rust).

**What's incomplete**: Memory is basic key-value only (no OM, semantic, or graph recall). No context compiler for intelligent prompt assembly. No self-learning pipeline. Skills are FS-based, not Lago artifacts. CLI stubs. Blob storage exists but unused in file endpoints. No default policy rules loaded.

**Architecture scorecard**:
| Dimension | Score | Blocker |
|-----------|-------|---------|
| Agent loop | 9/10 | No parallel tools |
| Persistence | 10/10 | — |
| Tool harness | 9/10 | No OS-level sandbox |
| Memory | 2/10 | Basic key-value only |
| Context quality | 3/10 | No compiler/assembly |
| Self-learning | 0/10 | Not started |
| Observability | 2/10 | Basic tracing only |
| Security | 4/10 | Soft sandbox, no auth |
| Operational tooling | 2/10 | CLI stubs |

---

## Phase 0: Stabilization & Test Coverage (Weeks 1-2)

**Goal**: Make what exists robust, tested, and demonstrable.

### M0.1 — Fix Broken Tests & Wire Unused Components
**Priority**: CRITICAL | **Value**: Confidence in existing code

- [ ] Fix `lago-api/tests/e2e_sessions.rs` (21 compilation errors)
- [ ] Wire BlobStore to file endpoints (FileWrite stores blob, FileRead retrieves)
- [ ] Store `blob_hash` in FileWrite events
- [ ] Verify blob deduplication: same content → one blob
- [ ] Cross-project integration test: arcan POST /chat → lago journal → verify

**Validation**: All tests green across both projects. File ops use content-addressed storage.

### M0.2 — Complete lago-cli
**Priority**: HIGH | **Value**: Developer experience

- [ ] `lago session list/create/show` — working session management
- [ ] `lago log --session <id> [--type <type>]` — filtered event log
- [ ] `lago cat <path> --session <id>` — print file from manifest
- [ ] `lago branch list/create` — branch management

**Validation**: `lago log --session s1 --type ToolInvoke` shows filtered events.

### M0.3 — Default Policy Rules
**Priority**: MEDIUM | **Value**: Baseline security posture

- [ ] Create `default-policy.toml` (deny destructive patterns, require approval for rm -rf etc.)
- [ ] Load at startup via `--policy` flag
- [ ] Test: denied tool call returns error, allowed call succeeds

**Validation**: `arcan --policy default-policy.toml` enforces rules out of the box.

### M0.4 — Demo Script
**Priority**: HIGH | **Value**: Stakeholder demonstration

- [ ] `scripts/demo.sh` starts Arcan with mock, runs sample session
- [ ] Shows: creation → tool execution → persistence → session replay
- [ ] Includes curl commands for manual exploration

---

## Phase 1: Memory & Context Compiler (Weeks 3-5)

**Goal**: Agents remember across sessions and receive intelligently assembled context.
**Why first**: Everything in the vision (self-learning, heartbeats, skills, governance)
depends on memory working. This is the highest-leverage phase.

### M1.1 — Memory Event Types & Scopes
**Priority**: CRITICAL | **Value**: Foundation for all memory features

New `EventPayload` variants in lago-core:
- `ObservationAppended`, `ReflectionCompacted`
- `MemoryProposed`, `MemoryCommitted`, `MemoryTombstoned`

Memory scopes: session / user / agent / org (as Lago namespace segments)
Each memory item: scope, tenant_id, principal_id, sensitivity, ttl, source_event_ids

- [ ] Add event types with forward-compatible serialization
- [ ] Add scope types and governance metadata
- [ ] Policy engine rules for memory read/write

**Validation**: Memory events serialize/deserialize. Policy blocks unauthorized reads.

### M1.2 — Observational Memory (OM) Baseline
**Priority**: CRITICAL | **Value**: Always-on memory, no infra required

Inspired by [Mastra Observational Memory](https://mastra.ai/docs/memory/observational-memory):
- **Observer job**: Consume Lago event stream → emit dense observations
- **Reflector job**: Compact observations into summaries
- Both run in trusted daemon plane (outside sandbox)
- Storage as Lago artifacts with provenance links

- [ ] Implement Observer (high-frequency, cheap)
- [ ] Implement Reflector (lower-frequency, heavier)
- [ ] Store observations + reflections as Lago blobs with events
- [ ] Auto-load today + yesterday observations on session start

**Validation**: New sessions see prior observations. Reflections compress over time.

### M1.3 — Context Compiler
**Priority**: CRITICAL | **Value**: Agent context quality = agent quality

Deterministic assembly in fixed order with per-block size budgets:
1. Persona (SOUL / identity) — always included
2. Operating rules (AGENTS / TOOLS) — always included
3. Memory (curated + recent daily + OM reflections) — budget-capped
4. Retrieval (semantic + graph, if enabled) — budget-capped
5. Workspace snapshot (targeted file excerpts) — budget-capped
6. Task/plan state — always included

- [ ] Implement `ContextCompiler` with block types and budgets
- [ ] Integrate into Orchestrator (replaces current message list)
- [ ] Persona/rules files stored as Lago artifacts (not loose FS files)
- [ ] Size budget enforcement with "drop least-relevant first" policy

**Validation**: Context assembly is deterministic, budget-aware, and auditable.

### M1.4 — Governed Memory Tools
**Priority**: HIGH | **Value**: Agent can remember, but safely

- [ ] `memory.query(scope, k)` → approved snippets + provenance
- [ ] `memory.propose(entries)` → agent proposes memory writes
- [ ] `memory.commit(proposal_id)` → policy-gated promotion
- [ ] All writes attributed, scoped, reversible (tombstone), auditable

**Validation**: Agent proposes memory, policy approves, Lago persists with full provenance.

---

## Phase 2: Self-Learning & Heartbeats (Weeks 6-7)

**Goal**: Agents improve over time through governed rule updates.
Heartbeats enable autonomous maintenance and memory consolidation.

### M2.1 — Learning Capture & Consolidation
**Priority**: HIGH | **Value**: Core differentiator

Inspired by [OpenClaw self-improving-agent](https://docs.openclaw.ai/concepts/memory):
- Capture: tool failures, user corrections, repeated confusion → structured entries
- Consolidate: dedupe/cluster → rule proposals with provenance
- Promote: policy-gated commit → versioned rulesets with rollback

- [ ] Learning logger (append to `/learnings/*.md` in Lago)
- [ ] Consolidation job (cluster → generate proposals)
- [ ] Promotion controller (validate → policy gate → commit or request approval)
- [ ] Versioned rulesets with effective dates and rollback pointers

**Validation**: Learnings captured. Proposals generated. Only promoted rules take effect.

### M2.2 — Heartbeat Scheduler
**Priority**: MEDIUM | **Value**: Autonomous maintenance

Inspired by [OpenClaw heartbeats](https://docs.openclaw.ai/gateway/heartbeat):
- `RunTrigger::Heartbeat` — periodic agent turns
- Cheap deterministic checks first (queue depth, disk, stuck tools)
- Only call LLM if there's something to interpret
- `HeartbeatOk` for silent no-op
- Schedule OM reflection and learning consolidation on heartbeat

- [ ] Implement heartbeat scheduler in arcand
- [ ] `HEARTBEAT.md` checklist support
- [ ] Wire OM reflector and learning consolidator to heartbeat ticks

**Validation**: Heartbeats fire on schedule. Memory consolidation runs automatically.

### M2.3 — Queue & Steering Semantics
**Priority**: MEDIUM | **Value**: Safe concurrent operation

Inspired by [OpenClaw queue](https://docs.openclaw.ai/concepts/queue) + [Pi agent loop](https://github.com/badlogic/pi-mono):
- Queue modes: collect / steer / followup / interrupt
- Steering at tool boundaries (safe preemption points)
- Prevents concurrent runs from corrupting shared state

- [ ] Implement run queue with lane-aware FIFO
- [ ] Implement steering injection at tool boundaries
- [ ] Implement followup scheduling

**Validation**: Steering cancels remaining tools. Followups queue correctly.

### M2.4 — Approval Workflow (Interactive)
**Priority**: HIGH | **Value**: Human-in-the-loop for risky actions

- [ ] `POST /approve` endpoint
- [ ] Tokio oneshot channel for pause/resume
- [ ] Auto-deny after configurable timeout (5min default)
- [ ] Approval events persisted to Lago

**Validation**: RequireApproval pauses run. POST /approve resumes it.

---

## Phase 3: Skills as Lago Artifacts + Multi-Provider (Weeks 8-10)

**Goal**: Skills are versioned, reproducible artifacts in Lago. Multiple LLMs work.

### M3.1 — Skill Package Format
**Priority**: HIGH | **Value**: Reproducible agent capabilities

- [ ] Skill manifest schema (skill_id, version, capabilities, prompt_blocks, tools, policy)
- [ ] Lago events: `SkillInstalled`, `SkillActivated`, `SkillRemoved`
- [ ] Immutable versioned snapshots (path: `/skills/<id>/versions/<hash>/...`)
- [ ] Skills ingest pipeline (fetch → verify → store → register)

**Validation**: `SkillInstalled` event in Lago. Skill replayable from journal.

### M3.2 — Skill Loader in Harness
**Priority**: HIGH | **Value**: No direct FS reads for agent config

- [ ] Compile active skill set from Lago at run start
- [ ] Assemble prompt blocks into context compiler
- [ ] Build tool registry from skill tool schemas
- [ ] `LagoResolver` trait replaces `PathBuf` for all config loading

**Validation**: Agent tools come from Lago-backed skills. No direct FS reads.

### M3.3 — OpenAI Provider + Fallback Routing
**Priority**: HIGH | **Value**: Provider flexibility

- [ ] OpenAI chat completions provider (function calling → ToolCall)
- [ ] Provider fallback: Claude → OpenAI (configurable priority)
- [ ] Retry middleware: exponential backoff for 429/5xx/timeout
- [ ] Circuit breaker on provider outages

**Validation**: `OPENAI_API_KEY=... cargo run -p arcan` works with GPT-4.

### M3.4 — Cost Controls
**Priority**: MEDIUM | **Value**: Predictable spend

- [ ] Per-session token budgets
- [ ] Budget exceeded → run stops with clear error
- [ ] Usage accounting events in Lago

**Validation**: Session with 1K token budget stops after budget exhausted.

---

## Phase 4: Observability & Operational Tooling (Weeks 11-13)

### M4.1 — OpenTelemetry + GenAI Conventions
- [ ] `tracing-opentelemetry` + OTLP exporter
- [ ] Spans: run → turn → tool → llm → memory
- [ ] GenAI semantic conventions (token usage, model, prompt hash)
- [ ] Structured audit events in Lago

**Validation**: Traces visible in Jaeger for complete agent session.

### M4.2 — Replay Debugger
- [ ] `lago replay --session s1 --to-seq 42` — reconstruct state at any point
- [ ] `lago diff --session s1 --from 10 --to 42` — state changes between sequences
- [ ] API: `GET /v1/sessions/{id}/state?at_seq=42`

**Validation**: Can reconstruct exact agent state at any historical point.

### M4.3 — Streaming Protocol Completion
- [ ] text-start/end, tool-start/end boundary signals
- [ ] Monotonic SSE event IDs + `retry:` headers
- [ ] Step markers for Vercel AI SDK compatibility

**Validation**: AI SDK `useChat` works with advanced features.

---

## Phase 5: Governance & Security Hardening (Weeks 14-16)

### M5.1 — Authentication & Authorization
- [ ] JWT/API key middleware on all HTTP endpoints
- [ ] Session isolation by user/tenant
- [ ] RBAC enforcement per request

### M5.2 — Secret Hygiene
- [ ] Redaction rules: never persist secrets in Lago journal
- [ ] Env var scrubbing in tool execution events
- [ ] Key rotation + audit trail

### M5.3 — Container Sandbox
- [ ] BubblewrapRunner or Apple Containers for tool execution
- [ ] Network egress deny-by-default + explicit allowlist
- [ ] CPU/mem/time limits per tool

### M5.4 — Data Governance
- [ ] Retention policies per scope/tenant
- [ ] Crypto-shredding for right-to-delete
- [ ] Encryption at rest for sensitive blobs

---

## Phase 6: Universal Data Plane & Platform (Weeks 17+)

Future work, driven by concrete need:

| Feature | Trigger |
|---------|---------|
| Lago Catalog (Assets + Representations) | Multi-tenant deployment |
| Lineage events (OpenLineage) | Compliance/audit requirement |
| Vector index (LanceDB/Qdrant) | Agent KB exceeds OM capacity |
| Graph memory (Mem0-style) | Multi-hop reasoning needed |
| DataFusion query layer | Tabular data analysis |
| Delta/Iceberg representations | Lakehouse interop |
| Arrow Flight transport | High-throughput data streaming |
| Multi-agent orchestration | Fleet operation |
| Frontend SDK (Next.js + Vercel AI SDK) | User-facing product |
| WASM tool execution | Portable sandboxed skills |

---

## Feature Priority Matrix

| Feature | Impact | Effort | Priority | Phase |
|------------------------------|--------|--------|----------|-------|
| Fix broken tests + wire blobs | High | Low | P0 | 0 |
| lago-cli completion | High | Medium | P0 | 0 |
| Default policy rules | Medium | Low | P0 | 0 |
| **Memory events + scopes** | Critical | Medium | **P0** | **1** |
| **Observational Memory** | Critical | Large | **P0** | **1** |
| **Context compiler** | Critical | Large | **P0** | **1** |
| Governed memory tools | High | Medium | P0 | 1 |
| Learning pipeline | High | Large | P1 | 2 |
| Heartbeat scheduler | Medium | Medium | P1 | 2 |
| Queue/steering semantics | Medium | Large | P1 | 2 |
| Approval workflow | High | Large | P1 | 2 |
| Skill manifest + Lago storage | High | Medium | P1 | 3 |
| Skill loader in harness | High | Large | P1 | 3 |
| OpenAI provider + retry | High | Medium | P1 | 3 |
| Cost controls | Medium | Medium | P2 | 3 |
| OpenTelemetry + GenAI | Medium | Large | P2 | 4 |
| Replay debugger | High | Medium | P2 | 4 |
| Streaming boundary signals | High | Medium | P2 | 4 |
| Auth + multi-tenancy | High | High | P3 | 5 |
| Container sandbox | Medium | XL | P3 | 5 |
| Secret redaction | Medium | Medium | P3 | 5 |

---

## Value Milestones

| Milestone | Description | Demonstrates |
|-----------|-----------------------------------------------------|--------------------------------|
| **V1** | Agent runs, persists events, replays sessions | Core loop + event sourcing |
| **V1.5** | Tests green, CLI works, blob storage wired | ← YOU ARE HERE (Phase 0) |
| **V2** | Agent remembers across sessions, context compiled | Memory + context quality |
| **V3** | Agent learns from mistakes, heartbeats maintain | Self-improvement + autonomy |
| **V4** | Skills versioned in Lago, multi-provider | Reproducibility + flexibility |
| **V5** | Full tracing, replay debugging, streaming polish | Operability |
| **V6** | Auth, secrets, container sandbox | Production security |
| **V7** | Universal data plane, multi-agent, frontend | Platform |

**Current position**: V1 is complete. Development should stabilize at V1.5 (Phase 0)
then advance to V2 (Phase 1 — memory) as the highest-leverage next step.

---

---

## Phase 7: Agent OS Unification (Ongoing — Parallel Track)

**Goal**: Unify aiOS + Arcan + Lago + Autonomic into a cohesive Agent OS with one canonical contract, shared event model, and proper separation of concerns.

### Phase 7A: Contract Extraction (aiOS)
**Priority**: CRITICAL | **Effort**: 2-3 sessions

- [ ] Create `agent-kernel` crate in aiOS with canonical types
- [ ] Merge three event models (Lago 35+, Arcan 24, aiOS 40+) into canonical ~55-variant `EventKind`
- [ ] Define `AgentStateVector`, `BudgetState`, `OperatingMode`, `GatingProfile`
- [ ] Define minimal kernel traits (`Journal`, `PolicyGate`, `Harness`, `MemoryStore`, `AutonomicController`)
- [ ] Add `BlobRef`, `Intent` lifecycle, `StatePatch` with `PatchOp`
- [ ] Forward-compatible `Custom` variant for unknown event types
- [ ] Conformance tests (schema roundtrip, provenance, replay)
- [ ] Refactor remaining aiOS crates to depend on `agent-kernel`

### Phase 7B: Lago Alignment
**Priority**: HIGH | **Effort**: 2-3 sessions

- [ ] Add `agent-kernel` git dependency to Lago workspace
- [ ] Align `lago-core::EventPayload` with `agent-kernel::EventKind`
- [ ] Update SSE format adapters for new event variants
- [ ] Create `lago-memory` crate (MemoryStore impl, retrieval, decay, consolidation)
- [ ] Update integration tests

### Phase 7C: Arcan Alignment
**Priority**: HIGH | **Effort**: 2-3 sessions

- [ ] Add `agent-kernel` git dependency to Arcan workspace
- [ ] Map `AgentEvent` to canonical `EventKind` at boundary
- [ ] Wire `AgentStateVector` + `BudgetState` into orchestrator
- [ ] Add homeostasis event emission (StateEstimated, BudgetUpdated, ModeChanged)
- [ ] Add `GatingProfile` enforcement at harness boundary
- [ ] Simplify `arcan-lago` bridge (both sides now canonical)
- [ ] Update AI SDK v6 adapter for new event variants

### Phase 7D: Autonomic MVP (NEW Project)
**Priority**: MEDIUM | **Effort**: 2-3 sessions

- [ ] Create `autonomic` repo with workspace
- [ ] Rule-based controller with hysteresis (mode switching, budget throttles)
- [ ] Heartbeat trigger handling (schedule + event-based)
- [ ] `GatingProfile` output enforced at Arcan harness boundary
- [ ] Memory maintenance triggers (consolidation, compaction, deprecation)

### Phase 7E: Memory Service + Forgetting
**Priority**: MEDIUM | **Effort**: 2-3 sessions

- [ ] Decay scoring in `lago-memory`
- [ ] Tombstone lifecycle (propose → approve → apply)
- [ ] Consolidation pipeline (event window → observations → summaries)
- [ ] Context assembly with token budgets + scope filtering
- [ ] Wire consolidation triggers into autonomic heartbeat

### Phase 7F: Conformance + Golden Tests
**Priority**: MEDIUM | **Effort**: 1-2 sessions

- [ ] Conformance test suite in `agent-kernel`
- [ ] Golden replay tests (curated session → deterministic verification)
- [ ] Cross-project integration tests (Arcan → Lago → replay → verify)

---

## Design Reference

The vision for the full platform architecture is documented in:
- `docs/CONTRACT.md` — Canonical event taxonomy, schema versioning, invariants, replay rules
- `docs/raw-dumps/lago-lakehouse-memory.md` — Comprehensive design notes covering:
  - Three planes model (data/compute/control)
  - Memory architecture (OM + semantic + graph)
  - Self-learning pipeline (capture → consolidate → promote)
  - Skills as Lago artifacts
  - Context compiler design
  - Heartbeat/queue semantics
  - Lakehouse governance (Unity Catalog-inspired)
  - Universal storage abstractions (Assets + Representations)
