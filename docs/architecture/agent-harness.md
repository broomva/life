# Agent Harness — Layered Runtime Architecture

**Date**: 2026-05-08
**Status**: Canonical — supersedes the §6/§10 framing of `2026-05-05-ergon-v0.1.md`
**Owner**: Life kernel + ergon teams jointly
**Related specs**:
- `docs/superpowers/specs/2026-05-05-ergon-v0.1.md` (workflow trait crate)
- `docs/superpowers/specs/2026-05-08-bro-1001-ergon-tick-body.md` (corrected adapter design)
- `docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md` (silicon contract, BRO-1019)

## Why this document exists

"Agent harness" is overloaded terminology. There is a Rust crate literally
named `arcan-harness` — it is ~300 LOC of tool / sandbox / hashline-edit
utilities, not the harness. The actual agent harness is a **7-layer stack
distributed across ~25 crates**.

This document is the canonical map of that stack. Any future spec that
mentions "the harness" should reference these layers by number.

A previous spec (`2026-05-05-ergon-v0.1.md`) framed `ergon` as a
session-level runtime that would replace `arcan-harness` over time.
**That framing was wrong on two counts**: (a) `arcan-harness` was never
the harness, and (b) ergon's workflow model can't replace the
long-horizon tick engine without losing the per-tick journal traceability
that autonomic / EGRI / branching depend on.

The corrected framing — documented here, in the BRO-1001 design spec,
and propagated through the ergon spec — is that **ergon supplies one
shape of tick body alongside the existing direct-call tick body**. Both
compose with the kernel; both produce per-tick events; neither replaces
anything.

## The 7-layer stack

```text
┌────────────────────────────────────────────────────────────────────┐
│ L6 — Process entry points                                          │
│   arcan (binary)         lifed (facade-aggregator daemon)          │
│   arcand (agent daemon)  lifegw (edge gateway)                     │
└────────────────────────────────────────────────────────────────────┘
                                   ↓
┌────────────────────────────────────────────────────────────────────┐
│ L5 — Session orchestration  ← OUTER LOOP scope                     │
│   arcand::ConsciousnessActor                                       │
│   - mailbox + queue + projection                                   │
│   - decides WHEN to start/stop/interrupt agent cycles              │
│   - drives KernelRuntime tick by tick                              │
└────────────────────────────────────────────────────────────────────┘
                                   ↓
┌────────────────────────────────────────────────────────────────────┐
│ L4 — Tick engine (long-horizon agent loop)                         │
│   aios_runtime::KernelRuntime::tick_on_branch(...)                 │
│   - one tick = one bounded operation                               │
│   - mode FSM (Execute/Verify/Recover/AskHuman/Sleep)               │
│   - emits kernel-typed EventKind events to journal                 │
│   - persists AgentStateVector, branch state, run state             │
└────────────────────────────────────────────────────────────────────┘
                                   ↓
┌────────────────────────────────────────────────────────────────────┐
│ L3.5 — Tick body (what runs INSIDE one tick)  ← INNER LOOP scope   │
│   Two shapes:                                                      │
│   • Direct: one model call via ModelProviderPort (current)         │
│   • Workflow: ergon::Workflow::execute (BRO-1001)                  │
└────────────────────────────────────────────────────────────────────┘
                                   ↓
┌────────────────────────────────────────────────────────────────────┐
│ L3 — Port traits (kernel ↔ substrate boundary)                     │
│   ModelProviderPort, ToolHarnessPort, ApprovalPort,                │
│   PolicyGatePort, EventStorePort                                   │
│   + TurnMiddleware, ToolCallGuard for cross-cutting                │
│   (aios-runtime owns the trait shapes)                             │
└────────────────────────────────────────────────────────────────────┘
                                   ↓
┌────────────────────────────────────────────────────────────────────┐
│ L2 — Substrate adapters (port impls over real backends)            │
│   arcan-aios-adapters    (ArcanProviderAdapter → ModelProviderPort)│
│   arcan-praxis           (PraxisToolHarness → ToolHarnessPort)     │
│   arcan-lago             (LagoEventStore → EventStorePort)         │
│   arcan-anima            (AgentSoul attestation hooks)             │
│   ergon-life-hooks       (4 auto-hooks for ergon ticks)            │
│   ergon-life-sinks       (3 stream sinks for ergon ticks)          │
└────────────────────────────────────────────────────────────────────┘
                                   ↓
┌────────────────────────────────────────────────────────────────────┐
│ L1.5 — Silicon contract  ← Spec E / BRO-1019 (in design)           │
│   inference-core::InferenceBackend                                 │
│   inference-mlx, inference-vllm, inference-tt, ...                 │
│   KV cache, speculative decode, model swap, tool-await reconnect   │
└────────────────────────────────────────────────────────────────────┘
                                   ↓
┌────────────────────────────────────────────────────────────────────┐
│ L1 — Substrate primitives                                          │
│   lago-core (event journal + blob store)                           │
│   praxis-core (tool execution + sandbox)                           │
│   anima-core (identity)                                            │
│   autonomic-core (homeostasis)                                     │
│   nous-core (evaluation)                                           │
│   life-vigil (observability)                                       │
└────────────────────────────────────────────────────────────────────┘
                                   ↓
┌────────────────────────────────────────────────────────────────────┐
│ L0 — Kernel contract                                               │
│   aios-protocol                                                    │
│   - EventKind taxonomy                                             │
│   - OperatingMode FSM                                              │
│   - AgentStateVector, BudgetState, PolicySet                       │
│   - SessionId, BranchId, RunId, ToolRunId                          │
│   - Capability tokens, SoulProfile                                 │
└────────────────────────────────────────────────────────────────────┘
```

