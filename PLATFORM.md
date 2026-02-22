# Agent OS: Platform Architecture

**From Runtime to Managed Service**

**Version**: 0.2.0 | **Date**: 2026-02-17 | **Status**: Living document

How aiOS, Arcan, and Lago compose into an operating system for agents — and the path to multi-tenant SaaS.

> Status badges: `[IMPLEMENTED]` `[PARTIAL]` `[PROPOSED]` `[FUTURE]`

---

## 1. The OS Analogy — Concrete Mapping

This is not metaphor. The dependency graph between aiOS, Arcan, and Lago mirrors the layered architecture of an operating system: kernel contract → runtime → persistence. Each "OS concept" below maps to a specific type, trait, or crate that exists in the codebase today.

| OS Concept | Traditional OS | Agent OS Primitive | Actual Type / Trait | Crate | Status |
|---|---|---|---|---|---|
| **Kernel contract** | POSIX, syscall table | Protocol types | `EventKind` (55 variants), `EventEnvelope` | `aios-protocol` | `[IMPLEMENTED]` |
| **Syscall interface** | `read()`, `write()` | Tool trait | `trait Tool { definition(); execute() }` | `arcan-core` | `[IMPLEMENTED]` |
| **Process** | Unix process | Agent session | `SessionId`, `AgentLoop` | `aios-protocol`, `arcand` | `[IMPLEMENTED]` |
| **Scheduler** | CFS, nice | Orchestrator + budget | `Orchestrator`, `BudgetState`, `OperatingMode` | `arcan-core`, `aios-protocol` | `[PARTIAL]` |
| **Virtual memory** | Page tables | Context compiler | `ContextCompiler`, `ContextBlock` | `arcan-core` | `[IMPLEMENTED]` |
| **Filesystem** | ext4, VFS | Lago FS + blobs | `Manifest`, `BranchManager`, `BlobStore` | `lago-fs`, `lago-store` | `[IMPLEMENTED]` |
| **Block device** | Disk I/O | Event journal | `RedbJournal` (ACID, 60B compound key) | `lago-journal` | `[IMPLEMENTED]` |
| **Mount / VFS** | `/dev`, `/proc` | Mount trait | `trait Mount { read, write, delete, list, stat }` | `lago-core` | `[DEFINED]` |
| **IPC** | Pipes, sockets | Event stream + SSE | `EventStreamHub`, SSE adapters | `lago-api` | `[IMPLEMENTED]` |
| **Security module** | SELinux, AppArmor | Policy + sandbox | `PolicyEngine`, `SandboxPolicy`, `FsPolicy` | `lago-policy`, `arcan-harness` | `[IMPLEMENTED]` |
| **Capabilities** | POSIX caps | Capability tokens | `Capability("fs:read:/session/**")`, `GatingProfile` | `aios-protocol` | `[PARTIAL]` |
| **Health monitor** | watchdog, OOM killer | Homeostasis | `AgentStateVector` (8 dims), `OperatingMode` (6 modes) | `aios-protocol` | `[PARTIAL]` |
| **Package manager** | apt, cargo | Skill registry | `SkillRegistry`, `SkillMetadata` | `arcan-harness` | `[PARTIAL]` |
| **Cron** | systemd timers | Heartbeat scheduler | `HeartbeatScheduler`, `EventKind::Heartbeat` | `arcand`, `aios-protocol` | `[PARTIAL]` |
| **Device driver** | USB, NIC | MCP bridge | `McpTool`, `McpServerConfig` | `arcan-harness` | `[IMPLEMENTED]` |
| **User / tenant** | UID, namespaces | Tenant isolation | — | — | `[PROPOSED]` |
| **Credential store** | keyring, Vault | Secret resolver | `Capability::secrets("scope")` exists | `aios-protocol` | `[PROPOSED]` |

### Why this layering matters

The relationship is structural, not decorative:

```
aiOS  = kernel contract    (defines what operations exist)
Arcan = runtime            (implements the operations)
Lago  = storage substrate  (persists all state changes)
```

A skill doesn't need to know about redb. A provider doesn't need to know about policy rules. The harness doesn't need to know about SSE formats. Each layer has a clean boundary mediated by traits and event conversion.

---

## 2. Layer Architecture — What Each Project Owns

