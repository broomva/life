# Spec: mlx-lm Gemma 4 MTP drafter (gemma4_assistant) — AS BUILT

**Date:** 2026-05-13 (design) · 2026-05-14 (built) · 2026-06-01 (as-built reconciliation)
**Authors:** broomva
**Status:** ✅ Implemented — [ml-explore/mlx-lm#1276](https://github.com/ml-explore/mlx-lm/pull/1276) OPEN, awaiting upstream review
**Linear:** [BRO-1128](https://linear.app/broomva/issue/BRO-1128) umbrella → [BRO-1129](https://linear.app/broomva/issue/BRO-1129) (PR 1, this doc) + [BRO-1130](https://linear.app/broomva/issue/BRO-1130) (PR 2, deferred)
**Upstream target:** `ml-explore/mlx-lm` (MIT-licensed)
**Strategic context:** Spec E (Agent Loop Silicon, [BRO-1019](https://linear.app/broomva/issue/BRO-1019)), `inference-mlx` sub-phase

> **Reading note.** This is the *as-built* spec: the original design intent reconciled with what actually shipped. Four design points changed during implementation and P20 adversarial review — each is flagged inline with **[AS-BUILT]** and summarized in §10. The PR is the source of truth for code; this doc is the source of truth for *why*.

---

## 1. Context

### 1.1 What Google shipped

On 2026-05-05 Google released [Multi-Token Prediction (MTP) drafters](https://blog.google/innovation-and-ai/technology/developers-tools/multi-token-prediction-gemma-4/) for the Gemma 4 family — small companion models that accelerate inference via speculative decoding. The drafter proposes N future tokens; the target verifies all N in parallel; output is bit-for-bit identical to standard generation.

Four drafters on Hugging Face, one per Gemma 4 size, ~80M params each, Apache-2.0:

| Target model | Drafter model |
|---|---|
| `google/gemma-4-E2B-it` | `google/gemma-4-E2B-it-assistant` |
| `google/gemma-4-E4B-it` | `google/gemma-4-E4B-it-assistant` |
| `google/gemma-4-26B-A4B-it` | `google/gemma-4-26B-A4B-it-assistant` |
| `google/gemma-4-31B-it` | `google/gemma-4-31B-it-assistant` |

mlx-community published bf16 MLX conversions of all four. Claimed speedup: 1.7–2.2× on Apple Silicon, up to 3× on RTX PRO 6000.

### 1.2 The gap (at design time)

`mlx-lm 0.31.3` had no `gemma4_assistant` model class — `mlx_lm.load("mlx-community/gemma-4-E4B-it-assistant-bf16")` raised `ValueError: Model type gemma4_assistant not supported.` No upstream PR existed; adjacent ecosystem (mlx-swift#389, lmstudio-ai/mlx-engine#301) was also missing it.

### 1.3 Empirical motivation (local benchmark, M4 Pro / 24GB / 4-bit)

| Configuration | tok/s | Speedup |
|---|---|---|
| Baseline (E4B target alone) | **24.72** | 1.00× |
| Standard spec-dec (E2B-it-4bit as drafter for E4B) | 15.31 | **0.62×** ⚠️ slower |
| MTP drafter (this spec, needs PR 2 for end-to-end) | n/a in PR 1 | targeting ≥1.5× |

Standard speculative decoding is *slower* because a generic 2B drafter doesn't pay for itself against a 4B target. The 0.62× measured here is the floor that MTP's purpose-built 80M drafter (with shared-activation input + clustered logit head) is designed to beat — which empirically motivates MTP even though PR 1 alone can't yet run it end-to-end.

### 1.4 Strategic motivation

Per Spec E, the broomva positioning is "POSIX of agent silicon" — own the runtime contract, hardware vendors compete underneath. Upstream contribution to mlx-lm (Apple's officially-maintained LLM runtime) reinforces that. Direct dependency: Spec E's `inference-mlx` sub-phase (E-Sub-B).

---

## 2. Architecture (as built)

### 2.1 Module layout

Single new file `mlx_lm/models/gemma4_assistant.py` (404 LOC), zero edits elsewhere.

```
Model (nn.Module)
├── pre_projection      Linear(5120, 256, bias=False)
├── model               AssistantTextModel
│   ├── embed_tokens    Embedding(262144, 256)
│   ├── layers          [AssistantDecoderLayer × 4]
│   │   └── each:
│   │       ├── self_attn  AssistantAttention (Q-only; cross-attends to shared_kv_states)
│   │       │   ├── q_proj      Linear(256, n_heads*head_dim, bias=False)
│   │       │   ├── q_norm      RMSNorm(head_dim)
│   │       │   └── o_proj      Linear(n_heads*head_dim, 256, bias=False)
│   │       ├── input_layernorm / post_attention_layernorm
│   │       ├── pre_feedforward_layernorm / post_feedforward_layernorm
│   │       ├── mlp             SwiGLU (gate/up/down, intermediate=2048)
│   │       └── layer_scalar    mx.ones((1,))   # ← the "11th tensor", see §10
│   └── norm            RMSNorm(256)
├── post_projection     Linear(256, 2560, bias=False)
└── masked_embedding    MaskedEmbedder (optional, gated by use_ordered_embeddings)
    ├── centroids       Linear(256, 2048, bias=False)
    └── _token_ordering buffer (262144,) int32   # ← underscore: see [AS-BUILT] §3.2
```

### 2.2 **[AS-BUILT]** Per-layer `head_dim` dispatch

The lone `full_attention` layer uses `global_head_dim` (512 in E4B); `sliding_attention` layers use `head_dim` (256). This mirrors `gemma4_text.Attention` and was **not in the original design** — discovered when real-weight load failed with `Expected shape (1024, 256) but received shape (2048, 256)` on `layers.3.self_attn.q_proj`. Fallback to `head_dim` when `global_head_dim` is unset keeps small synthetic test configs working. (Commit `6e2e94d`.)

### 2.3 Forward signature

```python
def __call__(
    self,
    inputs_embeds: mx.array,                        # (B, L, 5120) = concat(target_embed(last_tok), target_last_hidden)
    shared_kv_states: dict[str, tuple[mx.array, mx.array]],  # {"full_attention": (k,v), "sliding_attention": (k,v)}
    position_ids: Optional[mx.array] = None,
    mask: Optional[mx.array] = None,
) -> tuple[mx.array, mx.array]:                      # (last_hidden_2560, logits_262144)
```

### 2.4 Data flow

1. `h = pre_projection(inputs_embeds)` → (B, L, 256)
2. Per layer, dispatch K/V by `layer_type`: `q = q_proj(q_norm(input_layernorm(h)))`; GQA-expand target K/V (2→n_heads); RoPE Q with `position_ids` offset (K pre-RoPE'd by target); `scaled_dot_product_attention`; double-norm residuals around attn + MLP; `h *= layer_scalar`
3. `h = norm(h)` → (B, L, 256)
4. `last_hidden = post_projection(h)` → (B, L, 2560) — returned to caller for next draft step
5. Logits: `masked_embedding(h, embed_tokens.weight)` if `use_ordered_embeddings` else `embed_tokens.as_linear(h)`
6. Return `(last_hidden, logits)`

### 2.5 MaskedEmbedder (centroid-clustered logit head)

2048 clusters of 128 tokens; score centroids, take top-32, compute logits only inside selected clusters (~64× cheaper than full-vocab over 262144). MLX translation of HF's `torch.scatter_`: `mx.put_along_axis` (functional, verified overwrite-semantics with duplicate-index test). Top-K via `mx.argpartition(-x, kth=top_k-1)[..., :top_k]` (unsorted is fine — gather+matmul+scatter is order-independent).

**[AS-BUILT]** the non-selected fill value is built **on-device** (`mx.min(selected_logits) - 1.0` + broadcast-add), not via `.item()` — this path runs every draft step and a GPU→host sync per token would defeat the drafter. (P20 finding I4, commit `7b0d7d9`.)

---

## 3. Weight loading & quantization (as built)

### 3.1 Safetensors structure (verified)

50 tensors: `pre_projection`, `post_projection`, `model.embed_tokens`, `model.norm`, `masked_embedding.centroids`, `masked_embedding.token_ordering` (int64 buffer), plus 11/layer × 4. The 11 per-layer = q_proj/q_norm/o_proj (3) + 4 norms + gate/up/down (3) + **`layer_scalar`** (1).

### 3.2 **[AS-BUILT]** `sanitize` — buffer rename via side-channel

The original design just cast `token_ordering` int64→int32 in place. **P20 finding #6** showed that leaving it named `token_ordering` puts it in `Module.parameters()`, so `model.update(tree_map(astype, model.parameters()))` (e.g. a dtype cast or `nn.quantize`) corrupts the int32 gather indices into floats — silent miscompute.

Fix: rename to `_token_ordering` (leading underscore excludes it from `parameters()`). But `load_weights` is strict and *rejects* underscored keys it can't map to a parameter. Resolution: `sanitize` installs the buffer **directly on the submodule** and **strips it from the returned dict**:

```python
def sanitize(self, weights):
    out = {}
    for k, v in weights.items():
        if k == "masked_embedding.token_ordering":
            if self.masked_embedding is not None:
                self.masked_embedding._token_ordering = v.astype(mx.int32)
            continue          # do NOT return it — load_weights would reject the underscored key
        out[k] = v
    return out
```

Install order is safe: `sanitize` runs before `nn.quantize` (which skips `MaskedEmbedder` — no `to_quantized`) and before `load_weights` (which never sees the stripped key). Verified: `_token_ordering` absent from `parameters()`, survives `tree_map` cast with dtype+values intact.

### 3.3 `quant_predicate`

Excludes `masked_embedding.centroids` from quantization (0.5M params; 4-bit hurts cluster discrimination more than the ~0.25MB saves). `def predicate(path, _)` — single-underscore, matches sibling style.

### 3.4 **[AS-BUILT]** `make_cache` raises instead of returning `[]`

Original design returned `[]` ("fail-fast on `cache[0]`"). **P20 finding #9**: most mlx-lm cache paths use `zip(layers, cache)`, which silently no-ops on an empty list → attention without cached context, silently wrong. Fix: raise `NotImplementedError` with guidance to pass the target's `shared_kv_states` instead. (Commit `7b0d7d9`.)

---

## 4. Testing (as built)

Three methods in `tests/test_models.py`:

- `test_gemma4_assistant` — synthetic forward (fp32 + fp16), all submodules, GQA, centroid scatter, `copy.deepcopy`, `make_cache` raise, `quant_predicate` exclusions
- `test_gemma4_assistant_no_ordered_embeddings` — non-clustered logit path
- `test_gemma4_assistant_published_checkpoint_forward_shapes` — **[AS-BUILT]** opt-in via `MLX_LM_RUN_NETWORK_TESTS=1` (inverted from the design's `SKIP` polarity per P20 finding B2; default-skip protects CI from a 159MB download), uses public `huggingface_hub.snapshot_download` (not private `_download`)

Result: **10 passed, 1 skipped** (default) · **11 passed** (with network flag) · **77 passed, 2 skipped** full `test_models.py` (zero regressions).

---

## 5. PR shape (as built)

| File | Status | LOC |
|---|---|---|
| `mlx_lm/models/gemma4_assistant.py` | ADD | 404 |
| `tests/test_models.py` | MODIFY | +180 |

11 commits on `broomva:feat/gemma4-assistant`: 6 TDD feature commits + 1 global_head_dim fix + 3 test commits + 1 P20-fix consolidation. PR [#1276](https://github.com/ml-explore/mlx-lm/pull/1276). Apple CLA required before upstream CI/merge (external gate, user action).

---

## 6. Risks (resolution status)

| Risk (design) | Outcome |
|---|---|
| `torch.scatter_` → MLX has no in-place equiv | ✅ `mx.put_along_axis` (functional), verified |
| Unidentified 11th per-layer tensor | ✅ Resolved: `layer_scalar` (§10) |
| Bidirectional masking semantics | ⏸️ Deferred to PR 2; PR 1 uses `mask=None` = full attention (correct for L=1 single-position MTP; verified `mx.fast.scaled_dot_product_attention` with `mask=None` is no-mask not causal) |
| RoPE alignment Q vs target-K | ✅ Same `initialize_rope` config; offset passed as `mx.array` (no `.item()` sync, P20 I3) |
| `position_ids` shape contract | ⏸️ PR 1 takes last position as scalar offset; batched non-uniform positions flagged for PR 2 (P20 round-1 noted, acceptable for single-position MTP) |

---

## 7. Out of scope (→ PR 2, BRO-1130)

`stream_generate` integration · target's per-layer-type K/V emission · end-to-end speedup benchmark · bit-equivalence vs HF Transformers · bidirectional/SWA mask construction for L>1.

---

## 8. Acceptance criteria — ✅ all met (PR 1)

- [x] `mlx_lm.load("mlx-community/gemma-4-E4B-it-assistant-bf16")` → `Model` with 4 layers
- [x] Forward with synthetic `shared_kv_states` → (B,L,2560) + (B,L,262144)
- [x] Tier 1 synthetic test fp32 + fp16
- [x] Tier 2 `use_ordered_embeddings=False` path
- [x] Tier 3 real-weight load (opt-in)
- [x] All existing `test_gemma4_*` pass (no regression)
- [x] PR opened against `ml-explore/mlx-lm:main`
- [x] P20 adversarial review ≥7/10 (round 1: 6/10 → round 2: **9/10**)
- [ ] Apple CLA signed — **user action, external gate**

---

## 9. References

- Google blog: https://blog.google/innovation-and-ai/technology/developers-tools/multi-token-prediction-gemma-4/
- HF reference: `transformers/models/gemma4_assistant/modeling_gemma4_assistant.py`
- Checkpoint: https://huggingface.co/mlx-community/gemma-4-E4B-it-assistant-bf16
- PR: https://github.com/ml-explore/mlx-lm/pull/1276
- Implementation record: `docs/superpowers/plans/2026-05-14-mlx-lm-gemma4-assistant-pr1.md`
- PR explainer (human-read): `docs/pr-explainers/PR-1276.html`
- KG entities: `research/entities/concept/multi-token-prediction-drafter.md`, `research/entities/pattern/mlx-nonparameter-buffer-via-sanitize.md`

---

## 10. Build outcome & as-built deltas

Four changes between design and shipped code — all improvements forced by real-weight load or P20 review:

| # | Design said | As built | Trigger | Commit |
|---|---|---|---|---|
| 1 | uniform `head_dim` | per-layer-type dispatch (`global_head_dim` for full_attention) | real-weight load shape error | `6e2e94d` |
| 2 | `token_ordering` cast in place | `_token_ordering` installed via `sanitize` side-channel, stripped from weight dict | P20 #6 (tree_map corrupts int32 buffer) | `7b0d7d9` |
| 3 | `make_cache` returns `[]` | `make_cache` raises `NotImplementedError` | P20 #9 (zip-iteration silent no-op) | `7b0d7d9` |
| 4 | network test default-runs (`SKIP` env to disable) | opt-in (`MLX_LM_RUN_NETWORK_TESTS=1`), public `snapshot_download` | P20 B2 (5GB CI download, private API) | `7b0d7d9` |

**Open question §6.3 resolved:** the 11th per-layer tensor is `layer_scalar` — a per-layer scalar (`mx.ones((1,))`) applied after the FFN residual, identical to `gemma4_text.DecoderLayer:331`.

**P20 cross-model adversarial review:** round 1 scored 6/10 (parallel Strata-B devil's-advocate + Strata-C `pr-review-toolkit:code-reviewer`; converging blockers). Round 2 after fixes: **9/10, passed**. The three model-code blockers (`.item()` hot-path syncs, parameter-buffer corruption, silent empty-cache) were all robustness issues, not happy-path math errors — the structural tracking of the HF reference was correct from the first pass.

**Durability note (why this doc was rewritten 2026-06-01):** the original 2026-05-14 spec + plan were authored but never committed; an intervening `git clean` wiped them. This as-built version is committed. Lesson reinforced: documentation is only "done" when committed + pushed.
