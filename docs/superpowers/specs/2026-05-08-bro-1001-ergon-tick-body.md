# Spec — BRO-1001 — Ergon Tick-Body Adapter

**Date**: 2026-05-08
**Status**: Active — supersedes §6 and §10 of `2026-05-05-ergon-v0.1.md`
**Owner**: arcan-ergon (`crates/arcan/arcan-ergon/`) — new crate
**Type**: design-spec
**Linear**: [BRO-1001](https://linear.app/broomva/issue/BRO-1001)
**Related**:
- `core/life/docs/architecture/agent-harness.md` (canonical layer cake)
- `core/life/docs/superpowers/specs/2026-05-05-ergon-v0.1.md` (workflow trait crate; §6 + §10 superseded by this doc)
- `core/life/docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md` (silicon contract — composes with this)

## 0. Purpose (one sentence — the whole spec is governed by this)

**The arcan-ergon adapter (`crates/arcan/arcan-ergon/`) lets a kernel
tick run an `ergon::Workflow` as its body — preserving per-tick
journal traceability, autonomic gating, branch state, and the
OperatingMode FSM exactly as the existing direct-call tick body does.**

If a feature does not serve that sentence, it does not go in this spec.

## 1. Why this spec replaces the §6 / §10 framing of the original ergon spec

`2026-05-05-ergon-v0.1.md` proposed an `AgentKind` trait abstraction in
which ergon would be **one of N runtimes** dispatched at the *session*
level by arcand. The corrected understanding (see
`docs/architecture/agent-harness.md`):

1. The actual production agent loop is `aios_runtime::KernelRuntime`,
   driven tick-by-tick by `arcand::ConsciousnessActor`. There is no
   `AgentKind` trait in arcan.
2. The agent harness is a **7-layer stack** (L0 kernel contract → L6
   process daemons), not a single runtime crate.
3. Ergon's `Workflow::execute()` is one async fn — it cannot suspend
   mid-flight, cannot replay from event N>0, cannot survive a daemon
   restart. It is structurally **not a long-horizon runtime**.
4. Long-horizon agents need the tick engine: each tick is a checkpoint;
   sessions live across days; replay reconstructs from kernel-typed
   `EventKind` events.

What ergon legitimately adds is a richer *tick body shape*: one tick
runs an entire bounded workflow (multi-turn model+tool execution) and
returns a typed Output. The kernel still owns the agent loop. Workflow
events nest under the parent tick's `run_id` for sub-event
traceability.

This spec describes that integration.

## 2. Scope

### 2.1 In scope (v0.1)

- New crate `crates/arcan/arcan-ergon/` (sibling of `arcan-praxis`,
  `arcan-lago`, `arcan-anima`) with adapter impls of:
  - `ergon::Provider` over `aios_runtime::ModelProviderPort`
  - `ergon::ToolRegistry` over `aios_runtime::ToolHarnessPort` (with
    `PolicyGatePort` consulted for capability enforcement)
  - `ergon::RuntimeHandle` over a kernel `TickHandle`
  - The four `ergon-life-hooks` adapter traits (`CapabilityResolver`,
    `BudgetGate`, `ResponseScorer`, `SoulAttester`) over substrate
    types (`PolicySet`, `AutonomicGatingProfile`, `NousEvaluator`,
    `AgentSoul`)
- `WorkflowRegistry` — name → workflow lookup, populated at daemon
  startup
- `run_workflow_as_tick(name, input, tick_ctx)` — the entry point the
  kernel tick handler calls when a tick is workflow-bodied
- Integration smoke test: register a workflow, run it as a rich tick
  via a mock kernel, verify journal events are emitted at both tick
  granularity (kernel events) and workflow granularity (Custom
  ergon.stream events nested under run_id)

### 2.2 Out of scope (deferred)

- **arcand wiring** (BRO-1001b): the small change to `TickInput` and
  `run_agent_cycle_inner` that lets a session *select* a workflow tick
  body. This is its own ticket because it touches the daemon's hot
  path; it should land after BRO-1001 with a feature flag.
- **Workflow registry persistence**: v0.1 registers workflows in
  process memory at daemon startup. Lago-backed workflow definitions
  (so workflows can be deployed without redeploying arcand) are a v0.2
  concern.
- **Speculative execution / KV reuse across workflows**: that's Spec E
  / BRO-1019's job. After Spec E ships, `arcan-ergon::ergon::Provider`
  retargets through `InferenceRouter` and gets it for free.
- **Workflow-level branching**: a workflow runs in one tick. Tick
  branching (kernel-level) is unchanged. Workflows don't branch
  internally in v0.1.
- **bookkeeping-judge port** (BRO-1003): the first real workflow
  exercising this adapter. Its own PR.

## 3. Layered position

LOCKED:

```text
L4 — KernelRuntime (tick engine, aios-runtime)
        │
        │ tick_on_branch returns mode; outer loop continues until Sleep
        │
        ↓
L3.5 — Tick body
        │
        │ Two shapes:
        │   - Direct (existing): ModelProviderPort.complete + ToolHarnessPort.execute
        │   - Workflow (this spec): arcan_ergon::run_workflow_as_tick
        │
        ↓
L3   — Port traits (aios-runtime)
        │
        │ ModelProviderPort, ToolHarnessPort, ApprovalPort,
        │ PolicyGatePort, EventStorePort
        │
        ↓
L2   — Substrate adapters
        │
        │ arcan-aios-adapters, arcan-praxis, arcan-lago, arcan-anima,
        │ ergon-life-hooks, ergon-life-sinks, arcan-ergon (this spec)
        │
        ↓
L1   — Substrate primitives
```

`arcan-ergon` is **at L3.5 but composed with L2 adapters**. It's the
"bridge" between ergon (a vendor-neutral workflow primitive at the
non-substrate layer) and arcan's port-trait stack.

## 4. Public API — locked

### 4.1 Crate manifest

```toml
[package]
name        = "arcan-ergon"
description = "Tick-body adapter — runs ergon Workflows as the body of a kernel tick."
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true

[dependencies]
ergon              = { workspace = true }
ergon-life-hooks   = { workspace = true }
ergon-life-sinks   = { workspace = true }
aios-protocol      = { workspace = true }
aios-runtime       = { workspace = true }
arcan-core         = { workspace = true }
arcan-aios-adapters = { workspace = true }
lago-core          = { workspace = true }
praxis-core        = { workspace = true }
anima-core         = { workspace = true }
autonomic-core     = { workspace = true }
nous-core          = { workspace = true }
async-trait        = { workspace = true }
serde              = { workspace = true, features = ["derive"] }
serde_json         = { workspace = true }
tokio              = { workspace = true, features = ["sync", "macros"] }
tracing            = { workspace = true }
```

### 4.2 Module layout

```text
crates/arcan/arcan-ergon/src/
├── lib.rs              # public API
├── error.rs            # ArcanErgonError
├── registry.rs         # WorkflowRegistry: name → DynWorkflowExecutor
├── runner.rs           # run_workflow_as_tick(...) — the kernel's entry point
├── provider.rs         # impl ergon::Provider over ModelProviderPort
├── tools.rs            # impl ergon::ToolRegistry over ToolHarnessPort + PolicyGatePort
├── runtime_handle.rs   # impl ergon::RuntimeHandle over kernel TickHandle
└── auto_hooks/
    ├── capability.rs   # impl CapabilityResolver over PolicySet
    ├── budget.rs       # impl BudgetGate over AutonomicGatingProfile
    ├── score.rs        # impl ResponseScorer over NousEvaluator
    └── attestation.rs  # impl SoulAttester over AgentSoul
```

Eight active modules. ~700-900 LOC total.

### 4.3 Entry point: `run_workflow_as_tick`

```rust
/// Run an ergon workflow as the body of one kernel tick.
///
/// Called by the kernel tick handler when `tick_input.kind ==
/// TickKind::Workflow{name, input}`. Builds a `StepCtx` from the
/// kernel's `TickCtx` (provider, tools, hooks, sinks all configured
/// against substrate adapters), then calls
/// `WorkflowExecutor::run(ctx, input)`. The workflow runs entirely
/// within this function call.
///
/// All substrate sinks (Lago / Vigil / Lifegw) are tagged with the
/// parent tick's `run_id`, so workflow stream events nest under the
/// tick in the journal.
pub async fn run_workflow_as_tick(
    workflow_name: &str,
    input: serde_json::Value,
    tick_ctx: &TickBodyCtx<'_>,
) -> Result<serde_json::Value, ArcanErgonError>;

/// Per-tick context the kernel hands to the workflow body. Constructed
/// inside the tick handler from kernel state.
pub struct TickBodyCtx<'a> {
    pub session_id: SessionId,
    pub branch_id:  BranchId,
    pub run_id:     RunId,
    pub trace:      tracing::Span,

    // L2 adapter handles — wired at daemon startup
    pub provider_port:    Arc<dyn ModelProviderPort>,
    pub tool_harness:     Arc<dyn ToolHarnessPort>,
    pub policy_gate:      Arc<dyn PolicyGatePort>,
    pub journal:          Arc<dyn lago_core::Journal>,
    pub upstream_tx:      tokio::sync::mpsc::Sender<ergon::StreamEvent>,

    // Substrate state for auto-hooks
    pub policy_set:       Arc<aios_protocol::PolicySet>,
    pub gating_profile:   Arc<autonomic_core::AutonomicGatingProfile>,
    pub evaluator:        Arc<dyn nous_core::NousEvaluator>,
    pub soul:             Arc<anima_core::AgentSoul>,

    // Workflow registry (for sub-step lookup if a workflow composes others)
    pub workflows:        Arc<WorkflowRegistry>,
}
```

### 4.4 Workflow registry

```rust
pub struct WorkflowRegistry {
    workflows: dashmap::DashMap<String, Arc<dyn DynWorkflowExecutor>>,
}

