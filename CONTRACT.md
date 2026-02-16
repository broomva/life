# Agent OS: Kernel Contract

The `agent-kernel` crate (published by aiOS) defines the canonical types, event taxonomy, and interfaces that all Agent OS projects implement. This document is the reference for the contract.

## Schema Versioning

- `agent-kernel` follows **semantic versioning**:
  - **patch**: additive optional fields, documentation
  - **minor**: new event variants (forward-compatible via `Custom`), new traits
  - **major**: breaking changes to existing types or trait signatures
- Downstream projects pin to commit SHA during development, semver tags once stable
- Breaking changes require updating all downstream revs simultaneously

## Compatibility Matrix

| Arcan version | Lago version | agent-kernel version | Notes |
|---------------|-------------|---------------------|-------|
| (pre-unification) | (pre-unification) | N/A | Current: separate event models |
| TBD | TBD | 0.1.0 | First unified release |

---

## Canonical Event Taxonomy

All events are wrapped in `EventEnvelope`:

```
EventEnvelope {
    event_id:       EventId (ULID)
    session_id:     SessionId
    branch_id:      BranchId
    run_id:         Option<RunId>
    seq:            SeqNo (u64, monotonic per branch)
    timestamp:      u64 (microseconds since UNIX epoch)
    parent_id:      Option<EventId> (causal chain)
    kind:           EventKind (discriminated union)
    metadata:       HashMap<String, String>
    schema_version: u8 (default: 1)
}
```

### Event Categories

| Category | Events | Source |
|----------|--------|--------|
| **Session** | SessionCreated, SessionResumed, SessionClosed | Runtime |
| **Branch** | BranchCreated, BranchMerged | Runtime/Substrate |
| **Phase** | PhaseEntered (Perceive, Deliberate, Gate, Execute, Commit, Reflect, Sleep) | Runtime |
| **Run** | RunStarted, RunFinished, RunErrored | Runtime |
| **Step** | StepStarted, StepFinished | Runtime |
| **Text** | TextDelta, MessageCommitted | Runtime |
| **Tool** | ToolCallRequested, ToolCallStarted, ToolCallCompleted, ToolCallFailed | Runtime/Harness |
| **File** | FileWrite, FileDelete, FileRename, FileMutated | Harness |
| **State** | StatePatched, ContextCompacted | Runtime |
| **Policy** | PolicyEvaluated | Policy Engine |
| **Approval** | ApprovalRequested, ApprovalResolved | Runtime/Human |
| **Sandbox** | SandboxCreated, SandboxExecuted, SandboxViolation, SandboxDestroyed | Harness |
| **Memory** | ObservationAppended, ReflectionCompacted, MemoryProposed, MemoryCommitted, MemoryTombstoned | Memory Service |
| **Homeostasis** | Heartbeat, StateEstimated, BudgetUpdated, ModeChanged, GatesUpdated, CircuitBreakerTripped | Autonomic |
| **Checkpoint** | CheckpointCreated, CheckpointRestored | Runtime/Substrate |
| **Voice** | VoiceSessionStarted, VoiceInputChunk, VoiceOutputChunk, VoiceSessionStopped, VoiceAdapterError | I/O Adapter |
| **World** | WorldModelObserved, WorldModelRollout, WorldModelDeltaApplied | Simulation |
| **Intent** | IntentProposed, IntentEvaluated, IntentApproved, IntentRejected | Runtime |
| **Error** | ErrorRaised | Any |
| **Custom** | Custom { event_type, data } | Any (forward-compatible) |

### Forward Compatibility

Unknown `"type"` tags in the event payload deserialize to `Custom { event_type, data }` rather than failing. This ensures older code can read events from newer versions without data loss.

---

## Core Invariants

These invariants must hold across all implementations:

### 1. No Invisible State
If it matters for behavior, it must be in events or workspace. No hidden in-memory state that isn't derived from the journal.

### 2. Provenance is Mandatory
Every memory item (observation, reflection, soul update) must reference:
- Source event IDs (the events it was derived from)
- File hashes (if derived from file content)
- Timestamp and actor identity

### 3. Tool Execution is Mediated
No agent directly hits the outside world. All side effects flow through `Harness.execute_tool()` with policy evaluation.

### 4. Checkpoints Bracket Risk
Pre-risk checkpoint before destructive or irreversible actions. Post-success checkpoint after completion.

### 5. Replay Has Defined Meaning
"Deterministic-ish": same event stream + cached tool results + same workspace snapshot = reproducible behavior within defined bounds (LLM nondeterminism is the allowed exception).

### 6. Sequences Are Monotonic Per Branch
Each (session_id, branch_id) pair maintains a strictly monotonic sequence counter. No gaps, no duplicates.

### 7. Events Are Immutable
Once appended to the journal, events are never modified or deleted. "Forgetting" is achieved through tombstone events and projection changes, not deletion.

---

## Kernel Trait Interfaces

These traits define the boundaries between components. Implementations live in their respective projects.

### Journal (persistence)
```
append(event) -> SeqNo
read(query) -> Vec<EventEnvelope>
head_seq(session, branch) -> SeqNo
stream(session, branch, after_seq) -> EventStream
```

### PolicyGate (security)
```
evaluate(context) -> PolicyEvaluation { allowed, requires_approval, denied }
```

### Harness (execution)
```
execute_tool(call, gates) -> ToolResult
```

### MemoryStore (memory)
```
load_soul(session) -> SoulProfile
append_observation(session, observation) -> ()
retrieve(query) -> Vec<Observation>
```

### AutonomicController (homeostasis)
```
on_heartbeat(state_vector, event_window) -> Vec<AutonomicDecision>
```

---

## Replay Invariants

For conformance testing, the following must hold:

1. **Event roundtrip**: `EventEnvelope` -> JSON -> `EventEnvelope` = identical
2. **State reconstruction**: replaying events from seq 0 produces identical derived state
3. **Checkpoint integrity**: `checkpoint.state_hash` matches SHA-256 of reconstructed state
4. **Provenance validity**: every memory item's source event IDs exist in the journal
5. **Sequence continuity**: no gaps in per-branch sequence numbers