### 2.1 aiOS — Kernel Contract `[IMPLEMENTED]`

**Crate**: `aios-protocol` at `aiOS/crates/aios-protocol/`

Owns the canonical types that all other projects reference:

- **Event taxonomy**: `EventKind` enum with ~55 variants grouped into: Session (3), Branch (2), Phase (2), Run (3), Step (2), Text (2), Tool (5), File (4), State (2), Policy (1), Approval (2), Snapshot (1), Sandbox (4), Memory (5), Homeostasis (6), Checkpoint (2), Voice (5), World (2), Intent (2), Error (1), Custom (1)
- **Typed IDs**: `SessionId`, `BranchId`, `EventId`, `RunId`, `ApprovalId`, `MemoryId`, `ToolRunId`, `CheckpointId`, `BlobHash`, `SeqNo` — all opaque `String` wrappers via `typed_id!` macro
- **State vectors**: `AgentStateVector` (progress, uncertainty, risk_level, budget, error_streak, context_pressure, side_effect_pressure, human_dependency), `BudgetState` (tokens, time, cost, tool_calls, error_budget)
- **Operating modes**: `OperatingMode` (Explore, Execute, Verify, Recover, AskHuman, Sleep), `GatingProfile` (allow_side_effects, require_approval_for_risk, max_tool_calls_per_tick, max_file_mutations_per_tick, allow_network, allow_shell)
- **Memory**: `SoulProfile`, `Observation`, `Provenance`, `FileProvenance`, `MemoryScope` (Session, User, Agent, Org)
- **Policy**: `Capability` (pattern-based, e.g. `fs:read:/session/**`), `PolicySet` (allow/gate capabilities, resource limits), `PolicyEvaluation` (allowed/requires_approval/denied)
- **Tools**: `ToolCall` (call_id, tool_name, input, requested_capabilities), `ToolOutcome` (Success/Failure)
- **Sessions**: `SessionManifest` (session_id, owner, workspace_root, model_routing, policy), `BranchInfo` (branch_id, parent_branch, fork_sequence, head_sequence, merged_into)

**Forward compatibility**: Unknown `EventKind` variants deserialize as `Custom { event_type, data }` via custom `Deserialize` implementation. This means new event types can be added to `aios-protocol` without breaking existing Lago journals or Arcan deployments.

### 2.2 aiOS — Kernel Runtime `[IMPLEMENTED]`

**Crates**: `aios-kernel`, `aios-runtime`, `aios-events`, `aios-memory`, `aios-policy`, `aios-sandbox`, `aios-tools`

A separate, fully-functional kernel implementation that parallels Arcan+Lago:

- **8-phase tick lifecycle** in `aios-runtime`: Perceive → Deliberate → Gate → Execute → Commit → Reflect → Heartbeat → Sleep
- **4 core traits**: `EventStore` (append, read_from, latest_sequence), `MemoryStore` (load_soul, save_soul, append_observation, list_observations), `PolicyEngine` (evaluate_capabilities), `SandboxRunner` (run)
- **Implementations**: `FileEventStore` (JSONL), `WorkspaceMemoryStore` (soul.json + observations.jsonl), `SessionPolicyEngine` (glob matching), `LocalSandboxRunner` (tokio subprocess), `ToolDispatcher` with 3 core tools (fs.read, fs.write, shell.exec)
- **Homeostasis**: Rule-based mode estimation — pending approvals → AskHuman, error streak → Recover, progress ≥ 98% → Sleep, high uncertainty → Explore, high side-effect pressure → Verify, default → Execute

> **Note**: The aiOS kernel runtime and Arcan+Lago are currently parallel implementations. Phase 7 (Agent OS Unification) plans to merge them. `lago-core` already uses `pub type EventPayload = aios_protocol::EventKind` — the type alignment exists, the runtime unification does not.

### 2.3 Arcan — Runtime `[IMPLEMENTED]`

**9 crates** at `arcan/crates/`:

