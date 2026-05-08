# Spec E — Agent-Loop Compute Contract

**Date**: 2026-05-07
**Status**: Draft (locked decisions inline; sub-phase A approved for immediate implementation)
**Sibling of**: Spec D (Anima Custody) — same trait-and-multiple-backends pattern, applied to inference instead of identity.
**Owner**: inference layer (`crates/inference/`, new)
**Linear umbrella**: [BRO-1019](https://linear.app/broomva/issue/BRO-1019/spec-e-agent-loop-compute-contract) — *Spec E — Agent-Loop Compute Contract*
**Linear sub-tickets**:
- [BRO-1022](https://linear.app/broomva/issue/BRO-1022/spec-e-e-sub-a-inference-core-foundation-trait-inprocess-kvcache) — E-Sub-A `inference-core` foundation (BLOCKING)
- [BRO-1023](https://linear.app/broomva/issue/BRO-1023/spec-e-e-sub-b-inference-mlx-apple-silicon-backend) — E-Sub-B `inference-mlx` (Apple Silicon)
- [BRO-1024](https://linear.app/broomva/issue/BRO-1024/spec-e-e-sub-c-inference-vllm-cuda-backend) — E-Sub-C `inference-vllm` (CUDA)
- [BRO-1025](https://linear.app/broomva/issue/BRO-1025/spec-e-e-sub-d-inference-vigil-15-series-instrumentation) — E-Sub-D `inference-vigil` (15-series instrumentation)
- [BRO-1026](https://linear.app/broomva/issue/BRO-1026/spec-e-e-sub-e-inference-autonomic-lago-backed-kvcache-policy-hooks) — E-Sub-E `inference-autonomic` (Lago-backed KvCache + policy)
- [BRO-1027](https://linear.app/broomva/issue/BRO-1027/spec-e-e-sub-f-inference-conformance-cross-backend-test-matrix) — E-Sub-F `inference-conformance` (cross-backend test matrix)

## Problem

Arcan today calls out to Vercel AI SDK via `crates/arcan/arcan-core/src/aisdk.rs` as a single hard-coded path. That works against any model provider Vercel supports (Anthropic, OpenAI, Google, Bedrock), but it wires the runtime to one transport and one runtime model. Three structural costs follow:

1. **No silicon fan-out.** Specialty inference vendors (Groq LPU, Cerebras WSE, SambaNova RDU, Tenstorrent Tensix, Apple MLX) cannot be reached without rewriting the call site. Each new vendor would be a one-off integration.

2. **No agent-loop primitives.** Arcan's loop branches, backtracks, holds context across dozens of steps, and interleaves model calls with Praxis tool calls. Vercel AI SDK exposes a prompt-in / response-out shape — branching, KV reuse across forks, and persistent KV across the execution graph have no API surface. We pay 30–40% utilization on commodity GPUs and zero benefit from agent-loop-aware silicon when it ships.

3. **No Lago / Anima integration.** KV state lives transiently in the model provider's memory. It is not Lago-content-addressed, not Anima-identity-bound, not Vigil-instrumented, and not subject to the Autonomic policy budget. Every other Life substrate is event-sourced and identity-scoped; inference is the one substrate that escapes the contract.

The YC reel of 2026-05-05 ("Most AI chips are designed for prompt-in, response-out — agents don't work that way") describes the hardware-side gap. **Spec E describes the software-side gap and closes it.** The runtime contract Life needs from any silicon — present or future — is stated here as a trait, with multiple backends, the same way Spec D fanned out AnimaCustody across six identity backends in five days.

This spec is also explicitly designed to be **published as a vendor-neutral standard** (the *Agent-Loop Compute Contract*) under Apache-2.0 once Phase 1 ships. The strategic position is the same one CUDA occupied for general-purpose compute: whoever owns the runtime contract owns the leverage, regardless of which vendor builds the chip.

## Solution

A trait abstraction (`InferenceBackend`) with multiple backends, mirroring `AnimaCustody`'s shape but specialised for inference workloads. Five distinguishing features:

1. **Per-call backend dispatch.** Backend is resolved per `AgentSelf` at construction, but routing is per-call so Autonomic can pick a different backend mid-loop (small drafter for routing, large model for synthesis, escape to host CPU for orchestration).

2. **First-class KV cache.** A sibling `KvCache` trait persists transformer KV state across an execution graph. Default impl is Lago-backed (content-addressed, fork via CoW, identity-scoped to `AnimaId`).

3. **First-class streaming.** Token streams are the default return type; close-code semantics match Spec C₃ §6.5 so reconnection-by-`last_token_no` is consistent across the runtime.

4. **Speculative decoding as a primitive, not an implementation detail.** Backends that support it expose draft + target as a single call; Autonomic owns the budget (acceptance threshold, max draft length).

5. **Tool dispatch escapes silicon.** The trait does not hide Praxis. When a model emits a tool-call token, the backend returns control to the host so Praxis runs the call; the trait re-enters with the tool's result. This means agent-loop silicon never has to support arbitrary tool execution — it owns the model call, host owns everything else.

## Locked Decisions

### L5-D1 — Trait lives in a new `crates/inference/` workspace
Sibling of `crates/anima/`, `crates/autonomic/`, `crates/lago/`. Sub-crates: `inference-core` (trait + types), `inference-mlx`, `inference-vllm`, `inference-vigil`, `inference-autonomic`, `inference-conformance`, `life-inference` (facade). `arcan-core/src/aisdk.rs` is *not* deleted in Phase 1 — it becomes one backend (`InferenceBackend::AiSdk`) so the migration is non-breaking.

Rejected alternative: putting the trait in `crates/aios/` as a sub-crate. aios is the kernel ABI; inference deserves its own crate cluster because it will grow to ≥6 backend impls and per-vendor concerns (vLLM vendor lock, MLX feature flags, Tenstorrent SDK churn) shouldn't pollute the kernel package.

### L5-D2 — KV cache is Lago-backed by default
KV blocks are stored as Lago objects keyed by `(model_id, prompt_prefix_hash, position_range)`. Forking an execution graph is a CoW Lago operation (already supported). Persistence across sessions is automatic. Identity-scoping to `AnimaId` is enforced in the cache key derivation so an Anima rotation invalidates cached KV.

Rejected alternative: in-process `tch::Tensor` cache with manual lifecycle. That gives ~2× throughput on a single agent but loses persistence, multi-tenant fork, and identity binding. Backends that need raw tensor speed expose a `KvCache::pin(handle)` op that pins blocks in device memory while the agent loop holds them.

### L5-D3 — Speculative decoding is opt-in per call
The trait has `step_speculative(...)` as a separate method from `step(...)`. Backends without spec-decode panic-on-construction if asked to provide a `SpeculativeBackend` capability; Autonomic policy gates the call. Every backend impls `step`; only backends with native draft/target support impl `step_speculative`.

Rejected alternative: making spec-decode invisible via wrapping. Hiding it produces unpredictable latency variance and makes the Autonomic budget model lie. Honesty wins.

### L5-D4 — Streaming uses Spec C₃ §6.5 close codes
A `step` returns `impl Stream<Item = Result<Token, InferenceError>>`. On disconnect (network, OOM, deadline-exceeded, model-swap-required), the stream closes with a typed `CloseCode` matching the lifegw vocabulary (1000 normal, 1003 unsupported-frame, 4001 deadline, 4002 KV-evicted, 4003 model-swap, etc.). `last_token_no` is part of the `Token` struct so reconnect-by-sequence works.

### L5-D5 — Tool dispatch escapes to Praxis
When a model emits a tool-call token, the stream yields `Token::ToolCall(call)` and *closes normally with code 4010 (tool-await)*. The host (Praxis) executes the call, then reopens the stream with `step(.., from_token: last_token_no, with_tool_result: Some(...))`. This keeps the silicon side of the trait pure — no tool runtime on chip.

Rejected alternative: chip executes tool calls directly. This is what the reel implicitly proposes ("dispatch tool call on chip"). It's wrong because it breaks the Praxis sandbox model and forces every tool to be silicon-portable. Tools live on host.

### L5-D6 — Anima identity is the cache scope
Every `KvCache` is bound to an `AnimaId`. Rotation (`anima.identity_rotated`) invalidates all KV blocks scoped to the old DID. Cross-anima KV reuse requires an explicit `KvCache::share(from_anima, to_anima, scope)` operation that Anima must sign — preventing privacy leaks via cache poisoning.

This composes with Spec D directly: since Anima rotation is already a first-class event, KV invalidation is a Lago tag-walk over events newer than the rotation point.

### L5-D7 — Backend selection is dynamic, not static
The runtime resolves `InferenceBackend` per call via an `InferenceRouter`. Default policy: route by model size + workload class (routing → small backend, synthesis → large backend, tool-emission likely → backend with low TTFT). Autonomic overrides via `inference.policy` setpoints. Static defaults live in the policy YAML.

Rejected alternative: per-`AgentSelf` static binding. That breaks the bursty-workload-routing argument from the reel — you want different silicon for different loop phases.

### L5-D8 — The trait will be published as a vendor-neutral spec
After Phase 1 ships, `inference-core` and the conformance battery are extracted as a standalone `agent-loop-compute-contract` crate published to crates.io under Apache-2.0, with the spec as a separate document in the crate root. Goals: (a) any silicon vendor can target the contract, (b) any runtime can consume backends written against it, (c) Life is the reference implementation.

Rejected alternative: keep the trait private. That gives 6-month lead but cedes the standardisation play. The leverage is the contract, not the impl.

## Architecture

### Trait shape (locked for Phase 1)

```rust
// crates/inference/inference-core/src/lib.rs (new)

use std::pin::Pin;
use futures::Stream;

pub trait InferenceBackend: Send + Sync + 'static {
    /// Backend identity, used in metrics and policy.
    fn backend_id(&self) -> &str;

    /// Capability flags advertised to the router.
    fn capabilities(&self) -> BackendCapabilities;

    /// One model invocation. Returns a token stream.
    /// Closes with Spec C₃ §6.5-style close codes (see CloseCode below).
    fn step(
        &self,
        ctx: StepContext<'_>,
    ) -> Pin<Box<dyn Stream<Item = Result<Token, InferenceError>> + Send>>;

    /// Speculative decoding. Optional; backends without this set
    /// `BackendCapabilities::spec_decode = false` and panic if called.
    fn step_speculative(
        &self,
        ctx: SpeculativeStepContext<'_>,
    ) -> Pin<Box<dyn Stream<Item = Result<Token, InferenceError>> + Send>> {
        let _ = ctx;
        panic!("backend does not support speculative decoding");
    }

    /// Cheap model swap. O(µs) on agent-loop silicon, O(ms) on commodity GPUs.
    /// Routers that swap frequently track per-backend swap cost via Vigil.
    fn swap_model(
        &self,
        from: ModelId,
        to: ModelId,
    ) -> Pin<Box<dyn Future<Output = Result<(), InferenceError>> + Send + '_>>;
}

pub trait KvCache: Send + Sync + 'static {
    fn lookup(&self, key: &KvKey) -> Option<KvHandle>;
    fn fork(&self, base: KvHandle) -> KvHandle;
    fn evict(&self, handle: KvHandle);

    /// Persist a KV slice into Lago, scoped by AnimaId.
    fn persist(
        &self,
        handle: KvHandle,
        scope: AnimaId,
    ) -> Pin<Box<dyn Future<Output = Result<LagoOid, InferenceError>> + Send + '_>>;

    /// Rehydrate a Lago-stored KV slice. Used on session resume.
    fn rehydrate(
        &self,
        oid: LagoOid,
        scope: AnimaId,
    ) -> Pin<Box<dyn Future<Output = Result<KvHandle, InferenceError>> + Send + '_>>;

    /// Pin a handle in device memory while the host holds the lock.
    /// Used by tight inner loops to avoid Lago round-trips.
    fn pin(&self, handle: KvHandle) -> KvPinGuard;
}

pub struct StepContext<'a> {
    pub model: ModelId,
    pub anima: AnimaId,
    pub kv: &'a dyn KvCache,
    pub kv_root: KvHandle,
    pub prompt_tokens: &'a [Token],
    pub max_new_tokens: u32,
    pub deadline: Option<Instant>,
    pub from_token: Option<u64>,        // for resume after tool-await
    pub with_tool_result: Option<ToolResult>,
}

pub struct BackendCapabilities {
    pub spec_decode: bool,
    pub fast_swap: bool,                 // < 10 ms model swap
    pub on_chip_kv_persist: bool,        // can persist KV without Lago round-trip
    pub native_tool_emit: bool,          // tool-call token is a chip primitive
    pub max_context_tokens: u32,
    pub supported_models: Vec<ModelId>,
}

pub enum Token {
    Text(String),
    ToolCall(ToolCall),
    SpecDecodeAccepted { drafted: u8 },  // observability
    SpecDecodeRejected { drafted: u8 },
    Done { reason: FinishReason, last_token_no: u64 },
}

pub enum CloseCode {
    Normal = 1000,
    UnsupportedFrame = 1003,
    Deadline = 4001,
    KvEvicted = 4002,
    ModelSwap = 4003,
    BackendUnavailable = 4004,
    AnimaInvalidated = 4005,
    ToolAwait = 4010,                    // matches L5-D5
}

#[non_exhaustive]
pub enum InferenceError {
    Backend { code: CloseCode, message: String },
    Network(std::io::Error),
    Cancelled,
}
```

### Memory hierarchy

The reel's "memory built for KV caches that persist across an entire execution graph" maps onto a four-level hierarchy. Backends declare which levels they implement; the router and Autonomic compose them.

| Level | Where it lives | Persistence | Latency | Owner |
|---|---|---|---|---|
| L0 | Compute-local SRAM (HBM, on-chip) | hot path only | sub-µs | Backend |
| L1 | Host RAM (vLLM block manager, MLX shared mem) | within a session | µs | Backend |
| L2 | Lago object store | across sessions, identity-scoped | ms | `KvCache` impl |
| L3 | Lago archive + soma cold storage | indefinite | seconds | Autonomic eviction |

Spec D's anima-rotation events flow into this hierarchy as cache invalidation: a rotation event marks an `AnimaId` boundary, and any KV at L2/L3 newer than the rotation must be re-anchored to the new identity or evicted.

### Router

```rust
// crates/inference/inference-core/src/router.rs

pub struct InferenceRouter {
    backends: Vec<Box<dyn InferenceBackend>>,
    policy: InferencePolicy,
    vigil: Arc<life_vigil::Meter>,
    autonomic: Arc<dyn AutonomicPolicy>,
}

impl InferenceRouter {
    pub fn route(&self, hint: RoutingHint) -> &dyn InferenceBackend { /* policy + Autonomic */ }
}

pub struct RoutingHint {
    pub model: ModelId,
    pub workload: WorkloadClass,         // Routing | Synthesis | ToolEmit | Embed
    pub deadline: Option<Instant>,
    pub anima: AnimaId,
}
```

### RCS hierarchy mapping

| RCS Level | What runs there | Spec E artifact | Stability budget |
|---|---|---|---|
| L0 — plant | Per-step model call | Backend.step() on silicon | λ₀ ≈ 1.455 |
| L1 — agent internal | Loop-level regulation (spec-decode budget, swap, evict) | InferenceRouter + Autonomic policy hooks | λ₁ ≈ 0.411 |
| L2 — meta-control | Branch / backtrack / when-not-to-call | EGRI loop_engine; not in Spec E directly but consumes the trait | λ₂ ≈ 0.069 |
| L3 — governance | Per-tenant silicon allocation, audit | `.control/policy.yaml` `inference.*` setpoints | λ₃ ≈ 0.006 |

The Spec E trait is the L0/L1 interface. EGRI (L2) and governance (L3) consume it; nothing in this spec changes them, which keeps the L3 stability budget intact.

## Sub-phases

Each sub-phase is one PR, parallelisable, mirroring the Spec D fan-out pattern that shipped six AnimaCustody backends in five days.

### E-Sub-A — `inference-core` scaffolding (foundation, blocking)
- New crate `crates/inference/inference-core` with trait shape above.
- `InProcessInferenceBackend` — dev-mode backend that wraps existing `arcan-core/src/aisdk.rs` so nothing breaks.
- `InMemoryKvCache` — dev-mode, no Lago dependency, for unit tests.
- `InferenceRouter` with single-backend policy.
- ≥ 30 unit tests covering trait contracts, error types, close codes.
- Estimated 2–3 days.
- **Blocking for E-Sub-B..F.**

### E-Sub-B — `inference-mlx` (Apple Silicon reference impl, parallel after A)
- New sub-crate `crates/inference/inference-mlx`.
- Backend wraps Apple's MLX framework (Swift FFI via `mlx-rs`).
- `BackendCapabilities { spec_decode: true, fast_swap: true, on_chip_kv_persist: false, native_tool_emit: false }`.
- KV cache uses MLX shared memory (L0+L1); persists to Lago via `KvCache::persist`.
- 5+ models tested: Llama-3.1-8B, Llama-3.3-70B, Phi-4, Qwen-2.5, Mistral-Nemo.
- Estimated 3–4 days.

### E-Sub-C — `inference-vllm` (CUDA reference impl, parallel after A)
- New sub-crate `crates/inference/inference-vllm`.
- Backend speaks vLLM's OpenAI-compatible HTTP API + paged-attention KV cache.
- `BackendCapabilities { spec_decode: true, fast_swap: false, on_chip_kv_persist: false, native_tool_emit: false }`.
- KV cache uses vLLM's block manager (L1); persists to Lago via `KvCache::persist`.
- Same 5 models as E-Sub-B.
- Estimated 3 days.

### E-Sub-D — `inference-vigil` (instrumentation, parallel after A)
- New sub-crate `crates/inference/inference-vigil`.
- 15 metric series per backend: `step_latency_ms`, `ttft_ms`, `tokens_per_second`, `kv_hit_rate`, `kv_miss_rate`, `kv_evict_rate`, `swap_latency_ms`, `swap_count`, `spec_decode_accept_rate`, `spec_decode_reject_rate`, `tool_emit_rate`, `close_code{code}`, `backend_unavailable`, `routing_decision{backend}`, `concurrent_streams`.
- OTLP-native; consumed by Autonomic in E-Sub-E.
- Estimated 2 days.

### E-Sub-E — `inference-autonomic` (policy hooks, after D)
- New sub-crate `crates/inference/inference-autonomic`.
- Autonomic setpoints: `spec_decode.budget_per_session`, `swap.cost_budget_per_minute`, `kv.evict_threshold`, `routing.hint_priority`.
- Autonomic feedback loop: read 15 metric series, adjust setpoints to track them within the L1 stability budget.
- Estimated 2 days.

### E-Sub-F — `inference-conformance` (cross-backend battery, after B+C)
- New sub-crate `crates/inference/inference-conformance`.
- Same `digest_equivalence` pattern as Spec D's cross-curve battery (`anima-conformance`).
- Test matrix: every backend × every model × {greedy, sampled (seed-pinned), spec-decode} × {tool-await reconnect, KV-evict reconnect, deadline}.
- Run in CI on a single Apple Silicon runner against MLX (E-Sub-B); vLLM tests run on a self-hosted CUDA runner.
- Estimated 2 days.

**Critical path:** A (3d) → B+C+D in parallel (4d) → E (2d) → F (2d) = **~11 working days**, conservatively 2 weeks. Same shape as Spec D Wave 1+2A+2B which shipped in 5 calendar days using subagent-driven development.

## Out of scope (deferred)

| Item | Why deferred | Linear ticket |
|---|---|---|
| Tenstorrent backend (`inference-tt`) | Hardware not in hand; Wormhole n300 needs to be ordered + driver setup | E-Phase-2-A (file after Phase 1 ships) |
| Groq / Cerebras / SambaNova backends | API access + devrel partnership pending; not blocking Phase 1 | E-Phase-2-B/C/D |
| TT-Metalium kernel pack (custom agent-loop ops on silicon) | Depends on Tenstorrent backend + Phase 1 trait stability | E-Phase-3 |
| arcan migration from `aisdk.rs` to `InferenceRouter` | Non-breaking by design; do after Phase 1 stability | E-Sub-G (post-Phase-1) |
| chatOS migration to `aiOS::InferenceBackend` (sibling of M9 anima migration) | Apps-side work; needs `@broomva/life-sdk` extension | E-Sub-H (post-Phase-1) |
| Public spec publication + broomva.tech announcement | After conformance battery is green and trait shape proven stable | E-Sub-I |

## Risks and mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Trait shape changes during Phase 1, breaking backends downstream | Med | High | Lock trait in E-Sub-A merge; treat any change as a breaking semver bump; backends pin minor version of `inference-core`. |
| MLX or vLLM SDK churn | High | Med | Pin versions in Cargo.toml; CI pin verification; quarterly SDK upgrade window with conformance battery as the gate. |
| KV cache size explodes Lago disk usage | Med | Med | Autonomic eviction policy; `KvCache::persist` returns error if quota exceeded; fall back to L1-only mode. |
| Tool-await reconnect latency exceeds Praxis call latency, defeating the escape hatch | Low | High | E-Sub-F latency benchmark gates the design; if reconnect ≥ Praxis call, redesign L5-D5 to keep stream open and multiplex tool-call frames. |
| Per-backend swap costs make `InferenceRouter` policy churn dominate utilisation | Med | Med | Autonomic owns swap budget (E-Sub-E); router defaults to no-swap unless cost is amortised over ≥ N tokens. |
| Spec publication leaks before stability proven; vendor implements stale shape | Low | Med | Defer publication (E-Sub-I) to after Phase 1 + 30 days of internal use. |

## Success criteria

Phase 1 is done when:

1. `crates/inference/` ships with E-Sub-A through E-Sub-F merged.
2. Conformance battery green on both MLX and vLLM backends across 5 models.
3. Vigil dashboards show all 15 metric series per backend in production.
4. Autonomic owns at least one setpoint (`spec_decode.budget_per_session` or equivalent) and adjusts it in response to metrics.
5. `arcan-core/src/aisdk.rs` is *unchanged* (zero breakage); migration to `InferenceRouter` is a follow-up under a feature flag.
6. The trait + types in `inference-core` are stable enough that a fresh agent reading `core/life/crates/inference/inference-core/src/lib.rs` can write a new backend in < 1 day.

When all six are true, file E-Sub-I (public spec publication) and proceed to Phase 2.

## References

- **Spec D — Anima Production Custody** (`2026-04-29-spec-d-anima-custody.md`) — the trait-and-backends pattern this spec mirrors.
- **Spec C₃ — lifegw edge gateway design** (`2026-04-27-spec-c3-lifegw-design.md`) — close-code vocabulary referenced in L5-D4.
- **YC reel, 2026-05-05** — `https://www.instagram.com/reel/DX78w_thwv2/`. Hardware-side framing of the same gap; transcript captured at session start.
- **Tenstorrent TT-Metalium reference** — the most architecturally aligned existing silicon target for Phase 2; not in this phase.
- **`research/entities/concept/agent-loop-silicon.md`** — knowledge graph entity (P6 Layer-3) capturing the thesis.
- **CLAUDE.md → "Bstack Core Automation Primitives"** — P11 (empirical feedback loop) and P10 (worktree hygiene) are how Phase 1 will be executed.