impl WorkflowRegistry {
    pub fn new() -> Self;

    /// Register a workflow under the given name (e.g.,
    /// "bookkeeping.promotion-judge"). Typically called at daemon
    /// startup from a config or feature-flag list.
    pub fn register<W: ergon::Workflow>(&self, name: &str, workflow: Arc<W>);

    /// Look up by name. Returns None if not registered.
    pub fn get(&self, name: &str) -> Option<Arc<dyn DynWorkflowExecutor>>;
}

/// Type-erased executor. Internally holds an `Arc<dyn Workflow>`-like
/// thing plus the type-coercion machinery so `run_workflow_as_tick`
/// can take `serde_json::Value` and dispatch generically.
trait DynWorkflowExecutor: Send + Sync {
    async fn run_dyn(
        &self,
        ctx: &mut ergon::StepCtx<'_>,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ergon::ErgonError>;
}
```

`DynWorkflowExecutor` is the type-erasure trick that lets us hold
arbitrary `Workflow<Input, Output>` impls in a single registry.
Implementation detail: a wrapper struct `TypedExecutor<W>` does the
`serde_json` round-trip at the boundary.

## 5. Lifecycle — kernel tick → workflow execution

LOCKED:

```text
arcand consciousness owns the outer loop.
For each tick:

1.  ConsciousnessActor decides a tick is needed
2.  Builds TickInput { objective, kind, ... }
       kind = TickKind::Direct      (today's path, unchanged)
              | TickKind::Workflow { name, input }   (new, this spec)
3.  Calls KernelRuntime::tick_on_branch(...)
4.  Kernel emits TickStarted{run_id, kind}
5.  Kernel dispatches based on kind:
       Direct   → existing path (ModelProviderPort.complete + tool dispatch)
       Workflow → arcan_ergon::run_workflow_as_tick(name, input, body_ctx)
6.  arcan-ergon:
       a) WorkflowRegistry.get(name) → executor
       b) Build StepCtx<'_> from TickBodyCtx:
          - provider:  ErgonProviderAdapter wrapping body_ctx.provider_port
          - tools:     ErgonToolRegistryAdapter wrapping body_ctx.tool_harness
                                                      + body_ctx.policy_gate
          - hooks:     HookRegistry::default()
                          .with(PraxisCapabilityHook::new(/* PolicySet adapter */))
                          .with(AutonomicBudgetHook::new(/* gating-profile adapter */))
                          .with(NousScoreHook::new(/* evaluator adapter */))
                          .with(AnimaAttestHook::new(/* soul adapter */))
          - sink:      Arc::new(FanoutSink::new(vec![
                          Arc::new(LagoSink::new(body_ctx.journal,
                                                 body_ctx.session_id.clone())),
                          Arc::new(VigilSink::new()),
                          Arc::new(LifegwSink::new(body_ctx.upstream_tx.clone())),
                       ]))
                      // All sinks already inherit body_ctx.run_id via session scope
          - runtime:   ErgonRuntimeHandleAdapter
       c) executor.run_dyn(&mut ctx, input).await
            → fires on_workflow_start hooks
            → calls workflow.execute(ctx, input)
                  → user code runs ctx.run_inference_streaming / ctx.tools.invoke
                  → each StreamEvent flows through FanoutSink
                  → LagoSink appends EventKind::Custom{
                        event_type: "ergon.stream",
                        data: <event>
                    } to journal under body_ctx.run_id
            → fires on_workflow_end hooks
            → returns Output
       d) Output serialised to JSON, returned to kernel
