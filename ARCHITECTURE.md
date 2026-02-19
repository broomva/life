# Agent OS: Technical Architecture

## System Overview

The Agent OS is a four-project ecosystem forming a complete agentic operating system:

- **aiOS** — Kernel contract: canonical types, event taxonomy, invariants, conformance tests
- **Arcan** — Runtime: agent loop, providers, tool harness, daemon
- **Lago** — Persistence substrate: event journal, blob store, branching FS, policy engine
- **Autonomic** — Stability controller: homeostasis, heartbeats, simulation, gating (planned)

### 2026-02-17 Hard-Cutover Spine Additions

- Arcand now exposes canonical runtime APIs under `/v1/sessions/{session_id}`:
  - `POST /runs` (execute + stream)
  - `POST /signals` (external signal ingress)
  - `GET /state` (snapshot `{version,state}`)
  - `GET /stream` (SSE replay stream)
- Arcand streams canonical data-part events for UI sync:
  - `state.patch`
  - `intent.proposed|evaluated|approved|rejected`
  - `tool.started|completed`
- Arcan session persistence is branch-explicit (branch ID required in append/load/head paths).
- Lago journal sequence numbers are assigned atomically at append-time per branch head (caller `seq` is non-authoritative).

### Ecosystem Architecture

```
                    ┌─────────────────────────┐
                    │   Applications / UIs     │
                    │  (chat, IDE, dashboards)  │
                    └────────────┬─────────────┘
                                 │ AI SDK v6 / SSE / WebSocket
                    ┌────────────▼─────────────┐
                    │     Arcan Runtime         │
                    │  (loop, harness, tools,   │
                    │   providers, daemon)       │
                    ├───────────────────────────┤
                    │   arcan-lago (bridge)      │
                    └────────────┬──────────────┘
                                 │ EventEnvelope (canonical)
          ┌──────────────────────┼──────────────────────┐
          │                      │                      │
┌─────────▼──────────┐ ┌────────▼─────────┐ ┌──────────▼──────────┐
│  aiOS (contract)    │ │  Lago (substrate) │ │  Autonomic (NEW)    │
│                     │ │                   │ │                     │
│ aiOS protocol crate │ │  journal (redb)   │ │  controllers        │
│  - EventKind        │ │  blob store       │ │  heartbeat triggers │
│  - StateVector      │ │  branching FS     │ │  simulation kernel  │
│  - BudgetState      │ │  policy engine    │ │  gating profiles    │
│  - OperatingMode    │ │  SSE adapters     │ │  memory maintenance │
│  - Capabilities     │ │  gRPC ingest      │ │  decay/forgetting   │
│  - Soul/Memory      │ │  lago-memory (NEW)│ │                     │
│  - Intent lifecycle │ │                   │ │                     │
│  - GatingProfile    │ │                   │ │                     │
│  - BlobRef          │ │                   │ │                     │
└─────────────────────┘ └───────────────────┘ └─────────────────────┘
```

### Ownership Boundaries

| Project | Owns | Does NOT own |
|---------|------|-------------|
| **aiOS** | Canonical types, event taxonomy, kernel traits, schema versioning, conformance tests | Runtime implementation, persistence engine, provider/tool details |
| **Arcan** | Agent loop, providers, tool harness, sandbox, daemon HTTP/SSE, CLI | Event schema definition, persistence engine, policy rule storage |
| **Lago** | Journal, blob store, branching FS, projections, policy engine, SSE adapters, memory store | Runtime behavior, model routing, homeostasis decisions |
| **Autonomic** | Homeostasis controllers, heartbeat scheduling, simulation, gating profiles | Tool execution, persistence, streaming |

### Dependency Wiring (separate repos, git deps)

All projects depend on `aios-protocol` (published by aiOS) via git dependency:
```toml
aios-protocol = { git = "https://github.com/broomva/aiOS", package = "aios-protocol", rev = "<sha>" }
```

---

## Arcan + Lago: Runtime + Persistence

Arcan + Lago form the two-layer runtime core:

