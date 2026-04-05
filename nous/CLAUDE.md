# Nous — Metacognitive Evaluation Module

**Version**: 0.1.0 | **Status**: Phase N0 (Foundation)
**Tests**: See each crate | **Rust**: edition 2024, MSRV 1.85

Real-time quality evaluation layer for the Life Agent OS. The "Pepe Grillo" — judges agent behavior as it runs, feeding quality signals back into homeostatic control loops.

## Architecture

Hybrid deployment: embedded inline heuristics (< 2ms, Arcan middleware hooks) + optional async LLM-as-judge evaluators (`nousd` daemon).

## Crates

- `nous-core` — Types, traits, errors (zero I/O). `NousEvaluator` trait, `EvalScore`, `EvalResult`, `EvalLayer` taxonomy
- `nous-heuristics` — Inline evaluators: token efficiency, budget adherence, tool correctness, argument validity, safety compliance, step efficiency
- `nous-middleware` — `NousMiddleware: impl arcan_core::Middleware` — wires evaluators into agent loop
- `nous-judge` — Async LLM-as-judge: plan quality, plan adherence, task completion
- `nous-lago` — Lago persistence bridge for eval events
- `nous-api` — HTTP API (axum): `/eval/{session}`, `/eval/run`
- `nousd` — Daemon binary

## Build & Verify

```bash
cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace
```

## Dependency Order

```
aios-protocol (canonical contract)
    |
nous-core (types + traits, zero I/O)
    |           \              \
nous-heuristics  nous-judge    nous-lago (+ lago-core, lago-journal)
    |           /
nous-middleware (+ arcan-core)
    |
nous-api (axum)
    |
nousd (binary)
```

## Critical Patterns

- Inline evaluators must complete in < 2ms (no I/O, no allocations in hot path)
- Eval events use `EventKind::Custom` with `"eval."` prefix (following autonomic pattern)
- `EvalScore` is OTel-aligned: emits `gen_ai.evaluation.result` span events
- Dual score flow: inline → direct Autonomic fold; async → Lago → Autonomic subscription
- No circular deps: `nous-core` depends only on `aios-protocol`

## Rules

- **Formatting**: `cargo fmt` before every commit
- **Linting**: `cargo clippy --workspace -- -D warnings`
- **Testing**: All new code requires tests; `cargo test --workspace` must pass
- **Safe Rust**: No `unsafe`
- **Error handling**: `thiserror` for libraries, `anyhow` for binaries
- **Module style**: `name.rs` file-based modules (not `mod.rs`)
