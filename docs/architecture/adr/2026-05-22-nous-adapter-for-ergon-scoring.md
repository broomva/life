# ADR — Nous adapter for Ergon scoring (`ResponseScorer` ↔ Nous registry)

- **Status**: Proposed
- **Date**: 2026-05-22
- **Linear**: [BRO-1225](https://linear.app/broomva/issue/BRO-1225/ergon-phase-2-gap-2b-nous-adapter-design-responsescorer)
- **Parent**: [BRO-994](https://linear.app/broomva/issue/BRO-994) (Ergon v0.1 umbrella, Done)
- **Replaces**: nothing (greenfield design)
- **Scope**: paper-only design + minimal trait skeleton; the implementation lands in a follow-up

## Context

Ergon's response-scoring hook lives at `crates/ergon/ergon-life-hooks/src/score.rs` and is shaped around a single trait:

```rust
#[async_trait]
pub trait ResponseScorer: Send + Sync {
    async fn score(
        &self,
        response: &ModelResponse,
    ) -> std::result::Result<serde_json::Value, String>;
}
```

Nous, the metacognitive-evaluator substrate at `crates/nous/`, exposes a different shape. The actual `nous-core` API is:

```rust
// crates/nous/nous-core/src/evaluator.rs
pub trait NousEvaluator: Send + Sync {
    fn name(&self) -> &str;
    fn layer(&self) -> EvalLayer;
    fn timing(&self) -> EvalTiming;
    fn evaluate(&self, ctx: &EvalContext) -> NousResult<Vec<EvalScore>>;
}

// crates/nous/nous-core/src/registry.rs
pub struct EvaluatorRegistry { /* ... */ }
impl EvaluatorRegistry {
    pub fn evaluators_for(&self, hook: EvalHook) -> &[Arc<dyn NousEvaluator>];
}
```

The two surfaces differ on **input shape** (`&ModelResponse` vs `&EvalContext`), **output shape** (`serde_json::Value` vs `Vec<EvalScore>`), and **selection model** (one scorer per hook vs N evaluators per hook). The Ergon Phase 2 §Gap #2b ticket frames the adapter as the bridge.

> **Correction to the ticket's framing**: BRO-1225 references `EvaluatorRegistry::evaluate(&EvalContext)` as the Nous surface. As of `e893deba` (life main) the registry has no `evaluate` method directly — it has `evaluators_for(hook) -> &[Arc<dyn NousEvaluator>]`, and `evaluate` is on the individual evaluator. The adapter has to iterate. This ADR uses the actual code shape.

## Decisions

The five design questions named in BRO-1225, each with a one-sentence justification.

### 1. Wrapping direction

**Decision**: The adapter lives on the **ergon side**. Ergon's `ResponseScorer` impl wraps an `Arc<EvaluatorRegistry>` (or a thin handle to one) and translates per-call into the Nous `EvalContext` + N-evaluator-fanout shape.

**Justification**: Coupling stays one-directional. Nous remains a generic evaluator substrate with no Ergon awareness (no `ModelResponse` import, no Ergon hook semantics, no Anthropic-shape leaking into `nous-core`). The adapter is the only place that knows both sides.

**Adapter crate location**: a **new crate** `crates/ergon/ergon-nous-adapter/`. Not `ergon-life-hooks/src/nous_adapter.rs` — that would force `ergon-life-hooks` to depend on `nous-core`, and we want hooks decoupled from metacognition (a deployment without Nous should still get hooks). The new crate's `Cargo.toml` declares `ergon`, `nous-core`, `async-trait`, `serde_json`, `tracing`. Re-exports the impl from the crate root.

### 2. `EvalContext` construction

**Decision**: The adapter builds an `EvalContext` per-call from `(&HookCtx, &ModelResponse)`. Required fields: `session_id`, `run_id` (workflow run), `iteration` (current step index). Token fields from `response.usage`. Tool fields and knowledge fields left `None` at this layer (they're populated upstream by other hooks; Nous treats `None` as "not measured"). `metadata` carries the workflow name as `workflow_name` and the model name as `model`.

```rust
fn build_ctx(ctx: &HookCtx<'_>, response: &ModelResponse) -> EvalContext {
    let mut meta = HashMap::new();
    meta.insert("workflow_name".into(), ctx.workflow_name.to_string());
    if let Some(model) = response.model.as_ref() {
        meta.insert("model".into(), model.clone());
    }
    EvalContext {
        session_id: ctx.session_id.to_string(),
        run_id: ctx.run_id.map(|s| s.to_string()),
        iteration: ctx.iteration,
        input_tokens: response.usage.as_ref().and_then(|u| u.input_tokens),
        output_tokens: response.usage.as_ref().and_then(|u| u.output_tokens),
        // tool_* / knowledge_* left None — populated by other hooks if at all
        metadata: meta,
        ..EvalContext::new(ctx.session_id.to_string())
    }
}
```

**Justification**: `EvalContext` is rich and intentionally optional. The adapter doesn't have ground-truth for tool/knowledge fields — populating them with `None` is honest. The metadata HashMap is the escape hatch for adapter-specific tags without bloating the public struct.

### 3. Result reduction

**Decision**: The adapter does **not aggregate**. It calls every evaluator registered for `EvalHook::AfterModelCall`, collects their `Vec<EvalScore>` outputs, and returns the full vec as a JSON array — preserving per-evaluator detail.

```jsonc
// Example return value
[
  {"evaluator":"token_efficiency","value":0.82,"label":"good","layer":"execution",...},
  {"evaluator":"response_coherence","value":0.91,"label":"good","layer":"reasoning",...}
]
```

**Justification**: Ergon's `ResponseScorer::score` returns `serde_json::Value`. The `NousScoreHook` already logs the value as a tracing field and emits it on the trace span — downstream consumers (lago events, OTel `gen_ai.evaluation.result` span events, the bookkeeping judge) do their own reduction. Aggregation policy is a consumer concern; the adapter shouldn't impose `mean`/`max`/`weighted-sum` choices that lose information. The full vector preserves OTel-alignment (every `EvalScore` already serializes as one span-event payload).

A future v0.2 could add an `--aggregate` flag exposing a configurable reduction (e.g., `--reduce mean`, `--reduce min`); the v0.1 contract stays "raw fan-out, consumer reduces."

### 4. Failure modes

**Decision**: **Fail-open with instrumented warning**. The contract:

| Condition | Adapter behavior |
|---|---|
| Registry has 0 evaluators for `AfterModelCall` | Return `Ok(json!([]))`; emit `tracing::warn!` with `reason="no evaluators registered"` |
| One evaluator returns `Err(_)` | Skip that evaluator's scores; record on tracing span; continue with the rest |
| All evaluators return `Err(_)` | Return `Ok(json!([]))`; emit `tracing::warn!` with `reason="all evaluators errored"` and the count |
| `EvalContext` construction itself fails (invariant violation) | Return `Err(adapter_err.to_string())` — the hook caller's `Err` branch logs and continues |

**Justification**: `NousScoreHook::on_post_inference` is already non-fatal (it logs `warn!` on scorer error and returns `HookOutcome::Continue`). The adapter mirrors this — turning Nous infra failures into instrumented soft-failures rather than blocking inference. The hook's role is to **observe**, not to gate; failing-closed would make Nous deployment a hard prerequisite for inference, which is the opposite of what Phase 2 wants.

The single hard `Err` path (Context-construction failure) is reserved for invariant violations the adapter shouldn't paper over, not Nous infra issues.

### 5. Evaluator selection

**Decision**: **No per-step selection**. The adapter is constructed with `Arc<EvaluatorRegistry>` and a fixed `EvalHook` (default: `EvalHook::AfterModelCall`). It calls every evaluator registered for that hook. Operator configures which evaluators run by adding/removing them from the registry at startup, not per-step.

```rust
pub struct NousAdapter {
    registry: Arc<EvaluatorRegistry>,
    hook: EvalHook,  // Default: EvalHook::AfterModelCall
}
```

**Justification**: Per-step evaluator selection is operationally fragile (string-keyed dispatch hides typos until runtime), forces step authors to know the Nous evaluator catalog, and doesn't compose well with how Nous already organizes evaluators (by hook point, with `name()` as the dedup key). Lifting selection to registry-assembly-time keeps:

- The trait surface free of evaluator names.
- Step config free of Nous coupling.
- Nous's existing `EvalHook` enum as the authoritative dispatch axis.

If a future step needs a tighter slice of evaluators, the right move is a separate registry instance per step type, **not** name-keyed dispatch in the adapter.

## Skeleton trait

Committed at `crates/ergon/ergon-nous-adapter/src/lib.rs` in this PR:

```rust
//! Nous-backed implementation of [`ergon::ResponseScorer`].
//!
//! See `docs/architecture/adr/2026-05-22-nous-adapter-for-ergon-scoring.md`.

use std::sync::Arc;

use async_trait::async_trait;
use ergon::{ModelResponse, ResponseScorer};
use nous_core::{EvalContext, EvalHook, EvaluatorRegistry};
use serde_json::Value;

pub struct NousAdapter {
    registry: Arc<EvaluatorRegistry>,
    hook: EvalHook,
}

impl NousAdapter {
    pub fn new(registry: Arc<EvaluatorRegistry>) -> Self {
        Self { registry, hook: EvalHook::AfterModelCall }
    }

    pub fn with_hook(mut self, hook: EvalHook) -> Self {
        self.hook = hook;
        self
    }
}

#[async_trait]
impl ResponseScorer for NousAdapter {
    async fn score(
        &self,
        _response: &ModelResponse,
    ) -> Result<Value, String> {
        // Implementation in BRO-1225 follow-up; this is the skeleton
        // committed alongside the ADR per acceptance §3.
        Err("NousAdapter::score not yet implemented — see ADR §1".into())
    }
}
```

This compiles against the existing `ergon` and `nous-core` crates without modifying either. The implementation follow-up (`feat: implement NousAdapter::score`) wires in:

- `HookCtx` plumbing (the trait method receives `&ModelResponse` only — the adapter needs access to `HookCtx` for `session_id`/`run_id`/`iteration`; the v0.2 trait either takes `HookCtx` directly or routes through a thread-local context handle — see Open Questions §1).
- Per-evaluator fan-out + `Vec<EvalScore>` flatten + JSON serialization.
- The four failure-mode branches from §4.

## P14 dep-chain

**Upstream**:
- `crates/ergon/ergon/src/lib.rs` — exports `ResponseScorer`, `ModelResponse`, `HookCtx`
- `crates/ergon/ergon-life-hooks/src/score.rs` — production consumer (uses the trait)
- `crates/nous/nous-core/src/{evaluator,registry,score}.rs` — Nous types
- `crates/arcan/arcan-ergon/` — the arcan adapter that BRO-1001 wires up (see referenced "production wiring" comment in `score.rs:5-8`)

**Downstream**:
- BRO-1225 implementation follow-up ticket (the actual `NousAdapter::score` body)
- `apps/bookkeeping-judge/src/score.rs` — current bookkeeping-judge has a custom scorer; once `NousAdapter` is live, the judge can register itself as a `NousEvaluator` and use the unified path
- `crates/arcan/arcan-ergon/src/runner.rs` — the workflow runner currently wires `NousScoreHook` against a stub `ResponseScorer`; the wiring swap to `NousAdapter` lands in the implementation ticket
- BRO-1001 — the arcan adapter ticket that originally framed the v0.1 production-wiring path

## Open questions (deferred to implementation ticket)

1. **`HookCtx` access from `ResponseScorer::score`**. The trait currently takes only `&ModelResponse`. The adapter needs `session_id`/`run_id`/`iteration` to build `EvalContext`. Three options:
   - **(a)** Widen the trait: `async fn score(&self, ctx: &HookCtx<'_>, response: &ModelResponse)`. Breaking change for any external `ResponseScorer` impl; clean and ergonomic.
   - **(b)** Thread-local context. Hook sets a tokio task-local before calling the scorer. Hidden coupling; less obvious to readers.
   - **(c)** Pass context via response metadata. Stuff `session_id` into `ModelResponse.metadata`. Pollutes the response type with adapter concerns.
   
   Lean toward **(a)** — but it's a trait-shape decision worth a separate review pass before the implementation ticket lands.

2. **Async vs sync at the Nous boundary**. `NousEvaluator::evaluate` is **sync** today; `ResponseScorer::score` is **async**. The adapter wraps sync evaluators inside an async method (zero-cost; no `tokio::spawn_blocking` needed because evaluators must complete in < 2ms by Nous contract per `evaluator.rs:4`). Confirm the < 2ms invariant holds before merging the implementation.

3. **`EvalContext.metadata` keys**. The adapter currently emits `workflow_name` and `model`. Add `model_provider`, `prompt_hash`, `tool_count`? Defer to implementation review.

4. **Bookkeeping-judge migration**. Out of scope for the implementation ticket too. File a sub-ticket once `NousAdapter` ships.

## Acceptance (per BRO-1225)

- [x] ADR at `docs/architecture/adr/2026-05-22-nous-adapter-for-ergon-scoring.md`
- [x] 5 design questions answered with chosen direction + justification each
- [x] Skeleton trait `NousAdapter` committed at `crates/ergon/ergon-nous-adapter/src/lib.rs`
- [ ] **Review**: 1+ human reviewer on the Ergon project (open after PR opens)

## Backreferences

- BRO-994 (Ergon v0.1 umbrella, Done)
- BRO-1001 — arcan adapter ticket that frames v0.1 production wiring
- Decision 2 option (c) from 2026-05-21 orchestration session — substrate-design path while uncommitted main state is contested
- `crates/ergon/ergon-life-hooks/src/score.rs:5-8` — original "production wiring" comment that motivates the adapter
- `crates/nous/nous-core/src/{evaluator,registry,score}.rs` — actual Nous API surface (vs the ticket's `EvaluatorRegistry::evaluate` shorthand)