| Crate | LOC | Owns | Key Types |
|---|---|---|---|
| `arcan-core` | ~3,450 | Agent loop, traits, context | `Orchestrator`, `Tool`, `Middleware`, `Provider`, `ContextCompiler`, `AgentEvent` (23 variants), `AppState` |
| `arcan-harness` | ~2,520 | Tools, sandbox, skills, MCP | `SandboxPolicy`, `FsPolicy`, `BashTool`, `SkillRegistry`, `McpTool`, hashline editing |
| `arcan-aios-adapters` | ~330 | Canonical runtime adapters | aiOS provider/tool/policy/approval/memory adapter implementations |
| `arcan-provider` | ~1,270 | LLM backends | `AnthropicProvider`, `OpenAiCompatibleProvider`, `MockProvider` |
| `arcan-store` | ~430 | Session repo trait | `SessionRepository`, `EventRecord` |
| `arcan-tui` | ~1,430 | Terminal client | Canonical session + approval endpoint client integration |
| `arcand` | ~790 | HTTP server, heartbeat | `AgentLoop`, `HeartbeatScheduler`, axum routes |
| `arcan-lago` | ~4,470 | Bridge to Lago | `LagoSessionRepository`, `LagoPolicyMiddleware`, `ApprovalGate`, `event_map`, `SseBridge`, memory modules |
| `arcan` | ~550 | Binary entry point | CLI args, wiring |

**14 built-in tools**: read_file, write_file, list_dir, edit_file (hashline), glob, grep, bash, read_memory, write_memory, memory_query, memory_propose, memory_commit, MCP bridge tools, skill catalog

**Defense-in-depth harness**:
```
Tool invocation
  ├── Layer 1: PolicyEngine (LagoPolicyMiddleware → Allow/Deny/RequireApproval)
  ├── Layer 2: SandboxPolicy (workspace boundary, env whitelisting, timeout)
  ├── Layer 3: FsPolicy (canonicalize + starts_with path traversal prevention)
  ├── Layer 4: Tool execution (within harness constraints)
  └── Layer 5: Audit trail (every action → Lago journal event)
```

### 2.4 Lago — Persistence Substrate `[IMPLEMENTED]`

**10 crates** at `lago/crates/`:

| Crate | LOC | Owns | Key Types |
|---|---|---|---|
| `lago-core` | ~2,890 | Types, traits | `EventEnvelope`, `EventPayload` (= `aios_protocol::EventKind`), `Journal` trait (BoxFuture), `Projection` trait, `Mount` trait |
| `lago-journal` | ~1,520 | Event persistence | `RedbJournal` — compound key: session(26B) + branch(26B) + seq(8B BE) = 60B, tables: EVENTS, EVENT_INDEX, BRANCH_HEADS, SESSIONS, SNAPSHOTS |
| `lago-store` | ~320 | Blob storage | `BlobStore` — SHA-256 + zstd, shard layout `{root}/{hash[0:2]}/{hash[2:]}.zst`, atomic writes |
| `lago-fs` | ~1,050 | Filesystem | `Manifest` (BTreeMap), `BranchManager` (copy-on-write), `ManifestProjection`, `diff()` |
| `lago-policy` | ~1,340 | Access control | `PolicyEngine` (priority rules), `RbacManager` (role→permissions), `HookRunner`, TOML config |
| `lago-api` | ~3,220 | HTTP + SSE | REST routes (sessions, branches, files, blobs, events), SSE format adapters (OpenAI, Anthropic, Vercel, Lago) |
| `lago-ingest` | ~590 | gRPC streaming | Bidirectional event streaming via tonic |
| `lago-aios-eventstore-adapter` | ~145 | Canonical event store adapter | `EventStorePort` implementation over `lago_core::Journal` |
| `lago-cli` | ~1,160 | CLI binary | init, serve, session, branch, log, cat |
| `lagod` | ~310 | Daemon binary | gRPC (50051) + HTTP (8080), TOML config, graceful shutdown |

### 2.5 Autonomic — Homeostasis Controller `[FUTURE]`

Not yet started. Would own:
- Homeostasis rule engine consuming event streams
- `GatingProfile` output enforcement at harness boundary
- Memory maintenance triggers (compaction, promotion, forgetting)
- Heartbeat scheduling with hysteresis

Events it would emit: `StateEstimated`, `BudgetUpdated`, `ModeChanged`, `GatesUpdated`, `CircuitBreakerTripped`, `Heartbeat`

---

## 3. Data Flow — How Events Move Through the System

