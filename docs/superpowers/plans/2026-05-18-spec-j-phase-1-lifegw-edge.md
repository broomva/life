# Plan — Spec J Phase 1: lifegw `/v1/messages` edge route

**Date**: 2026-05-18
**Spec**: `docs/superpowers/specs/2026-05-18-spec-j-claude-code-interop.md`
**Linear umbrella**: [BRO-1140](https://linear.app/broomva/issue/BRO-1140)
**Target duration**: ~16 working days (3 weeks calendar with parallel agent dispatch)
**Dispatch mechanism**: Bstack P5 fan-out via `Agent` subagents, one worktree per sub-stream (mirrors the Spec D Wave 1/2A/2B/3 pattern that shipped 6 backends in 5 days).

## Status

Phase 0 (spec + this plan + entity + PR) **complete on user sign-off**. Implementation sub-tickets file when the user approves the spec. This document is the **dispatch handoff** — when the user nods, the steps below execute without further negotiation.

## arcan-proxy::AnthropicArcan landing (folded into J-Sub-B per Q2 resolution)

`crates/life-runtime/arcan-proxy/src/anthropic.rs` is uncommitted on the local working tree of main. Per user review, **the precursor is folded into J-Sub-B** rather than a separate PR. J-Sub-B's first commit lands the AnthropicArcan adapter; subsequent commits add the `/v1/messages` route that consumes it. Single PR, single review cycle.

Risks of folding (mitigated):
- Larger PR diff — managed via clear commit boundaries (`feat(arcan-proxy): land AnthropicArcan ArcanCall adapter` → `feat(lifegw): add /v1/messages route consuming AnthropicArcan`).
- Reviewer must validate both surfaces — acceptable because the route directly exercises the adapter via integration tests.

## Sub-tickets (file at dispatch time)

Each sub-ticket gets one worktree at `core/life/.worktrees/feat-bro-XXX-<slug>` off main. Cross-review (P20) runs before push for every PR with diff >200 LOC. Auto-merge fires on CI green via `p9 auto-merge`.

### J-Sub-A — `lifegw-anthropic-codec` crate scaffold + SSE encoder

**Estimate**: 5 working days
**Owner**: subagent on worktree `feat-bro-XXXA-anthropic-codec-scaffold`
**Blocks**: J-Sub-B (codec is a build dep)

**Deliverables**:
- New crate at `crates/life-runtime/lifegw-anthropic-codec/`
- `Cargo.toml` with `serde`, `serde_json`, `tokio`, `futures`, `bytes`, `sha2`, `hex`, `thiserror`, `tracing`, `life-runtime-proto` (NO `tiktoken-rs` per Q8 resolution / L10-D7; NO substrate deps per Spec J §[CI lane])
- `src/lib.rs` exports: `Encoder`, `BlockPolicyState`, `EmittedTracker`, `AnthropicSseEvent`, `AnthropicMessagesRequest`, `synthesize_sid`
- `src/encoder.rs` — `pb::AgentEvent` → `AnthropicSseEvent` translation (state machine driven by `BlockPolicyState`)
- `src/block_policy.rs` — port of `core/anthropic/native_sse_block_policy.py` (up-stream block re-mapping)
- `src/thinking.rs` — thinking block lifecycle
- `src/tools.rs` — tool_use block construction
- `src/state.rs` — `EmittedTracker` for replay de-dup
- `src/sid.rs` — `synthesize_sid` per Spec J L10-D2
- `src/errors.rs` — Anthropic error event format
- `src/contracts.rs` — wire-shape assertion helpers (test-only)
- 25+ unit tests under `tests/`
- `scripts/verify_dependencies_lifegw_anthropic_codec.sh` per Spec J §12

**Explicitly NOT in this crate**: `tokens.rs` / token counting. Per Q8 / L10-D7, token counting reuses `arcan-core::context_compiler::estimate_tokens` (currently `fn`-private — pub-expose in J-Sub-F) and `life-vigil::pricing::PRICING_SNAPSHOT`. The codec crate stays free of cost/token concerns; tokens are a Vigil/Haima surface.

**Reference**: `Alishahryar1/free-claude-code` files listed in Spec J §11 — port each one as a Rust module with the same behavior.

**Acceptance**: `cargo test -p lifegw-anthropic-codec` ≥ 25 tests green; `bash scripts/verify_dependencies_lifegw_anthropic_codec.sh` exits 0; clippy clean with `-D warnings`.

**Worktree command**:
```bash
git worktree add -b feat/bro-XXXA-lifegw-anthropic-codec-scaffold \
  .worktrees/feat-bro-XXXA-lifegw-anthropic-codec-scaffold origin/main
```

**Subagent brief**: see Spec J §[Rust port surface] for file-by-file mapping; port each Python module with TDD (failing test → impl → green). Reference free-claude-code's `tests/core/anthropic/test_native_sse_block_policy.py` for block-policy test cases — port them 1:1.

### J-Sub-B — lifegw `/v1/messages` route + Tier-1↔Tier-2 wiring + arcan-proxy AnthropicArcan commit

**Estimate**: 5 working days (slightly larger after folding precursor, ~6 days realistic)
**Owner**: subagent on worktree `feat-bro-XXXB-lifegw-v1-messages-route`
**Blocked by**: J-Sub-A only (precursor folded in per Q2 resolution)
**Blocks**: J-Sub-D, J-Sub-E, J-Sub-F

**First commit (folded precursor)**: `feat(arcan-proxy): land AnthropicArcan ArcanCall adapter` — moves `crates/life-runtime/arcan-proxy/src/anthropic.rs` from working tree to committed history, with its module-level docs + the existing test (`parses_text_delta_and_finish`).

**Deliverables**:
- `crates/life-runtime/lifegw/src/services/anthropic_messages.rs` (new) — sibling of `agent_http.rs`, `anima_custody.rs`, `ws.rs`
  - `AnthropicMessagesBody` struct with `#[serde(deny_unknown_fields)]`
  - `router(state: AnthropicMessagesState) -> Router` exporting `POST /v1/messages`, `OPTIONS /v1/messages`, `HEAD /v1/messages`
  - Handler logic:
    1. Verify Tier-1 JWS via shared `AuthLayer::verify_tier1`
    2. Synthesize sid via `lifegw_anthropic_codec::synthesize_sid(req, did)`
    3. Mint Tier-2 cap via `Tier2Minter::mint(...)`
    4. `lifed.Agent.CreateSession{resume_sid: Some(sid)}` (idempotent — uses existing routing-cache hit-or-saga path)
    5. `lifed.Agent.SendMessage{sid, content}` (last user msg from body)
    6. `lifed.Agent.StreamSession{sid, from_sequence: 0}` returns `Stream<Item = pb::AgentEvent>`
    7. Wrap stream with `lifegw_anthropic_codec::Encoder` → `Stream<Item = AnthropicSseEvent>`
    8. Return as `Response::builder().body(StreamBody::new(...))` with `Content-Type: text/event-stream`
  - Heartbeat: ping every 15s during silence (per Spec J §[Streaming + Reconnect])
  - Timeout: 600s hard cap on the response body
- Mount the new router in `bootstrap.rs` next to the existing `anima_custody` and `agent_http` mounts
- Mount the `OPTIONS /v1/messages` probe per the same shape `agent_http.rs` uses (`Allow: POST, HEAD, OPTIONS`)
- 8+ integration tests in `tests/anthropic_messages_integration.rs`:
  - `simple_chat_completion`, `multi_turn_no_tools`, `auth_missing`, `auth_invalid`, `connection_drop_resume`, `rate_limit_engaged`, `large_request_body`, `unknown_anthropic_version`

**Acceptance**: `cargo test -p lifegw -- anthropic_messages` ≥ 8 green; `cargo run -p lifegw` starts; manually `curl -N -H "Authorization: Bearer dev-token-for-user1" -H "Content-Type: application/json" -d '{"model":"claude-sonnet-4-20250514","messages":[{"role":"user","content":"hello"}],"max_tokens":100,"stream":true}' http://localhost:8443/v1/messages` returns a clean SSE stream.

**Subagent brief**: model the handler tightly on `services/agent_http.rs::create_session_handler` (same `AgentHttpState` pattern; same Tier-1 verify → Tier-2 mint → upstream call shape). The novel part is the SSE response body — use `axum::response::sse::Sse::new(...)` and feed it the codec's stream.

### J-Sub-C — sid synthesis + stateless conversation mapping

**Estimate**: 2 working days
**Owner**: folded into J-Sub-A (`src/sid.rs` is part of the codec crate)
**Status**: covered by J-Sub-A; this ticket exists only as a placeholder for the synthesis logic. If needed as a separate dispatch, it parallelizes with J-Sub-A.

**Deliverables**: `lifegw_anthropic_codec::synthesize_sid(req: &AnthropicMessagesRequest, did: &str) -> Sid`
- Canonicalizes first user message (strip tool_result re-injection prefix, normalize whitespace)
- Hashes with `sha256(format!("{did}::{canon}"))`
- 16-hex-char prefix → `claude-code:abc123def456...`
- 4 unit tests: deterministic, sensitive to DID, sensitive to content, robust to whitespace

### J-Sub-D — tool-use bridge (HTTP ToolAwait semantics)

**Estimate**: 4 working days
**Owner**: subagent on worktree `feat-bro-XXXD-lifegw-tool-use-bridge`
**Blocked by**: J-Sub-B
**Blocks**: J-Sub-G

**Deliverables**:
- `lifegw_anthropic_codec::Encoder` handles `pb::AgentEventKind::ToolCallEmit` (verify variant exists; if not, file precursor proto bump)
- Tool-use content block emission with full JSON-streaming (input_json_delta chunking)
- On `CloseCode::ToolAwait` from upstream → emit `message_delta{stop_reason: "tool_use"}` + `message_stop`
- Next request with `tool_result` in `messages[-1]` re-injects via `lifed.Agent.SendMessage` with the lago-recoverable session
- 6+ integration tests: `tool_use_single_round`, `tool_use_two_rounds`, `tool_use_multi_tool_simultaneous`, `tool_use_with_partial_json`, `tool_use_after_thinking`, `tool_use_with_error`

**Acceptance**: integration tests green; manual smoke with `claude --tool-use` round-trips against a local lifegw + mocked lifed.

### J-Sub-E — Vigil GenAI semconv spans + haima billing

**Estimate**: 3 working days
**Owner**: subagent on worktree `feat-bro-XXXE-lifegw-anthropic-vigil-haima`
**Blocked by**: J-Sub-B
**Blocks**: J-Sub-G

**Deliverables**:
- Span: `life.anthropic.messages` (root) with attributes per Spec J §[Vigil span emission]
- Child spans: `life.anthropic.sid_synthesis`, `life.anthropic.auth_verify`, `life.anthropic.haima_check`, `life.anthropic.codec_encode` (aggregated)
- W3C `traceparent` propagated to upstream `lifed.Agent.StreamSession`
- `haima_check(did, estimated_cost)` before stream start; returns `Ok | Err(InsufficientCredits)`
- On Err: `402 Payment Required` with x402 challenge body per Spec J §[Cost gate]
- On stream complete: `haima_settle(did, actual_usage, backend_price)` emits `haima.charged` lago event
- 4+ integration tests: `vigil_span_emitted`, `haima_check_passes`, `haima_check_fails_402`, `haima_settle_on_complete`

**Acceptance**: Vigil span visible in OTLP exporter; `haima ledger show <did>` reflects new entries; integration tests green.

### J-Sub-F — `/v1/models` + `/v1/messages/count_tokens`

**Estimate**: 2 working days
**Owner**: subagent on worktree `feat-bro-XXXF-lifegw-anthropic-models-tokens`
**Blocked by**: J-Sub-B
**Blocks**: nothing (parallelizable with J-Sub-D, J-Sub-E)

**Deliverables**:
- `GET /v1/models` handler in `services/anthropic_messages.rs`
- Static model list (Phase 1) per Spec J §[Model picker]; placeholder for Spec E backend discovery integration in Phase 2
- `POST /v1/messages/count_tokens` handler that:
  - Calls `arcan_core::context_compiler::estimate_tokens` (currently `fn`-private at line 85 — small precursor commit in this sub-PR pub-exposes it)
  - Emits `life.anthropic.count_tokens` Vigil span with `gen_ai.usage.input_tokens` + `life.estimated_cost_usd_micros` (from `life_vigil::pricing::lookup_model`)
  - Optionally adds `X-Life-Cost-Estimate-Usd-Micros: <n>` response header
  - Returns Anthropic-compat `{"input_tokens": <usize>}` body
- Probe endpoints (`HEAD`, `OPTIONS`) for both
- 6+ tests: `models_endpoint`, `models_with_no_thinking_variants`, `count_tokens_simple`, `count_tokens_multi_turn`, `count_tokens_with_tools`, `count_tokens_vigil_span_emitted`

**Precursor inside this sub-PR**: `pub use context_compiler::estimate_tokens` (or `pub fn`) in `arcan-core`. Tiny API surface change; gated by existing arcan-core tests. Document the export in arcan-core/CLAUDE.md.

**Acceptance**: Claude Code's `/model` picker shows the gateway-discovered list; `/v1/messages/count_tokens` returns plausible counts (±5% of anthropic.com reference).

### J-Sub-G — E2E smoke

**Estimate**: 2 working days
**Owner**: human + subagent
**Blocked by**: J-Sub-D, J-Sub-E, J-Sub-F
**Blocks**: Phase 1 merge

**Deliverables**:
- Deploy lifegw to staging (Railway or VPS) with this branch
- Point real Claude Code CLI at it: `ANTHROPIC_BASE_URL=https://lifegw-spec-j.broomva.dev`, `ANTHROPIC_AUTH_TOKEN=<dev-token>`
- Run a real coding session for ≥ 15 minutes including:
  - ≥ 3 tool calls (file read, edit, bash)
  - ≥ 1 connection drop + recovery
  - ≥ 1 `/model` switch via Claude Code's picker
- Capture:
  - Screen recording (Loom or similar)
  - Vigil trace in Langfuse/Tempo
  - `lago replay --tree <synthesized_sid>` output
  - Haima ledger entries
- Write up at `docs/conformance/2026-05-XX-claude-code-smoke.md`

**Acceptance**: clean session, evidence captured, no regressions vs upstream Anthropic API at the Claude Code feature level.

## Dispatch sequence (when user nods)

```
Day 1:  Dispatch J-Sub-A (codec scaffold)        — subagent A worktree
        Dispatch J-Sub-C inside J-Sub-A's worktree (sid module is the codec's sid.rs)
Day 6:  J-Sub-A merged → unblock J-Sub-B
Day 7:  Dispatch J-Sub-B (lifegw route)          — subagent B worktree
        First commit in this PR: land arcan-proxy::AnthropicArcan (folded precursor per Q2)
Day 13: J-Sub-B merged → unblock J-Sub-D, J-Sub-E, J-Sub-F
Day 14: PARALLEL dispatch:
        - J-Sub-D (tool-use bridge)              — subagent D worktree
        - J-Sub-E (vigil + haima)                — subagent E worktree
        - J-Sub-F (models + count_tokens via existing estimator) — subagent F worktree
Day 18: All three merged → unblock J-Sub-G
Day 19: J-Sub-G smoke
Day 20: Phase 1 done, BRO-1140 closes
```

Critical path now ~17-20 working days (was ~16 — folding precursor adds ~1 day to J-Sub-B for the AnthropicArcan commit + tests). Trade-off accepted per Q2 resolution.

## Risks during execution

1. **`pb::AgentEventKind::ToolCallEmit` / `Thinking` variants may not exist** — discovered during J-Sub-A's encoder work. If absent, file a small `life-runtime-proto` bump as a precursor to J-Sub-D (NOT a blocker for J-Sub-A's text-only path).
2. **arcan-proxy::AnthropicArcan upstream behavior may not match Anthropic exactly** — its SSE parser is text-only (line 270-305 in `anthropic.rs`), no tool_use parsing. J-Sub-D may need to extend the parser too. **Mitigation**: J-Sub-D's first task is to verify; if extension is needed, fold in.
3. **arcan-proxy uses `ArcanCall`, not `InferenceBackend`** — fine for Phase 1; Phase 2's J-Sub-H closes this gap.
4. **lifegw bootstrap.rs router-mount ordering matters** — must mount `/v1/messages` route BEFORE the catch-all WS/tonic fallback. The pattern at `agent_http.rs::router()` is exact-route match; follow it.
5. **Codec crate may attract creep** — keep it tightly scoped to wire-shape translation. Anything substrate-aware (auth, billing, sessions) belongs in lifegw.