**The harness** = layers L1.5 + L2 + L3 + L3.5 + L4 + L5. Below it: the
kernel contract and substrate primitives. Above it: process daemons.

## Layer-by-layer responsibilities

### L0 — Kernel contract (`aios-protocol`)

Owns the canonical types every layer above must agree on. No code that
*does* anything; only types. Stability commitment: changes are major
version bumps with migration notes.

### L1 — Substrate primitives

Each substrate (lago / praxis / anima / autonomic / nous / vigil) is its
own crate cluster. Each provides typed Rust APIs over a domain (events,
tools, identity, homeostasis, evaluation, observability). They depend on
L0 and not on each other (some bridge crates exist; those are L2).

### L1.5 — Silicon contract (Spec E, in design)

The `InferenceBackend` trait + KV cache + speculative decoding + model
swap + tool-await reconnect. Sits between the substrate primitives and
the port adapters at L2/L3. **Lives in `inference-core` once shipped**;
today this layer is occupied by `arcan-core/src/aisdk.rs` (single
hard-coded path to Vercel AI SDK).

### L2 — Substrate adapters

The crates that implement port traits over real substrate. Naming
convention is `arcan-<substrate>` for kernel-facing adapters:
- `arcan-aios-adapters` — `ArcanProviderAdapter` implements
  `ModelProviderPort` (also `AutonomicPolicyAdapter` for
  `PolicyGatePort`)
- `arcan-praxis` — implements `ToolHarnessPort`
- `arcan-lago` — implements `EventStorePort`
- `arcan-anima` — soul attestation hooks
- `ergon-life-hooks` — the four auto-hooks for ergon-shaped ticks
- `ergon-life-sinks` — the three stream sinks (Lago / Vigil / Lifegw)

These are *adapter* crates: they translate between kernel/ergon-side
abstractions (port traits, hook traits) and substrate-side concrete
types (Journal, AutonomicGatingProfile, NousEvaluator, AgentSoul, etc.).

### L3 — Port traits

`aios-runtime` defines the kernel-facing trait shapes. These are the
ABI between L4 (kernel) and L2 (adapters). Critically, L3 doesn't know
which adapter is wired in — it sees only the trait. This is what lets
us swap arcan-praxis for a different tool runtime without changing the
kernel.

### L3.5 — Tick body

What runs inside one kernel tick. **This is the layer where ergon plugs
in.** Two shapes today:

- **Direct tick body**: one model call via `ModelProviderPort.complete`,
  optional tool dispatch via `ToolHarnessPort.execute`. Existing
  pattern, used by every tick in production today.
