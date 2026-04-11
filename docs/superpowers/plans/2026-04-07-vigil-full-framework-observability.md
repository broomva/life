# Vigil Full-Framework Observability Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every agent lifecycle event across ALL Life framework modules observable through Vigil → OTel → LangSmith, with proper semantic conventions, input/output data, and parent-child span relationships.

**Architecture:** Vigil is the single observability SDK. All modules emit `info_span!` (visible in OTel) or `debug_span!` (local-only) via Vigil helpers. Each span carries semantic attributes describing WHAT it does and WHY. The OTel layer exports INFO+ spans to LangSmith with proper thread grouping.

**Tech Stack:** Rust `tracing` → `life-vigil` spans/events → `tracing-opentelemetry` → OTLP HTTP/protobuf → LangSmith

---

## Current State (from audit)

| Module | Vigil? | Spans | Attributes | Status |
|--------|--------|-------|------------|--------|
| arcand | YES | invoke_agent, consciousness | Rich | OK |
| arcan-aios-adapters | YES | chat, execute_tool | Rich | OK |
| aios-runtime | YES | loop_phase, tick_on_branch | Rich | OK |
| lago-journal | NO | lago.journal.* (debug) | Sparse | **Needs enrichment + restore to INFO** |
| lago-api | NO | #[instrument] implicit | Sparse | **Needs vigil integration** |
| autonomic | NO | None | None | **Needs full instrumentation** |
| praxis-tools | NO | debug_span fs ops | Moderate | **Needs vigil integration** |
| praxis-mcp-bridge | NO | Raw tracing | Sparse | **Needs vigil integration** |
| nous | NO | None | None | **Needs full instrumentation** |
| haima | NO | None | None | **Needs full instrumentation** |
| arcan-provider | NO | None | None | OK (provider adapter handles it) |
| arcan-harness | NO | None | None | OK (praxis-tools handles it) |

## Design Principles

1. **Everything the agent does is visible** — no silent persistence or evaluation
2. **Every span tells a story** — carries WHAT event kind, WHO (session/agent), and meaningful I/O
3. **Parent-child hierarchy is correct** — spans nest under the agent loop, never orphaned
4. **Noise is controlled by span naming, not filtering** — all spans at INFO, use span names/attributes for filtering in LangSmith
5. **OTel level filter stays at INFO** — truly internal debug spans (retries, connection pooling) stay at DEBUG

---

### Task 1: Add `EventKind::variant_name()` helper

**Files:**
- Modify: `crates/aios/aios-protocol/src/event.rs`
- Test: same file (`#[cfg(test)]` module)