```
┌─────────────────────────────────────────────────────┐
│                   Arcan (Runtime)                    │
│                                                     │
│  ┌──────────┐  ┌────────────┐  ┌─────────────────┐ │
│  │ Provider  │  │Orchestrator│  │    Harness       │ │
│  │(Anthropic,│──│  (Loop +   │──│ (Sandbox, FS,   │ │
│  │ Mock, Rig)│  │ Middleware)│  │  MCP, Skills,   │ │
│  └──────────┘  └─────┬──────┘  │  Memory, Edit)  │ │
│                      │         └─────────────────┘ │
│                      │                              │
│  ┌───────────────────┴───────────────────────────┐ │
│  │              arcan-lago (Bridge)               │ │
│  │  event_map | repository | policy_middleware    │ │
│  │  state_projection | sse_bridge                 │ │
│  └───────────────────┬───────────────────────────┘ │
└──────────────────────┼──────────────────────────────┘
                       │
┌──────────────────────┼──────────────────────────────┐
│                   Lago (Persistence)                 │
│                      │                               │
│  ┌─────────┐  ┌──────┴──────┐  ┌──────────────────┐│
│  │lago-core│  │lago-journal │  │   lago-store     ││
│  │ (types) │  │  (RedbJrnl) │  │ (SHA256+zstd)    ││
│  └─────────┘  └─────────────┘  └──────────────────┘│
│                                                      │
│  ┌─────────┐  ┌─────────────┐  ┌──────────────────┐│
│  │lago-fs  │  │lago-policy  │  │  lago-ingest     ││
│  │(branch) │  │(RBAC+rules) │  │  (gRPC stream)   ││
│  └─────────┘  └─────────────┘  └──────────────────┘│
│                                                      │
│  ┌─────────────────┐  ┌───────────────────────────┐ │
│  │    lago-api      │  │       lagod               │ │
│  │(HTTP REST + SSE) │  │  (Daemon: gRPC + HTTP)    │ │
│  └─────────────────┘  └───────────────────────────┘ │
└──────────────────────────────────────────────────────┘
```

---

## Arcan: Agent Runtime

### Dependency Graph

```
arcan-core ────────────────────────────┐
  ├── arcan-harness (sandbox, tools)   │
  ├── arcan-store (session repo trait)  │
  ├── arcan-provider (LLM backends)    │
  │                                    │
  ├── arcand (agent loop + server)     │
  │                                    │
  └── arcan-lago (bridge) ◄────────── lago-core, lago-journal,
        │                              lago-store, lago-api,
        │                              lago-policy
        │
        └── arcan (binary — wires everything)
```

### Agent Loop (Core Execution Path)

```
POST /chat { session_id, message }
         │
         ▼
    AgentLoop.run(session_id, message)
         │
         ├── 1. RECONSTRUCT: Load events from Lago journal
         │   └── LagoSessionRepository.load_session()
         │       └── RedbJournal.read(EventQuery)
         │           └── event_map::lago_to_arcan() for each event
         │
         ├── 2. REPLAY: Rebuild state from event history
         │   ├── StatePatch → apply to AppState
         │   ├── TextDelta → aggregate into ChatMessage::assistant
         │   └── ToolCallCompleted → add ChatMessage::tool_result
         │
         ├── 3. PREPARE: Add user message to context
         │
         ├── 4. ORCHESTRATE: Run provider + tools loop
         │   ├── Send context to LLM provider
         │   ├── Stream TextDelta events
         │   ├── Parse tool calls from model output
         │   ├── LagoPolicyMiddleware.before_tool_call()
         │   │   └── PolicyEngine.evaluate(context) → Allow/Deny
         │   ├── Execute tool in harness sandbox
         │   ├── Record tool result
         │   └── Loop until RunFinished or max_iterations
         │
         ├── 5. PERSIST: Every event → Lago journal
         │   ├── event_map::arcan_to_lago() converts event
         │   └── LagoSessionRepository.append() stores in redb
         │
         └── 6. STREAM: SSE to client
             └── Format as AI SDK / OpenAI / Anthropic / Vercel
```

### Harness Architecture

The harness provides defense-in-depth:

```
Tool Invocation
    │
    ├── Layer 1: Policy Middleware (LagoPolicyMiddleware)
    │   └── Lago PolicyEngine evaluates rules → Allow/Deny/RequireApproval
    │
    ├── Layer 2: Sandbox Policy (FsPolicy)
    │   ├── SandboxTier::None — unrestricted
    │   ├── SandboxTier::Basic — path allowlist/denylist
    │   └── SandboxTier::Restricted — read-only + approved writes
    │
    ├── Layer 3: Tool Execution
    │   ├── Filesystem tools (read, write, list, search, patch)
    │   ├── Memory tools (read, write, list, search)
    │   ├── MCP bridge tools (external servers via stdio)
    │   └── Hashline edit tools (content-hash addressed edits)
    │
    └── Layer 4: Audit Trail
        └── Every action persisted as event in Lago journal
```