- **Workflow tick body** (BRO-1001): one
  `ergon::WorkflowExecutor::run` call that runs an entire bounded
  multi-turn operation inside the tick's lifetime. Returns a typed
  `Output` that becomes the tick's payload.

Both produce kernel `EventKind` events at tick boundaries. Workflow
bodies additionally produce `EventKind::Custom("ergon.stream", ...)`
events nested under the parent tick's `run_id` for sub-event
traceability.

### L4 — Tick engine (`KernelRuntime::tick_on_branch`)

The long-horizon loop. Each call to `tick_on_branch`:
- Reads session state (AgentStateVector, queued approvals, mode)
- Runs the appropriate tick body (direct or workflow)
- Updates state
- Persists events to the event journal via `EventStorePort`
- Returns a `TickOutput` with the new mode

The tick engine is *bounded per call but unbounded per session*. A
single agent run is N ticks; a session can run for weeks.

### L5 — Session orchestration (`arcand::ConsciousnessActor`)

Decides *when* to tick. Owns:
- The session's mailbox (incoming user messages, tool results, timer
  fires)
- The work queue (current run, pending follow-ups)
- The interruption flag (for steering)
- The cadence (re-tick while `mode == Execute`; pause on `Sleep` /
  `AskHuman`)

The consciousness actor is what makes long-horizon agents real —
without it, there's no answer to "when does the next tick run?"

### L6 — Process entry points

The daemons and binaries that hold sessions in memory:
- `arcand` — the agent daemon, hosts ConsciousnessActor instances
- `lifed` — facade-aggregator routing requests to the right backend
- `lifegw` — edge gateway with TLS, JWT, scope intersection
- `arcan` — installable CLI binary

## The two scopes of the agent loop

This is the most important distinction in this document.

### Outer loop — session scope

**Owned by**: L5 + L4 (ConsciousnessActor + KernelRuntime).

**Lifetime**: across days. Bounded only by user lifecycle / explicit
session close.

**Step granularity**: one tick (one call to `tick_on_branch`).

```text
ConsciousnessActor receives mailbox event
  ↓
decides to start/continue an agent cycle
  ↓
calls run_agent_cycle_inner
  ↓
loops: KernelRuntime::tick_on_branch(...)
  - returns mode
  - if mode == Execute: continue ticking
  - if mode == Verify: nous evaluation pass
  - if mode == Recover: error-handling tick
  - if mode == AskHuman: pause, wait for input
  - if mode == Sleep: stop until next mailbox event
  ↓
each tick produces:
  TickStarted, TickCompleted, mode transitions, AgentStateVector
  updates, RunStarted/Completed, ToolCallRequested/Completed,
  ProviderResponded, Steered, Compacted, ... (kernel-typed
  EventKind variants in lago journal)
```

This is the long-horizon agent. **It can run for weeks. Each tick is a
checkpoint. Lago has every tick. Replay reconstructs the whole
agent.**

### Inner loop — tick scope

**Owned by**: L3.5 (tick body).

**Lifetime**: one tick (seconds for direct ticks, seconds-to-minutes
for workflow ticks).

**Step granularity**: model call + optional tool dispatch (direct), or
multiple model + tool turns within `Workflow::execute` (workflow).

**Direct tick body**:

```text
TickStarted (run_id = R, kind = Direct)
  ↓
  ModelProviderPort.complete(req)
  ↓
  optional: ToolHarnessPort.execute(call)
  ↓
  emit ProviderResponded / ToolCallCompleted
  ↓
TickCompleted (mode_after = ?)
```

**Workflow tick body** (BRO-1001):

```text
TickStarted (run_id = R, kind = Workflow{name, input})
  ↓
  arcan_ergon::run_workflow_as_tick(name, input, ctx)
       ↓
       ergon::WorkflowExecutor::run(ctx, input)
            ↓
            workflow.execute(ctx, input)  [user-written async fn]
                 ↓
                 ctx.run_inference_streaming(req)  [autonomous turn 1]
                      ↓ each StreamEvent → LagoSink (tagged run_id=R)
                      ↓                  → VigilSink (tracing)
                      ↓                  → LifegwSink (user SSE)
                 ↓
                 ctx.tools.invoke(call)
                 ↓
                 ctx.run_inference_streaming(req)  [autonomous turn 2]
                 ↓
                 ...
            ↓
            return Workflow::Output
  ↓
TickCompleted (run_id = R, output = JSON, mode_after = ?)
```

