# Changelog

## Unreleased

### Added
- `ergon-life-sinks` — new sibling crate at
  `crates/ergon/ergon-life-sinks/` housing three Life-flavored
  implementations of `ergon::StreamSink`:
  * `LagoSink` — durable replay via `lago_core::Journal`. Each
    `StreamEvent` becomes an `aios_protocol::EventKind::Custom` event
    with `event_type = "ergon.stream"` and the full event JSON in
    `data`. Failures propagate as `ErgonError::Internal` (durable
    replay critical).
  * `VigilSink` — emits each `StreamEvent` as a structured
    `tracing::info!` (or `warn!` for `Error` variants) on the current
    span, target `"ergon::stream"`. Despite the name it does NOT
    depend on `life-vigil` — vigil configures the subscriber, not the
    emitter. Infallible.
  * `LifegwSink` — bounded `tokio::sync::mpsc` forwarder; consumer
    disconnect surfaces as `ErgonError::StreamClosed` and propagates
    cancellation. Default capacity 64 per spec §3.10.
  19 unit tests across the three sinks (mock `Journal` impl for Lago,
  in-memory event-flow tests for Vigil, send/receive + capacity +
  closed-receiver tests for Lifegw). Crate dependency surface: only
  `ergon`, `aios-protocol`, `lago-core`, plus standard async/serde —
  no `arcan-*`, no `praxis-*`, no `anima-*`, no `autonomic-*`, no
  `nous-*`, no `life-vigil`. Failure-semantics tiers documented in
  `crates/ergon/ergon-life-sinks/CLAUDE.md`. (BRO-999b)
- `ergon-life-hooks` — new sibling crate at `crates/ergon/ergon-life-hooks/`
  housing the four "Life-native" auto-registered hooks (spec §3.8):
  `PraxisCapabilityHook` (`on_pre_tool_use`),
  `AutonomicBudgetHook` (`on_pre_inference`),
  `NousScoreHook` (`on_post_inference`),
  `AnimaAttestHook` (`on_workflow_start` / `on_workflow_end`).
  Each hook is paired with a small **adapter trait**
  (`CapabilityResolver`, `BudgetGate`, `ResponseScorer`, `SoulAttester`)
  the hook consumes via `Arc<dyn _>` at construction. The arcan
  adapter (BRO-1001) implements those traits against `aios_protocol::PolicySet`,
  `autonomic::AutonomicGatingProfile`, `nous_core::NousEvaluator`,
  and `anima_core::AgentSoul`.
  Architectural win: the crate has **zero substrate dependencies** —
  Cargo.toml lists only `ergon` + standard async/serde. Substrate dep
  lives in BRO-1001's adapter, where it belongs. 15 unit tests across
  the four modules (mocked adapters); `ergon` itself unchanged.
  Failure semantics differ by hook: capability and budget denials are
  hard veto (`Deny` outcome); score and attestation failures are
  observe-only (`tracing::warn!` + `Continue`) — design rationale
  documented in `crates/ergon/ergon-life-hooks/CLAUDE.md`.
  Spec deviation: this crate replaces the spec's "auto-hooks live in
  ergon itself" placement (§3.8). Reason: ergon stays vendor-neutral;
  Life-specific governance hooks belong in their own crate so a future
  ergon consumer (TS port, alternate agent OS) can ship its own
  governance set without forking. (BRO-1000)
- `ergon` — Layer-2 agent-harness primitive: workflow trait + executor.
  Ships the fourth slice per spec §12: `workflow` module (`Workflow`
  trait — the user-implementation surface with typed Input/Output;
  `WorkflowExecutor` — the driver that fires `on_workflow_start` /
  `on_workflow_end` hooks around `Workflow::execute`; placeholder
  `SkillSet` trait + `EmptySkillSet` impl until praxis-skills wiring
  in BRO-1001). Ten new unit tests cover: empty-hooks execution,
  start-hook deny short-circuiting, end-hook firing on execute error,
  end-hook errors not overriding workflow result, sequential first-deny
  short-circuit, default skill-set behaviour. Three more spec deviations
  documented in `crates/ergon/ergon/CLAUDE.md`: `WorkflowExecutor`
  doesn't auto-register hooks (caller passes pre-built `StepCtx`,
  matching the BRO-1000 separation); `without_default_hooks` flag
  dropped (no longer meaningful); `SkillSet` placeholder trait until
  BRO-1001 wires real praxis-skills. Result: ergon compiles and passes
  69 unit tests with **still zero substrate dependencies**. 59 → 69
  tests. (BRO-999)