### Tool System

```
Tool trait:
  ├── definition() → ToolDefinition { name, description, input_schema, annotations }
  └── execute(call, context) → Result<ToolResult, CoreError>

Built-in tools (arcan-harness):
  ├── ReadFileTool — read file with sandbox check
  ├── WriteFileTool — write file with sandbox + hashline verification
  ├── ListDirectoryTool — directory listing with path filtering
  ├── SearchFilesTool — glob-based file search
  ├── PatchFileTool — hashline edit (content-hash addressed)
  ├── ReadMemoryTool — persistent agent memory read
  ├── WriteMemoryTool — persistent agent memory write
  ├── ListMemoryTool — enumerate memory keys
  ├── SearchMemoryTool — search memory contents
  └── McpTool — wraps any MCP server tool into Arcan's Tool trait

ToolRegistry:
  ├── register(tool) — add tool with definition
  ├── definitions() → Vec<ToolDefinition> (for LLM function-calling)
  └── execute(tool_name, call, context) → Result<ToolResult>
```

---

## Lago: Event-Sourced Persistence

### Data Model

```
EventEnvelope {
    event_id:    EventId (ULID — time-ordered, globally unique)
    session_id:  SessionId
    branch_id:   BranchId
    parent_id:   Option<EventId>
    seq:         SeqNo (u64, monotonically increasing per branch)
    timestamp:   DateTime<Utc>
    payload:     EventPayload (15+ variants)
    metadata:    HashMap<String, Value>
}
```

### Journal (redb Backend)

```
redb Database Layout:

┌──────────────┬──────────────────────┬──────────────────┐
│ Table        │ Key                  │ Value            │
├──────────────┼──────────────────────┼──────────────────┤
│ EVENTS       │ [session:26][branch: │ JSON EventEnvelo │
│              │  26][seq:8BE] = 60B  │ pe string        │
├──────────────┼──────────────────────┼──────────────────┤
│ EVENT_INDEX  │ event_id string      │ 60B compound key │
├──────────────┼──────────────────────┼──────────────────┤
│ BRANCH_HEADS │ [session:26][branch: │ u64 (head seq)   │
│              │  26] = 52B           │                  │
├──────────────┼──────────────────────┼──────────────────┤
│ SESSIONS     │ session_id string    │ JSON Session     │
├──────────────┼──────────────────────┼──────────────────┤
│ SNAPSHOTS    │ snapshot_id string   │ zstd compressed  │
└──────────────┴──────────────────────┴──────────────────┘

Critical pattern: All redb operations use spawn_blocking
to avoid blocking the tokio async runtime.
```

### Content-Addressed Storage

```
BlobStore Layout:

.lago/blobs/
  ├── ab/
  │   └── cdef1234...zst    ← SHA-256 hash, zstd compressed
  ├── 12/
  │   └── 3456abcd...zst
  └── ff/
      └── 0011aabb...zst

Operations:
  put(data) → hash (idempotent, deduplicates)
  get(hash) → data (decompress on read)
  exists(hash) → bool
  delete(hash) → remove file
```

### Filesystem Branching

```
BranchManager:
  ├── main (default branch)
  │   └── Events: [seq:1] → [seq:2] → [seq:3] → ...
  │
  └── feature-x (forked at seq:2)
      └── Events: [seq:3'] → [seq:4'] → ...

Manifest (per branch):
  BTreeMap<String, ManifestEntry>
  ├── "/src/main.rs" → { hash: "abc...", size: 1024, type: "text/x-rust" }
  ├── "/src/" → { hash: "", size: 0, type: "inode/directory" }  ← auto-created
  └── "/README.md" → { hash: "def...", size: 512, type: "text/markdown" }

Projection:
  Replay events from fork_point → head to reconstruct branch state
```

### SSE Streaming Formats

```
Client Request:
  GET /v1/sessions/{id}/events?format=openai
  Header: Last-Event-ID: 42  (reconnection)

Format Adapters:
  ├── OpenAI:     MessageDelta → chat.completion.chunk
  ├── Anthropic:  MessageDelta → content_block_delta (5-frame lifecycle)
  ├── Vercel:     MessageDelta → ui_message (with tool lifecycle)
  └── Lago:       All events → type-discriminated frames

Wire Format:
  event: {type}\n
  id: {seq}\n
  data: {json}\n\n
```

