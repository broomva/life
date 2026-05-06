# CLAUDE.md — `ergon` crate

> Instructions for AI agents working in this crate.
> Last updated: 2026-05-06.

## What this crate is

**ergon** is Life's Layer-2 agent-harness primitive. The trait set that lets
a Broomva developer write a `Workflow` in Rust whose deterministic outer body
orchestrates autonomous inner LLM steps, integrated end-to-end with the Life
substrate.

**Layered position** (locked):

```
Layer 4 — Life (Agent OS substrate)
Layer 3 — arcan (runtime daemon)
Layer 2 — ergon (HARNESS — this crate)
Layer 1 — arcan-provider (model wire connectors)
Layer 0 — model
```

## Spec & tracker

- Spec: `core/life/docs/superpowers/specs/2026-05-05-ergon-v0.1.md`
- Linear umbrella: BRO-994 — https://linear.app/broomva/issue/BRO-994

## Status (2026-05-06)

Shipping incrementally per spec §12 work order. **Currently landed** in this
crate:

| File | BRO ticket | State |
|---|---|---|
| `error.rs` | BRO-996 | Done |
| `role.rs` | BRO-996 | Done |
| `stream.rs` (StreamEvent, StreamSink, BufferSink, FanoutSink) | BRO-996 | Done |
| `model.rs` (Message, ContentBlock, ToolCall, ToolResult, ToolDefinition, ModelRequest, ModelResponse, Usage) | BRO-997 | Done |
| `hook.rs` (Hook trait, HookCtx, HookRegistry, outcome types) | BRO-997 | Done |
| `runtime.rs` (Provider, ToolRegistry, RuntimeHandle traits — runtime extension points owned by ergon, translated by the arcan adapter) | BRO-998 | Done |
| `step.rs` (Step, StepCtx, InferenceRequest, run_inference_streaming + autonomous loop body, dispatch_tool) | BRO-998 | Done |

**Not yet landed** (follow-up PRs):

| File | BRO ticket | Notes |
|---|---|---|
| `workflow.rs` | BRO-999 | Workflow + WorkflowExecutor (the outer driver that wires the auto-hook registry and calls `Workflow::execute`) |
| `LagoSink`, `VigilSink`, `LifegwSink` | BRO-999 (or its own PR) | Substrate-coupled default sinks. Pull in `lago-journal`, `life-vigil`, `tokio::sync::mpsc`. |
| `attestation.rs`, `budget.rs`, `score.rs`, `capability.rs` | BRO-1000 | Auto-registered hooks (anima / autonomic / nous / praxis) |
| `LagoSink`, `VigilSink`, `LifegwSink` | BRO-998 | Default substrate sinks (deferred — pull in lago-journal / life-vigil / mpsc deps) |

## Invariants (DO NOT VIOLATE)

1. **Roles are NEVER persisted in session history.** `Role::merge` produces a
   transient overlay applied only at `ModelRequest` build time. Inserting a
   role into history breaks the call > session > agent precedence rule.

2. **`StreamEvent` variants are append-only after v1.0.** Never remove,
   never reorder semantically. New variants land in any minor version;
   consumers MUST handle `StreamEvent::VendorEvent` for forward compat.

3. **No `unwrap()` / `expect()` / `panic!()` in non-test code.** Workspace
   clippy lints catch these. Use `ErgonError::*` variants.

4. **No emojis in source files.** LLM-friendly diffs.

5. **`async_trait` for now.** When Rust's native `async fn in trait` is
   stable enough for our MSRV (currently 1.93), we'll migrate workspace-wide.

6. **No Life-runtime deps in this crate's foundational layer.** This crate
   currently has zero dependencies on `lago-journal`, `life-vigil`,
   `praxis-*`, `arcan-*`, `anima-*`, `autonomic-*`, or `nous-*`. Those land
   incrementally as their consuming modules ship (per the work-order in
   spec §12).

## Spec deviations (documented)

1. **License**: spec §3.1 says `license = "Apache-2.0"`. This crate uses
   `license.workspace = true` (= MIT) for monorepo coherence — life is
   MIT throughout.

2. **Hook event defaults**: spec §3.7 only defaults `on_workflow_start` to
   `Continue`; the other 7 events are abstract. This crate defaults **all
   8 events** to `Ok(_::Continue)`. Rationale: a real-world hook (e.g.
   `NousScoreHook`) only cares about one event; forcing eight no-op
   implementations on every hook is boilerplate without safety, since
   the same `Continue` ships either way. The original spec choice was a
   compile-time push to "force the implementer to think about each
   event" — ergonomically counterproductive in practice.

3. **Provider / ToolRegistry traits owned by ergon**: spec §3.4 declared
   `StepCtx::provider: Arc<dyn arcan_provider::Provider>` and
   `StepCtx::tools: Arc<dyn praxis_core::ToolRegistry>`. We deliberately
   redirect both to ergon-owned traits in `runtime.rs`. Same logic that
   drove BRO-997's wire-types decision: hook signatures depend on these
   types, so coupling them to substrate crates ripples every substrate
   change through every hook. The arcan adapter (BRO-1001) implements
   `ergon::Provider` over `arcan_provider::Provider` and
   `ergon::ToolRegistry` over `praxis_core::ToolRegistry` at the
   boundary.

4. **StepCtx fields cut**: spec §3.4 included `journal`, `homeostasis`,
   `soul`, `skills`, `sandbox` as direct fields on `StepCtx`. We dropped
   all five. Rationale: `journal`/`homeostasis`/`soul` are auto-hook
   concerns (each hook holds its own substrate handle from construction
   time); `skills` is reachable via `Workflow::skills()`; `sandbox` is
   internal to each `ToolRegistry` impl. Cutting these makes `step.rs`
   substrate-free — ergon compiles and tests with zero dependencies on
   `lago-journal`, `autonomic`, `anima`, `praxis-skills`, or
   `praxis-core`.

5. **RuntimeHandle narrowed**: spec §3.12 listed `aios_caps()`, `span()`,
   `edit_hashline()`, `operating_mode()`. v0.1 ships only
   `operating_mode()`. The other three are exclusively used by
   substrate-aware code (auto-hooks for `aios_caps`, the arcan adapter
   for `edit_hashline`); they don't belong in the workflow-author-facing
   surface. The trait can grow each method as a deliberate boundary
   expansion when a workflow demonstrably needs it.

All deviations are recorded in `core/life/CHANGELOG.md`.

## Useful commands

```bash
cargo check -p ergon
cargo test  -p ergon --all-targets
cargo clippy -p ergon --all-targets -- -D warnings
cargo fmt -p ergon
```

## Don't

- Do not pull in Life-substrate crate deps speculatively — wire them only
  when the consuming module needs them.
- Do not bypass the role-merge precedence rule.
- Do not introduce `unwrap()` / `expect()` to "fix" a clippy warning.
- Do not rename `StreamEvent` variants after v1.0.
