# Spec E — Linear Tickets Draft

> **STATE: SUPERSEDED 2026-05-07 — TICKETS FILED.** All 7 tickets filed under [BRO-1019](https://linear.app/broomva/issue/BRO-1019) umbrella. Mapping below for traceability:
>
> | Section in this draft | Filed as |
> |---|---|
> | Umbrella | [BRO-1019](https://linear.app/broomva/issue/BRO-1019) |
> | E-Sub-A | [BRO-1022](https://linear.app/broomva/issue/BRO-1022) |
> | E-Sub-B | [BRO-1023](https://linear.app/broomva/issue/BRO-1023) |
> | E-Sub-C | [BRO-1024](https://linear.app/broomva/issue/BRO-1024) |
> | E-Sub-D | [BRO-1025](https://linear.app/broomva/issue/BRO-1025) |
> | E-Sub-E | [BRO-1026](https://linear.app/broomva/issue/BRO-1026) |
> | E-Sub-F | [BRO-1027](https://linear.app/broomva/issue/BRO-1027) |
>
> Original draft body retained below for historical reference; ticket bodies in Linear are the canonical source going forward.

## Umbrella

**Title:** Spec E — Agent-Loop Compute Contract

**Body:**
```
Spec: core/life/docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md
Phase 1 plan: core/life/docs/superpowers/plans/2026-05-07-spec-e-sub-a-inference-foundation.md

Locks the runtime contract Life consumes from any inference silicon — present (vLLM, MLX, Groq, Cerebras, SambaNova, Tenstorrent) or future (agent-loop ASICs). Trait + multiple backends pattern, identical in shape to Spec D's AnimaCustody fan-out which shipped 6 backends in 5 days.

Phase 1 = E-Sub-A through E-Sub-F (foundation + 2 reference backends + Vigil + Autonomic + conformance battery). ~11 working days end-to-end via parallel sub-phase dispatch.

Phase 2+ deferred (Tenstorrent, Groq, Cerebras, SambaNova, public spec publication, apps migration).

Success criteria documented in spec.
```

**Labels:** `spec`, `inference`, `foundation`, `phase-1`
**Priority:** High
**Project:** Life

---

## E-Sub-A — `inference-core` foundation (BLOCKING)

**Title:** Spec E E-Sub-A — `inference-core` trait + InProcessBackend + InMemoryKvCache + Router

**Body:**
```
Plan: core/life/docs/superpowers/plans/2026-05-07-spec-e-sub-a-inference-foundation.md

Foundation for all subsequent sub-phases. Locks the trait shape from Spec E (L5-D1..L5-D8) so backends can fan out in parallel after merge.

Files:
- crates/inference/inference-core/{Cargo.toml, src/{lib,types,error,ids,kv,kv_inmem,backend,backend_inprocess,router}.rs}
- crates/inference/life-inference/{Cargo.toml, src/lib.rs}
- crates/inference/inference-core/tests/{conformance,inprocess_smoke}.rs

Estimated: 2–3 days. Single agent, TDD per task.
```

**Parent:** umbrella
**Labels:** `spec-e`, `sub-a`, `foundation`, `blocking`
**Priority:** Urgent
**Estimate:** 3

---

## E-Sub-B — `inference-mlx` (Apple Silicon backend)

**Title:** Spec E E-Sub-B — `inference-mlx` Apple Silicon backend

**Body:**
```
Spec ref: §Sub-phases → E-Sub-B
Depends on: E-Sub-A merged (trait shape locked)

New crate `crates/inference/inference-mlx`. Wraps Apple's MLX framework (Swift FFI via `mlx-rs`).
Capabilities: { spec_decode: true, fast_swap: true, on_chip_kv_persist: false, native_tool_emit: false }
Models tested: Llama-3.1-8B, Llama-3.3-70B, Phi-4, Qwen-2.5, Mistral-Nemo.

KV cache uses MLX shared memory (L0+L1). Persists to Lago via KvCache::persist (tested but stubbed until E-Sub-E lands the Lago-backed cache).

Plan to be written when E-Sub-A merges.
```

**Parent:** umbrella
**Depends on:** E-Sub-A
**Labels:** `spec-e`, `sub-b`, `backend`, `mlx`, `apple-silicon`
**Estimate:** 4

---

## E-Sub-C — `inference-vllm` (CUDA backend)

**Title:** Spec E E-Sub-C — `inference-vllm` CUDA backend

**Body:**
```
Spec ref: §Sub-phases → E-Sub-C
Depends on: E-Sub-A merged

New crate `crates/inference/inference-vllm`. Speaks vLLM's OpenAI-compatible HTTP API + paged-attention KV cache.
Capabilities: { spec_decode: true, fast_swap: false, on_chip_kv_persist: false, native_tool_emit: false }
Same 5 models as E-Sub-B. CI runs on a self-hosted CUDA runner (provision before merge).

Plan to be written when E-Sub-A merges.
```

**Parent:** umbrella
**Depends on:** E-Sub-A
**Labels:** `spec-e`, `sub-c`, `backend`, `vllm`, `cuda`
**Estimate:** 3

---

## E-Sub-D — `inference-vigil` (instrumentation)

**Title:** Spec E E-Sub-D — `inference-vigil` 15-series metric instrumentation

**Body:**
```
Spec ref: §Sub-phases → E-Sub-D
Depends on: E-Sub-A merged

New crate `crates/inference/inference-vigil`. 15 metric series per backend:
step_latency_ms, ttft_ms, tokens_per_second, kv_hit_rate, kv_miss_rate, kv_evict_rate,
swap_latency_ms, swap_count, spec_decode_accept_rate, spec_decode_reject_rate,
tool_emit_rate, close_code{code}, backend_unavailable, routing_decision{backend},
concurrent_streams.

OTLP-native (mirror life-vigil pattern). Consumed by E-Sub-E.

Plan to be written when E-Sub-A merges.
```

**Parent:** umbrella
**Depends on:** E-Sub-A
**Labels:** `spec-e`, `sub-d`, `instrumentation`, `vigil`
**Estimate:** 2

---

## E-Sub-E — `inference-autonomic` (policy hooks)

**Title:** Spec E E-Sub-E — `inference-autonomic` policy + Lago-backed KvCache

**Body:**
```
Spec ref: §Sub-phases → E-Sub-E
Depends on: E-Sub-D (metrics), E-Sub-A (trait)

New crate `crates/inference/inference-autonomic`. Setpoints:
- spec_decode.budget_per_session
- swap.cost_budget_per_minute
- kv.evict_threshold
- routing.hint_priority

Autonomic feedback: read 15 metric series from E-Sub-D, adjust setpoints to track within L1 stability budget (λ₁ ≈ 0.411).

Also lands the Lago-backed KvCache impl (currently InMemoryKvCache only). This crate composes inference-vigil + lago-* + autonomic-controller.

Plan to be written when E-Sub-D merges.
```

**Parent:** umbrella
**Depends on:** E-Sub-A, E-Sub-D
**Labels:** `spec-e`, `sub-e`, `autonomic`, `lago`, `kv-cache`
**Estimate:** 3

---

## E-Sub-F — `inference-conformance` (cross-backend battery)

**Title:** Spec E E-Sub-F — `inference-conformance` cross-backend test matrix

**Body:**
```
Spec ref: §Sub-phases → E-Sub-F
Depends on: E-Sub-B + E-Sub-C merged (need ≥2 real backends)

Promote inference-core/tests/conformance.rs scaffold into a standalone crate with full matrix:
backend × model × {greedy, sampled (seed-pinned), spec-decode} × {tool-await reconnect, KV-evict reconnect, deadline}.

Apple Silicon CI runner runs MLX tests; CUDA runner runs vLLM tests. Cross-backend digest equivalence in the same shape as anima-conformance (Spec D D-Sub-A).

Plan to be written when E-Sub-B + E-Sub-C merge.
```

**Parent:** umbrella
**Depends on:** E-Sub-B, E-Sub-C
**Labels:** `spec-e`, `sub-f`, `testing`, `conformance`
**Estimate:** 2

---

## Filing batch (one-shot when MCP auth lands)

```
1. Create umbrella with the body above.
2. Create E-Sub-A with parent set to umbrella, priority Urgent.
3. Create E-Sub-B..F with parent + dependencies as marked.
4. Update CLAUDE.md / Spec E to replace "Linear (pending)" with the umbrella URL.
5. Update Phase 1 plan §Linear to point at E-Sub-A's URL.
```

OAuth URL the parent session received (paste into a browser when ready):

```
https://mcp.linear.app/authorize?response_type=code&client_id=...&state=...
```

(actual URL is in the Claude session log; surface it again if expired).