### Policy Engine

```
PolicyEngine:
  Rules (priority-ordered):
    ├── Rule { condition: ToolPattern("shell/*") AND RiskAtLeast(High),
    │         decision: RequireApproval }
    ├── Rule { condition: ToolName("delete_file"),
    │         decision: Deny }
    └── Rule { condition: Any([ToolPattern("read_*"), SandboxTierAtLeast(Basic)]),
              decision: Allow }

  Evaluation: First matching rule wins → Allow/Deny/RequireApproval
  Default: Allow (if no rules match)

RBAC:
  User → Role → Permissions
  ├── ViewEvents
  ├── ModifyFiles
  ├── ExecuteTools
  ├── ApproveActions
  ├── ManagePolicy
  └── Admin
```

---

## Communication Protocols

### Internal (Arcan ↔ Lago)

Currently: **Direct Rust API calls** (arcan-lago imports lago crates as library dependencies)

```toml
# arcan-lago/Cargo.toml
lago-core.workspace = true
lago-journal.workspace = true
lago-store.workspace = true
lago-api.workspace = true
lago-policy.workspace = true
```

Future: Can switch to **gRPC** (lago-ingest) for distributed deployment.

### External (Client ↔ Arcan)

**HTTP + SSE** via axum:
- `POST /chat` — Send user message, start agent run
- `GET /events` — SSE stream of AgentEvents (AI SDK format)

### External (Client ↔ Lago)

**HTTP + SSE** via axum (port 8080):
- Full REST API for sessions, branches, files, blobs, events
- SSE streaming with format negotiation

**gRPC** via tonic (port 50051):
- Bidirectional streaming for high-throughput event ingestion
- Session and event management RPCs

---

## Shared Technology Stack

| Component      | Library       | Version | Both? |
|----------------|---------------|---------|-------|
| Async runtime  | tokio         | 1.x     | Yes   |
| HTTP framework | axum          | 0.8     | Yes   |
| Serialization  | serde + json  | 1.x     | Yes   |
| Error handling | thiserror     | 2.x     | Yes   |
| Logging        | tracing       | 0.1     | Yes   |
| gRPC           | tonic + prost | 0.12    | Lago  |
| Embedded DB    | redb          | 2.x     | Lago  |
| Compression    | zstd          | 0.13    | Lago  |
| Hashing        | sha2          | 0.10    | Lago  |
| MCP client     | rmcp          | latest  | Arcan |
| YAML parsing   | serde_yaml    | 0.9     | Arcan |
| File walking   | walkdir       | 2.x     | Arcan |
| Glob matching  | glob          | 0.3     | Both  |

---

## aiOS: Kernel Contract

aiOS defines the canonical types and interfaces that all other projects implement.

### `aios-protocol` Crate (canonical contract)

```
aios-protocol/src/
  ids.rs          # SessionId, BranchId, EventId, RunId, BlobHash, SeqNo
  event.rs        # EventEnvelope + EventKind (~55 variants, forward-compatible)
  state.rs        # AgentStateVector, BudgetState, CanonicalState
  mode.rs         # OperatingMode, GatingProfile
  intent.rs       # Intent, IntentKind, RiskLevel
  tool.rs         # ToolCall, ToolResult, ToolDefinition, ToolAnnotations
  policy.rs       # Capability, PolicyEvaluation, PolicyDecisionKind
  memory.rs       # SoulProfile, Observation, Provenance, MemoryScope
  checkpoint.rs   # CheckpointManifest
  session.rs      # SessionManifest, ModelRouting
  blob.rs         # BlobRef (for arbitrary payloads: tensors, audio, etc.)
  patch.rs        # StatePatch, PatchOp (Set, Merge, Append, Tombstone, SetRef)
  stream.rs       # UiMessage, UiPart (AI SDK data parts mapping)
  traits.rs       # Journal, PolicyGate, Harness, MemoryStore, AutonomicController
  error.rs        # KernelError
```

### Canonical Event Taxonomy (~55 variants)