## Cross-review (P20) plan

For every sub-PR with diff > 200 LOC OR public API change:
- Fire `cross-review pre-push --diff-base origin/main` before push
- Strata A (Codex CLI cross-vendor) preferred; Strata B (fresh-context subagent) as fallback
- Strata C (parallel: `superpowers:constructive-dissent`, `pr-review-toolkit:code-reviewer`, `critique`, `premortem`) always-on
- Anti-slop score must be ≥ 7/10 to push
- Verdict logged as PR comment

For J-Sub-A (the codec — most subtle code): also run `pr-review-toolkit:type-design-analyzer` because `EncoderState` / `BlockPolicyState` are central types with invariants.

## Janitor (P8) post-merge

After each sub-PR merges:
```bash
make janitor   # branch-janitor.sh: delete merged branch + worktree
git status     # must be clean
git worktree list   # must show no orphans
```

## Phase 1 acceptance (overall)

- All 7 sub-tickets (A through G) closed Done
- BRO-1140 in Done state
- `cargo test --workspace` green (~3800 → ~3900 tests with the additions)
- `cargo clippy --workspace -- -D warnings` clean
- `bash scripts/verify_dependencies_lifegw_anthropic_codec.sh` exits 0 in CI
- `docs/conformance/2026-05-XX-claude-code-smoke.md` exists with capture evidence
- Spec J marked Status: **Phase 1 Shipped** in the spec header

## What this plan deliberately does not cover

- Phase 2 (J-Sub-H Spec E unification + J-Sub-I Praxis tools + J-Sub-J life-claude launcher) — separate plan after Phase 1 ships.
- Public spec extraction (`claude-code-protocol` crates.io publish) — Phase 3+.
- Discord/Telegram bot ingress — Spec G — External Trigger Ingress; not in scope here.
- Cursor/Cline/Aider/OpenHands integration validation — assumed-to-work via the Anthropic protocol contract; verify each opportunistically.

---

*End of plan. Awaiting user approval to dispatch.*