This enables lago journal spans to carry the event kind as a human-readable attribute.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn event_kind_variant_name() {
    let kind = EventKind::PhaseEntered { phase: LoopPhase::Perceive };
    assert_eq!(kind.variant_name(), "PhaseEntered");

    let kind = EventKind::TextDelta { delta: "hi".into(), index: None };
    assert_eq!(kind.variant_name(), "TextDelta");

    let kind = EventKind::RunFinished {
        reason: "completed".into(),
        total_iterations: 1,
        final_answer: None,
        usage: None,
    };
    assert_eq!(kind.variant_name(), "RunFinished");
}
```

- [ ] **Step 2: Implement `variant_name()` on EventKind**

Add to the `impl EventKind` block:

```rust
/// Returns the serde tag name for this variant (e.g. "PhaseEntered", "TextDelta").
///
/// Used by Vigil to label lago journal spans with the event kind being persisted.
pub fn variant_name(&self) -> &'static str {
    match self {
        Self::UserMessage { .. } => "UserMessage",
        Self::ExternalSignal { .. } => "ExternalSignal",
        Self::SessionCreated { .. } => "SessionCreated",
        Self::SessionResumed { .. } => "SessionResumed",
        Self::SessionClosed { .. } => "SessionClosed",
        Self::BranchCreated { .. } => "BranchCreated",
        Self::BranchMerged { .. } => "BranchMerged",
        Self::PhaseEntered { .. } => "PhaseEntered",
        Self::DeliberationProposed { .. } => "DeliberationProposed",
        Self::RunStarted { .. } => "RunStarted",
        Self::RunFinished { .. } => "RunFinished",
        Self::RunErrored { .. } => "RunErrored",
        Self::StepStarted { .. } => "StepStarted",
        Self::StepFinished { .. } => "StepFinished",
        Self::AssistantTextDelta { .. } => "AssistantTextDelta",
        Self::AssistantMessageCommitted { .. } => "AssistantMessageCommitted",
        Self::TextDelta { .. } => "TextDelta",
        Self::Message { .. } => "Message",
        Self::ToolCallRequested { .. } => "ToolCallRequested",
        Self::ToolCallStarted { .. } => "ToolCallStarted",
        Self::ToolCallCompleted { .. } => "ToolCallCompleted",
        Self::ToolCallFailed { .. } => "ToolCallFailed",
        Self::FileWrite { .. } => "FileWrite",
        Self::FileDelete { .. } => "FileDelete",
        Self::FileRename { .. } => "FileRename",
        Self::FileMutated { .. } => "FileMutated",
        Self::StatePatchCommitted { .. } => "StatePatchCommitted",
        Self::StatePatched { .. } => "StatePatched",
        Self::ContextCompacted { .. } => "ContextCompacted",
        Self::PolicyEvaluated { .. } => "PolicyEvaluated",
        Self::ApprovalRequested { .. } => "ApprovalRequested",
        Self::ApprovalResolved { .. } => "ApprovalResolved",
        Self::SnapshotCreated { .. } => "SnapshotCreated",
        Self::SandboxCreated { .. } => "SandboxCreated",
        Self::SandboxExecuted { .. } => "SandboxExecuted",
        Self::SandboxViolation { .. } => "SandboxViolation",
        Self::SandboxDestroyed { .. } => "SandboxDestroyed",
        // ... all remaining variants
        _ => "Custom",
    }
}
```

NOTE: The implementer must enumerate ALL variants from event.rs. Use a catch-all `_` only for `Custom` variants.

- [ ] **Step 3: Run tests**

```bash
cargo test -p aios-protocol -- event_kind_variant_name
```

- [ ] **Step 4: Commit**

```bash
git add crates/aios/aios-protocol/src/event.rs
git commit -m "feat(aios): add EventKind::variant_name() for observability spans"
```

---

### Task 2: Restore and enrich lago journal spans

**Files:**
- Modify: `crates/lago/lago-journal/src/redb_journal.rs`
- Modify: `crates/lago/lago-journal/Cargo.toml` (add aios-protocol dep if needed)

Revert `debug_span!` → `info_span!` and add `lago.event_kind` attribute using `variant_name()`.

- [ ] **Step 1: Update `append()` span**

```rust
fn append(&self, event: EventEnvelope) -> ... {
    let event_kind = event.kind.variant_name();
    let span = tracing::info_span!(
        "lago.journal.append",
        lago.stream_id = %event.session_id,
        lago.event_kind = event_kind,
        lago.event_count = 1,
    );
    // ... rest unchanged
}
```

- [ ] **Step 2: Update `append_batch()` span**

```rust
fn append_batch(&self, events: Vec<EventEnvelope>) -> ... {
    let first_kind = events.first().map(|e| e.kind.variant_name()).unwrap_or("empty");
    let span = tracing::info_span!(
        "lago.journal.append_batch",
        lago.event_kind = first_kind,
        lago.event_count = events.len(),
    );
    // ... rest unchanged
}
```

- [ ] **Step 3: Update `read()` span**

```rust
fn read(&self, query: EventQuery) -> ... {
    let span = tracing::info_span!(
        "lago.journal.read",
        lago.stream_id = %query.session_id,
        lago.branch_id = %query.branch_id,
    );
    // ... rest unchanged
}
```

- [ ] **Step 4: Update `head_seq()` and `stream()` similarly**

- [ ] **Step 5: Run tests**

```bash
cargo test -p lago-journal
```

- [ ] **Step 6: Commit**

```bash
git add crates/lago/lago-journal/src/redb_journal.rs
git commit -m "feat(lago): enrich journal spans with event_kind attribute"
```

---

### Task 3: Remove OTel INFO LevelFilter (restore full visibility)

**Files:**
- Modify: `crates/vigil/life-vigil/src/lib.rs`

Since lago spans are now meaningful at INFO level, and we WANT all agent activity visible, remove the filter. Keep the `debug_span!` convention for truly internal spans (connection pooling, retries, etc.).

- [ ] **Step 1: Revert the OTel layer filter**

Change:
```rust
let otel_layer = tracing_opentelemetry::layer()
    .with_tracer(tracer)
    .with_filter(tracing_subscriber::filter::LevelFilter::INFO);
```

Back to:
```rust
let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
```

NOTE: The `debug_span!` convention in lago-journal (and future modules) already controls visibility — DEBUG spans won't reach OTel because the overall subscriber filter is INFO by default. But the OTel layer itself should not duplicate this filtering, because it prevents INFO-level spans from modules with custom EnvFilter overrides.

Wait — actually this IS needed. Without it, the OTel layer processes ALL spans including debug. The EnvFilter on the fmt layer is per-layer, not global. So keep the LevelFilter but explain why.

Actually, the correct understanding: `tracing_subscriber::registry()` dispatches to ALL layers. Each layer can have its own filter. Without a filter on the OTel layer, it receives debug+info+warn+error spans. We want only INFO+ in OTel.

**Decision: Keep the LevelFilter::INFO on the OTel layer.** This is correct. The lago spans are now back at INFO so they pass the filter.

- [ ] **Step 1: Verify the OTel layer filter is present (no change needed)**

Confirm `lib.rs` has:
```rust
let otel_layer = tracing_opentelemetry::layer()
    .with_tracer(tracer)
    .with_filter(tracing_subscriber::filter::LevelFilter::INFO);
