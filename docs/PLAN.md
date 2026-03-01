# Arcan + Lago: Consolidated Development Plan

**Last updated**: 2026-02-28
**Version**: 0.2.0

## Progress Tracker

| Phase | Milestone | Status | Tests Added |
|-------|-----------|--------|-------------|
| 0.1 | Context Window Management | DONE | +11 |
| 0.2 | Integration Test Infrastructure | DONE | +18 (10 arcand + 8 lago) |
| 0 | Stabilization & Test Coverage | **DONE** | +4 (1 policy test, 3 E2E integration) |
| 1 | Memory & Context Compiler | **DONE** | +82 |
| 2.1 | Learning Capture | DONE | +8 |
| 2.4 | Heartbeat Scheduler | DONE | +9 |
| 2.6 | Approval Workflow | DONE | +14 |
| 2 | Self-Learning & Heartbeats (remaining: 2.2, 2.3, 2.5) | Planned | target: +23 |
| P1 | AI SDK v6 UI Message Stream Protocol | **DONE** | +10 (18 unit + 5 integration, net of replaced v5 tests) |
| P2.2 | OpenAI + Ollama Provider + Retry Logic | **DONE** | +13 |
| P2 | Safety & Multi-Provider (remaining: edits, sandbox) | Planned | target: +25 |
| P3 | Session Management & Clients | Planned | target: +24 |
| P4 | Advanced Runtime (subagents, web client, WASM) | Future | — |

**Current test counts**: aiOS 61 passing, Arcan 236 passing (+1 ignored), Lago 299 passing, Total 596 passing (+1 ignored)
**Target**: 700+ tests by end of Phase 5

---

## Current State Assessment

**What works end-to-end**: POST /chat -> agent loop -> tools -> Lago journal -> SSE stream.
Core event sourcing, blob storage, FS branching, policy engine, gRPC ingest, multi-format SSE
are all implemented and tested across 19 crates (~27.8K lines Rust).

**Critical gaps vs the vision** (from `docs/raw-dumps/lago-lakehouse-memory.md`):
1. **Memory**: Agents have basic key-value memory only; no observational memory, semantic recall, or graph memory
2. **Context compiler**: No deterministic assembly of persona + rules + memory + retrieval + workspace
3. **Self-learning**: No capture -> consolidate -> promote pipeline with governance gates
4. **Skills as Lago artifacts**: Skills discovered from local FS, not versioned in Lago
5. **Observability**: Basic tracing only, no OTel + GenAI semantic conventions
6. ~~**Operational tooling**: CLI stubs, no replay debugger~~ CLI fully implemented; replay debugger still needed

---

## Phase 0: Stabilization & Test Coverage — COMPLETE

**Goal**: Fix what's broken, establish integration test infrastructure, wire unused components.

### 0.1 Fix Lago E2E Tests — DONE (already passing)
- E2E tests were already fixed and compiling. 25 E2E tests pass.

### 0.2 Wire Blob Storage to File Operations — DONE (already wired)
- Blob storage was already fully wired: `write_file` → `blob_store.put()`, `read_file` → `blob_store.get()`, `patch_file` reads/writes through blob store
- 17 E2E tests confirm round-trip through blob storage

### 0.3 Complete lago-cli Command Handlers — DONE (already implemented)
- All command handlers were already fully implemented:
  - `session` (create/list/show with API client + DB fallback)
  - `log` (comprehensive formatting for all EventPayload variants)
  - `cat` (manifest reconstruction → blob fetch → stdout)
  - `branch` (create with fork_at support + formatted list)

### 0.4 Create Default Policy Rules — DONE
- Created `default-policy.toml` with 5 rules, 3 roles, 2 hooks
- `lago init` now scaffolds `policy.toml` alongside `lago.toml`
- `lagod` loads policy on startup when file exists
- Compile-time test verifies the default policy file parses correctly

### 0.5 Cross-Project Integration Test — DONE
- Added `arcan-lago/tests/end_to_end.rs` with 3 comprehensive tests:
  - `full_session_lifecycle_round_trip`: 9 events → journal → projection → verify state+history
  - `policy_middleware_with_default_rules`: deny-shell, allow-filesystem, default-allow
  - `multiple_sessions_isolated`: concurrent sessions share journal without interference

### 0.6 Cross-Project Integration Test
- Full round-trip: arcan POST /chat -> lago journal -> verify event sequence
- Session replay: create session -> reload -> verify state matches

### 0.5 Default Policy Rules
- Create `default-policy.toml` with baseline rules (deny dangerous patterns, require approval for destructive ops)
- Load at Arcan startup via `--policy` flag

