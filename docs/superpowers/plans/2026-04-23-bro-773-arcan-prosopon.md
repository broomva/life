# BRO-773 — arcan-prosopon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `arcan-prosopon`, a new Rust crate in the Life monorepo that subscribes to Arcan's `KernelRuntime` event broadcast, translates each `aios_protocol::EventKind` into a `prosopon-core::ProsoponEvent` envelope, and publishes the stream to a `prosopon-daemon::EnvelopeFanout` so any Prosopon compositor (text, glass, field, …) can render a live Arcan session.

**Architecture:** Producer-only sidecar (Pneuma<L0ToExternal> for Arcan). A `Translator` is a pure, per-session state machine that maps `EventKind → Vec<ProsoponEvent>`. A `Bridge` spawns a tokio task that pulls `EventRecord`s from a `broadcast::Receiver<EventRecord>`, runs them through the translator, mints `Envelope`s via `prosopon-sdk::Session`, and publishes to the fanout. Wiring into the `arcand` CLI is additive, gated on a `--prosopon-port <addr>` flag, and degrades gracefully (log on bind failure, never panic; existing `arcan-console` continues to work without Prosopon).

**Tech Stack:** Rust 2024, tokio (broadcast + spawn), `prosopon-sdk`, `prosopon-daemon`, `prosopon-protocol`, `prosopon-core`, `aios-protocol` (for `EventRecord` / `EventKind`), `tracing`, `thiserror`, `anyhow`. Path dependency on sibling `core/prosopon/` workspace.