```

This is correct. INFO+ spans (including enriched lago spans) are exported. DEBUG spans (internal) are not.

- [ ] **Step 2: Commit (no-op if already correct)**

---

### Task 4: Add Vigil spans to autonomic controller

**Files:**
- Modify: `crates/autonomic/autonomic-controller/Cargo.toml` (add tracing dep)
- Modify: `crates/autonomic/autonomic-controller/src/lib.rs` or `ruleset.rs`
- Modify: `crates/arcan/arcan-aios-adapters/src/autonomic.rs` (where autonomic is called)

Autonomic evaluations (mode transitions, gating decisions) should appear as spans in the agent trace.

- [ ] **Step 1: Add tracing to autonomic evaluation call site in arcan-aios-adapters**

In `embedded_autonomic.rs` or wherever `evaluate_after_run()` is called, wrap it in a span:

```rust
let autonomic_span = tracing::info_span!(
    "autonomic.evaluate",
    autonomic.economic_mode = tracing::field::Empty,
    autonomic.ruling = tracing::field::Empty,
);
let _enter = autonomic_span.enter();
autonomic.evaluate_after_run(&tick.state);
if let Some(ref advice) = autonomic.last_ruling {
    autonomic_span.record("autonomic.economic_mode", advice.mode.as_str());
    autonomic_span.record("autonomic.ruling", advice.ruling.as_str());
}
```

- [ ] **Step 2: Run tests and commit**

---

### Task 5: Add Vigil spans to praxis tool execution

**Files:**
- Modify: `crates/praxis/praxis-tools/src/shell.rs`
- Modify: `crates/praxis/praxis-tools/src/fs.rs`
- Modify: `crates/praxis/praxis-tools/src/edit.rs`

Praxis tools are the hands of the agent. Each tool execution should carry the command/path being operated on.

- [ ] **Step 1: Upgrade shell.rs spans from debug_span to info_span with attributes**

```rust
let span = tracing::info_span!(
    "praxis.shell.execute",
    praxis.command = %command_name,
    praxis.exit_code = tracing::field::Empty,
    praxis.duration_ms = tracing::field::Empty,
);
```

- [ ] **Step 2: Upgrade fs.rs spans similarly**

```rust
let span = tracing::info_span!(
    "praxis.fs.read",
    praxis.path = %path,
    praxis.size_bytes = tracing::field::Empty,
);
```

- [ ] **Step 3: Upgrade edit.rs spans**

```rust
let span = tracing::info_span!(
    "praxis.edit",
    praxis.path = %path,
    praxis.operation = "replace", // or "insert", "delete"
);
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p praxis-tools
```

- [ ] **Step 5: Commit**

---

### Task 6: Add Vigil spans to nous evaluation

**Files:**
- Modify: `crates/nous/nous-middleware/src/lib.rs` (or wherever evaluators are invoked)

Nous evaluations (safety, coherence, budget) should appear as events on the agent span.

- [ ] **Step 1: Emit eval events using existing `life_vigil::spans::eval_event()`**

The vigil SDK already has `eval_event()`. Wire it at the call site where nous evaluators produce results:

```rust
life_vigil::spans::eval_event(
    evaluator_name,
    score,
    label, // "good", "warning", "critical"
    layer, // "reasoning", "action", "execution", "safety", "cost"
    timing, // "inline", "async"
);
```

- [ ] **Step 2: Run tests and commit**

---

### Task 7: Add Vigil spans to haima finance operations

**Files:**
- Modify: `crates/haima/haima-core/src/lib.rs` or billing call sites

Finance operations (cost tracking, payment decisions) should carry economic context.

- [ ] **Step 1: Add spans for billing operations**

```rust
let span = tracing::info_span!(
    "haima.billing",
    haima.operation = "task_billed",
    haima.cost_usd = cost,
    haima.payment_method = "x402",
);
```

- [ ] **Step 2: Run tests and commit**

---

### Task 8: Verify end-to-end in LangSmith

- [ ] **Step 1: Start arcan with OTel config**
- [ ] **Step 2: Send multi-turn conversation**
- [ ] **Step 3: Check LangSmith trace waterfall shows:**
  - `invoke_agent` (root, with input/output)
    - `tick_on_branch`
      - `loop_phase` (perceive, deliberate, execute, commit, reflect)
      - `lago.journal.append` (with event_kind: "PhaseEntered", "TextDelta", etc.)
      - `lago.journal.read` (with session/branch context)
      - `chat` (llm, with model name)
      - `autonomic.evaluate` (with economic mode)
      - Praxis tool spans (if tools used)
- [ ] **Step 4: Check Threads tab groups correctly by arcan session**
- [ ] **Step 5: Screenshot and document**

---

## Priority Order

1. **Task 1** (EventKind::variant_name) — foundational, unblocks Task 2
2. **Task 2** (lago journal enrichment) — biggest visual improvement
3. **Task 3** (verify OTel filter) — quick check
4. **Task 4** (autonomic spans) — medium effort, high value
5. **Task 5** (praxis tool spans) — medium effort, high value for tool-use traces
6. **Task 6** (nous eval spans) — low effort (existing vigil helper)
7. **Task 7** (haima finance spans) — low priority unless payments are active
8. **Task 8** (e2e verification) — final validation
