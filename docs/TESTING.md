# Agent OS: Testing Strategy

## 2026-02-28 Coverage Update

- aiOS workspace: `61/61` tests passing.
- Arcan workspace: `236/236` tests passing (`+1 ignored`).
- Lago workspace: `299/299` tests passing.
- Combined aiOS+Arcan+Lago: `596/596` tests passing (`+1 ignored`).
- Combined crate count (aiOS+Arcan+Lago): `19`.
- Combined Rust LOC (aiOS+Arcan+Lago): `~27.8K`.
- Added root cross-stack conformance entrypoint:
  - `/Users/broomva/broomva.tech/live/conformance/run.sh`
- Conformance runner validates:
  - protocol model tests (`aios-protocol`)
  - Arcand v1 API + canonical SSE parts
  - Arcan-Lago bridge replay path
  - Lago journal sequence assignment invariants
  - Lago API session/SSE behavior

## Current Coverage

### By Crate (Declared Tests + LOC)

| Crate | Declared Tests | Rust LOC |
|---|---:|---:|
| arcan-core | 67 | 3,450 |
| arcan-harness | 39 | 2,519 |
| arcan-aios-adapters | 0 | 327 |
| arcan-store | 7 | 428 |
| arcan-provider | 23 | 1,270 |
| arcan-tui | 14 | 1,432 |
| arcand | 3 | 790 |
| arcan-lago | 80 | 4,471 |
| arcan | 4 | 547 |
| **Arcan Total** | **237** | **15,234** |
| lago-core | 118 | 2,893 |
| lago-journal | 24 | 1,520 |
| lago-store | 17 | 322 |
| lago-fs | 34 | 1,049 |
| lago-ingest | 10 | 588 |
| lago-api | 62 (37 unit + 17 e2e-files + 8 e2e-sessions) | 3,218 |
| lago-policy | 34 | 1,339 |
| lago-aios-eventstore-adapter | 0 | 145 |
| lago-cli | 0 | 1,156 |
| lagod | 0 | 311 |
| **Lago Total** | **299** | **12,541** |
| **Combined Total** | **597 declared** (`596 passing`, `1 ignored`) | **27,775** |

### Coverage Gaps (Priority Order)

1. **arcan-harness** (39 tests, 2,519 lines) — Sandbox enforcement, filesystem guardrails, hashline edits, MCP bridge, memory tools, skill loading
2. **arcand** (3 tests, 790 lines) — Agent loop execution, HTTP server endpoints, SSE streaming
3. **arcan-tui** (14 tests, 1,432 lines) — Canonical API client flows and stream handling paths
4. **lago-cli** (0 tests, 1,156 lines) — Command parsing, formatting, and end-to-end command behavior
5. **lago-aios-eventstore-adapter** (0 tests, 145 lines) — Canonical adapter behavior and conversion edge cases

---

## Testing Layers

### Layer 1: Unit Tests (per-module)

Each module should have a `#[cfg(test)] mod tests` section testing its core logic in isolation.

