# Arcan + Lago: Technical Architecture

## System Overview

Arcan + Lago form a two-layer agent operating system:

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