**Validation**: Both projects pass all tests. CLI usable for inspection. Blob storage wired.

---

## Phase 1: Memory & Context Compiler — COMPLETE

**Goal**: Agents remember across sessions and receive intelligently assembled context.
This is the highest-leverage phase — everything else depends on memory working.

**Result**: +57 tests (415 → 472). All milestones complete, zero warnings.

### 1.1 Memory Event Types in Lago — DONE
Added 5 new `EventPayload` variants to `lago-core`:
- `ObservationAppended { scope, observation_ref, source_run_id }`
- `ReflectionCompacted { scope, summary_ref, covers_through_seq }`
- `MemoryProposed { scope, proposal_id, entries_ref, source_run_id }`
- `MemoryCommitted { scope, memory_id, committed_ref, supersedes }`
- `MemoryTombstoned { scope, memory_id, reason }`

Plus `MemoryScope` enum (Session/User/Agent/Org), `MemoryId` typed ID. 13 new tests in lago-core.

### 1.2 Observational Memory Baseline — DONE
Created in `arcan-lago`:
- `Observer` (implements `Projection`) — extracts observations from Message, ToolResult, Error, StatePatched events
- `Reflector` — text-based compaction (no LLM call needed)
- `MemoryProjection` (implements `Projection`) — queryable memory state from events, handles tombstones and supersedes
- `MemoryScopeConfig` + `MemoryEntry` — runtime configuration and entry types
20 new tests across observation.rs, memory_projection.rs, memory_scope.rs.

### 1.3 Context Compiler — DONE
Created `context_compiler.rs` in `arcan-core`:
- `ContextBlockKind`: Persona, Rules, Memory, Retrieval, Workspace, Task
- `compile_context()`: fixed assembly order, per-block token budgets, priority-based overflow dropping, word-boundary truncation
- Persona block never dropped (highest priority)
- Wired into `OrchestratorConfig` as optional field
14 new tests.

### 1.4 Governed Memory Tools — DONE
Created 3 `Tool` implementations in `arcan-lago`:
- `MemoryQueryTool` — reads from `MemoryProjection` (Arc<RwLock>)
- `MemoryProposeTool` — writes `MemoryProposed` event to Journal
- `MemoryCommitTool` — writes `MemoryCommitted` event after policy check
10 new tests using RedbJournal in tempdir.

**Validation**: Agent has event-sourced, governed memory. Context assembly is structured and budget-aware.

---

## Phase 2: Self-Learning & Heartbeats (Weeks 6-7)

**Goal**: Agents improve over time. Heartbeats enable maintenance.

### 2.1 Learning Capture
- Structured entries on tool failures, user corrections, repeated confusion
- Written to `/learnings/{errors,learnings,features}.md` in Lago

### 2.2 Consolidation Job
- Dedupe/cluster learnings -> generate rule proposals
- Proposals stored in Lago with provenance links: `/proposals/<id>/rules.patch`

### 2.3 Promotion Gate
- Policy-gated commit: `rules.commit(proposal_id)`
- Only promoted rules take effect; full rollback possible
- Human approval optional (configurable)

### 2.4 Heartbeat Scheduler
- `RunTrigger::Heartbeat` periodic turns
- Cheap deterministic checks first (queue depth, stuck tools, disk quota)
- Only call LLM if there's something to interpret
- `HeartbeatOk` for silent no-op

### 2.5 Queue & Steering Semantics
- `collect` / `steer` / `followup` / `interrupt` modes
- Steering at tool boundaries (safe preemption points)
- Prevents concurrent state corruption

### 2.6 Approval Workflow ✅ DONE (2026-02-15)
- ~~`POST /approve` endpoint with approval queue~~ DONE
- ~~`RequireApproval` pauses run (tokio oneshot), resumes on approval~~ DONE
- ~~Auto-deny after configurable timeout (5min default)~~ DONE
- `ApprovalGate` in arcan-lago with `ApprovalGateHook`/`ApprovalResolver` traits in arcan-core
- `POST /approve` + `GET /approvals` HTTP endpoints in arcand
- 14 new tests (7 gate, 2 event-map, 2 middleware, 3 HTTP)

**Validation**: Agent learns from mistakes via governed pipeline. Heartbeats fire.
Approval workflow is interactive.

---

## Phase 3: Skills as Lago Artifacts + Multi-Provider (Weeks 8-10)

**Goal**: Skills are versioned, reproducible. Multiple LLM providers work.