**Pattern**:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptive_test_name() {
        // Arrange
        let input = ...;

        // Act
        let result = function_under_test(input);

        // Assert
        assert_eq!(result, expected);
    }
}
```

**Conventions**:
- Test names describe the behavior being verified (not the function name)
- Use `tempdir` for filesystem tests (auto-cleanup)
- Use `tokio::test` for async tests
- Use mock providers/journals for isolated testing
- Tests must not require environment variables or network access

### Layer 2: Integration Tests (per-crate)

Located in `tests/` directory at crate level. Test interactions between modules within a crate.

**Existing**: `lago-api/tests/e2e_files.rs` — 17 tests covering full REST API workflow.

**Needed**:
- `arcan/tests/agent_loop.rs` — Full agent loop with mock provider and in-memory journal
- `arcan/tests/session_replay.rs` — Run session, replay from journal, verify identical state
- `lago/tests/journal_stress.rs` — High-volume event append/read consistency

### Layer 3: End-to-End Tests (cross-crate)

Test the full system from HTTP request to event persistence to SSE output.

**Proposed**: `tests/e2e/` directory at root level or in `arcan/tests/`:

```rust
#[tokio::test]
async fn full_session_lifecycle() {
    // 1. Start Arcan server with mock provider + in-memory Lago
    let server = start_test_server().await;

    // 2. POST /chat with user message
    let response = client.post("/chat")
        .json(&json!({"session_id": "test-1", "message": "hello"}))
        .send().await;

    // 3. Consume SSE stream, collect events
    let events: Vec<AgentEvent> = consume_sse_stream(response).await;

    // 4. Verify event sequence: RunStarted → TextDelta+ → RunFinished
    assert!(matches!(events[0], AgentEvent::RunStarted { .. }));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::TextDelta { .. })));
    assert!(matches!(events.last().unwrap(), AgentEvent::RunFinished { .. }));

    // 5. Replay session from journal
    let replayed = session_repo.load_session("test-1")?;
    assert_eq!(events.len(), replayed.len());

    // 6. Verify state reconstruction matches
    let state = project_state(replayed);
    assert_eq!(state.messages.last().unwrap().content, "mock response");
}
```

### Layer 4: Property-Based Tests

For critical data structures where exhaustive testing is impractical.

**Candidates**:
- Event serialization round-trip (any `EventPayload` → JSON → back)
- Hashline edit idempotency (apply same edit twice = same result)
- Journal key encoding/decoding (any valid session+branch+seq → encode → decode = original)
- Manifest operations (any sequence of write/delete/rename → consistent state)
- Policy evaluation (rules are order-independent when priorities differ)

**Tool**: `proptest` crate for Rust property-based testing.

---

## Test Categories

### Correctness Tests

| Category              | What to test                                          | Where         |
|-----------------------|-------------------------------------------------------|---------------|
| Event round-trip      | Arcan event → Lago envelope → Arcan event = identical | arcan-lago    |
| Journal ACID          | Concurrent appends maintain ordering                  | lago-journal  |
| Blob deduplication    | Same content → same hash → one blob on disk           | lago-store    |
| Sandbox enforcement   | Disallowed paths → error, allowed paths → success     | arcan-harness |
| Policy evaluation     | Rules match/deny correctly                            | lago-policy   |
| State reconstruction  | Replay events → same AppState as original run         | arcan-lago    |
| SSE format compliance | OpenAI/Anthropic/Vercel frames match spec             | lago-api      |

### Safety Tests

| Category                | What to test                                      | Where         |
|-------------------------|---------------------------------------------------|---------------|
| Path traversal          | `../` in file paths rejected                      | arcan-harness |
| Sandbox escape          | Tool execution confined to allowed directories    | arcan-harness |
| Policy denial           | Denied tool calls produce errors, not execution   | arcan-lago    |
| Malformed input         | Invalid JSON, empty strings, oversized payloads   | arcan-core    |
| Concurrent access       | Multiple sessions don't corrupt each other        | lago-journal  |

### Performance Tests

| Category              | What to test                                       | Target       |
|-----------------------|----------------------------------------------------|--------------|
| Journal append        | 10K events/second sustained                        | lago-journal |
| Event read            | 100K events loaded in <1 second                    | lago-journal |
| Blob throughput       | 1MB blob write + read in <100ms                    | lago-store   |
| SSE latency           | Event to SSE frame in <10ms                        | lago-api     |
| Session reconstruction| 1000-event session replayed in <500ms              | arcan-lago   |
| Snapshot restore      | Restore from snapshot in <100ms                    | lago-journal |

---

## Testing Workflows

### Developer Workflow (per-change)

```bash
# Quick check (< 30 seconds)
cd arcan && cargo fmt && cargo clippy --workspace && cargo test --workspace
cd ../lago && cargo fmt && cargo clippy --workspace && cargo test --workspace
```

### Full Validation (pre-commit)

```bash
# Cross-project validation (< 2 minutes)
(cd arcan && cargo fmt && cargo clippy --workspace && cargo test --workspace) && \
(cd lago && cargo fmt && cargo clippy --workspace && cargo test --workspace)
```

### Specific Crate Testing

```bash
# Test only the bridge
cd arcan && cargo test -p arcan-lago

# Test only the journal
cd lago && cargo test -p lago-journal

# Test with output
cd arcan && cargo test -p arcan-lago -- --nocapture
```

### Integration Test Execution

```bash
# Run lago-api integration tests
cd lago && cargo test -p lago-api --test e2e_files

# Run with specific test
cd lago && cargo test -p lago-api --test e2e_files -- test_name
```

---

## Immediate Test Plan (Phase 1)

### Week 1: arcan-harness Tests

```
arcan-harness/src/sandbox.rs:
  - test_sandbox_none_allows_all_paths
  - test_sandbox_basic_blocks_denied_paths
  - test_sandbox_basic_allows_listed_paths
  - test_sandbox_restricted_blocks_writes
  - test_sandbox_restricted_allows_approved_writes
  - test_fs_policy_glob_matching

arcan-harness/src/fs.rs:
  - test_read_file_within_sandbox
  - test_read_file_outside_sandbox_denied
  - test_write_file_within_sandbox
  - test_write_file_outside_sandbox_denied
  - test_list_directory_within_sandbox
  - test_search_files_respects_sandbox

arcan-harness/src/edit.rs:
  - test_hashline_edit_applies_correctly
  - test_hashline_edit_idempotent
  - test_hashline_edit_stale_hash_fails
  - test_hashline_edit_concurrent_safe

arcan-harness/src/skills.rs:
  - test_skill_discovery_finds_skill_md
  - test_skill_parsing_valid_frontmatter
  - test_skill_parsing_invalid_frontmatter_skipped
  - test_skill_catalog_system_prompt

arcan-harness/src/memory.rs:
  - test_write_and_read_memory
  - test_list_memory_keys
  - test_search_memory_content

arcan-harness/src/mcp.rs:
  - test_mcp_tool_definition_conversion
  - test_mcp_tool_annotation_mapping
```

### Week 2: arcand + End-to-End Tests

```
arcand/src/loop.rs:
  - test_agent_loop_mock_provider_runs_to_completion
  - test_agent_loop_persists_all_events
  - test_agent_loop_respects_max_iterations
  - test_agent_loop_handles_tool_calls