**Crucially**: workflow stream events are appended to the journal
*tagged with the parent tick's `run_id`*. They're sub-events under the
tick. Replay can choose:
- Tick-level granularity: just `TickStarted` / `TickCompleted`
  boundaries (this is what autonomic / EGRI / branching consume)
- Workflow-level granularity: re-stream every `TextDelta`,
  `ToolUseStart`, etc. (this is for inner-loop debugging)

## Bottom-up dependency chain

Each layer depends only on layers below it. **No backwards arrows. No
peer reaches up.**

```text
Step 0: aios-protocol             (zero deps; kernel contract)
            ↓
Step 1: lago-core, praxis-core, anima-core, autonomic-core,
        nous-core, life-vigil    (substrate primitives)
            ↓
Step 2: aios-runtime              (KernelRuntime + port traits)
            ↓
Step 3: arcan-core                (legacy types: Provider/Tool/
                                   Orchestrator; mostly used in
                                   tests + some adapters)
            ↓
Step 4: ergon                     (workflow trait, autonomous loop;
                                   ZERO substrate deps — the
                                   architectural commitment)
            ↓
Step 5: substrate adapters
        a) arcan-aios-adapters    (kernel ports)
        b) arcan-praxis           (tool harness)
        c) arcan-lago             (event store)
        d) arcan-anima            (identity hooks)
        e) ergon-life-hooks       (4 auto-hooks)
        f) ergon-life-sinks       (3 stream sinks)
            ↓
Step 6: arcan-ergon (BRO-1001)
        - implements ergon::Provider over ModelProviderPort
        - implements ergon::ToolRegistry over ToolHarnessPort
        - implements 4 ergon-life-hooks adapter traits
        - exposes run_workflow_as_tick(name, input, tick_ctx)
            ↓
Step 7: arcand (kernel daemon)
        - ConsciousnessActor (session orchestration)
        - run_agent_cycle_inner (the outer-loop driver)
        - dispatches each tick to the right body
            ↓
Step 8: process binaries
        arcan binary, lifed, lifegw
```

The ordering is enforced at the Cargo level: each crate's `Cargo.toml`
lists deps from layers below. Workspace-level CI (`cargo deny`,
`verify_dependencies_*`) catches violations.

## Top-down execution trace

A real example, end-to-end. **Scenario**: long-running coding agent
(kernel-shaped session) needs to score 5 research extracts as part of
its work; uses bookkeeping-judge (ergon workflow) for each.