- `ergon` — Layer-2 agent-harness primitive: autonomous loop body.
  Ships the third slice per spec §12: `runtime` module (`Provider`,
  `ToolRegistry`, `RuntimeHandle` traits — the substrate-independent
  seam between ergon and the host runtime; production impls translate
  these to `arcan_provider::Provider` / `praxis_core::ToolRegistry` /
  `arcan_core::TickCtx` in BRO-1001); `step` module (`Step` trait,
  `StepCtx` orchestration arena, `InferenceRequest` builder,
  `DEFAULT_INFERENCE_MAX_TURNS` constant, `run_inference_streaming`
  autonomous loop body that fires every Hook event per spec §5,
  sequential tool dispatch, max-turn budget enforcement). Three more
  spec deviations documented in `crates/ergon/ergon/CLAUDE.md`:
  Provider/ToolRegistry are ergon-owned (not arcan_provider /
  praxis_core direct deps); StepCtx fields trimmed (`journal`,
  `homeostasis`, `soul`, `skills`, `sandbox` cut — moved to auto-hook
  construction or `ToolRegistry` impl internals); `RuntimeHandle`
  narrowed to just `operating_mode()` for v0.1. Result: ergon compiles
  and passes 14 new loop-body unit tests (mocked Provider, ToolRegistry,
  RuntimeHandle, plus BufferSink) with **zero substrate dependencies**.
  43 → 59 tests. (BRO-998)
- `ergon` — Layer-2 agent-harness primitive: hook lifecycle + wire types.
  Ships the second slice per spec §12: `model` module (provider-agnostic
  wire types — `Message` with block-structured content, `ContentBlock`
  enum with Text/Reasoning/ToolUse/ToolResult variants, `ToolCall`,
  `ToolResult`, `ToolDefinition`, `ModelRequest`, `ModelResponse`,
  `Usage`); `hook` module (8-event `Hook` trait, `HookCtx`,
  `HookRegistry`, `HookOutcome` / `ToolHookOutcome` / `InferenceHookOutcome`).
  Hooks observe / deny / stub at workflow / step / inference / tool seams.
  Ergon owns these wire types so the hook contract is decoupled from any
  specific provider crate; `step.rs` (BRO-998) translates between ergon's
  shapes and `arcan_provider` / `praxis_core`. Spec deviation: all 8
  events default to `Ok(_::Continue)` (spec §3.7 only defaulted
  `on_workflow_start`); rationale documented in
  `crates/ergon/ergon/CLAUDE.md`. 18 new unit tests; 43 total. (BRO-997)
- `ergon` — new Layer-2 agent-harness primitive crate. First slice ships
  the foundational trait surface with no Life-substrate dependencies:
  `ErgonError` + `Result` (`error`), `Role` + `RoleScope` with
  call > session > agent precedence (`role`), and `StreamEvent` canonical
  taxonomy + `StreamSink` trait + `BufferSink` + `FanoutSink` (`stream`).
  25 unit tests pass. Spec at
  `docs/superpowers/specs/2026-05-05-ergon-v0.1.md`. Tracker BRO-994.
  Note: spec lists `license = "Apache-2.0"`; the crate uses
  `license.workspace = true` (MIT) for monorepo coherence — life is
  MIT-licensed throughout. (BRO-995, BRO-996)
- `arcan-prosopon` — Pneuma<L0ToExternal> bridge. Subscribes to KernelRuntime
  events, translates to ProsoponEvents, publishes to prosopon-daemon fanout.
  Opt-in via `cargo run -p arcan --features prosopon --prosopon-port <addr>`.
  (BRO-773)

## 0.2.0

- 1077/1077 tests passing, 37 crates, ~43K LOC
- Arcan agent runtime v0.2.1 with full agent loop
- Lago event-sourced persistence v0.2.1
- Autonomic homeostasis controller v0.1.0
- Haima agentic finance engine v0.1.0
- Praxis tool execution sandbox with MCP bridge
- Spaces distributed networking (SpacetimeDB 2.0)
- Vigil OpenTelemetry observability foundation
- Rust 2024 Edition (MSRV 1.85) across all projects
