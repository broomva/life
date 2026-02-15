# Arcan + Lago: Implementation Status Report

**Date**: 2026-02-15
**Version**: 0.2.0 (both projects)
**Rust Toolchain**: 1.93.0 (requires >= 1.85, Rust 2024 Edition)

---

## Health Summary

| Metric              | Arcan        | Lago         | Combined     |
|---------------------|--------------|--------------|--------------|
| Compilation         | CLEAN        | CLEAN        | CLEAN        |
| Clippy warnings     | 0            | 0            | 0            |
| Tests passing       | 240/240      | 286/286      | 526/526      |
| Tests failing       | 0            | 0            | 0            |
| Lines of Rust       | ~8,500       | ~12,200      | ~20,700      |
| Workspace crates    | 7            | 9            | 16           |

Both projects compile cleanly, pass all tests, and have zero warnings.

---

## Arcan — Crate-by-Crate Status

### arcan-core (~2,300 lines)

**Status**: FULLY IMPLEMENTED

| Module                | Lines | Description                                    | Status |
|-----------------------|-------|------------------------------------------------|--------|
| `protocol.rs`         | 355   | AgentEvent enum (incl. ApprovalRequested/Resolved), ToolCall, ToolResult, ChatMessage | Done |
| `runtime.rs`          | 1,148 | Orchestrator, AgentLoop, Tool/Middleware traits, ToolRegistry | Done |
| `state.rs`            | 288   | AppState, StatePatch (JSON merge-patch), revision tracking | Done |
| `aisdk.rs`            | 416   | Vercel AI SDK streaming format (AiSdkPart), SSE wire format | Done |
| `context_compiler.rs` | ~400  | ContextBlock, ContextCompilerConfig, compile_context() — deterministic context assembly | Done |
| `error.rs`            | 15    | CoreError enum (thiserror)                     | Done |

**Key Types**:
- `AgentEvent` — 10 variants covering full agent lifecycle (RunStarted through RunFinished)
- `Orchestrator` — Drives the LLM provider + tool execution loop with middleware pipeline
- `AgentLoop` — Session-level wrapper: reconstruct state → run orchestrator → persist events
- `Tool` trait — `definition()` + `execute()` for all tool implementations
- `Middleware` trait — `before_tool_call()` hook for policy enforcement
- `ToolRegistry` — Registered tools with definitions for LLM function-calling
- `ContextCompiler` — Deterministic context assembly with typed blocks (Persona/Rules/Memory/Retrieval/Workspace/Task), per-block token budgets, priority-based overflow

**Tests**: 60 (runtime + context_compiler)

### arcan-harness (2,167 lines)

**Status**: FULLY IMPLEMENTED

| Module       | Lines | Description                                    | Status |
|--------------|-------|------------------------------------------------|--------|
| `sandbox.rs` | 590   | SandboxTier (None/Basic/Restricted), SandboxPolicy, FsPolicy guardrails | Done |
| `fs.rs`      | 569   | Filesystem tools (ReadFile, WriteFile, ListDir, SearchFiles, PatchFile) | Done |
| `skills.rs`  | 406   | SkillRegistry, SKILL.md parsing, frontmatter YAML metadata | Done |
| `edit.rs`    | 357   | Hashline edit primitives (content-hash addressed line edits) | Done |
| `memory.rs`  | 308   | Memory tools (ReadMemory, WriteMemory, ListMemory, SearchMemory) | Done |
| `mcp.rs`     | 281   | MCP client bridge (stdio transport, McpTool wraps MCP into Arcan Tool trait) | Done |

**Key Capabilities**:
- **Sandbox tiers**: None (unrestricted), Basic (path allowlist), Restricted (read-only + approved writes)
- **FsPolicy**: Path-based allowlist/denylist with glob patterns
- **Hashline edits**: Content-hash addressed file edits — robust against concurrent modifications
- **MCP integration**: Full MCP client via `rmcp` crate, bridges MCP tools to Arcan's Tool trait
- **Skills**: Discovers SKILL.md files, parses YAML frontmatter, provides system prompt catalog
- **Memory**: File-backed memory with search (supports agent persistent knowledge)