**Linear:** [BRO-773](https://linear.app/broomva/issue/BRO-773) — v0.3.0 milestone.

---

## File structure

```
core/life/crates/arcan/arcan-prosopon/
├── Cargo.toml
├── README.md
├── src/
│   ├── lib.rs            ← public API: ArcanProsoponBridge, glass_surface() re-export
│   ├── error.rs          ← BridgeError (thiserror)
│   ├── state.rs          ← TranslationState (per-session seq, stream registry, root NodeId)
│   ├── translator.rs     ← translate(&mut TranslationState, &EventKind) -> Vec<ProsoponEvent>
│   └── bridge.rs         ← ArcanProsoponBridge — spawn / run loop
└── tests/
    └── bridge_integration.rs  ← end-to-end: mock EventRecord stream → fanout subscriber
```

`translator.rs` owns all `EventKind → ProsoponEvent` mapping logic and is pure (no I/O). It is easy to unit-test variant-by-variant. `bridge.rs` owns the tokio wiring, session id allocation, and graceful-shutdown plumbing. `state.rs` isolates mutable translator state so `translator.rs` can be a collection of stateless `fn`s.

---

## Dependencies (Cargo.toml for arcan-prosopon)

```toml
[package]
name = "arcan-prosopon"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "Apache-2.0"
description = "Pneuma<L0ToExternal> for Arcan — emits ProsoponEvents from the runtime's event stream."
repository = "https://github.com/broomva/life"

[dependencies]
# Prosopon — path dep on sibling workspace while v0.2.0-alpha.2 is unpublished.
prosopon-core     = { path = "../../../../../prosopon/crates/prosopon-core" }
prosopon-protocol = { path = "../../../../../prosopon/crates/prosopon-protocol" }
prosopon-sdk      = { path = "../../../../../prosopon/crates/prosopon-sdk" }
prosopon-daemon   = { path = "../../../../../prosopon/crates/prosopon-daemon" }

# aiOS event source.
aios-protocol = { path = "../../aios/aios-protocol" }

# Runtime.
tokio     = { workspace = true, features = ["sync", "rt", "macros"] }
tracing   = { workspace = true }
thiserror = { workspace = true }
anyhow    = { workspace = true }
serde_json = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["full", "test-util"] }
```

Five path hops from `core/life/crates/arcan/arcan-prosopon/` to `core/prosopon/crates/…`:
`../` → arcan-prosopon parent → `../../` → arcan/ → `../../../` → crates/ → `../../../../` → life/ → `../../../../../` → core/. Verify with `ls ../../../../../prosopon/crates/` from the new crate dir during Task 1.

---

## Translation table (authoritative; drives translator.rs)

Every `EventKind` variant from `core/life/crates/aios/aios-protocol/src/event.rs` (declared `#[non_exhaustive]`) maps as follows. The translator MUST include a `_` wildcard arm that returns an empty `Vec` for forward-compat.

| EventKind variant | ProsoponEvent(s) emitted | Notes |
|---|---|---|
| `UserMessage { content }` | `NodeAdded` under root, `ir::section("User").child(ir::prose(content))` with `attrs["semantic_role"]="info"` | One node per user turn |
| `SessionCreated { name, .. }` | `SceneReset { scene: Scene::new(ir::section(name)) }` | Fresh scene per session |
| `RunStarted { provider, max_iterations }` | `SignalChanged { topic="run.status", value="running" }`, `SignalChanged { topic="run.provider", value=provider }`, `SignalChanged { topic="run.max_iterations", value=max_iterations }` | Boot signals |
| `RunFinished { reason, total_iterations, final_answer, .. }` | If `final_answer.is_some()`: `NodeAdded` with `ir::section("Answer").child(ir::prose(final_answer))`; then `SignalChanged { topic="run.status", value=reason }`; then `Heartbeat` | Status + final prose |
| `RunErrored { error }` | `NodeAdded` with `ir::prose(error).attr("semantic_role","error").priority(Priority::Urgent)` + `SignalChanged { topic="run.status", value="errored" }` | |
| `AssistantTextDelta { delta, .. }` / `TextDelta { delta, .. }` | First delta per iteration: `NodeAdded` with `ir::stream(<fresh StreamId>, StreamKind::Text)`; subsequent deltas: `StreamChunk { id, chunk: StreamChunk::Text(delta) }` | `TranslationState` holds active `StreamId` per iteration |
| `AssistantMessageCommitted { content, .. }` / `Message { content, .. }` | `NodeAdded` with `ir::section("Assistant").child(ir::prose(content))` | Committed messages |
| `ToolCallRequested { call_id, tool_name, arguments, .. }` | `NodeAdded` with `ir::tool_call(tool_name, arguments)`; node id = deterministic from `call_id` so follow-up events can update it | |
| `ToolCallCompleted { call_id?, tool_name, result, status, .. }` | `NodeUpdated { id: tool_node_id(call_id.clone()), patch: NodePatch::child_append(ir::tool_result(status == SpanStatus::Ok, result)) }` if `call_id` set; else `NodeAdded` | `SpanStatus::Ok` → `success=true` |
| `ToolCallFailed { call_id, tool_name, error }` | `NodeUpdated { id: tool_node_id(call_id), patch: NodePatch::child_append(ir::tool_result(false, json!({"error": error}))) }` | |
| `ApprovalRequested { approval_id, tool_name, arguments, risk, .. }` | `NodeAdded` with `ir::confirm(format!("Approve {tool_name}?"), severity_for(risk))` carrying `attrs["approval_id"]=approval_id` | Severity maps: Low→Normal, Medium→High, High/Critical→Urgent |
| `ApprovalResolved { approval_id, decision, .. }` | `NodeUpdated { id: approval_node_id(approval_id), patch: NodePatch::lifecycle(NodeStatus::Resolved) }` + `SignalChanged { topic=format!("approval.{approval_id}"), value=decision }` | |
| `StatePatched { patch, revision, .. }` | `SignalChanged { topic="state.revision", value=revision }` (patch content not forwarded — too noisy) | |
| `ContextCompacted { tokens_before, tokens_after, .. }` | `SignalChanged { topic="context.tokens", value=tokens_after }` + `NodeAdded` with `ir::prose(format!("Compacted {b}→{a} tokens", b=tokens_before, a=tokens_after)).attr("emphasis","low")` | |
| `StepStarted { index }` | `SignalChanged { topic="iteration", value=index }` | |
| `StepFinished { index, stop_reason, .. }` | No translation | Redundant with StateChanged + next StepStarted |
| `PolicyEvaluated { tool_name, decision, .. }` | `SignalChanged { topic=format!("policy.{tool_name}"), value=decision }` | |
| `KnowledgeSearched { query, result_count, .. }` | `NodeAdded` with `ir::prose(format!("Searched: {query} ({result_count})")).attr("emphasis","low")` | |
| `FileWrite { path, size_bytes, .. }` / `FileDelete { path }` / `FileRename { .. }` / `FileMutated { .. }` | `NodeAdded` with `ir::prose(summary)` under a `ir::section("Files")` if present | Nice-to-have; acceptable to skip in v0.1 |
| `SessionResumed`, `SessionClosed`, `BranchCreated`, `BranchMerged`, `PhaseEntered`, `DeliberationProposed`, `ToolCallStarted`, `KnowledgeRetrieved`, `KnowledgeEvaluated`, `StatePatchCommitted`, `ExternalSignal`, every future variant | Empty `Vec` via `_` wildcard | |

Helper `tool_node_id(call_id) -> NodeId` = `NodeId::from(format!("tool:{call_id}"))` — stable so updates land on the same node.
Helper `approval_node_id(approval_id)` analogous.

---

## Task 1: Scaffold the crate

**Files:**
- Create: `core/life/crates/arcan/arcan-prosopon/Cargo.toml`
- Create: `core/life/crates/arcan/arcan-prosopon/src/lib.rs`
- Create: `core/life/crates/arcan/arcan-prosopon/src/error.rs`
- Create: `core/life/crates/arcan/arcan-prosopon/README.md`
- Modify: `core/life/Cargo.toml` — add `"crates/arcan/arcan-prosopon"` to `members` under the `# Arcan` block.

- [ ] **Step 1: Read `core/prosopon/crates/prosopon-sdk/src/session.rs` lines 1-120.**

Run: `cat /Users/broomva/broomva/core/prosopon/crates/prosopon-sdk/src/session.rs | head -120`
Expected: see the exact signature of `Session::new`, `Session::envelope`, `Session::signal`, etc. Confirm the public surface referenced in this plan still exists. If any method name differs, update the references below before proceeding.

- [ ] **Step 2: Read `core/prosopon/crates/prosopon-daemon/src/fanout.rs` and `surface.rs`.**

Run: `cat /Users/broomva/broomva/core/prosopon/crates/prosopon-daemon/src/fanout.rs /Users/broomva/broomva/core/prosopon/crates/prosopon-daemon/src/surface.rs`
Expected: see `EnvelopeFanout::send`, `EnvelopeReceiver::recv`, `SurfaceBundle` constructor. Record exact signatures.

- [ ] **Step 3: Create crate skeleton.**

Write `core/life/crates/arcan/arcan-prosopon/Cargo.toml` using the content from the **Dependencies** section above exactly.

Write `core/life/crates/arcan/arcan-prosopon/src/error.rs`:

```rust
//! Error type for the arcan-prosopon bridge.

use thiserror::Error;

/// Errors raised while running the Arcan → Prosopon bridge.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BridgeError {
    /// The upstream Arcan event broadcast closed.
    #[error("arcan event stream closed")]
    UpstreamClosed,

    /// Publishing to the Prosopon fanout failed.
    #[error("prosopon fanout send failed: {0}")]
    Fanout(#[from] prosopon_daemon::FanoutError),

    /// Envelope encoding failed (should be unreachable — JSON serialisation of known types).
    #[error("envelope encoding failed: {0}")]
    Encoding(#[from] prosopon_protocol::ProtocolError),
}
```

Write `core/life/crates/arcan/arcan-prosopon/src/lib.rs`:

```rust
//! # arcan-prosopon
//!
//! `Pneuma<L0ToExternal>` for Arcan. Subscribes to the runtime's
//! `EventRecord` broadcast, translates each `EventKind` into a
//! `ProsoponEvent`, and publishes envelopes to a `prosopon-daemon`
//! `EnvelopeFanout` for downstream compositors.
//!
//! See `docs/superpowers/plans/2026-04-23-bro-773-arcan-prosopon.md` for the
//! full design and the translation table.

#![forbid(unsafe_code)]

pub mod error;
pub mod state;
pub mod translator;
pub mod bridge;

pub use bridge::ArcanProsoponBridge;
pub use error::BridgeError;
pub use state::TranslationState;
```

(`state`, `translator`, `bridge` modules will be stub files in this task; implementation follows in Tasks 2-7.)

Stub `core/life/crates/arcan/arcan-prosopon/src/state.rs`:

```rust
//! Per-session translator state (stream registry, iteration counter).

use prosopon_core::StreamId;
use std::collections::HashMap;

/// Mutable state maintained across a single arcan session's event stream.
#[derive(Debug, Default)]
pub struct TranslationState {
    /// Active streaming intent id per iteration, for folding `*TextDelta` events.
    pub streams_by_iteration: HashMap<u32, StreamId>,
    /// Current iteration number, if an assistant turn is in progress.
    pub current_iteration: Option<u32>,
}

impl TranslationState {
    pub fn new() -> Self {
        Self::default()
    }
}
```

Stub `core/life/crates/arcan/arcan-prosopon/src/translator.rs`:

```rust
//! Pure translation layer: `aios_protocol::EventKind` → `Vec<ProsoponEvent>`.

use aios_protocol::EventKind;
use prosopon_core::ProsoponEvent;

use crate::state::TranslationState;

/// Translate a single `EventKind` into zero or more `ProsoponEvent`s.
///
/// Total over every currently-known variant and includes a `_` wildcard
/// for `#[non_exhaustive]` forward compatibility.
pub fn translate(_state: &mut TranslationState, kind: &EventKind) -> Vec<ProsoponEvent> {
    match kind {
        _ => Vec::new(),
    }
}
```

Stub `core/life/crates/arcan/arcan-prosopon/src/bridge.rs`:

```rust
//! Bridge — spawns the subscriber task that drains arcan events into the
//! prosopon fanout.

use crate::{BridgeError, TranslationState};
use prosopon_daemon::EnvelopeFanout;
use prosopon_sdk::Session;

/// Drains an `aios_protocol::EventRecord` broadcast into a Prosopon fanout.
pub struct ArcanProsoponBridge {
    _fanout: EnvelopeFanout,
    _session: Session,
    _state: TranslationState,
}

impl ArcanProsoponBridge {
    pub fn new(fanout: EnvelopeFanout) -> Self {
        Self {
            _fanout: fanout,
            _session: Session::new(),
            _state: TranslationState::new(),
        }
    }
}
```

Write a minimal `README.md` pointing to this plan and the Linear issue.

Register the crate in `core/life/Cargo.toml` by inserting `"crates/arcan/arcan-prosopon",` in the arcan block (alphabetical — after `arcan-praxis`, before `arcan-provider`).

- [ ] **Step 4: `cargo check -p arcan-prosopon`.**

Run from `core/life/`: `cargo check -p arcan-prosopon`
Expected: `Checking arcan-prosopon v0.1.0 … Finished dev profile …` with no errors (warnings on unused fields acceptable for the stub).

- [ ] **Step 5: Commit.**

```bash
cd /Users/broomva/broomva/core/life
git checkout -b bro-773-arcan-prosopon
git add crates/arcan/arcan-prosopon/ Cargo.toml
git commit -m "feat(arcan-prosopon): scaffold crate (BRO-773)"
```

---

## Task 2: Translator — session & run lifecycle

**Files:**
- Modify: `core/life/crates/arcan/arcan-prosopon/src/translator.rs`
- Test: same file, `#[cfg(test)] mod tests` at bottom.

- [ ] **Step 1: Write the failing tests.**

Append to `translator.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::EventKind;
    use prosopon_core::{Intent, ProsoponEvent};

    fn st() -> TranslationState { TranslationState::new() }

    #[test]
    fn session_created_emits_scene_reset() {
        let kind = EventKind::SessionCreated {
            name: "sess-a".into(),
            config: serde_json::json!({}),
        };
        let events = translate(&mut st(), &kind);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ProsoponEvent::SceneReset { .. }));
    }

    #[test]
    fn run_started_emits_three_signal_changes() {
        let kind = EventKind::RunStarted {
            provider: "anthropic".into(),
            max_iterations: 8,
        };
        let events = translate(&mut st(), &kind);
        assert_eq!(events.len(), 3);
        for e in &events {
            assert!(matches!(e, ProsoponEvent::SignalChanged { .. }));
        }
    }

    #[test]
    fn run_errored_emits_error_prose_and_status_signal() {
        let kind = EventKind::RunErrored { error: "boom".into() };
        let events = translate(&mut st(), &kind);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ProsoponEvent::NodeAdded { .. }));
        assert!(matches!(events[1], ProsoponEvent::SignalChanged { .. }));
    }

    #[test]
    fn unknown_variant_is_empty() {
        // SessionClosed is in the wildcard set per the translation table.
        let kind = EventKind::SessionClosed { reason: "idle".into() };
        assert!(translate(&mut st(), &kind).is_empty());
    }
}
```

- [ ] **Step 2: Run failing tests.**

Run: `cargo test -p arcan-prosopon translator::`
Expected: compilation fails because `translate` only returns `Vec::new()` — assertions like `events.len() == 1` fail.

- [ ] **Step 3: Implement the three variants.**

Replace the `match` in `translate` with:

```rust
pub fn translate(state: &mut TranslationState, kind: &EventKind) -> Vec<ProsoponEvent> {
    use prosopon_core::{Intent, Node, NodeId, ProsoponEvent, Scene, SignalValue, Topic};

    match kind {
        EventKind::SessionCreated { name, .. } => {
            let root = Node::new(Intent::Section {
                title: Some(name.clone()),
                collapsible: false,
            });
            vec![ProsoponEvent::SceneReset { scene: Scene::new(root) }]
        }

        EventKind::RunStarted { provider, max_iterations } => {
            vec![
                ProsoponEvent::SignalChanged {
                    topic: Topic::from("run.status"),
                    value: SignalValue::Text("running".into()),
                    ts: chrono::Utc::now(),
                },
                ProsoponEvent::SignalChanged {
                    topic: Topic::from("run.provider"),
                    value: SignalValue::Text(provider.clone()),
                    ts: chrono::Utc::now(),
                },
                ProsoponEvent::SignalChanged {
                    topic: Topic::from("run.max_iterations"),
                    value: SignalValue::Number(*max_iterations as f64),
                    ts: chrono::Utc::now(),
                },
            ]
        }

        EventKind::RunErrored { error } => {
            let mut node = Node::new(Intent::Prose { text: error.clone() });
            node.attrs.insert("semantic_role".into(), serde_json::json!("error"));
            vec![
                ProsoponEvent::NodeAdded { parent: NodeId::root(), node },
                ProsoponEvent::SignalChanged {
                    topic: Topic::from("run.status"),
                    value: SignalValue::Text("errored".into()),
                    ts: chrono::Utc::now(),
                },
            ]
        }

        _ => Vec::new(),
    }
}
```

Note: the exact constructors (`NodeId::root`, `SignalValue::Text`, `Topic::from`, etc.) come from `prosopon-core`; Task 1 Step 1 verified them. If any signature differs, adjust here.

- [ ] **Step 4: Run tests green.**

Run: `cargo test -p arcan-prosopon translator::`
Expected: all four tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/arcan/arcan-prosopon/src/translator.rs
git commit -m "feat(arcan-prosopon): translate Session + Run lifecycle"
```

---

## Task 3: Translator — user + assistant text

**Files:** `translator.rs` (modify).

- [ ] **Step 1: Tests.**

Append to `mod tests`:

```rust
#[test]
fn user_message_adds_section_with_prose() {
    let kind = EventKind::UserMessage { content: "hi".into() };
    let events = translate(&mut st(), &kind);
    assert_eq!(events.len(), 1);
    match &events[0] {
        ProsoponEvent::NodeAdded { node, .. } => {
            assert!(matches!(node.intent, Intent::Section { .. }));
            assert_eq!(node.children.len(), 1);
            assert!(matches!(node.children[0].intent, Intent::Prose { .. }));
        }
        _ => panic!("expected NodeAdded"),
    }
}

#[test]
fn first_text_delta_creates_stream_node_then_chunks() {
    let mut s = st();
    s.current_iteration = Some(3);
    let first = EventKind::TextDelta { delta: "he".into(), index: Some(3) };
    let second = EventKind::TextDelta { delta: "llo".into(), index: Some(3) };

    let a = translate(&mut s, &first);
    let b = translate(&mut s, &second);

    // First delta: NodeAdded (stream) + StreamChunk.
    assert_eq!(a.len(), 2);
    assert!(matches!(a[0], ProsoponEvent::NodeAdded { .. }));
    assert!(matches!(a[1], ProsoponEvent::StreamChunk { .. }));
    // Second delta: StreamChunk only.
    assert_eq!(b.len(), 1);
    assert!(matches!(b[0], ProsoponEvent::StreamChunk { .. }));
}

#[test]
fn assistant_message_committed_adds_assistant_section() {
    let kind = EventKind::AssistantMessageCommitted {
        role: "assistant".into(),
        content: "answer".into(),
        model: None,
        token_usage: None,
    };
    let events = translate(&mut st(), &kind);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ProsoponEvent::NodeAdded { .. }));
}
```

- [ ] **Step 2: Run — fails.**

Run: `cargo test -p arcan-prosopon translator::`
Expected: three new failures.

- [ ] **Step 3: Implement.**

Add these arms inside `translate`'s match (before the `_` wildcard):

```rust
EventKind::UserMessage { content } => {
    let prose = Node::new(Intent::Prose { text: content.clone() });
    let mut section = Node::new(Intent::Section {
        title: Some("User".into()),
        collapsible: false,
    });
    section.children.push(prose);
    vec![ProsoponEvent::NodeAdded { parent: NodeId::root(), node: section }]
}

