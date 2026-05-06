# Changelog

## Unreleased

### Added
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