### 3.1 The Happy Path

```
User → POST /chat { session_id, message }

1. ROUTE    arcan/src/main.rs → arcand/src/server.rs (axum handler)
2. LOAD     arcand/src/loop.rs → LagoSessionRepository.load_session()
              → RedbJournal.read(EventQuery { session_id, branch: "main" })
              → event_map::lago_to_arcan() for each EventEnvelope
3. REPLAY   state_projection: TextDelta→aggregate, StatePatch→apply
4. PREPARE  Add user ChatMessage to context
5. COMPILE  ContextCompiler assembles system prompt + context blocks
              (Persona → Rules → Memory → Retrieval → Workspace → Task)
6. LOOP     Orchestrator.run() [up to max_iterations]:
   6a.        Provider.complete() → ModelTurn { directives }
   6b.        For each ToolCall directive:
                Middleware.before_tool_call()
                  → LagoPolicyMiddleware → PolicyEngine.evaluate()
                  → Allow: proceed
                  → Deny: error
                  → RequireApproval: ApprovalGate.wait() (oneshot + timeout)
                Tool.execute(call, ctx) [within SandboxPolicy]
              For each TextDelta: emit AgentEvent::TextDelta
7. PERSIST  event_map::arcan_to_lago() → EventEnvelope
              → LagoSessionRepository.append() → RedbJournal.append()
8. STREAM   SSE: AgentEvent → UiStreamPart (AI SDK v6) or other format
```

### 3.2 Event Representation Lifecycle

An event passes through three representations as it moves through the system:

```
AgentEvent (Arcan-internal)
    │  arcan-lago/src/event_map.rs::arcan_to_lago()
    ▼
EventEnvelope { payload: aios_protocol::EventKind } (Lago storage)
    │  lago-api/src/sse/ format adapters
    ▼
SSE wire format (AI SDK v6, OpenAI, Anthropic, or Lago native)
```

**Type alignment is clean**: Lago stores `aios_protocol::EventKind` directly (via `pub type EventPayload = aios_protocol::EventKind` in `lago-core/src/event.rs:17`). Arcan maintains its own `AgentEvent` enum to avoid pulling Lago as a dependency of `arcan-core`, and converts at the `arcan-lago` bridge boundary.

### 3.3 System Invariants

These hold across all layers:

1. **No invisible state** — Every mutation is an event in the journal. State = projection of events.
2. **Provenance is mandatory** — Observations carry `Provenance { event_start, event_end, files }`.
3. **Tool execution is mediated** — Policy → Sandbox → FsPolicy → Tool. No unmediated access.
4. **Checkpoints bracket risk** — Before destructive operations, checkpoints enable rollback.
5. **Replay has defined meaning** — Replaying events from seq 0 produces identical state.
6. **Sequences are monotonic per branch** — `RedbJournal` enforces `seq = head + 1` atomically.
7. **Events are immutable** — Once written to journal, events are never modified. Use compensating events.

---

## 4. The Skill Ecosystem

### 4.1 Current State `[IMPLEMENTED]`

Skills are filesystem-discovered SKILL.md files with YAML frontmatter.

**`SkillMetadata`** (at `arcan-harness/src/skills.rs`):
```yaml
---
name: commit-helper          # required
description: Helps create commits  # required
license: MIT                 # optional
compatibility: ">=0.2"       # optional
tags: [git, workflow]        # optional
allowed_tools: [bash, read_file]  # optional, restricts tool access
user_invocable: true         # optional, shows in /command list
disable_model_invocation: false  # optional
---
# Skill Body
Instructions for the agent when this skill is active...
```

**`SkillRegistry`**:
- `discover(dirs)`: Scans directories for SKILL.md via walkdir
- `system_prompt_catalog()`: Generates compact listing for LLM injection (~100 tokens/skill)
- `activate(name)`: Returns full `LoadedSkill` (metadata + body + root_dir)
- `allowed_tools(name)`: Returns tool whitelist per skill

**What's missing**:
- Skills are not persisted to Lago journal (ephemeral in runtime)
- No resource declarations (secrets, network, filesystem scope, schedule)
- No versioning or artifact hashing
- No runtime activation (skills discovered but not injected into agent loop)
- No lifecycle events (install, activate, deactivate)