EventKind::AssistantTextDelta { delta, index, .. }
| EventKind::TextDelta { delta, index, .. } => {
    use prosopon_core::{StreamChunk, StreamId, StreamKind};
    let iteration = index.or(state.current_iteration).unwrap_or(0);
    let mut events = Vec::with_capacity(2);

    let stream_id = state
        .streams_by_iteration
        .entry(iteration)
        .or_insert_with(|| {
            let id = StreamId::from(format!("stream:iter-{iteration}"));
            let stream_node = Node::new(Intent::Stream { id: id.clone(), kind: StreamKind::Text });
            events.push(ProsoponEvent::NodeAdded { parent: NodeId::root(), node: stream_node });
            id
        })
        .clone();

    events.push(ProsoponEvent::StreamChunk {
        id: stream_id,
        chunk: StreamChunk::Text(delta.clone()),
    });
    events
}

EventKind::AssistantMessageCommitted { content, .. } | EventKind::Message { content, .. } => {
    let mut section = Node::new(Intent::Section {
        title: Some("Assistant".into()),
        collapsible: false,
    });
    section.children.push(Node::new(Intent::Prose { text: content.clone() }));
    vec![ProsoponEvent::NodeAdded { parent: NodeId::root(), node: section }]
}
```

If `StreamChunk` / `StreamKind` / `StreamId` constructor shapes differ from this, grep `core/prosopon/crates/prosopon-core/src/intent.rs` and adjust.

- [ ] **Step 4: Run green.**

Run: `cargo test -p arcan-prosopon translator::`
Expected: all tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/arcan/arcan-prosopon/src/translator.rs
git commit -m "feat(arcan-prosopon): translate user + assistant text events"
```