```text
1. User sends message via lifegw (L6)
   ↓
2. lifed routes to arcand (L6)
   ↓
3. ConsciousnessActor (L5) wakes up
   - reads mailbox
   - decides this is a continuation of session S1 (existing kernel agent)
   - calls run_agent_cycle_inner(run_id=R7, session_id=S1, ...)
   ↓
4. run_agent_cycle_inner enters the outer loop
   ↓
5. tick #1: KernelRuntime::tick_on_branch(S1, "main", input) (L4)
   - mode = Execute, kind = Direct
   - goes through ModelProviderPort (L3)
   - ArcanProviderAdapter (L2) calls aisdk (L1.5 today, future Spec E)
   - model returns: "I need to score 5 extracts"
   - kernel emits TickStarted/ProviderResponded/TickCompleted
     (kernel-typed events)
   - tick body decides next tick should be a Workflow tick
   ↓
6. tick #2: KernelRuntime::tick_on_branch(S1, "main", input) (L4)
   - mode = Execute, kind = Workflow{
       name="bookkeeping.promotion-judge",
       input={"extract": "ext1.md"}
     }
   - emits TickStarted{run_id=R7-T2, kind=Workflow}
   ↓
7. tick body dispatches to arcan_ergon::run_workflow_as_tick (L3.5)
   - looks up the workflow by name in WorkflowRegistry
   - constructs StepCtx from TickCtx:
     • provider: ergon::Provider impl wrapping ArcanProviderAdapter
     • tools: ergon::ToolRegistry impl wrapping arcan-praxis
     • hooks: HookRegistry pre-populated with 4 ergon-life-hooks (L2)
     • sink: FanoutSink([LagoSink, VigilSink, LifegwSink])
       from ergon-life-sinks (L2), each tagged with run_id=R7-T2
     • runtime: arcan's TickHandle as RuntimeHandle
   - calls WorkflowExecutor::run(ctx, input)
   ↓
8. WorkflowExecutor.run fires on_workflow_start hooks (L3.5)
   - AnimaAttestHook signs SessionStart (sub-event under R7-T2)
   - PraxisCapabilityHook (no-op for workflow_start)
   - AutonomicBudgetHook (no-op for workflow_start)
   - NousScoreHook (no-op for workflow_start)
   ↓
9. workflow.execute(ctx, input) runs (user's code, L3.5)
   pseudo:
   ```
   let extract = ctx.tools.invoke("fs_read",
                                  {"path": "ext1.md"}).await?;
                 ↑ on_pre_tool_use → PraxisCapabilityHook
                                     checks PolicySet (L2)
                 ↑ tools.invoke → ergon::ToolRegistry impl
                                  → arcan-praxis (L2)
                                  → praxis-core::Tool (L1)
                 ↑ on_post_tool_use
   let req = InferenceRequest::new("claude-sonnet-4")
                 .with_role(Role::agent("Score this extract..."));
   let resp = ctx.run_inference_streaming(req).await?;
                 ↑ on_pre_inference → AutonomicBudgetHook (L2)
                                     → reads HomeostaticState
                                     → may Continue/Deny
                 ↑ provider.stream → ergon::Provider impl
                                    → ArcanProviderAdapter (L2)
                                    → ModelProviderPort (L3)
                                    → aisdk → Anthropic API (L0)
                 ↑ each StreamEvent → sink.emit
                                      (LagoSink → journal,
                                       VigilSink → tracing,
                                       LifegwSink → user's SSE stream)
                 ↑ on_post_inference → NousScoreHook scores response
   Ok(parse_verdict(&resp))
   ```
   ↓
10. WorkflowExecutor.run fires on_workflow_end hooks
    - AnimaAttestHook signs SessionClose
    ↓
11. arcan_ergon::run_workflow_as_tick returns Output (PromotionVerdict)
    ↓
12. KernelRuntime emits TickCompleted{run_id=R7-T2, output=verdict,
                                       mode_after=Execute}
    ↓
13. ConsciousnessActor sees the result
    - decides to continue ticking (4 more extracts to score)
    - back to step 6 with input={"extract": "ext2.md"}
    ↓
14. After all 5 extracts: tick #N has mode_after=Verify
    - kernel runs evaluation tick (could be another workflow or direct)
    ↓
15. Eventually: mode_after=Sleep
    - ConsciousnessActor stops the cycle
    - session persists in lago
    - waits for next mailbox event
```

**Two scoped agent loops in one execution**:
- The outer loop ran 6+ ticks across the session. Each tick is durable,
  replayable, autonomic-gated, branchable, traceable as kernel events.
- The inner loop (steps 7–11) ran 5 times — one per extract — each as
  a rich tick. Each was bounded, returned a typed Output, didn't have
  to know about ticks.

**Crucially**: workflow events from steps 7–11 are in the journal
tagged with `run_id=R7-T2` etc. They nest under the parent tick.
Replay at tick granularity sees only `TickStarted{Workflow} →
TickCompleted{Output}`. Replay at workflow granularity also sees every
TextDelta, ToolCall, score.

## Where each layer lives in the workspace

| Layer | Crates / paths |
|---|---|
| L0 | `crates/aios/aios-protocol` |
| L1 | `crates/lago/*`, `crates/praxis/praxis-core`, `crates/anima/anima-core`, `crates/autonomic/autonomic-core`, `crates/nous/nous-core`, `crates/vigil/life-vigil` |
| L1.5 | `crates/inference/*` (planned, BRO-1019); today `crates/arcan/arcan-core/src/aisdk.rs` |
| L2 | `crates/arcan/arcan-aios-adapters`, `crates/arcan/arcan-praxis`, `crates/arcan/arcan-lago`, `crates/arcan/arcan-anima`, `crates/ergon/ergon-life-hooks`, `crates/ergon/ergon-life-sinks` |
| L3 | `crates/aios/aios-runtime` (port traits) |
| L3.5 | Direct: `arcan-aios-adapters` (existing). Workflow: `crates/arcan/arcan-ergon` (BRO-1001, planned) |
| L4 | `crates/aios/aios-runtime` (KernelRuntime impl) |
| L5 | `crates/arcan/arcand` (ConsciousnessActor) |
| L6 | `crates/arcan/arcan` (binary), `crates/life-runtime/lifed`, `crates/life-runtime/lifegw` |

## Built today vs pending

**Built and on main:**
- L0 aios-protocol
- L1 all six substrate primitives
- L2 substrate adapters (kernel side); ergon-life-hooks; ergon-life-sinks (after #1170)
- L3 port traits in aios-runtime
- L4 KernelRuntime
- L5 ConsciousnessActor
- L6 arcan, arcand, lifed, lifegw
- ergon trait crate (workflow, hook, model, runtime, step modules)

**Pending:**
- L3.5 workflow tick body — `arcan-ergon` adapter (BRO-1001, with the
  *corrected* design per
  `2026-05-08-bro-1001-ergon-tick-body.md`)
- arcand `TickKind` dispatch — small change to TickInput + the tick
  body match in run_agent_cycle_inner (BRO-1001b, follow-up)
- bookkeeping-judge port — the first real `Workflow` impl that runs
  as a rich tick (BRO-1003)

**Spec E future** (BRO-1019, parallel track):
- L1.5 `inference-core` — silicon contract trait. When this lands,
  `ergon::Provider` impls in `arcan-ergon` re-target through
  `InferenceRouter` for KV reuse + speculative decode + multi-vendor
  silicon.

## Glossary

| Term | Meaning in this document |
|---|---|
| **Agent** | A long-running entity bound to a session, ticked by KernelRuntime |
| **Session** | A persistent identity-scoped run, identified by `SessionId` |
| **Tick** | One bounded execution unit; a call to `tick_on_branch`; produces a `TickOutput` |
| **Tick body** | What runs *inside* a tick — direct (one call) or workflow (multi-call) |
| **Workflow** | A bounded multi-turn operation defined by `ergon::Workflow::execute`; runs *inside* one tick |
| **Run** | A single agent cycle within a session, identified by `RunId`; spans multiple ticks |
| **Branch** | A divergent path within a session, identified by `BranchId`; sessions can branch and merge |
| **OperatingMode** | The FSM state of a session: Execute / Verify / Recover / AskHuman / Sleep |
| **Outer loop** | The tick stream owned by ConsciousnessActor + KernelRuntime; long-horizon |
| **Inner loop** | The model+tool loop within one tick body; bounded |

## Stability commitments

1. **The 7 layers above will not change without a versioned spec
   amendment.** Layers may grow internal complexity; the layer
   boundaries themselves are stable.
2. **The dependency chain is bottom-up only.** Any PR that adds a
   reverse arrow (e.g., aios-protocol depending on lago) is
   architecturally invalid and must be rejected at review.
3. **The two scopes of the agent loop are stable.** Outer loop is
   tick-driven and lives across days. Inner loop is tick-bounded and
   lives within seconds-to-minutes per tick. New tick body shapes
   may be added without changing this contract.
4. **Per-tick events in the journal are kernel-typed `EventKind`
   variants.** Workflow stream events are sub-events nested under the
   parent tick's `run_id`. Both granularities are replayable; neither
   replaces the other.

## References

- `aios-protocol` source — kernel contract types
- `aios-runtime/src/lib.rs` — `KernelRuntime`, port traits
- `arcand/src/consciousness.rs` — ConsciousnessActor + outer loop driver
- `crates/ergon/ergon/src/step.rs` — autonomous inner loop
- `docs/superpowers/specs/2026-05-05-ergon-v0.1.md` — workflow trait spec (with §6/§10 corrections)
- `docs/superpowers/specs/2026-05-08-bro-1001-ergon-tick-body.md` — corrected adapter design
- `docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md` — silicon contract
