# Plan: mlx-lm Gemma 4 MTP integration (PR 2)

**Status:** 🟡 In progress — foundational slice built; blocked on PR 1 (#1276) merging before this can open
**Linear:** [BRO-1130](https://linear.app/broomva/issue/BRO-1130) (blocked-by [BRO-1129](https://linear.app/broomva/issue/BRO-1129))
**Branch:** `broomva:feat/gemma4-mtp-generate` (stacked on `feat/gemma4-assistant`)
**Spec:** `docs/superpowers/specs/2026-05-13-mlx-lm-gemma4-assistant.md` §7 (out-of-scope-for-PR-1 list)
**Goal:** Wire the `gemma4_assistant` drafter (PR 1) into mlx-lm's speculative-decoding loop so it actually produces the 1.7–2.2× speedup on Apple Silicon.

> **Grounding.** Every integration site below is a real line in the local clone (`~/code/mlx-lm-pr1`), read 2026-06-01. This plan was written *after* scouting the actual `speculative_generate_step` and `gemma4_text` forward, not from the design sketch.

---

## What's already done (P9 productive-wait, 2026-06-01)

The **target-side emission** — the irreducible foundation, certain regardless of how the loop API bikesheds — is built, tested, committed (`fac35d5`):

- `gemma4_text.Gemma4TextModel.__call__` and `Model.__call__` take `return_shared_kv_states: bool = False`.
- When `True`, `Model.__call__` returns `(logits, last_hidden, shared_kv_states)` where `shared_kv_states = {"full_attention": (k,v), "sliding_attention": (k,v)}` — the K/V of the last layer of each attention type, pulled from the `intermediates` list the forward already computes (and previously discarded at the norm).
- Gated so the default path is **byte-identical**: `test_gemma4_emits_shared_kv_states` asserts `allclose(default_logits, emitted_logits)`; all 10 prior gemma4 tests unchanged → 11 total.

This de-risks the hardest dependency ("can the target expose its internals cleanly without perturbing normal generation?" → **yes**).

---

## Integration sites (the remaining work)

### Site A — `speculative_generate_step` MTP branch · `mlx_lm/generate.py:473`

The current loop (`generate.py:473–654`) assumes a **homogeneous-tokenizer drafter with its own KV cache**:

- `_draft_generate(y, num_draft)` (L593) loops `_step(draft_model, draft_cache, y)` — feeds only the last token; drafter has `make_prompt_cache(draft_model)` (L524).
- The MTP drafter **has no cache** (`gemma4_assistant.Model.make_cache` raises) and needs, per draft step:
  - `inputs_embeds = concat(target_embed(last_token), target_last_hidden)` → (B, L, 5120)
  - `shared_kv_states` from the target's most recent forward
  - constant `position_ids`

**Change:** detect an MTP drafter and dispatch an alternate `_draft_generate`. Detection options (decide during impl):
1. `hasattr(draft_model, "pre_projection")` — duck-typing, simplest.
2. A marker attribute/protocol on the drafter class — explicit, slightly more code.
3. `isinstance(draft_model, gemma4_assistant.Model)` — couples generate.py to a model module (mlx-lm avoids this; **reject**).

Lean: option 1 or 2. The alternate draft loop pulls `target_last_hidden` + `shared_kv_states` from the target's prior `_step` (Site B), embeds the last token via `model.model.embed_tokens` (target), concatenates, and calls the drafter.

### Site B — target `_step` must surface its internals · `generate.py:553`

`_step(model, cache, y, n_predict)` (L553) currently calls `model(y[None], cache=cache)` and returns only sampled tokens + logprobs. For MTP, the **verification** `_step` on the target (L618) must *also* capture `last_hidden` + `shared_kv_states` (now available via the PR-2-foundation flag) so the next draft round can consume them.

**Change:** when an MTP drafter is present, call the target with `return_shared_kv_states=True`, stash `(last_hidden, shared_kv_states)` in the loop closure. Threading note: the target verifies `num_draft + 1` positions at once (L618); the drafter needs the `last_hidden` of the **last accepted** position (cf. HF `candidate_generator.py` `last_hidden_state[:, n_last_matches : n_last_matches+1]`).

### Site C — drafter autoregressive inner loop

Port HF's `SinglePositionMultiTokenCandidateGenerator.get_candidates` loop (transformers `generation/candidate_generator.py`):

```
for _ in range(num_draft):
    inputs_embeds = concat(target_embed(last_token_id), last_hidden)   # (B,1,5120)
    last_hidden, logits = drafter(inputs_embeds, shared_kv_states, position_ids=const)
    last_token_id = argmax(logits)            # greedy draft
    drafted.append(last_token_id)
```

Note `last_hidden` updates each step (drafter's own output feeds the next), while `shared_kv_states` stays fixed for the round.

### Site D — bidirectional / SWA masking for L>1

PR 1 ships `mask=None` (full attention; correct for single-position L=1 drafting). For L>1 multi-position drafting, port HF's `create_bidirectional_mask` + `create_bidirectional_sliding_window_mask` (flipped on the kv axis). **Defer unless** the L=1 path underperforms — single-position MTP is the documented common case.

---

## Tests / validation (PR 2)

1. **Foundation (done):** `test_gemma4_emits_shared_kv_states` — emission shapes + default-path invariance.
2. **End-to-end equivalence (greedy):** `stream_generate(model, draft_model=mtp_drafter)` produces a **bit-identical token sequence** to `stream_generate(model)` on a fixed prompt/seed — the speculative-decoding correctness guarantee. This is the key correctness test.
3. **Speedup (opt-in, network):** E4B target + E4B-assistant drafter ≥1.5× tok/s vs baseline on M4 Pro (baseline ~24.72 tok/s → ≥37). Benchmark table across the 4 prompts from the original 2026-05-13 benchmark.
4. **Regression:** existing homogeneous-tokenizer spec-dec path unchanged (a non-Gemma4 drafter still works).

---

## Open design questions (surfaced by the real code)

| # | Question | Lean |
|---|---|---|
| 1 | Drafter detection mechanism | duck-type `pre_projection` or a small marker protocol; **never** `isinstance` into a model module |
| 2 | Is target `last_hidden` pre- or post-final-norm? | PR-2-foundation returns **post-norm** (what becomes logits). HF uses `hidden_states[-1]`. The bit-equivalence test (#2 above) is the arbiter — if it fails, try pre-norm |
| 3 | Will upstream accept target-model (`gemma4_text`) changes? | The emission is additive + default-off (byte-identical default path). If maintainers resist, fall back to a wrapper that re-runs the last layer — uglier; raise in the PR description proactively |
| 4 | `num_draft_tokens` default for MTP | HF uses the drafter's trained depth; expose via existing `--num-draft-tokens`, document the sweet spot empirically |

---

## Sequencing

1. **Now (P9 wait):** foundation built ✅. Plan committed ✅. **Stop** — do not build Sites A–C speculatively before PR 1 is reviewed, because maintainer feedback on PR 1's `shared_kv_states` API shape could ripple into the loop design (rework risk).
2. **On #1276 merge:** rebase `feat/gemma4-mtp-generate` onto updated `main`; build Sites A–C with the end-to-end equivalence test as the gate; run the speedup benchmark; P20 review; open PR 2.
3. **External gate first:** #1276 needs the Apple CLA (user) + maintainer review before any of step 2.

---

## References

- PR 1: [ml-explore/mlx-lm#1276](https://github.com/ml-explore/mlx-lm/pull/1276)
- Foundation commit: `fac35d5` on `feat/gemma4-mtp-generate`
- HF reference loop: `transformers/generation/candidate_generator.py::SinglePositionMultiTokenCandidateGenerator`
- Spec: `docs/superpowers/specs/2026-05-13-mlx-lm-gemma4-assistant.md`
- PR explainer: `docs/pr-explainers/PR-1276.html`