```
Session:      SessionCreated, SessionResumed, SessionClosed
Branch:       BranchCreated, BranchMerged
Phase:        PhaseEntered (Perceive|Deliberate|Gate|Execute|Commit|Reflect|Sleep)
Run:          RunStarted, RunFinished, RunErrored
Step:         StepStarted, StepFinished
Text:         TextDelta, MessageCommitted
Tool:         ToolCallRequested, ToolCallStarted, ToolCallCompleted, ToolCallFailed
File:         FileWrite, FileDelete, FileRename, FileMutated
State:        StatePatched, ContextCompacted
Policy:       PolicyEvaluated
Approval:     ApprovalRequested, ApprovalResolved
Sandbox:      SandboxCreated, SandboxExecuted, SandboxViolation, SandboxDestroyed
Memory:       ObservationAppended, ReflectionCompacted, MemoryProposed, MemoryCommitted, MemoryTombstoned
Homeostasis:  Heartbeat, StateEstimated, BudgetUpdated, ModeChanged, GatesUpdated, CircuitBreakerTripped
Checkpoint:   CheckpointCreated, CheckpointRestored
Voice:        VoiceSessionStarted, VoiceInputChunk, VoiceOutputChunk, VoiceSessionStopped
World:        WorldModelObserved, WorldModelRollout, WorldModelDeltaApplied
Intent:       IntentProposed, IntentEvaluated, IntentApproved, IntentRejected
Error:        ErrorRaised
Custom:       Custom { event_type, data }  (forward-compatible catch-all)
```

### Homeostasis State Vector

```
AgentStateVector:
  progress:              f32  [0.0, 1.0]
  uncertainty:           f32  [0.0, 1.0]
  risk_level:            RiskLevel (Low|Medium|High|Critical)
  budget:                BudgetState (tokens, time, cost, tool_calls, error_budget)
  error_streak:          u32
  context_pressure:      f32  [0.0, 1.0]
  side_effect_pressure:  f32  [0.0, 1.0]
  human_dependency:      f32  [0.0, 1.0]

OperatingMode: Explore | Execute | Verify | Recover | AskHuman | Sleep

GatingProfile:
  allow_side_effects:        bool
  require_approval_for_risk: RiskLevel
  max_tool_calls_per_tick:   u32
  max_file_mutations:        u32
  allow_network:             bool
  allow_shell:               bool
```

---

## Autonomic: Homeostasis Controller (Planned)

### Architecture

```
autonomic/crates/
  autonomic-model/        # vitals, decisions, triggers, config
  autonomic-core/         # rule-based controller with hysteresis
  autonomic-heartbeat/    # trigger scheduling (time + event-based)
  autonomic-sim/          # simulator trait + statistical rollout
  autonomic-adapters/
    autonomic-arcan/      # adapter: wire into Arcan daemon
    autonomic-lago/       # adapter: replay from Lago journal
```

### Control Loop

```
Heartbeat fires (time-based or event-triggered)
  │
  ├── Read event window from journal
  ├── Compute AgentStateVector
  ├── Run homeostasis controllers:
  │   ├── Uncertainty high → restrict action power (read-only tools)
  │   ├── Error streak high → Recover mode (rollback, change strategy)
  │   ├── Budget low → simplify (cheaper model, narrower plans)
  │   ├── Context pressure high → externalize (write state, retrieve selectively)
  │   └── Side-effect pressure high → transaction discipline
  │
  ├── Output GatingProfile (enforced at Arcan harness boundary)
  ├── Output maintenance triggers (memory consolidation, compaction)
  └── Emit events: StateEstimated, ModeChanged, GatesUpdated
```

---

## Memory Service (Planned — `lago-memory`)

### Layers

```
Raw event log (Lago journal) — truth, replay, never deleted
  │
  ├── Observations — extracted facts with provenance
  ├── Reflections — compressed summaries over event ranges
  ├── Soul — stable identity/preferences (governed writes)
  ├── Decisions — key architectural choices with evidence
  └── KG/Embeddings — entity graphs + semantic retrieval (future)
```

### Forgetting Model

Memory is never deleted from the journal. "Forgetting" is achieved through:
- **Tombstones**: mark items deprecated, stop returning in retrieval
- **Decay scoring**: `score = salience * reliability * reinforcement / (1 + age_decay + conflict_penalty)`
- **Compaction**: summarize old observations, link to event ranges
- **Promotion/demotion**: session scratch → agent durable → OS knowledge (governed)
- **Context assembly**: budget-aware, scope-filtered, trust-weighted retrieval