**Tests**: 39 (sandbox, skills, memory, edit, mcp)

### arcan-provider (657 lines)

**Status**: FULLY IMPLEMENTED

| Module          | Lines | Description                                  | Status |
|-----------------|-------|----------------------------------------------|--------|
| `anthropic.rs`  | 360   | AnthropicProvider (real LLM), message building, response parsing | Done |
| `rig_bridge.rs` | 295   | Rig framework bridge (alternative provider path) | Done |

**Providers**:
- **AnthropicProvider**: Full implementation with streaming, tool use, system prompts
- **MockProvider**: Built into `arcand/src/mock.rs` (55 lines) — returns canned responses for testing
- **Rig bridge**: Alternative provider path via the `rig` crate

**Tests**: 9 (message building, response parsing for both providers)

### arcan-store (390 lines)

**Status**: FULLY IMPLEMENTED

| Module       | Lines | Description                                    | Status |
|--------------|-------|------------------------------------------------|--------|
| `session.rs` | 387   | SessionRepository trait, InMemoryRepo, JsonlRepo (file-backed) | Done |

**Implementations**:
- `InMemorySessionRepository` — HashMap-based, suitable for testing
- `JsonlSessionRepository` — File-backed append-only JSONL, one file per session

**Tests**: 7 (round-trip, isolation, head queries)

### arcan-lago (~2,800 lines) — THE BRIDGE

**Status**: FULLY IMPLEMENTED

| Module                  | Lines | Description                                | Status |
|-------------------------|-------|--------------------------------------------|--------|
| `event_map.rs`          | 437   | Bidirectional AgentEvent ↔ EventEnvelope mapping | Done |
| `repository.rs`         | 372   | LagoSessionRepository (Arcan SessionRepository backed by Lago Journal) | Done |
| `policy_middleware.rs`  | 250   | LagoPolicyMiddleware (Arcan Middleware backed by Lago PolicyEngine) | Done |
| `state_projection.rs`   | 234   | AppStateProjection (rebuild Arcan AppState from Lago events) | Done |
| `sse_bridge.rs`         | 195   | SseBridge (multi-format SSE output for AgentEvents) | Done |
| `observation.rs`        | ~250  | Observer (Projection) extracts observations; Reflector compacts them | Done |
| `memory_projection.rs`  | ~370  | MemoryProjection — queryable memory state from events | Done |
| `memory_scope.rs`       | ~100  | MemoryScopeConfig, MemoryEntry structs | Done |
| `memory_tools.rs`       | ~350  | MemoryQueryTool, MemoryProposeTool, MemoryCommitTool — governed memory tools | Done |

**Integration Points**:
- Event mapping is lossless and bidirectional with round-trip tests
- Session repo uses `tokio::runtime::Handle::block_on()` for sync/async bridge (safe because agent loop runs in `spawn_blocking`)
- Policy middleware derives risk levels from tool annotations
- State projection handles TextDelta aggregation, StatePatch application, and tool result insertion
- SSE bridge supports all 4 Lago formats (OpenAI, Anthropic, Vercel, Lago)
- **Memory system**: Observer extracts observations from events, Reflector compacts them, MemoryProjection provides queryable state, governed memory tools (query/propose/commit) write event-sourced memory

**Tests**: 58 (comprehensive coverage across all 9 modules + 3 E2E integration tests)

### arcand (288 lines)

**Status**: FULLY IMPLEMENTED

| Module      | Lines | Description                                    | Status |
|-------------|-------|------------------------------------------------|--------|
| `server.rs` | 132   | Axum HTTP server with SSE streaming, /chat and /events endpoints | Done |
| `loop.rs`   | 98    | AgentLoop execution: reconstruct → run → persist → stream | Done |
| `mock.rs`   | 55    | MockProvider for development without API keys  | Done |

**Tests**: 0 (binary crate, tested through integration)