---

## Task 4: Translator — tool calls

**Files:** `translator.rs`.

- [ ] **Step 1: Tests.**

```rust
#[test]
fn tool_call_requested_adds_tool_call_node() {
    let kind = EventKind::ToolCallRequested {
        call_id: "call-1".into(),
        tool_name: "shell".into(),
        arguments: serde_json::json!({"cmd": "ls"}),
        category: None,
    };
    let events = translate(&mut st(), &kind);
    assert_eq!(events.len(), 1);
    match &events[0] {
        ProsoponEvent::NodeAdded { node, .. } => {
            assert!(matches!(node.intent, Intent::ToolCall { .. }));
        }
        _ => panic!(),
    }
}

#[test]
fn tool_call_completed_updates_node() {
    use aios_protocol::SpanStatus;
    let kind = EventKind::ToolCallCompleted {
        tool_run_id: aios_protocol::ToolRunId::default(),
        call_id: Some("call-1".into()),
        tool_name: "shell".into(),
        result: serde_json::json!("ok"),
        duration_ms: 12,
        status: SpanStatus::Ok,
    };
    let events = translate(&mut st(), &kind);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ProsoponEvent::NodeUpdated { .. }));
}

#[test]
fn tool_call_failed_updates_with_error_result() {
    let kind = EventKind::ToolCallFailed {
        call_id: "call-2".into(),
        tool_name: "shell".into(),
        error: "denied".into(),
    };
    let events = translate(&mut st(), &kind);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ProsoponEvent::NodeUpdated { .. }));
}
```

