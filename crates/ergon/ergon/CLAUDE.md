# CLAUDE.md — `ergon` crate

> Instructions for AI agents working in this crate.
> Last updated: 2026-05-08.

## What this crate is

**ergon** is the workflow primitive for Life's agent harness. It's a small
trait crate (`Workflow`, `Step`, `StepCtx`, `Hook`, `StreamSink`,
`InferenceRequest`) that a Broomva developer implements to express a
**bounded multi-turn agent operation** as plain async Rust whose body
delegates to autonomous model + tool calls.

A workflow is **not a long-horizon agent**. It is **one shape of tick
body** (see `docs/architecture/agent-harness.md`): the kernel runs it as
the contents of a single tick. The kernel still owns the agent loop;
ergon just supplies a richer alternative to the existing single-call
tick body.

**Position in the harness stack** (canonical):

```
L5 — Session orchestration (arcand::ConsciousnessActor)
L4 — Tick engine (aios_runtime::KernelRuntime)
L3.5 — Tick body — direct OR ergon::Workflow      ← THIS CRATE supplies the workflow shape
L3 — Port traits (aios-runtime)
L2 — Substrate adapters (incl. arcan-ergon)
L1 — Substrate primitives (lago, praxis, anima, ...)
L0 — Kernel contract (aios-protocol)
```

See `docs/architecture/agent-harness.md` for the full 7-layer stack and
the two scopes of the agent loop (outer / session vs inner / tick).

**Critical invariant**: ergon does NOT replace `KernelRuntime`. It does
NOT replace `arcan-harness` (which isn't the harness anyway — it's a
~300 LOC utility crate). It does NOT compete with the tick engine.
Workflows are **bounded operations that run inside one kernel tick**.

## Spec & tracker

- Spec: `core/life/docs/superpowers/specs/2026-05-05-ergon-v0.1.md`
  (§§0-5, 7-9, 11-13 valid; §6 + §10 superseded)
- Adapter spec: `core/life/docs/superpowers/specs/2026-05-08-bro-1001-ergon-tick-body.md`
  (the corrected §6 + §10 framing)
- Architecture: `core/life/docs/architecture/agent-harness.md`
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
| `workflow.rs` (Workflow trait, WorkflowExecutor, SkillSet/EmptySkillSet stub) | BRO-999 | Done |

**Sibling crates landed:**

| Crate | BRO ticket | Notes |
|---|---|---|
| `ergon-life-hooks` | BRO-1000 | 4 auto-hooks (PraxisCapabilityHook / AutonomicBudgetHook / NousScoreHook / AnimaAttestHook) + 4 adapter traits. Substrate-free; arcan adapter (BRO-1001) implements adapter traits against actual substrate. |

**Not yet landed** (follow-up PRs):

| File / Crate | BRO ticket | Notes |
|---|---|---|
| `ergon-life-sinks` (LagoSink, VigilSink, LifegwSink) | BRO-999b (follow-up) | Substrate-coupled stream sinks. Pull in `lago-journal`, `life-vigil`, `tokio::sync::mpsc`. Sibling crate, mirrors the BRO-1000 pattern. |
| arcan adapter | BRO-1001 | `crates/arcan/arcan/src/agent_kind/ergon.rs`: implements `Provider` against `arcan_provider`, `ToolRegistry` against `praxis_core`, all four auto-hook adapter traits against actual substrate, builds `StepCtx` + `HookRegistry` from `TickCtx`, calls `WorkflowExecutor::run`. |
| lifed route | BRO-1002 | `Agent.StreamSession` route in `crates/life-runtime/lifed`. |
| bookkeeping-judge port | BRO-1003 | First production workflow on ergon. Parity test against the Bellows-shipped version. |
| docs/architecture/ergon.md | BRO-1004 | Final architecture doc. |

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

6. **`WorkflowExecutor::without_default_hooks` flag dropped**: spec §3.8 /
   §8 Q3 proposed an executor flag to opt out of auto-hooks. We dropped
   it. Rationale: the executor doesn't construct any hooks (auto or
   otherwise), so there's nothing to opt out of. Auto-hook registration
   lives at the *adapter* level (BRO-1001 — the arcan adapter is the
   thing that decides which hooks to add to the registry it passes into
   the executor). Opt-out is therefore a deployment-time decision, not
   a per-workflow flag.

7. **Caller builds StepCtx, not executor**: spec §3.8's pseudocode
   showed `WorkflowExecutor::run` building the StepCtx + auto-hook
   registry inside the executor. We invert that: the caller passes a
   fully-built `StepCtx` (with hooks already in `ctx.hooks`); executor
   only fires the workflow boundary events. This separation lets
   workflows be tested with a hand-built StepCtx and lets the arcan
   adapter own substrate-handle assembly without smuggling it through
   the executor.

8. **`praxis_skills::SkillSet` placeholder**: workflow.rs ships an
   internal `SkillSet` trait + `EmptySkillSet` stub. This preserves
   ergon's "zero substrate deps" property until BRO-1001 wires real
   praxis skill sets via the adapter. Trait shape matches what
   `praxis_skills::SkillSet` exposes (read-only iteration), so the
   migration is mechanical when it lands.

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