### arcan (binary, 162 lines)

**Status**: FULLY IMPLEMENTED

The main binary wires everything together:
1. Opens Lago persistence (RedbJournal + BlobStore)
2. Discovers tools (filesystem, memory, MCP) with sandbox policies
3. Creates LagoPolicyMiddleware
4. Builds Orchestrator with provider + tools + middleware
5. Creates AgentLoop with LagoSessionRepository
6. Starts HTTP server with SSE streaming

**CLI flags**: `--port`, `--data-dir`, `--max-iterations`, `--sandbox-tier`

---

## Lago — Crate-by-Crate Status

### lago-core (~2,500 lines)

**Status**: FULLY IMPLEMENTED

**Key Types**:
- `EventPayload` — 20+ variants (Message, MessageDelta, ToolInvoke, ToolResult, RunStarted, RunFinished, FileWrite, FileDelete, SnapshotCreated, SandboxEvent, PolicyDecision, Approval, ObservationAppended, ReflectionCompacted, MemoryProposed, MemoryCommitted, MemoryTombstoned, Custom, etc.)
- `MemoryScope` — Session, User, Agent, Org scopes for memory events
- `MemoryId` — Typed ID for memory entries
- `EventEnvelope` — event_id (ULID), session_id, branch_id, seq, timestamp, payload, metadata
- `Journal` trait — Append, read, query, stream, session management (uses BoxFuture for dyn-compatibility)
- `Mount` trait — Virtual filesystem interface (defined but not yet implemented)
- Forward-compatible: unknown event types deserialize to `Custom` variant

**Tests**: 110

### lago-journal (1,660 lines)

**Status**: FULLY IMPLEMENTED

**redb Tables**:
| Table           | Key                    | Value              | Purpose                    |
|-----------------|------------------------|--------------------|----------------------------|
| `EVENTS`        | 60B compound key       | JSON EventEnvelope | Primary append-only journal |
| `EVENT_INDEX`   | event_id string        | 60B compound key   | O(1) event lookup by ID    |
| `BRANCH_HEADS`  | 52B compound key       | u64 (seq)          | Track max seq per branch   |
| `SESSIONS`      | session_id string      | JSON Session       | Session metadata & config  |
| `SNAPSHOTS`     | snapshot_id string     | zstd compressed    | Fast session restoration   |

**Features**: ACID transactions, compound key encoding, range-based queries, event streaming via broadcast channel, WAL buffering, snapshot creation/loading

**Tests**: 23 (was reported as 30 by one agent — includes key encoding, range scans, sessions)

### lago-store (680 lines)

**Status**: FULLY IMPLEMENTED

- SHA-256 hashing + zstd compression
- Git-like shard layout: `{root}/{first-2-chars}/{remaining-hash}.zst`
- Atomic writes (temp file + rename)
- Automatic deduplication
- Put, get, exists, delete operations

**Tests**: 17

### lago-fs (1,046 lines)

**Status**: FULLY IMPLEMENTED

- `Manifest` — BTreeMap-backed, implicit parent directory creation
- `BranchManager` — Copy-on-write branching with fork points
- `Tree` — Depth-first traversal, directory listing
- `Diff` — Added/Modified/Deleted entries between manifests
- `Projection` — Replay events on manifest for state reconstruction

**Tests**: 26

### lago-policy (856 lines)

**Status**: FULLY IMPLEMENTED

- `PolicyEngine` — Priority-ordered rules, evaluate → Allow/Deny/RequireApproval
- `Rule` — Glob patterns, risk levels, sandbox tier requirements, AND/OR/NOT combinators
- `RbacManager` — User → Role → Permission mapping
- `HookRunner` — Pre/Post approval and execution hooks
- TOML-based configuration

**Tests**: 34 (includes default-policy.toml parse verification)

### lago-ingest (577 lines)

**Status**: FULLY IMPLEMENTED