arcand/src/server.rs:
  - test_chat_endpoint_returns_sse
  - test_health_endpoint
  - test_concurrent_sessions

End-to-end (arcan/tests/):
  - test_full_session_lifecycle
  - test_session_replay_matches_original
  - test_tool_execution_with_sandbox
  - test_policy_middleware_blocks_denied_tools
```

### Week 3: Performance + Stress Tests

```
lago-journal stress:
  - test_append_10k_events_performance
  - test_read_10k_events_performance
  - test_concurrent_append_consistency

lago-store stress:
  - test_large_blob_roundtrip_1mb
  - test_1000_blob_deduplication

Session reconstruction:
  - test_reconstruct_1000_event_session
  - test_reconstruct_with_snapshot
```

---

## Test Infrastructure

### Helpers Needed

```rust
// Test server builder
pub async fn start_test_server() -> TestServer {
    // In-memory Lago journal + mock provider + random port
}

// SSE consumer
pub async fn consume_sse_stream(url: &str) -> Vec<AgentEvent> {
    // Connect to SSE, collect all events until RunFinished
}

// Session factory
pub fn create_test_session(events: Vec<AgentEvent>) -> SessionId {
    // Pre-populate a session with known events
}

// Assertion helpers
pub fn assert_event_sequence(events: &[AgentEvent], expected: &[&str]) {
    // Verify event type sequence matches expected pattern
}
```

### CI Integration

```yaml
# .github/workflows/test.yml
name: Test
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: "1.85"
      - name: Test Arcan
        run: cd arcan && cargo test --workspace
      - name: Test Lago
        run: cd lago && cargo test --workspace
      - name: Clippy
        run: |
          cd arcan && cargo clippy --workspace -- -D warnings
          cd ../lago && cargo clippy --workspace -- -D warnings
```

---

## Test Quality Rules

1. **Every new feature has tests** — No PR merged without tests for new functionality
2. **Tests are deterministic** — No flaky tests, no time-dependent assertions, no network calls
3. **Tests are fast** — Unit tests < 1s each, integration tests < 10s each
4. **Tests document behavior** — Test names describe what behavior is being verified
5. **Failed tests block merges** — CI must pass before merge
6. **Coverage trends up** — Track coverage over time, never decrease

---

## Conformance Test Suite (Planned — `aios-protocol`)

Once the `aios-protocol` crate is extracted from aiOS, a conformance test suite will validate that all implementations (Arcan, Lago, aiOS reference) adhere to the kernel contract.

### Schema Conformance

| Test | What it validates |
|------|-------------------|
| Event roundtrip | `EventEnvelope` -> JSON -> `EventEnvelope` = identical for all ~55 variants |
| Forward compatibility | Unknown event types deserialize to `Custom { event_type, data }` |
| ID format | All IDs are valid ULIDs (26 chars, sortable) |
| Sequence monotonicity | Per-branch sequences are strictly monotonic, no gaps |

### Provenance Conformance

| Test | What it validates |
|------|-------------------|
| Memory provenance | Every `ObservationAppended` references valid event IDs |
| Checkpoint integrity | `checkpoint.state_hash` matches SHA-256 of reconstructed state |
| Tombstone validity | `MemoryTombstoned` references existing `MemoryCommitted` |

### Replay Conformance

| Test | What it validates |
|------|-------------------|
| State reconstruction | Replaying events from seq 0 produces identical derived state |
| Checkpoint restore | Restoring from checkpoint + replaying remaining events = same state |
| Cross-project replay | Events stored in Lago, replayed in Arcan = consistent state |

---

## Golden Replay Tests (Planned)

Curated sessions that serve as regression tests for the entire ecosystem:

### Test Protocol

1. **Record**: Run Arcan with a scripted session (mock provider, deterministic tool results)
2. **Capture**: Store event log + tool results + workspace hashes in golden test data
3. **Replay**: Re-run in "replay mode" (cached tool results, no LLM calls)
4. **Assert**:
   - Same derived state (AgentStateVector, BudgetState)
   - Same artifact hashes
   - Same event sequence class (within allowed nondeterminism bounds)

### Golden Test Sessions (planned)

| Session | What it tests |
|---------|---------------|
| `simple-chat` | Text-only conversation, no tools |
| `file-edit` | Read file → edit with hashline → verify |
| `tool-failure-recovery` | Tool fails → error streak → mode switch to Recover |
| `approval-flow` | Risky tool → approval requested → approved → executed |
| `memory-lifecycle` | Observe → reflect → propose → commit → tombstone |
| `branch-fork` | Fork session → diverge → merge |

### End-to-End Verification Command

```bash
# Full ecosystem verify (all 4 projects)
(cd aiOS && cargo fmt && cargo clippy --workspace && cargo test --workspace) && \
(cd arcan && cargo fmt && cargo clippy --workspace && cargo test --workspace) && \
(cd lago && cargo fmt && cargo clippy --workspace && cargo test --workspace) && \
(cd autonomic && cargo fmt && cargo clippy --workspace && cargo test --workspace)
```