- [ ] **Step 2: Run — fails.**
- [ ] **Step 3: Implement.**

Add a helper at the top of `translator.rs`:

```rust
fn tool_node_id(call_id: &str) -> prosopon_core::NodeId {
    prosopon_core::NodeId::from(format!("tool:{call_id}"))
}
```

Add arms:

```rust
EventKind::ToolCallRequested { call_id, tool_name, arguments, .. } => {
    let mut node = Node::new(Intent::ToolCall {
        name: tool_name.clone(),
        args: arguments.clone(),
        stream: None,
    });
    node.id = tool_node_id(call_id);
    vec![ProsoponEvent::NodeAdded { parent: NodeId::root(), node }]
}

EventKind::ToolCallCompleted { call_id, result, status, .. } => {
    use aios_protocol::SpanStatus;
    use prosopon_core::NodePatch;
    let id = call_id.as_ref().map(|c| tool_node_id(c)).unwrap_or_else(NodeId::root);
    let success = matches!(status, SpanStatus::Ok);
    let result_node = Node::new(Intent::ToolResult { success, payload: result.clone() });
    vec![ProsoponEvent::NodeUpdated {
        id,
        patch: NodePatch {
            append_children: vec![result_node],
            ..NodePatch::default()
        },
    }]
}

EventKind::ToolCallFailed { call_id, error, .. } => {
    use prosopon_core::NodePatch;
    let id = tool_node_id(call_id);
    let result_node = Node::new(Intent::ToolResult {
        success: false,
        payload: serde_json::json!({ "error": error }),
    });
    vec![ProsoponEvent::NodeUpdated {
        id,
        patch: NodePatch {
            append_children: vec![result_node],
            ..NodePatch::default()
        },
    }]
}
```

If `NodePatch`'s field for child appends is named differently (e.g. `children_append` or `add_children`), inspect `core/prosopon/crates/prosopon-core/src/event.rs` and use the real name.

- [ ] **Step 4: Run green.**
- [ ] **Step 5: Commit.**

```bash
git commit -am "feat(arcan-prosopon): translate tool call lifecycle"
```

---

## Task 5: Translator — approvals, state, compaction, wildcard

**Files:** `translator.rs`.

- [ ] **Step 1: Tests.**