### 3.1 Skill Manifest Schema
```json
{
  "skill_id": "...",
  "version": "1.0.0",
  "requires": { "capabilities": [...], "approvals": [...] },
  "prompt_blocks": { "system": "prompts/system.md" },
  "tools": "tools/schema.json",
  "policy": "policies/allowlist.json"
}
```
- Events: `SkillInstalled`, `SkillActivated`, `SkillRemoved`
- Immutable versioned snapshots in Lago

### 3.2 Skill Loader in Harness
- Compile active skill set from Lago
- Assemble prompt blocks into context compiler
- Build tool registry from skill tool schemas
- `LagoResolver` trait instead of `PathBuf` for all config loading

### 3.3 Skills Ingest
- Fetch bundle -> verify signature/hash -> store in Lago -> register events
- Policy bind: map skill capabilities to Lago policy rules

### 3.4 OpenAI Provider
- Chat completions API with function calling -> ToolCall mapping
- Fallback routing: Claude -> OpenAI (configurable)

### 3.5 Provider Retry & Cost Tracking
- Exponential backoff for 429/5xx/timeout
- Circuit breaker on provider errors
- Per-session token budgets; budget exceeded -> run stops

**Validation**: Skills are Lago artifacts with full provenance. OpenAI works.
Cost tracked per session.

---

## Phase 4: Observability & Operational Tooling (Weeks 11-13)

### 4.1 OpenTelemetry Integration
- Spans: run -> turn -> tool -> llm -> memory
- `tracing-opentelemetry` + OTLP exporter

### 4.2 GenAI Semantic Conventions
- Token usage, model, prompt hash in span attributes
- OpenTelemetry GenAI conventions for LLM spans

### 4.3 Structured Audit Events
- Who triggered what, what data touched
- Queryable from Lago events

### 4.4 Replay CLI
- `lago replay <session>` reconstructs agent view at event N
- `lago diff --from N --to M` shows state changes

### 4.5 Streaming Boundary Signals
- text-start/end, tool-start/end SSE events
- SSE event IDs + `retry:` headers for reconnection

**Validation**: Full distributed tracing. Replay works. AI SDK clients handle boundaries.

---

## Phase 5: Governance & Security Hardening (Weeks 14-16)

### 5.1 HTTP AuthN/AuthZ (JWT/API key middleware)
### 5.2 Secret Redaction (never persist secrets in Lago journal)
### 5.3 Container Sandbox (bubblewrap or Apple Containers)
### 5.4 Retention Policies (per-scope TTL + crypto-shredding)
### 5.5 Network Egress Policy (deny-by-default for tool execution)

---

## Phase 6: Universal Data Plane & Platform (Weeks 17+)

Future work, driven by need:
- Lago Catalog (Assets + Representations, Unity Catalog-like hierarchy)
- Lineage events (OpenLineage-compatible)
- Semantic retrieval (vector index: LanceDB or Qdrant)
- Graph memory (Mem0-style entity/relationship extraction)
- DataFusion query layer (optional embedded analytics)
- Delta/Iceberg representations (lakehouse interop)
- Arrow Flight transport (high-throughput data streaming)
- Multi-agent orchestration (fleet operation)

---

## Critical Path

```
Phase 0 (stabilize)
    |
Phase 1 (memory + context)  <-- biggest value unlock
    |
Phase 2 (self-learning + heartbeats)
    |
Phase 3 (skills + multi-provider)  <-- can parallel with Phase 2
    |
Phase 4 (observability)
    |
Phase 5 (security)
    |
Phase 6 (platform)
```

## Dependency Chain

```
lago-core events  -->  arcan-memory (new)  -->  context compiler  -->  self-learning
                  \                                                  /
                   -->  skill manifest  -->  skill loader  ----------
                                                           \
lago-policy       -->  memory governance  -->  promotion gates
                  \
                   -->  approval workflow  -->  heartbeat scheduler
```

## Testing Strategy

| Phase | Test Type | Count Target | Validates |
|-------|-----------|-------------|-----------|
| 0 | Unit + Integration | 450+ | All crates compile, E2E round-trips |
| 1 | Memory integration | 500+ | Memory persists, scopes enforced, context assembled |
| 2 | Self-learning | 550+ | Proposals generated, gates enforced, heartbeats fire |
| 3 | Multi-provider | 600+ | Provider fallback, skill loading, cost tracking |
| 4 | Observability | 650+ | Traces correlate, replay matches, boundaries correct |
| 5 | Security | 700+ | Auth rejects unauthorized, secrets scrubbed, container isolation |

Test pyramid: ~70% unit, ~20% integration, ~10% E2E.