7.  Kernel emits TickCompleted{
        run_id, kind: Workflow, output: Some(json),
        mode_after, last_seq, ...
    }
8.  ConsciousnessActor reads TickOutput, decides next tick
```

**Result**: the workflow runs *inside* one kernel tick. Tick boundaries
emit kernel-typed events. Workflow internals emit nested Custom events.
Replay can choose granularity. Autonomic / EGRI / branching all see
the kernel tick events normally.

## 6. The TickKind extension (small aios-runtime change)

`aios-runtime` needs a new field on `TickInput`:

```rust
pub struct TickInput {
    pub objective: String,
    pub proposed_tool: Option<ToolCall>,
    pub system_prompt: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub kind: TickKind,            // NEW
}

#[non_exhaustive]
pub enum TickKind {
    Direct,
    Workflow {
        name: String,
        input: serde_json::Value,
    },
}

impl Default for TickKind {
    fn default() -> Self { Self::Direct }    // backward-compatible
}
```

`Default::default() == TickKind::Direct` keeps every existing call
site working without modification. New call sites that want a workflow
tick set `kind: TickKind::Workflow { ... }` explicitly.

The kernel tick handler in `aios_runtime::KernelRuntime` matches on
`tick_input.kind` and dispatches to the right body. This is a small
diff (~20 LOC) but it's the **seam** that makes ergon useful.

This change is part of BRO-1001's scope. Not deferred to BRO-1001b
(that's the *daemon*-side wiring; this is the *kernel*-side seam).

## 6.1 Composition with `OperatingMode::Verify` (forward-compatible note)

`TickKind::Workflow` is what enables a clean future composition where
the kernel's `Verify` mode dispatches to evaluator workflows directly.
That's not in v0.1 scope, but the `TickKind` enum is intentionally
non-exhaustive so the kernel can grow:

```rust
#[non_exhaustive]
pub enum TickKind {
    Direct,
    Workflow { name: String, input: serde_json::Value },
    // Future variants (NOT in v0.1):
    // VerifyWorkflow { evaluator: String, target: RunId }
    // RecoverWorkflow { strategy: String, error_summary: String }
}
```

When this composition lands (post-v0.1, post-BRO-1003 proves the
pattern), evaluator workflows like `bookkeeping.promotion-judge` and
`session.evaluator` can be invoked by the kernel automatically when
`mode == Verify`. See `docs/architecture/agent-harness.md` § "Where
evaluators live (nous metacognition)" for the detailed composition.

This is mentioned here so future agents reading this spec understand
that `TickKind`'s shape is anticipating evaluator-workflow dispatch
without committing to it. v0.1 ships only `Direct` and `Workflow`.

## 7. Definition of done — v0.1 ships when

ALL of the following are true:

1. `cargo check -p arcan-ergon` passes.
2. `cargo test -p arcan-ergon --all-targets` passes (~25 unit tests +
   1 integration test against an in-process kernel).
3. `cargo clippy -p arcan-ergon --all-targets -- -D warnings` passes.
4. `cargo fmt -p arcan-ergon -- --check` passes.
5. `aios-runtime` has the `TickKind` extension; default behaviour is
   unchanged for `TickKind::Direct` (every existing call site
   compiles and runs without modification).
6. The integration test drives the path end-to-end:
   - Register a trivial workflow ("echo input")
   - Build a `TickBodyCtx` over mock adapters
   - Call `run_workflow_as_tick` with `kind=Workflow{name="echo", input=...}`
   - Assert the kernel emits `TickStarted{Workflow}` and
     `TickCompleted{output: ...}`
   - Assert the journal contains nested `Custom("ergon.stream", ...)`
     events tagged with the parent `run_id`
   - Assert the four auto-hooks fired in order
7. No regression in any existing arcan / aios-runtime / arcand test.
8. CHANGELOG entry on the Life monorepo lists arcan-ergon v0.1.

## 8. Open questions

Five questions whose final answers fall out naturally during the first
200 LOC of `runner.rs`. Proposals locked unless impl reveals a problem.

1. **Q1.** Type erasure for the workflow registry — `Box<dyn DynWorkflowExecutor>`
   with serde-Value boundaries, or generic `Workflow<I, O>` per-call?
   *Resolution: erasure via `DynWorkflowExecutor` + JSON boundary.
   Generic per-call breaks the "register once, dispatch by string"
   pattern.*

2. **Q2.** Where does the `WorkflowRegistry` live — owned by arcand,
   or a static singleton?
   *Resolution: owned by arcand at startup; passed via `TickBodyCtx`.
   No statics.*

3. **Q3.** How does `LifegwSink` get the right upstream `tx` for the
   current session?
   *Resolution: arcand's session state owns the tx; passes it into
   `TickBodyCtx` per tick.*

4. **Q4.** Should `RunId` for workflow stream events be the parent
   tick's run_id, or a child run_id?
   *Resolution: parent tick's run_id, per the architecture doc's
   "sub-events nest under the tick" invariant. A workflow is one tick;
   it does not start a new run.*

5. **Q5.** Where does the model name come from — `InferenceRequest` or
   tick-level config?
   *Resolution: `InferenceRequest::model` (set by the workflow body).
   The kernel tick handler doesn't impose a model.*

## 9. Validation criteria (post-ship)

Within 2 weeks of v0.1 shipping, the adapter must demonstrate:

1. **Bookkeeping-judge parity** (BRO-1003): the same workflow that
   shipped on Bellows produces verdicts identical to the Bellows
   version when fed the same input. Behavior parity proves the adapter
   layering is correct.

2. **Journal traceability**: a real session running a workflow tick
   can be:
   - Replayed at tick granularity (autonomic / EGRI / branching see
     only `TickStarted` / `TickCompleted` events, normally)
   - Replayed at workflow granularity (debugger sees every
     `StreamEvent` nested under the parent tick)

3. **No regression**: arcan's existing test suite plus all
   aios-runtime tests pass. KernelRuntime tick latency for a
   `TickKind::Direct` tick is within 1% of pre-spec baseline.

4. **Backward compatibility**: every existing arcand call site that
   builds a `TickInput` without setting `kind` continues to work
   (`Default::default() == TickKind::Direct`).

## 10. Composition trajectory

There is no "arcan-harness retirement." The original spec framed ergon
as a successor to arcan-harness; that's wrong. arcan-harness is a
small ~300 LOC tool/sandbox/hashline-edit utility crate that stays as
it is.

The actual agent harness is the kernel runtime stack
(`aios_runtime::KernelRuntime` + port adapters + `arcand`
consciousness). That stack is **augmented** by ergon, not replaced:

| Phase | When | Action |
|---|---|---|
| v0.1 | now | ergon shipped; arcan-ergon adapter (this spec) lets workflows run as rich ticks; bookkeeping-judge (BRO-1003) is the proof-of-shape |
| v0.2 | after Spec E (BRO-1019) ships | `ergon::Provider` re-targets `InferenceRouter`; KV reuse + speculative decode + multi-vendor silicon for free |
| v0.3 | as workflows accumulate | more agents structure their tick bodies as workflows; direct-tick remains for simple per-turn flows |
| ongoing | indefinite | both tick body shapes coexist; the choice is per-tick, not per-session |

There is no migration to "everything on ergon." Ergon is one shape of
tick body. Different tasks favor different shapes.

## 11. Versioning & stability

- arcan-ergon v0.x: pre-1.0; minor versions may break trait surfaces
  with CHANGELOG migration notes.
- arcan-ergon v1.0: trait surface frozen; `#[non_exhaustive]` on every
  public enum/struct that may grow.
