# Implementation Record: mlx-lm gemma4_assistant (PR 1)

**Status:** ✅ Executed 2026-05-14 → [ml-explore/mlx-lm#1276](https://github.com/ml-explore/mlx-lm/pull/1276)
**Spec:** `docs/superpowers/specs/2026-05-13-mlx-lm-gemma4-assistant.md`
**Linear:** [BRO-1129](https://linear.app/broomva/issue/BRO-1129)
**Execution mode:** `/autonomous` (subagent-driven; P5 parallel for the P20 review stage)

> This was an executable TDD plan (13 tasks, each: write failing test → implement → pass → commit). Post-execution it serves as the build record. **Code lives in the PR**; this captures the task structure, commit mapping, and deviations. PR 2 (BRO-1130) reuses this structure for the `stream_generate` integration.

---

## Working environment

- Fork `broomva/mlx-lm`, clone at `~/code/mlx-lm-pr1/`, branch `feat/gemma4-assistant` off `upstream/main`
- venv `~/code/mlx-lm-pr1/.venv` (editable `pip install -e .` + `pytest` + `huggingface_hub`)
- Drafter checkpoint `mlx-community/gemma-4-E4B-it-assistant-bf16` (159MB) pre-cached for the Tier-3 test

---

## Task → commit map

| Task | Description | Commit | Result |
|---|---|---|---|
| 1 | Bootstrap fork/clone/venv; baseline `pytest -k gemma4` | — | 8 baseline tests green |
| 2 | `ModelArgs` + `Model` skeleton | `da578be` | parses real drafter config |
| 3 | `MaskedEmbedder` centroid logit head | `292c53d` | top-32 of 2048 clusters |
| 4 | `AssistantAttention` (Q-only cross-attn) | `b9018d6` | GQA expand, no k/v proj |
| 5 | `AssistantMLP` + `AssistantDecoderLayer` | `8ffd06f` | double-norm + `layer_scalar` |
| 6 | `AssistantTextModel` (4-layer stack) | `a3bb0f2` | dispatch K/V by layer_type |
| 7 | top-level `Model` forward + sanitize + quant_predicate + make_cache | `f7689d5` | `(last_hidden, logits)` tuple |
| 8 | real-weight load + iterate sanitize | `6e2e94d` | **global_head_dim fix** (deviation 1) |
| 9 | Tier 1 synthetic test | `0ed84c3` | fp32 + fp16 |
| 10 | Tier 2 `use_ordered_embeddings=False` | `8f488d8` | non-clustered path |
| 11 | Tier 3 gated real-weight test | `47858bc` | (later inverted polarity in `7b0d7d9`) |
| 12 | full regression + P20 review | `7b0d7d9` | **6/10 → fixes → 9/10** (deviations 2–4) |
| 13 | push + open PR + watcher + Linear | — | PR #1276 |

---

## Deviations from plan (the interesting part)

The plan's code was the *design*; four things changed when it met reality. All four are documented in the spec §10. Summary of why each happened during execution:

1. **`global_head_dim` (Task 8)** — the plan assumed uniform `head_dim=256`. Real-weight load threw a shape error on the one `full_attention` layer (it uses `global_head_dim=512`). One-line dispatch fix mirroring `gemma4_text.Attention`. *Lesson: synthetic tests with tiny configs don't exercise the global-head-dim path; only real weights do — Task 8's "load real weights and iterate" caught it exactly as the plan intended.*

2. **`_token_ordering` buffer rename (Task 12)** — P20 reviewer proved that the int32 gather buffer, named without a leading underscore, sits in `parameters()` and gets corrupted by `tree_map` dtype casts. The plan's Tier-1 test had a fragile "re-cast after update" workaround; the real fix moved the buffer out of `parameters()` and installed it via `sanitize` side-channel.

3. **`make_cache` raises (Task 12)** — P20 reviewer showed `return []` silently no-ops in `zip(layers, cache)` iteration. Changed to `raise NotImplementedError`.

4. **Network test opt-in (Task 12)** — P20 reviewer flagged that the default-run polarity would force a 159MB CI download and used a private `_download` import. Inverted to `MLX_LM_RUN_NETWORK_TESTS=1` opt-in with public `snapshot_download`.

The plan's subagent (Tasks 9–11) also made 3 minimal *test-only* fixes (int32 restore after tree_map, dtype-cast `shared_kv` in the dtype loop, per-layer-type head_dim in Tier-3) — two of which were obsoleted by deviation 2's cleaner fix.

---

## Orchestration (P5 / P19)

- Tasks 2–7: **sequential** single subagent (same-file TDD chain — no parallelism possible)
- Tasks 9–11: **sequential** single subagent (same-file appends)
- Task 12 P20 review: **parallel** — 2 simultaneous subagents (Strata B devil's-advocate + Strata C `pr-review-toolkit:code-reviewer`). Convergence on the same blockers was high-signal.
- Task 12 round-2 verification: single follow-up subagent (read-only re-score)

No worktrees: upstream work is its own git tree; the only parallel writes were the two read-only reviewers.

---

## Validation evidence (P11)

```
pytest tests/test_models.py -k gemma4 -q          → 10 passed, 1 skipped
MLX_LM_RUN_NETWORK_TESTS=1 pytest …Tier3          → 1 passed
pytest tests/test_models.py -q                    → 77 passed, 2 skipped (no regression)
python -c "from mlx_lm import load; m,_=load('…assistant-bf16'); print(len(m.layers))"  → 4
```

---

## Follow-up (PR 2 — BRO-1130)

Reuses this task structure for `stream_generate` MTP integration: target emits per-layer-type K/V → drafter consumes via `shared_kv_states` → bidirectional/SWA mask construction for L>1 → end-to-end ≥1.5× speedup benchmark on M4 Pro → bit-equivalence test vs baseline. Blocked by #1276 merging.