### 4.2 Proposed: `skill-manifest.toml` `[PROPOSED]`

A TOML manifest extending the current SKILL.md frontmatter with resource declarations for managed execution. Each field maps to an existing type.

```toml
[skill]
name = "garmin-health"
version = "0.1.0"
description = "Sync health metrics from Garmin Connect"
tags = ["health", "garmin", "sync"]

[skill.prompt]
# SKILL.md instructions, path relative to manifest
instruction_file = "SKILL.md"

[resources.secrets]
# Logical secret names → SecretResolver keys
# Maps to: Capability::secrets("tenant/garmin_email")
garmin_email = { scope = "tenant", required = true }
garmin_password = { scope = "tenant", required = true }

[resources.filesystem]
# Workspace paths the skill needs
# Maps to: FsPolicy allowlist globs
writes = ["health/{date}.md"]
reads = ["health/*.md"]

[resources.network]
# Egress requirements
# Maps to: NetworkPolicy::AllowList(vec!["connect.garmin.com", ...])
allow = ["connect.garmin.com", "sso.garmin.com"]

[resources.schedule]
# Cron-like trigger (requires SkillScheduler)
# Maps to: EventKind::Heartbeat + AgentLoop activation
cron = "0 6 * * *"

[tools]
# Tool allowlist for this skill
# Maps to: SkillMetadata.allowed_tools
allowed = ["bash", "write_file", "read_file"]

[policy]
# Minimum sandbox tier
# Maps to: SandboxTier (None/Basic/Restricted)
sandbox_tier = "basic"
# Risk level for policy evaluation
# Maps to: RiskLevel (Low/Medium/High/Critical)
risk_level = "medium"
# Whether human approval is needed for first activation
require_activation_approval = true
```

### 4.3 Manifest → Existing Type Mapping

Every manifest field maps to a type that already exists in the codebase:

| Manifest Field | Existing Type | Crate | Status |
|---|---|---|---|
| `tools.allowed` | `SkillMetadata.allowed_tools` | `arcan-harness` | `[IMPLEMENTED]` |
| `policy.sandbox_tier` | `SandboxTier` (None/Basic/Restricted) | `arcan-harness` | `[IMPLEMENTED]` |
| `resources.network.allow` | `NetworkPolicy::AllowList(Vec<String>)` | `arcan-harness` | `[IMPLEMENTED]` |
| `resources.filesystem.*` | `FsPolicy` allowlist/denylist | `arcan-harness` | `[IMPLEMENTED]` |
| `policy.risk_level` | `RiskLevel` (Low/Medium/High/Critical) | `aios-protocol` | `[IMPLEMENTED]` |
| `resources.secrets.*` | `Capability::secrets(scope)` | `aios-protocol` | Type exists, no resolver |
| `resources.schedule.cron` | `EventKind::Heartbeat` | `aios-protocol` | Event exists, no scheduler |
| `skill.version` | — | — | `[PROPOSED]` |
| `require_activation_approval` | `ApprovalGate` | `arcan-lago` | `[IMPLEMENTED]` for tools |

### 4.4 Skills as Lago Artifacts `[PROPOSED]`

To make skills durable, versioned, and auditable:

**Storage**: Skill manifests stored as content-addressed blobs in `BlobStore` (SHA-256 + zstd, automatic deduplication). Skill files tracked in `Manifest` entries pointing to blob hashes.

**Lifecycle events** (new `EventKind` variants, forward-compatible via `Custom` until formalized):

| Event | Fields | Purpose |
|---|---|---|
| `SkillInstalled` | skill_id, version, manifest_hash: BlobHash, installed_by | Skill added to platform |
| `SkillActivated` | skill_id, session_id, capabilities_granted: Vec\<Capability\> | Skill enabled for a session |
| `SkillDeactivated` | skill_id, session_id, reason | Skill disabled |
| `SkillScheduled` | skill_id, schedule (cron), next_run (timestamp) | Recurring execution registered |

**Versioning**: Each skill version is a unique `BlobHash` of its manifest + instruction files. The journal provides a complete audit trail of installs, activations, and deactivations.

---

## 5. The SaaS Trajectory — What Needs to Be Built

### 5.1 Three Hard Dependencies

