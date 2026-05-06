# Changelog

## Unreleased

### Added
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
