# Nous

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)
[![docs](https://img.shields.io/badge/docs-broomva.tech-purple.svg)](https://docs.broomva.tech/docs/life/nous)

**Metacognitive evaluation module for the Life Agent OS** -- the "Pepe Grillo" that judges agent behavior in real time, feeding quality signals back into homeostatic control loops.

Nous (Greek: mind, intellect) provides a hybrid evaluation architecture: fast inline heuristics (< 2ms) that run as Arcan middleware, plus optional async LLM-as-judge evaluators for deeper quality assessment.

## Architecture

```
                    +------------------+
                    |   Arcan (Agent)  |
                    +--------+---------+
                             |
              +--------------+--------------+
              |                             |
     +--------v---------+         +--------v---------+
     | nous-middleware   |         |    nous-api      |
     | (Arcan Middleware)|         |  (HTTP daemon)   |
     +--------+---------+         +--------+---------+
              |                             |
    +---------+---------+         +---------+---------+
    |                   |         |                   |
+---v---+         +-----v---+  +-v--------+    +-----v----+
| nous- |         | nous-   |  | nous-    |    | nous-    |
| heur- |         | core    |  | judge    |    | lago     |
| istics|         | (types) |  | (LLM)   |    | (events) |
+-------+         +---------+  +----------+    +----------+
                                                     |
                                              +------v------+
                                              |    Lago     |
                                              +-------------+
```

### Hybrid Deployment

| Path | Latency | Where | What |
|------|---------|-------|------|
| Inline heuristics | < 2ms | Arcan middleware hooks | Token efficiency, budget adherence, tool correctness, safety compliance |
| Async LLM-as-judge | 1-10s | `nousd` daemon | Plan quality, plan adherence, task completion, reasoning depth |

### 5 Evaluator Layers

1. **Safety** -- Does the action violate policy constraints?
2. **Correctness** -- Are tool arguments valid? Are results well-formed?
3. **Efficiency** -- Token usage, step count, budget adherence
4. **Quality** -- Plan coherence, reasoning depth, task completion
5. **Alignment** -- Does behavior match the agent's soul and beliefs (Anima)?

## Crates

| Crate | Purpose |
|-------|---------|
| `nous-core` | Types, traits, errors (zero I/O). `NousEvaluator` trait, `EvalScore`, `EvalResult`, `EvalLayer` taxonomy |
| `nous-heuristics` | Inline evaluators: token efficiency, budget adherence, tool correctness, argument validity, safety compliance, step efficiency |
| `nous-middleware` | `NousMiddleware: impl arcan_core::Middleware` -- wires evaluators into the Arcan agent loop |
| `nous-judge` | Async LLM-as-judge: plan quality, plan adherence, task completion |
| `nous-lago` | Lago persistence bridge for eval events (`eval.*` namespace) |
| `nous-api` | HTTP API (axum): `/eval/{session}`, `/eval/run` |
| `nousd` | Daemon binary |

## Quick Start

```bash
# Run the evaluation daemon
cargo run -p nousd

# With Lago persistence
cargo run -p nousd -- --lago-data-dir /path/to/data
```

## Integration with Autonomic

Nous feeds evaluation scores into the Autonomic homeostasis controller:

```
Inline heuristics --> direct fold into Autonomic state (synchronous)
Async LLM judge  --> Lago events --> Autonomic subscription (asynchronous)
```

When quality scores drop below thresholds, Autonomic can trigger recovery actions: switching operating modes, adjusting budgets, or requesting human intervention.

## Event Namespace

All events use `EventKind::Custom` with prefix `"eval."`:

- `eval.heuristic_result` -- inline evaluator output
- `eval.judge_result` -- LLM-as-judge output
- `eval.score_aggregated` -- combined score across layers

Scores are OTel-aligned: emitting `gen_ai.evaluation.result` span events for observability via Vigil.

## Build and Test

```bash
# Full verification
cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace

# Run tests only
cargo test --workspace

# Build release
cargo build --workspace --release
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

## Documentation

Full documentation: [docs.broomva.tech/docs/life/nous](https://docs.broomva.tech/docs/life/nous)

## License

[MIT](LICENSE)