The Garmin Connect skill (from skills.sh) exposes the exact gaps between single-user runtime and multi-tenant platform:

| Dependency | What the skill needs | What exists today | Gap |
|---|---|---|---|
| **Credential isolation** | Per-tenant `garmin_email` + `garmin_password` | API keys from env vars only. `Capability::secrets("scope")` exists as a type. MCP child processes inherit parent env unsanitized. | No `SecretResolver`, no `TenantId`, no per-tenant scoping |
| **Tenant workspace** | Writes to `health/YYYY-MM-DD.md` isolated per tenant | `FsPolicy` enforces workspace boundaries. `BranchManager` supports copy-on-write forking. Both session-scoped only. | No tenant dimension in compound key or branching |
| **Scheduled execution** | Daily sync at 6am | `HeartbeatScheduler` with pluggable checks. `EventKind::Heartbeat` exists. | Heartbeat is monitoring-only, not a trigger system |

### 5.2 Proposed: TenantId `[PROPOSED]`

New type in `aios-protocol/src/ids.rs` using the existing `typed_id!` macro:

```rust
typed_id!(
    /// Unique identifier for a tenant (organization or user boundary).
    TenantId
);
```

**Where it appears**:
- `EventEnvelope` gains `tenant_id: Option<TenantId>` (backward-compatible via `skip_serializing_if`)
- `SessionManifest` gains `tenant_id: Option<TenantId>`
- `PolicyContext` gains `tenant_id: Option<TenantId>`
- `RbacManager` gains tenant-scoped role assignment

**Isolation strategy** (two options, not mutually exclusive):
1. **Logical**: Tenant_id as filter in queries. Same redb instance, compound key gains optional tenant prefix.
2. **Physical**: Separate redb instance per tenant. Operationally heavier but strongest isolation.

### 5.3 Proposed: SecretResolver Trait `[PROPOSED]`

New trait in `arcan-core` (or `aios-protocol` if it should be kernel-level):

```rust
/// Resolves secrets for a given tenant at runtime.
/// Secrets are never stored in the event journal.
pub trait SecretResolver: Send + Sync {
    fn resolve(
        &self,
        tenant_id: &TenantId,
        secret_name: &str,
        scope: SecretScope,
    ) -> Result<SecretValue, SecretError>;
}

pub enum SecretScope {
    Tenant,    // per-tenant (e.g., user's Garmin credentials)
    Platform,  // platform-wide (e.g., shared API keys)
    Session,   // ephemeral, session-scoped
}
```

**Integration points**:
- `SandboxPolicy` gains `secret_resolver: Option<Arc<dyn SecretResolver>>`
- `LocalCommandRunner` uses resolver to inject env vars instead of inheriting parent env
- MCP child process construction sanitizes env, injects only resolved secrets
- `PolicyEngine` evaluates `Capability::secrets("tenant/garmin_email")` before resolution

**Implementations**:
- `EnvVarSecretResolver`: Wraps current behavior (reads from process env). Zero-migration for existing single-user deployments.
- `VaultSecretResolver`: HashiCorp Vault integration (production SaaS).
- `EncryptedBlobSecretResolver`: Secrets encrypted in Lago blob store with per-tenant keys.

### 5.4 Proposed: SkillScheduler `[PROPOSED]`

New component in `arcand` (or a separate `arcan-scheduler` crate):

```rust
pub struct SkillScheduler {
    journal: Arc<dyn Journal>,
    skill_registry: Arc<SkillRegistry>,
    schedules: Vec<ScheduleEntry>,
}

pub struct ScheduleEntry {
    skill_id: String,
    tenant_id: TenantId,
    cron: CronExpression,
    next_run: DateTime<Utc>,
    last_run: Option<DateTime<Utc>>,
}
```

**Execution model**: The scheduler emits `EventKind::Heartbeat` events that trigger `AgentLoop.run()` with the scheduled skill's context injected. This reuses the existing heartbeat event type and the existing agent loop — no new execution path required.

**Lifecycle tracking**: Each scheduled execution produces journal events (`SkillActivated` → tool events → `SkillDeactivated`), providing a complete audit trail.

### 5.5 Implementation Crate Map