```rust
#[test]
fn approval_requested_adds_confirm_node() {
    use aios_protocol::{ApprovalId, RiskLevel};
    let kind = EventKind::ApprovalRequested {
        approval_id: ApprovalId::default(),
        call_id: "c".into(),
        tool_name: "shell".into(),
        arguments: serde_json::json!({}),
        risk: RiskLevel::High,
    };
    let events = translate(&mut st(), &kind);
    assert_eq!(events.len(), 1);
    match &events[0] {
        ProsoponEvent::NodeAdded { node, .. } => {
            assert!(matches!(node.intent, Intent::Confirm { .. }));
        }
        _ => panic!(),
    }
}

#[test]
fn state_patched_emits_revision_signal() {
    let kind = EventKind::StatePatched {
        index: None,
        patch: serde_json::json!([]),
        revision: 42,
    };
    let events = translate(&mut st(), &kind);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ProsoponEvent::SignalChanged { .. }));
}

#[test]
fn context_compacted_emits_signal_and_prose() {
    let kind = EventKind::ContextCompacted {
        dropped_count: 3,
        tokens_before: 1000,
        tokens_after: 500,
    };
    let events = translate(&mut st(), &kind);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], ProsoponEvent::SignalChanged { .. }));
    assert!(matches!(events[1], ProsoponEvent::NodeAdded { .. }));
}
```

- [ ] **Step 2: Run — fails.**
- [ ] **Step 3: Implement.**

Helpers:

```rust
fn severity_for(risk: &aios_protocol::RiskLevel) -> prosopon_core::Severity {
    use aios_protocol::RiskLevel;
    use prosopon_core::Severity;
    match risk {
        RiskLevel::Low => Severity::Normal,
        RiskLevel::Medium => Severity::High,
        RiskLevel::High | RiskLevel::Critical => Severity::Urgent,
    }
}

fn approval_node_id(approval_id: &str) -> prosopon_core::NodeId {
    prosopon_core::NodeId::from(format!("approval:{approval_id}"))
}
```

Match arms:

```rust
EventKind::ApprovalRequested { approval_id, tool_name, risk, .. } => {
    let message = format!("Approve {tool_name}?");
    let mut node = Node::new(Intent::Confirm { message, severity: severity_for(risk) });
    node.id = approval_node_id(&approval_id.to_string());
    node.attrs.insert("approval_id".into(), serde_json::json!(approval_id.to_string()));
    vec![ProsoponEvent::NodeAdded { parent: NodeId::root(), node }]
}

EventKind::ApprovalResolved { approval_id, decision, .. } => {
    use prosopon_core::{NodePatch, Lifecycle, NodeStatus};
    let id = approval_node_id(&approval_id.to_string());
    vec![
        ProsoponEvent::NodeUpdated {
            id,
            patch: NodePatch {
                lifecycle: Some(Lifecycle { status: NodeStatus::Resolved, ..Default::default() }),
                ..NodePatch::default()
            },
        },
        ProsoponEvent::SignalChanged {
            topic: Topic::from(format!("approval.{approval_id}")),
            value: SignalValue::Text(decision.clone()),
            ts: chrono::Utc::now(),
        },
    ]
}

EventKind::StatePatched { revision, .. } => vec![
    ProsoponEvent::SignalChanged {
        topic: Topic::from("state.revision"),
        value: SignalValue::Number(*revision as f64),
        ts: chrono::Utc::now(),
    },
],

EventKind::ContextCompacted { tokens_before, tokens_after, .. } => {
    let mut node = Node::new(Intent::Prose {
        text: format!("Compacted {tokens_before}→{tokens_after} tokens"),
    });
    node.attrs.insert("emphasis".into(), serde_json::json!("low"));
    vec![
        ProsoponEvent::SignalChanged {
            topic: Topic::from("context.tokens"),
            value: SignalValue::Number(*tokens_after as f64),
            ts: chrono::Utc::now(),
        },
        ProsoponEvent::NodeAdded { parent: NodeId::root(), node },
    ]
}

EventKind::StepStarted { index } => vec![
    ProsoponEvent::SignalChanged {
        topic: Topic::from("iteration"),
        value: SignalValue::Number(*index as f64),
        ts: chrono::Utc::now(),
    },
],

EventKind::PolicyEvaluated { tool_name, decision, .. } => vec![
    ProsoponEvent::SignalChanged {
        topic: Topic::from(format!("policy.{tool_name}")),
        value: SignalValue::Text(format!("{decision:?}")),
        ts: chrono::Utc::now(),
    },
],

EventKind::KnowledgeSearched { query, result_count, .. } => {
    let mut node = Node::new(Intent::Prose {
        text: format!("Searched: {query} ({result_count})"),
    });
    node.attrs.insert("emphasis".into(), serde_json::json!("low"));
    vec![ProsoponEvent::NodeAdded { parent: NodeId::root(), node }]
}
```

Leave the `_ => Vec::new()` wildcard for all other variants.

- [ ] **Step 4: Run green.**
- [ ] **Step 5: Commit.**

```bash
git commit -am "feat(arcan-prosopon): translate approvals, state, compaction, policy"
```

---

## Task 6: Bridge — subscribe, translate, publish

**Files:**
- Modify: `core/life/crates/arcan/arcan-prosopon/src/bridge.rs`
- Test: `core/life/crates/arcan/arcan-prosopon/tests/bridge_integration.rs`

- [ ] **Step 1: Integration test first.**

Create `tests/bridge_integration.rs`:

```rust
//! End-to-end: feed a canned EventRecord stream, assert envelopes appear on the fanout.

use aios_protocol::{BranchId, EventKind, EventRecord, SeqNo, SessionId};
use arcan_prosopon::ArcanProsoponBridge;
use prosopon_daemon::EnvelopeFanout;
use tokio::sync::broadcast;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_forwards_run_started_as_three_signal_envelopes() {
    // Upstream = arcan's event broadcast.
    let (event_tx, event_rx) = broadcast::channel::<EventRecord>(16);
    // Downstream = prosopon fanout.
    let fanout = EnvelopeFanout::new();
    let mut subscriber = fanout.subscribe();

    // Start the bridge.
    let bridge = ArcanProsoponBridge::new(fanout);
    let handle = bridge.spawn(event_rx);

    // Emit one RunStarted event.
    let record = EventRecord::new(
        SessionId::default(),
        BranchId::main(),
        SeqNo::from(1u64),
        EventKind::RunStarted { provider: "anthropic".into(), max_iterations: 5 },
    );
    event_tx.send(record).unwrap();

    // Expect three SignalChanged envelopes.
    for _ in 0..3 {
        let env = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            subscriber.recv(),
        )
        .await
        .expect("timeout waiting for envelope")
        .expect("recv ok");
        assert!(matches!(
            env.event,
            prosopon_core::ProsoponEvent::SignalChanged { .. }
        ));
    }

    // Shut down.
    drop(event_tx);
    let _ = handle.await;
}
```

Run: `cargo test -p arcan-prosopon --test bridge_integration`
Expected: compilation fails because `ArcanProsoponBridge::spawn` doesn't exist.

- [ ] **Step 2: Implement `spawn`.**

Replace `bridge.rs` with:

```rust
//! Bridge — spawns the subscriber task that drains arcan events into the
//! prosopon fanout.

use crate::{translator::translate, BridgeError, TranslationState};
use aios_protocol::EventRecord;
use prosopon_daemon::EnvelopeFanout;
use prosopon_sdk::Session;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// Drains an `aios_protocol::EventRecord` broadcast into a Prosopon fanout.
pub struct ArcanProsoponBridge {
    fanout: EnvelopeFanout,
    session: Session,
    state: TranslationState,
}

impl ArcanProsoponBridge {
    pub fn new(fanout: EnvelopeFanout) -> Self {
        Self {
            fanout,
            session: Session::new(),
            state: TranslationState::new(),
        }
    }

    /// Spawn the drain loop on the current tokio runtime. The loop exits when
    /// the upstream broadcast closes.
    pub fn spawn(mut self, mut events: broadcast::Receiver<EventRecord>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(record) => {
                        if let Err(err) = self.drain_one(&record).await {
                            warn!(error = %err, "arcan-prosopon: translation/publish failed");
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(lagged = n, "arcan-prosopon: dropped events due to lag");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("arcan-prosopon: upstream closed, exiting bridge");
                        return;
                    }
                }
            }
        })
    }

    async fn drain_one(&mut self, record: &EventRecord) -> Result<(), BridgeError> {
        for event in translate(&mut self.state, &record.kind) {
            let envelope = self.session.envelope(event);
            self.fanout.send(envelope)?;
        }
        Ok(())
    }
}
```

- [ ] **Step 3: Run test.**

Run: `cargo test -p arcan-prosopon --test bridge_integration`
Expected: PASS.

- [ ] **Step 4: Commit.**

```bash
git add crates/arcan/arcan-prosopon/src/bridge.rs crates/arcan/arcan-prosopon/tests/bridge_integration.rs
git commit -m "feat(arcan-prosopon): spawn bridge loop + integration test"
```

---

## Task 7: Wire into arcand (optional daemon mode)

**Files:**
- Modify: `core/life/crates/arcan/arcand/Cargo.toml` — add optional `arcan-prosopon`, `prosopon-daemon`, `prosopon-compositor-glass` deps behind a `prosopon` feature.
- Modify: `core/life/crates/arcan/arcand/src/main.rs` — add `--prosopon-port` CLI flag and conditional boot code.
- Modify: `core/life/crates/arcan/arcand/src/canonical.rs` — no change if `subscribe_events()` is already public.

- [ ] **Step 1: Add optional dependency block.**

Edit `core/life/crates/arcan/arcand/Cargo.toml`:

```toml
[dependencies]
# ... existing deps ...
arcan-prosopon                 = { path = "../arcan-prosopon", optional = true }
prosopon-daemon                = { path = "../../../../../prosopon/crates/prosopon-daemon", optional = true }
prosopon-compositor-glass      = { path = "../../../../../prosopon/crates/prosopon-compositor-glass", optional = true }

[features]
default = []
prosopon = ["dep:arcan-prosopon", "dep:prosopon-daemon", "dep:prosopon-compositor-glass"]
```

- [ ] **Step 2: Add CLI flag.**

In the Clap struct inside `arcand/src/main.rs` (locate the `struct Cli` or `struct ServeArgs`; ~line 1283 for `fn main`), add:

```rust
/// Enable the Prosopon display-server sidecar on this address.
/// Requires `--features prosopon` at build time.
#[arg(long, value_name = "ADDR")]
prosopon_port: Option<std::net::SocketAddr>,
```

- [ ] **Step 3: Conditional boot.**

In the daemon bootstrap (after `CanonicalState` is constructed), add:

