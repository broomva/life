"""Real-weight dogfood validation for Gemma 4 MTP speculative decoding.

Loads the real target + MTP drafter and validates:
  1. Smoke      — models load, MTP generates coherent text without error
  2. Bit-equiv  — MTP greedy output == target-only greedy output (real weights;
                  this is what proves shared_kv is threaded *correctly*, which
                  the synthetic tests could not)
  3. Speedup    — MTP tok/s vs baseline tok/s
  4. Accept rate — fraction of yielded tokens that came from the drafter

Evidence is written next to this file.
"""

import json
import time
from pathlib import Path

import mlx.core as mx

from mlx_lm import load, stream_generate
from mlx_lm.generate import is_mtp_drafter
from mlx_lm.sample_utils import make_sampler

TARGET = "mlx-community/gemma-4-e4b-it-4bit"
DRAFTER = "mlx-community/gemma-4-E4B-it-assistant-bf16"
OUT = Path(__file__).parent
MAX_TOKENS = 120
NUM_DRAFT = 1

PROMPTS = [
    "Explain how speculative decoding works in two short paragraphs.",
    "Write a Python function that returns the n-th Fibonacci number iteratively.",
    "Summarize the difference between dense and mixture-of-experts transformers.",
]


def _gen(model, tokenizer, prompt, draft_model=None):
    sampler = make_sampler(temp=0.0)  # greedy → deterministic
    messages = [{"role": "user", "content": prompt}]
    p = tokenizer.apply_chat_template(messages, add_generation_prompt=True)
    toks, from_draft = [], []
    t0 = time.perf_counter()
    text = ""
    for r in stream_generate(
        model, tokenizer, p, max_tokens=MAX_TOKENS,
        draft_model=draft_model, num_draft_tokens=NUM_DRAFT, sampler=sampler,
    ):
        toks.append(r.token)
        text += r.text
        from_draft.append(getattr(r, "from_draft", False))
    dt = time.perf_counter() - t0
    return {
        "tokens": toks, "text": text, "n": len(toks),
        "elapsed_s": dt, "tok_per_s": len(toks) / dt if dt else 0,
        "accepts": sum(bool(x) for x in from_draft),
    }


def main():
    print(f"Loading target {TARGET} ...")
    model, tok = load(TARGET)
    print(f"  target type: {type(model).__name__}")
    print(f"Loading drafter {DRAFTER} ...")
    drafter, _ = load(DRAFTER)
    assert is_mtp_drafter(drafter), "drafter not detected as MTP"
    print(f"  drafter type: {type(drafter).__name__}  is_mtp={is_mtp_drafter(drafter)}")

    results = []
    bit_equiv_all = True
    for i, prompt in enumerate(PROMPTS, 1):
        print(f"\n=== Prompt {i}: {prompt[:55]}...")
        base = _gen(model, tok, prompt)
        print(f"  baseline: {base['tok_per_s']:.2f} tok/s ({base['n']} toks)")
        mtp = _gen(model, tok, prompt, draft_model=drafter)
        print(f"  mtp:      {mtp['tok_per_s']:.2f} tok/s ({mtp['n']} toks) "
              f"accepts={mtp['accepts']}/{mtp['n']}")
        equal = base["tokens"] == mtp["tokens"]
        bit_equiv_all = bit_equiv_all and equal
        speedup = mtp["tok_per_s"] / base["tok_per_s"] if base["tok_per_s"] else 0
        accept_rate = mtp["accepts"] / mtp["n"] if mtp["n"] else 0
        print(f"  bit-equivalent: {equal}   speedup: {speedup:.2f}x   "
              f"accept-rate: {accept_rate:.1%}")
        results.append({
            "prompt": prompt, "baseline_tps": base["tok_per_s"],
            "mtp_tps": mtp["tok_per_s"], "speedup": speedup,
            "bit_equivalent": equal, "accept_rate": accept_rate,
            "n_tokens": mtp["n"], "sample_text": mtp["text"][:400],
        })

    avg_base = sum(r["baseline_tps"] for r in results) / len(results)
    avg_mtp = sum(r["mtp_tps"] for r in results) / len(results)
    avg_speedup = avg_mtp / avg_base if avg_base else 0
    avg_accept = sum(r["accept_rate"] for r in results) / len(results)

    summary = {
        "target": TARGET, "drafter": DRAFTER,
        "target_class": type(model).__name__, "drafter_class": type(drafter).__name__,
        "num_draft_tokens": NUM_DRAFT, "max_tokens": MAX_TOKENS,
        "avg_baseline_tps": avg_base, "avg_mtp_tps": avg_mtp,
        "avg_speedup": avg_speedup, "avg_accept_rate": avg_accept,
        "bit_equivalent_all": bit_equiv_all, "per_prompt": results,
    }
    (OUT / "results.json").write_text(json.dumps(summary, indent=2))

    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)
    print(f"target class:     {type(model).__name__} (multimodal wrapper path)")
    print(f"bit-equivalent:   {bit_equiv_all}  (MTP == target-only greedy)")
    print(f"avg baseline:     {avg_base:.2f} tok/s")
    print(f"avg MTP:          {avg_mtp:.2f} tok/s")
    print(f"avg speedup:      {avg_speedup:.2f}x")
    print(f"avg accept-rate:  {avg_accept:.1%}")
    print(f"\nEvidence: {OUT/'results.json'}")


if __name__ == "__main__":
    main()