| Component | Target Crate | Dependencies | New/Modified |
|---|---|---|---|
| `TenantId` | `aios-protocol` | None (ids.rs macro) | Modified |
| `SecretScope` enum | `aios-protocol` | None | New in policy.rs |
| `SecretResolver` trait | `arcan-core` | `aios-protocol` | New module |
| `EnvVarSecretResolver` | `arcan-harness` | `arcan-core` | New in sandbox.rs |
| `SkillManifest` parser | `arcan-harness` | `toml`, `serde` | New module |
| Skill lifecycle events | `aios-protocol` | None | Modified (EventKind) |
| `SkillScheduler` | `arcand` | `arcan-core`, `lago-core` | New module |
| Tenant-scoped branching | `lago-fs`, `arcan-lago` | `lago-core` | Modified |
| Auth middleware (JWT) | `lago-api`, `arcand` | `axum`, `jsonwebtoken` | New module |

### 5.6 Build Order

Dependencies flow top-down; each phase unlocks the next:

```
Phase A: Contract Extensions (aios-protocol)
  ├── TenantId type
  ├── SecretScope enum
  └── SkillInstalled/Activated/Deactivated/Scheduled EventKind variants
        │
Phase B: Credential Isolation (arcan-harness, arcan-core)
  ├── SecretResolver trait
  ├── SandboxPolicy gains secret_resolver
  ├── MCP bridge sanitizes child env via resolver
  └── PolicyEngine evaluates Capability::secrets()
        │
Phase C: Skill Manifest System (arcan-harness, arcan-lago)
  ├── skill-manifest.toml parser
  ├── Manifest → SandboxPolicy + FsPolicy + NetworkPolicy mapping
  ├── Lago blob storage for skill artifacts
  └── Skill lifecycle events in journal
        │
Phase D: Tenant-Scoped Workspace (lago-fs, lago-journal, arcan-lago)
  ├── EventEnvelope gains optional tenant_id
  ├── BranchManager gains tenant-scoped default branches
  ├── RbacManager gains tenant-scoped roles
  └── Session creation includes tenant context
        │
Phase E: Scheduled Execution (arcand)
  ├── SkillScheduler with cron parsing
  ├── Heartbeat → skill activation pipeline
  └── AgentLoop integration for scheduled runs
        │
Phase F: Auth & API Gateway (lago-api, arcand)
  ├── JWT / API key middleware
  ├── Tenant extraction from auth token
  ├── Session isolation enforcement
  └── Per-request RBAC enforcement
```

---

## 6. The Full Platform Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                    SaaS Control Plane                        │
│   tenant mgmt / billing / skill marketplace / auth gateway   │
└───────────────┬─────────────────────────┬───────────────────┘
                │                         │
    ┌───────────▼───────────┐  ┌──────────▼──────────────┐
    │     arcand daemon     │  │    lago-api (HTTP/SSE)   │
    │  orchestrator loop    │  │   multi-format streams   │
    │  harness + sandbox    │  │   per-tenant feeds       │
    │  skill scheduler      │  │   REST endpoints        │
    │  secret resolver      │  └──────────┬──────────────┘
    └───────────┬───────────┘             │
                │                         │
    ┌───────────▼─────────────────────────▼──────────────┐
    │              arcan-lago bridge                      │
    │   event_map ∙ policy_middleware ∙ approval_gate     │
    │   state_projection ∙ memory_projection ∙ sse_bridge │
    └───────────────────────┬────────────────────────────┘
                            │
    ┌───────────────────────▼────────────────────────────┐
    │                    Lago substrate                   │
    │                                                     │
    │   lago-journal          lago-store                  │
    │   (redb, ACID,          (SHA-256 + zstd,            │
    │    60B compound key)     content-addressed)          │
    │                                                     │
    │   lago-fs               lago-policy                 │
    │   (manifest,            (rules, RBAC,               │
    │    branching, diff)      hooks, TOML)                │
    └───────────────────────┬────────────────────────────┘
                            │
    ┌───────────────────────▼────────────────────────────┐
    │              aios-protocol (kernel contract)        │
    │                                                     │
    │   EventKind (55 variants)  ∙  Typed IDs             │
    │   AgentStateVector  ∙  OperatingMode  ∙  BudgetState│
    │   Capability  ∙  PolicySet  ∙  MemoryScope          │
    │   SoulProfile  ∙  Observation  ∙  Provenance        │
    └────────────────────────────────────────────────────┘