- The `TickKind` enum is `#[non_exhaustive]` — new variants land in
  any minor version without breakage.

## 12. Implementation work — ordered

1. Add `TickKind` enum + field to `aios_runtime::TickInput`. Verify
   no test breaks (`cargo test -p aios-runtime --all-targets`).
2. Add the kernel-side dispatch match in `KernelRuntime::tick_on_branch`
   for `TickKind::Workflow{...}`. Initially returns
   `Err(KernelError::NotImplemented(...))`. Verify default `Direct`
   path unchanged.
3. Create `crates/arcan/arcan-ergon/` with the locked Cargo.toml from
   §4.1 and module skeleton from §4.2.
4. Land `error.rs`, `registry.rs`, `runtime_handle.rs` (small, no
   substrate complexity).
5. Land `provider.rs` (translates `ergon::ModelRequest` ↔
   `aios_runtime::ModelCompletionRequest`; this is the meatiest of the
   adapters).
6. Land `tools.rs` (translates `ergon::ToolCall` ↔
   `aios_runtime::ToolHarnessPort` invocation; integrates
   `PolicyGatePort` for capability checks).
7. Land the four auto-hook adapter impls in `auto_hooks/`.
8. Land `runner.rs` — `run_workflow_as_tick` ties everything together.
9. Wire the kernel-side dispatch (step 2) to call
   `arcan_ergon::run_workflow_as_tick`. Now the path is end-to-end.
10. Integration test in `tests/end_to_end.rs` per §7 item 6.
11. CHANGELOG entry.

Estimated effort: 3-5 days focused work.

## 13. Sign-off

This spec is **ready to file**. It supersedes §6 and §10 of
`2026-05-05-ergon-v0.1.md` and references the canonical layered
architecture in `docs/architecture/agent-harness.md`.

Filed at: `core/life/docs/superpowers/specs/2026-05-08-bro-1001-ergon-tick-body.md`.
Linear: BRO-1001 (description updated to point here).