```rust
#[cfg(feature = "prosopon")]
if let Some(addr) = args.prosopon_port {
    use arcan_prosopon::ArcanProsoponBridge;
    use prosopon_compositor_glass::glass_surface;
    use prosopon_daemon::{DaemonConfig, DaemonServer};

    match DaemonServer::bind(DaemonConfig { addr, surface: Some(glass_surface()) }).await {
        Ok(server) => {
            let fanout = server.fanout();
            let event_rx = state.runtime.subscribe_events();
            let bridge = ArcanProsoponBridge::new(fanout);
            let _bridge_handle = bridge.spawn(event_rx);
            let _daemon_handle = tokio::spawn(async move {
                if let Err(err) = server.serve().await {
                    tracing::error!(error = %err, "prosopon-daemon serve failed");
                }
            });
            tracing::info!(addr = %addr, "arcan-prosopon: bridge online");
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                addr = %addr,
                "arcan-prosopon: failed to bind, arcan will continue without Prosopon"
            );
        }
    }
}
```

- [ ] **Step 4: Build with feature.**

Run: `cargo build -p arcand --features prosopon`
Expected: success.

Run (no feature): `cargo build -p arcand`
Expected: success (arcand compiles without any Prosopon code).

- [ ] **Step 5: Smoke-run.**

Run in one terminal: `cargo run -p arcand --features prosopon -- serve --prosopon-port 127.0.0.1:4321` (or whatever the serve subcommand is — verify from `arcand --help`).
In another: `curl http://127.0.0.1:4321/healthz`
Expected: `{"status":"ok"}` or similar from `prosopon-daemon`'s health endpoint. If a different health path, inspect `prosopon-daemon/src/server.rs`.

Stop both.

- [ ] **Step 6: Commit.**

```bash
git commit -am "feat(arcand): optional arcan-prosopon sidecar behind 'prosopon' feature"
```

---

## Task 8: Docs + CHANGELOG + Linear + PR

**Files:**
- Create: `core/life/crates/arcan/arcan-prosopon/README.md`
- Modify: `core/life/CHANGELOG.md` (if present) or crate-level CHANGELOG
- Create: `/Users/broomva/broomva/research/entities/project/arcan-prosopon.md` (entity page for knowledge graph)
- Linear: BRO-773 status update + PR link

- [ ] **Step 1: Expand the README.**

Write a concise README covering: what the crate is, the translation table (copy from this plan's Translation table), `Bridge::new` + `Bridge::spawn` API, feature-flag usage in `arcand`, the graceful-fallback contract, and links to prosopon-core + the plan.

- [ ] **Step 2: Add entity page.**

`/Users/broomva/broomva/research/entities/project/arcan-prosopon.md`:

```markdown
---
name: arcan-prosopon
description: Pneuma<L0ToExternal> for Arcan — translates aios EventKind to Prosopon envelopes and publishes to the prosopon-daemon fanout.
type: project
status: shipped
layer: 3
score: 8
related:
  - research/entities/project/prosopon.md
  - research/entities/project/arcan.md
  - research/entities/concept/pneuma.md
  - research/entities/project/sensorium.md
---

# arcan-prosopon

(summary + link to `core/life/docs/superpowers/plans/2026-04-23-bro-773-arcan-prosopon.md`)
```

- [ ] **Step 3: Run full life smoke.**

Run: `make smoke -C /Users/broomva/broomva/core/life` (or the workspace's equivalent).
Expected: all green.

Run: `cargo test -p arcan-prosopon`
Expected: all green.

- [ ] **Step 4: Commit + push + PR.**

```bash
git add -A
git commit -m "docs(arcan-prosopon): README + entity page + CHANGELOG"
git push origin bro-773-arcan-prosopon
gh pr create --title "BRO-773: arcan-prosopon — emit Arcan session through Prosopon" \
  --body "$(cat <<'EOF'
## Summary
- New crate \`arcan-prosopon\` in Life — \`Pneuma<L0ToExternal>\` for Arcan.
- Subscribes to \`KernelRuntime\` events, translates \`EventKind → ProsoponEvent\`, publishes to \`EnvelopeFanout\`.
- Opt-in via \`arcand --features prosopon --prosopon-port <addr>\`. Graceful fallback if daemon can't bind.

## Test Plan
- [x] unit tests per EventKind family pass
- [x] integration test: mock kernel → fanout subscriber observes expected envelopes
- [x] \`arcand\` builds with and without the \`prosopon\` feature
- [ ] Manual: glass compositor renders a live arcan session at http://127.0.0.1:4321/

Linear: https://linear.app/broomva/issue/BRO-773

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 5: Flip Linear BRO-773 to In Progress now, Done on merge.**

Use MCP linear-server to set state = "In Progress" when the PR opens, then "Done" once merged.

- [ ] **Step 6: Announce in docs.**

Add one line to the workspace-level index at `/Users/broomva/broomva/docs/knowledge-index.md` if the index is updated on ship. Update `core/prosopon/PLANS.md` to mark BRO-773 `[x]`.

---

## Self-review checklist (executor: run this before opening the PR)

- All `EventKind` variants present today are handled: covered in Tasks 2-5 + wildcard.
- All tasks have TDD skeleton (failing test → implementation → passing test → commit).
- Type / method / field names used in Task N referenced in Task M match. In particular: `NodePatch`'s field for child appends is called `append_children` in this plan. Verify during Task 4 Step 3 and correct in Task 5 if different.
- Placeholder scan: plan contains no "TBD" / "fill in" / "similar to above" markers in implementation steps.
- Graceful degradation proven: Task 7 Step 3's `warn!` path keeps arcand alive when the daemon can't bind.
- Feature flag prevents arcand's hard dependency on Prosopon (verified in Task 7 Step 4).