- gRPC bidirectional streaming via tonic
- Proto: `IngestService { Ingest(stream) → stream, CreateSession, GetSession }`
- Codec: proto ↔ core type conversions with JSON payload serialization
- Client/server implementations

**Tests**: 6 (codec roundtrips)

### lago-api (2,100+ lines)

**Status**: FULLY IMPLEMENTED

**HTTP Routes**:
| Endpoint                            | Method | Description              |
|-------------------------------------|--------|--------------------------|
| `/health`                           | GET    | Liveness probe           |
| `/v1/sessions`                      | POST   | Create session           |
| `/v1/sessions`                      | GET    | List sessions            |
| `/v1/sessions/{id}`                 | GET    | Get session              |
| `/v1/sessions/{id}/events`          | GET    | SSE stream (format negotiation) |
| `/v1/sessions/{id}/branches`        | POST   | Create branch            |
| `/v1/sessions/{id}/branches`        | GET    | List branches            |
| `/v1/sessions/{id}/files/{*path}`   | GET/PUT/DELETE/PATCH | File operations |
| `/v1/sessions/{id}/manifest`        | GET    | Full manifest            |
| `/v1/blobs/{hash}`                  | GET/PUT | Blob operations         |

**SSE Formats**: OpenAI, Anthropic, Vercel, Lago (all fully implemented with format-specific framing)

**Tests**: 54 (37 unit + 17 integration)

### lagod (282 lines)

**Status**: FULLY IMPLEMENTED

- Daemon: starts gRPC (50051) + HTTP (8080) servers
- TOML configuration with CLI overrides
- Graceful shutdown (SIGTERM/SIGINT)
- Data directory: `.lago/journal.redb` + `.lago/blobs/`

**Tests**: 0 (binary crate)

### lago-cli (420+ lines)

**Status**: FULLY IMPLEMENTED

- CLI structure and arg parsing: complete
- Commands: init (scaffolds `.lago/`, `lago.toml`, `policy.toml`), serve, session (create/list/show), branch (create/list), log (full event formatting), cat (manifest→blob→stdout)
- Session handlers have API client + DB fallback
- Log formats all EventPayload variants with color and detail

**Tests**: 0 (handlers delegate to API client and journal — tested through integration)

---

## Integration Status

| Integration Point                 | Status    | Notes                                          |
|-----------------------------------|-----------|------------------------------------------------|
| Event mapping (Arcan ↔ Lago)     | COMPLETE  | Bidirectional, lossless, tested                |
| Session persistence               | COMPLETE  | LagoSessionRepository → RedbJournal            |
| State reconstruction (replay)     | COMPLETE  | AppStateProjection from event stream           |
| Policy middleware                  | COMPLETE  | LagoPolicyMiddleware → Lago PolicyEngine       |
| SSE multi-format streaming        | COMPLETE  | All 4 formats via SseBridge                    |
| Agent loop → Lago round-trip      | COMPLETE  | reconstruct → run → persist → stream           |
| Memory system (Phase 1)           | COMPLETE  | Event types, observer, projection, governed tools |
| Context compiler                   | COMPLETE  | Deterministic assembly with typed blocks and budgets |
| Branching in agent sessions       | PARTIAL   | Lago supports it; Arcan defaults to "main"     |
| Blob storage for file content     | COMPLETE  | BlobStore fully wired to file endpoints (write/read/patch) with 17 E2E tests |
| Policy rules from config          | COMPLETE  | `default-policy.toml` ships with project; `lago init` scaffolds it; `lagod` loads it on startup |
| SseBridge in HTTP server          | OPTIONAL  | server.rs uses AI SDK format directly instead  |

---

## What Works End-to-End

```
User → POST /chat → AgentLoop.run()
  → Load session from Lago journal (RedbJournal)
  → Reconstruct AppState via event replay
  → Add user message to context
  → Send to LLM (Anthropic or Mock)
  → Stream TextDelta events via SSE
  → Execute tool calls through harness sandbox
  → LagoPolicyMiddleware evaluates each tool call
  → Persist every AgentEvent to Lago journal
  → Client receives formatted SSE stream
  → Session fully replayable from journal
```

