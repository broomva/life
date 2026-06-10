# Ergon — Layer-2 Workflow Primitive

**Date**: 2026-05-20
**Status**: Canonical — v0.1 architectural reference
**Owner**: Ergon + Life-kernel teams jointly
**Related specs**:
- `core/life/docs/superpowers/specs/2026-05-05-ergon-v0.1.md` (workflow
  trait spec; §6 and §10 superseded)
- `core/life/docs/superpowers/specs/2026-05-08-bro-1001-ergon-tick-body.md`
  (corrected adapter design)
- `core/life/docs/architecture/agent-harness.md` (the 7-layer harness map
  in which ergon is Layer 2)
- `core/life/docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md`
  (Spec E / BRO-1019 — silicon contract at Layer 1.5)
**Linear umbrella**: [BRO-994](https://linear.app/broomva/issue/BRO-994)

## 0. Purpose (one sentence — governs the whole crate cluster)

**Ergon is Life's agent-harness primitive: the trait set that lets a
Broomva developer write a `Workflow` in Rust whose deterministic outer
body orchestrates autonomous inner LLM steps, integrated end-to-end with
the Life substrate (praxis tools, lago events, anima identity, autonomic
budgets, nous scoring, vigil traces, lifegw delivery).**

If a feature does not serve that sentence, it does not belong in ergon.

## 1. Position in the 7-layer harness stack

Ergon occupies **Layer 2** of the agent-harness stack documented in
`docs/architecture/agent-harness.md`. More precisely, the `ergon` crate
supplies the *workflow shape* of tick body at Layer 3.5:

```text
L6 — Process entry points         (arcan, arcand, lifed, lifegw)
L5 — Session orchestration        (arcand::ConsciousnessActor)
L4 — Tick engine                  (aios_runtime::KernelRuntime)
L3.5 — Tick body                  ← ergon supplies one of two shapes
        ├─ Direct (one call via ModelProviderPort)
        └─ Workflow (ergon::Workflow::execute)              ← THIS
L3 — Port traits                  (aios-runtime)
L2 — Substrate adapters           ← ergon-life-hooks + ergon-life-sinks
                                    + arcan-ergon live here
L1.5 — Silicon contract           (Spec E, in design)
L1 — Substrate primitives         (lago, praxis, anima, autonomic, nous,
                                    life-vigil)
L0 — Kernel contract              (aios-protocol)
```

**Critical invariant**: ergon does NOT replace `KernelRuntime`. It does
NOT replace `arcan-harness` (which isn't the harness anyway — it's a
~300 LOC tool/sandbox utility crate). It does NOT compete with the
tick engine. Workflows are **bounded operations that run inside one
kernel tick**.

The original framing in `2026-05-05-ergon-v0.1.md` §6 + §10 ("ergon
replaces arcan-harness over time") was *wrong* on two counts:

1. `arcan-harness` was never the agent harness.
2. Ergon's `Workflow::execute()` cannot replace the long-horizon tick
   engine without losing per-tick journal traceability that autonomic
   / EGRI / branching depend on.

The corrected framing — documented in
`2026-05-08-bro-1001-ergon-tick-body.md` — is that **ergon supplies
one shape of tick body alongside the existing direct-call tick body**.
Both compose with the kernel; both produce per-tick events; neither
replaces anything.

## 2. Crate taxonomy

Ergon ships as **three crates** at `crates/ergon/`:

| Crate | What it owns | Substrate coupling |
|---|---|---|
| `ergon` | Trait surface — `Workflow`, `Step`, `StepCtx`, `Hook`, `StreamSink`, `Role`, `Provider`, `ToolRegistry`, `RuntimeHandle`. Canonical `StreamEvent` taxonomy. Authored-agent interpreter (`Agent`, `AgentSpec`, `TypedAgent`). | **None.** Vendor-neutral; zero deps on `lago-journal`, `life-vigil`, `praxis-*`, `arcan-*`, `anima-*`, `autonomic-*`, `nous-*`. |
| `ergon-life-hooks` | Four auto-registered hooks bridging ergon to Life substrate: `PraxisCapabilityHook`, `AutonomicBudgetHook`, `NousScoreHook`, `AnimaAttestHook`. Each hook ships an *adapter trait* implemented by the runtime. | **Adapter traits only.** Substrate calls go through traits that the runtime (arcan-ergon) implements; the hooks themselves stay portable. |
| `ergon-life-sinks` | Three `StreamSink` impls: `LagoSink` (durable replay), `VigilSink` (observability), `LifegwSink` (user-facing SSE). | **Life-coupled.** Depends on `lago-core::Journal`, `tracing`, `tokio::sync::mpsc`. **No** vigil/arcan/praxis/anima/autonomic/nous deps. |

The adapter that wires all three crates into arcan's tick body lives at
`crates/arcan/arcan-ergon/` (sibling crate cluster — adapter, not
harness).

## 3. Trait taxonomy — public API

### 3.1 `Workflow` — the unit of authoring

```rust
#[async_trait]
pub trait Workflow: Send + Sync + 'static {
    type Input: DeserializeOwned + Send + Sync;
    type Output: Serialize + Send + Sync;

    fn name(&self) -> &str;
    fn role(&self) -> Role;
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
    fn sandbox_policy(&self) -> Arc<SandboxPolicy>;

    async fn execute(
        &self,
        ctx: &mut StepCtx<'_>,
        input: Self::Input,
    ) -> Result<Self::Output, ErgonError>;
}
```

A `Workflow` is **a bounded multi-turn operation**. Its `execute`
function is plain async Rust whose body delegates to autonomous
inference + tool dispatch via `StepCtx`. The kernel runs it as the
contents of a single tick.

### 3.2 `Step` + `StepCtx` — the autonomous inner loop

`StepCtx` is the runtime handle a workflow uses to delegate to the
model and tools. It exposes:

- `run_inference_streaming(req: InferenceRequest)` — calls the
  configured `Provider`, emits `StreamEvent` to the configured
  `StreamSink`, returns a `ModelResponse`.
- `dispatch_tool(call: ToolCall)` — runs one tool via the configured
  `ToolRegistry`.
- `swap_scope(role: Role)` — pushes a transient role overlay for a
  sub-section of the workflow (call-scope > session-scope >
  agent-scope precedence; never persisted).
- `runtime()` — narrowed projection of the underlying
  `arcan_core::TickHandle` (`operating_mode()` only in v0.1; other
  methods land via deliberate boundary expansion).

`Step` is the autonomous loop primitive (used internally by
`run_inference_streaming` to loop model → tool → model until the
model issues a terminal response).

### 3.3 `Hook` — cross-cutting policy / observability

```rust
#[async_trait]
pub trait Hook: Send + Sync + 'static {
    async fn on_workflow_start(&self, ctx: &mut HookCtx<'_>) -> HookOutcome { Ok(Continue) }
    async fn on_workflow_end(&self, ctx: &mut HookCtx<'_>) -> HookOutcome { Ok(Continue) }
    async fn on_step_start(&self, ctx: &mut HookCtx<'_>) -> HookOutcome { Ok(Continue) }
    async fn on_step_end(&self, ctx: &mut HookCtx<'_>) -> HookOutcome { Ok(Continue) }
    async fn on_inference_start(&self, ctx: &mut HookCtx<'_>) -> HookOutcome { Ok(Continue) }
    async fn on_inference_end(&self, ctx: &mut HookCtx<'_>) -> HookOutcome { Ok(Continue) }
    async fn on_tool_start(&self, ctx: &mut HookCtx<'_>) -> HookOutcome { Ok(Continue) }
    async fn on_tool_end(&self, ctx: &mut HookCtx<'_>) -> HookOutcome { Ok(Continue) }
}
```

All eight events default to `Continue` (deliberate deviation from spec
§3.7 — see `crates/ergon/ergon/CLAUDE.md` §"Spec deviations"). The four
production hooks live in `ergon-life-hooks`; the registry is built by
the arcan adapter at session boundary.

### 3.4 `StreamSink` + `StreamEvent` — the event-stream contract

`StreamEvent` is a 19-variant enum covering the canonical workflow
event taxonomy:

| Category | Variants |
|---|---|
| Workflow lifecycle | `WorkflowStart`, `WorkflowEnd` |
| Step lifecycle | `StepStart`, `StepEnd` |
| Inference | `InferenceStart`, `InferenceChunk`, `InferenceEnd` |
| Tools | `ToolStart`, `ToolChunk`, `ToolEnd` |
| Role | `RolePushed`, `RolePopped` |
| Errors | `Error`, `Cancelled` |
| Hook outcomes | `HookEmitted` |
| Audit | `BudgetSnapshot`, `ScoreSnapshot`, `AttestationSnapshot` |
| Forward-compat | `VendorEvent { tag, payload }` |

**Stability commitment**: `StreamEvent` variants are append-only after
v1.0. Consumers MUST handle `VendorEvent` for forward compatibility.

Default sink impls in `ergon`:
- `BufferSink` — collects events in memory (tests).
- `FanoutSink` — composes multiple sinks; first error short-circuits.

Production sink impls in `ergon-life-sinks`:
- `LagoSink` — appends `EventKind::Custom("ergon.stream", payload)` to
  the lago journal. Durable; errors propagate.
- `VigilSink` — emits `tracing::info!` on the current span. Infallible.
- `LifegwSink` — sends to a `tokio::sync::mpsc::Sender<StreamEvent>`
  consumed by lifegw's SSE encoder. Surfaces backpressure as
  `StreamClosed`.

**Recommended fanout order**: durable (Lago) → observability (Vigil) →
user-facing (Lifegw). A client-side disconnect cannot lose journal
events.

### 3.5 `Role` + `RoleScope` — system-prompt precedence

A workflow declares its agent-scope role via `Workflow::role()`. A
session can override with `Role::session_scope(...)`. A call can
override either via `StepCtx::swap_scope(...)`. Precedence is
**call > session > agent**, applied transiently at `ModelRequest`
build time. Roles are **never persisted in session history** — inserting
a role into history breaks the precedence rule.

### 3.6 `Provider` + `ToolRegistry` + `RuntimeHandle` — runtime extension points

Ergon owns these traits rather than re-exporting `arcan_provider::Provider`
or `praxis_core::ToolRegistry`. Rationale: hook signatures depend on
these types; coupling them to substrate crates would ripple every
substrate change through every hook. The arcan adapter implements
`ergon::Provider` over `arcan_provider::Provider` and
`ergon::ToolRegistry` over `praxis_core::ToolRegistry` at the boundary.

### 3.7 Authored agents (BRO-1007)

Ergon also ships an **authored-agent substrate**: markdown files at
`core/life/agents/*.md` carry frontmatter (input schema, role, tools,
sandbox policy) and a body that becomes the workflow execute logic.

- `Agent` — runtime trait (yields a workflow at registration).
- `AgentSpec` — parsed frontmatter shape (gray_matter + jsonschema).
- `run_spec(...)` — interpreter that compiles a spec to a workflow.
- `TypedAgent<I, O>` — typed sibling with auto-`Agent` impl;
  `AgentStep` / `TypedAgentStep` wrappers compose typed agents inside
  steps.

This unlocks the *agents-as-data* pattern: production workflows can be
authored as markdown without recompiling the workspace, and the same
trait surface accepts both Rust-coded and markdown-authored agents.

## 4. Lifecycle — what one workflow tick looks like

```text
arcand::ConsciousnessActor decides to tick
  ↓
KernelRuntime::tick_on_branch(tick_ctx)
  ↓
arcan-ergon adapter (Tick body = Workflow shape)
  ├─ resolve Workflow by AgentKind
  ├─ build StepCtx from TickCtx (provider, tools, sandbox, runtime,
  │   hook registry with 4 auto-hooks attached, sink fanout
  │   [LagoSink → VigilSink → LifegwSink])
  ├─ WorkflowExecutor::run(workflow, ctx, input)
  │    fires `WorkflowStart` event → registry → 4 hooks fire
  │    workflow.execute(ctx, input):
  │      loop:
  │        ctx.run_inference_streaming(req)
  │          emit InferenceStart → 4 hooks
  │          model streams chunks → emit InferenceChunk
  │          model issues tool calls? → ctx.dispatch_tool(...) → emit
  │            ToolStart / ToolChunk / ToolEnd
  │          model returns terminal response → emit InferenceEnd
  │        if step complete: break
  │      return Output
  │    fires `WorkflowEnd` event → 4 hooks fire
  └─ TickOutput { payload: Output, mode_after: ... }
  ↓
KernelRuntime persists TickCompleted event + workflow's stream events
to lago journal via EventStorePort
  ↓
ConsciousnessActor re-ticks if mode == Execute
```

**Outer loop scope** (session, weeks): owned by L5 + L4 — what runs
across days, replayable from the journal.

**Inner loop scope** (one tick, seconds-to-minutes): owned by L3.5 (the
workflow body) — what runs *inside* one tick.

Both scopes produce replayable events; neither replaces the other.

## 5. Integration with Life

### 5.1 The `arcan-ergon` adapter

`crates/arcan/arcan-ergon/` implements:

- `ErgonAgentKind<W>` — registers as an `arcan_core::AgentKind` so
  arcand can dispatch a workflow as a tick body.
- The four hook *adapter traits* from `ergon-life-hooks` over real
  substrate (praxis-core / autonomic / nous / anima).
- A `BoxedProvider` and `BoxedToolRegistry` that adapt
  `arcan_provider::Provider` and `praxis_core::ToolRegistry` to ergon's
  trait surface.
- The `StepCtx` builder that reads `TickCtx`, attaches the 4 auto-hooks
  + the 3 sink fanout, and hands the `StepCtx` to
  `WorkflowExecutor::run`.

This is the adapter; it is NOT the harness. Replace it with a
different runtime and ergon still works — that is the abstraction
guarantee.

### 5.2 lifed route — `Agent.StreamSession` ([BRO-1002](https://linear.app/broomva/issue/BRO-1002), in flight)

`crates/life-runtime/lifed/src/route/ergon.rs` exposes a tonic-web
`Agent.StreamSession` RPC:

```protobuf
service Agent {
  rpc StreamSession(StreamSessionRequest) returns (stream StreamSessionResponse);
}
```

Lifed is responsible for:

1. Translating the JWT-scoped request into an `arcan_core::TickCtx`
   with proper capability tokens.
2. Looking up the `ErgonAgentKind<W>` from the workflow registry by
   `agent` name.
3. Spawning the arcan tick in a tokio task.
4. Forwarding the `LifegwSink` mpsc → gRPC stream.
5. Closing the upstream stream on client disconnect (cancellation
   through `tokio::select!`).

When BRO-1002 merges, this section grows a "concrete RPC shape" link
back to the implementation file.

### 5.3 First production workflow — bookkeeping-judge ([BRO-1003](https://linear.app/broomva/issue/BRO-1003), in flight)

`core/life/apps/bookkeeping-judge/` ports the existing Bellows-shipped
Nous-gate evaluator to an ergon `Workflow`. The Bellows binary stays
canonical until parity is proven (≥0.95 LLM-judged similarity over 5
raw extracts from `research/notes/`).

This is the spec's primary surface-correctness validation: if the
ergon port produces identical verdicts to the Bellows version on
identical inputs, the abstraction is correct. If not, the abstraction
is wrong and v0.1 holds until fixed.

When BRO-1003 merges, this section grows a "first production
workflow" link back to the parity test.

## 6. Delegation map — what ergon does NOT own

| Concern | Owned by | Why ergon doesn't own it |
|---|---|---|
| Long-horizon agent loop | `aios-runtime::KernelRuntime` | Workflows are *one tick body shape* — the kernel still drives the outer loop. |
| Session state / mailbox / queue | `arcand::ConsciousnessActor` (L5) | Session orchestration is above ergon. |
| Event journal storage | `lago-core::Journal` + `EventStorePort` | Persistence is an L1 substrate, not a harness concern. |
| Tool execution / sandbox | `praxis-core` + `praxis-tools` | Ergon defines the `ToolRegistry` *trait*; impls are praxis. |
| Identity / capability tokens | `anima-core` + `anima-identity` | Capability gating is an auto-hook concern (`PraxisCapabilityHook`). |
| Budget / homeostasis | `autonomic-core` + `autonomic-runtime` | Budget gating is an auto-hook concern (`AutonomicBudgetHook`). |
| Post-execution scoring | `nous-core` + `nous-judge` | Scoring is an auto-hook concern (`NousScoreHook`). |
| Soul attestation | `anima-lago` | Attestation is an auto-hook concern (`AnimaAttestHook`). |
| Silicon / KV / spec-decode | `inference-core` (Spec E / BRO-1019, in design) | Silicon contract is Layer 1.5 — beneath ergon's port traits. |
| Authentication / TLS / JWKS | `lifegw` | Edge gateway concerns are above lifed. |
| Subagent / handoff | Deferred to v0.2 (pneuma will likely host) | Out of scope for v0.1. |
| Resume from event | `WorkflowExecutor::resume_from_event` (v0.2) | Surface is reserved; impl deferred. |

## 7. Known gaps in v0.1

### 7.1 `ergon-life-sinks` consumer wiring (workflow-tick blind spot)

`ergon-life-sinks` ships three sinks, but the **workflow-tick path
does not yet plumb them**. The current arcan-ergon adapter constructs
a `BufferSink` for unit tests; production sink fanout is wired only
for the direct tick body. As a result:

- Workflow tick stream events never reach the lago journal — replay
  of a workflow-bodied tick reconstructs the tick boundary but not
  the inner step / inference / tool events.
- `lago replay --tree` sees workflow-bodied ticks as opaque.
- Vigil traces for workflow ticks rely on the workflow body's own
  `tracing::info_span!` calls, not on `VigilSink`.

Tracked at `research/entities/concept/workflow-tick-stream-blind-spot.md`.
~30 LOC fix at the arcan-ergon adapter (replace the `BufferSink`
construction with the production fanout). Targeted for a Tier-A
follow-up wave after v0.1 closes.

### 7.2 Hook auto-registration coverage (3 of 4 Noop on workflow path)

The arcan-ergon adapter wires `NousScoreHook` for workflow ticks, but
the other three (`PraxisCapabilityHook`, `AutonomicBudgetHook`,
`AnimaAttestHook`) auto-register as `NoopHookAdapter` on the workflow
path — the Workflow tick body bypasses budget / capability /
attestation gating, while the direct tick body honors all four.

Tracked at `research/entities/concept/hook-adapter-noop-gap.md`. Fix
is to lift the four adapter constructions from arcan-core's tick body
into the shared `StepCtx` builder. Targeted for the same follow-up
wave as §7.1.

### 7.3 Authored-agent runtime test against fixtures

The `Agent` / `AgentSpec` interpreter has unit tests but no end-to-end
test that loads `core/life/agents/*.md` files and runs them as
workflows. The fixture pack lands with `bookkeeping-judge` (BRO-1003)
since that crate is the first authored-agent consumer.

## 8. References

- **Spec — Ergon v0.1**: `core/life/docs/superpowers/specs/2026-05-05-ergon-v0.1.md`
- **Spec — Tick body adapter**: `core/life/docs/superpowers/specs/2026-05-08-bro-1001-ergon-tick-body.md`
- **Architecture — Agent Harness**: `core/life/docs/architecture/agent-harness.md`
- **Crate CLAUDE files**:
  - `crates/ergon/ergon/CLAUDE.md` — core crate invariants + spec deviations
  - `crates/ergon/ergon-life-hooks/CLAUDE.md` — auto-hook architecture
  - `crates/ergon/ergon-life-sinks/CLAUDE.md` — sink failure tiers
- **Knowledge-graph entities**:
  - `research/entities/concept/workflow-tick-stream-blind-spot.md`
  - `research/entities/concept/hook-adapter-noop-gap.md`
  - `research/entities/pattern/authored-agents-as-data.md`
- **Spec E (sibling track)**: `core/life/docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md`
- **Linear**:
  - [BRO-994 — Ergon v0.1 umbrella](https://linear.app/broomva/issue/BRO-994)
  - [BRO-1002 — lifed route](https://linear.app/broomva/issue/BRO-1002) (parallel-dispatch, may not be merged when this doc lands)
  - [BRO-1003 — bookkeeping-judge port](https://linear.app/broomva/issue/BRO-1003) (parallel-dispatch, may not be merged when this doc lands)
  - [BRO-1004 — this CI + CHANGELOG + doc PR](https://linear.app/broomva/issue/BRO-1004)