```

In the SaaS configuration, skills are stateless callables. All statefulness — credentials, filesystem, scheduling, outputs — is owned by the platform layer, not the skill.

---

## 7. Type Alignment Across Projects

Type duplication is minimal and well-managed:

| Category | Canonical Home | Arcan | Lago | Notes |
|---|---|---|---|---|
| Event taxonomy | `aios-protocol::EventKind` | Converts via `event_map.rs` | Uses directly (`EventPayload = EventKind`) | Clean separation |
| IDs (ULID/UUID) | `aios-protocol::ids` | String conversions | `lago-core::id` (re-exports) | Trivial bridge |
| Enums (RiskLevel, etc.) | `aios-protocol::event` | String parsing in event_map | Re-exports from aios | No duplication |
| Tool metadata | `arcan-core::protocol::ToolDefinition` | Arcan-specific (MCP-aligned) | Not used | Not yet canonical |
| AppState | `arcan-core::state::AppState` | Arcan-specific (JSON + patches) | Not used | Not yet canonical |
| Policy types | Divergent | Middleware hooks | Rule engine + RBAC | Complementary, not duplicated |
| Skill metadata | `arcan-harness::skills::SkillMetadata` | Arcan-only | Not used | Not yet persisted |

**Unification path**: Skills, AppState, and tool definitions should eventually be canonicalized in `aios-protocol` (Phase 7). The `event_map.rs` bridge would then convert fewer types at the boundary.

---

## 8. Appendix — The skills.sh Bridge Pattern

Concrete example: translating the Garmin Connect skill from local filesystem execution to managed platform execution.

### Before (local, single-user)

```
~/.agents/skills/garmin-pulse/
├── SKILL.md                    # Instructions
├── scripts/
│   └── sync_garmin.py          # Execution script
└── health/
    └── 2026-02-17.md           # Output files

Auth: ~/.garminconnect/ (cached tokens)
Schedule: crontab entry
Isolation: None (runs as current user)
```

### After (managed, multi-tenant)

```toml
# skill-manifest.toml
[skill]
name = "garmin-health"
version = "0.1.0"
description = "Sync health metrics from Garmin Connect"

[resources.secrets]
garmin_email = { scope = "tenant", required = true }
garmin_password = { scope = "tenant", required = true }

[resources.filesystem]
writes = ["health/{date}.md"]
reads = ["health/*.md"]

[resources.network]
allow = ["connect.garmin.com", "sso.garmin.com"]

[resources.schedule]
cron = "0 6 * * *"

[tools]
allowed = ["bash", "write_file", "read_file"]

[policy]
sandbox_tier = "basic"
risk_level = "medium"
require_activation_approval = true
```

**What the platform does at activation time**:

1. **Credential resolution**: `SecretResolver.resolve(tenant_id, "garmin_email", Tenant)` → inject into `SandboxPolicy.allowed_env`
2. **Workspace provisioning**: `BranchManager` creates `tenant/{id}/skills/garmin-health/` branch in Lago
3. **Policy construction**: `FsPolicy` allowlist set to `health/*.md`; `NetworkPolicy::AllowList(["connect.garmin.com", "sso.garmin.com"])`; `SandboxTier::Basic`
4. **Schedule registration**: `SkillScheduler` adds `ScheduleEntry { cron: "0 6 * * *", tenant_id, skill_id }`
5. **Execution**: At 6am UTC, scheduler emits `Heartbeat` → `AgentLoop.run()` with skill context → tool calls execute within sandbox → events persisted to journal → output in tenant's workspace branch

The skill itself is unchanged — it's still a SKILL.md with bash commands. The platform mediates all resource access through the kernel primitives that already exist.

---

## References

- `docs/ARCHITECTURE.md` — System internals and crate diagrams
- `docs/ROADMAP.md` — 7-phase development roadmap (Phase 3: Skills, Phase 5: Security)
- `docs/STATUS.md` — Implementation status, test counts, known gaps
- `docs/arcan.md` — Vision document and market positioning
- `arcan/CLAUDE.md` / `lago/CLAUDE.md` — Project-specific conventions