---

## Known Gaps

### Critical (blocks production for long sessions)

1. ~~**No context window management**~~ **DONE** — `context.rs` implements sliding window with recency bias, system/last-user preservation, configurable limits. 11 tests. Wired into Orchestrator + event mapping.
2. ~~**No end-to-end integration tests**~~ **DONE** — 3 E2E integration tests in `arcan-lago/tests/end_to_end.rs` covering full session lifecycle, policy evaluation, and session isolation
19. ~~**No memory system**~~ **DONE (Phase 1)** — 5 memory event types in lago-core (ObservationAppended, ReflectionCompacted, MemoryProposed, MemoryCommitted, MemoryTombstoned), MemoryScope (Session/User/Agent/Org), Observer + Reflector + MemoryProjection in arcan-lago, 3 governed memory tools (query/propose/commit).
20. ~~**No context compiler**~~ **DONE (Phase 1)** — `context_compiler.rs` in arcan-core: typed blocks (Persona/Rules/Memory/Retrieval/Workspace/Task), per-block token budgets, priority-based overflow, word-boundary truncation.

### High (significantly limits use)

3. ~~**AI SDK v5 streaming missing boundary signals**~~ **DONE** — Full v6 UI Message Stream Protocol: UiStreamPart enum (18 variants), TextStart/End boundaries, StartStep/FinishStep, ReasoningStart/Delta/End, data-* extensions, x-vercel-ai-ui-message-stream: v1 header, monotonic SSE event IDs, [DONE] termination. 18 unit tests + 5 integration tests.
4. ~~**SSE has no event IDs**~~ **DONE** — Monotonic id: field on every v6 SSE frame (per-stream counter)
5. ~~**Approval workflow incomplete**~~ **DONE (M2.6)** — `ApprovalGate` blocks on oneshot, `POST /approve` resolves, `GET /approvals` lists pending, auto-timeout (5min default), full SSE+journal event persistence. 14 new tests.
6. ~~**No OpenAI provider**~~ **DONE** — `OpenAiCompatibleProvider` supports OpenAI, Ollama, Together, Groq, and any OpenAI-compatible API. Built-in retry (429/5xx/timeout). `ARCAN_PROVIDER=openai|ollama|anthropic` env var + auto-detect. 13 tests.
7. ~~**arcand has 0 tests**~~ **DONE** — 10 integration tests (5 agent loop + 5 HTTP/SSE server)

### Medium (quality/safety)

8. ~~**No default policy rules**~~ **DONE** — `default-policy.toml` with 5 rules (deny-shell, approve-critical, sandbox-high-risk, allow-filesystem, default-allow), 3 roles (agent, admin, readonly), 2 hooks (audit-file-ops, audit-high-risk). Loaded by `lagod` on startup.
9. ~~**Blob storage unused**~~ **DONE** — Fully wired in `lago-api` file endpoints (write_file, read_file, patch_file). 17 E2E tests confirm round-trip.
10. **Branching not exposed** — Lago branching exists but Arcan always uses "main"
11. ~~**CLI mostly stubs**~~ **DONE** — All command handlers fully implemented: session (create/list/show with API+DB fallback), log (formats all event types), cat (manifest→blob→stdout), branch (create/list), init (scaffolds config + policy)
12. **No OS-level sandbox isolation** — No BubblewrapRunner/DockerRunner for process limits
13. **Network isolation declared but not enforced** — SandboxPolicy has network policy enum but no enforcement

### Low (nice-to-have)

14. **Mount trait unimplemented** — Virtual filesystem interface defined but no concrete implementation
15. **Backpressure not enforced** — gRPC ingest has BackpressureSignal type but doesn't enforce it
16. **Snapshot auto-persistence** — Snapshots can be created but aren't auto-triggered
17. **Parallel tool execution** — Orchestrator runs tools sequentially only
18. **Session fork API** — parent_id field exists in EventRecord but no endpoint exposed
