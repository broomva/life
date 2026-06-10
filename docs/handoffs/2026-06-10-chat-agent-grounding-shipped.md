# Chat Agent — Grounding & Harness — Stage 1 (grounding shipped)

**TL;DR.** The chat agent's **grounding gap is closed and merged** (life
#1676 → `main` `447891bb`): the live agent now ships a source-controlled,
embedded default persona so it answers "Who is Carlos?" / "What is
broomva.tech?" factually instead of "I don't have enough context." The
prior handoff's suggested approach (lifed → arcan-praxis + context
compiler) is **forbidden by CI** and was corrected to a dependency-clean
embedded string. **FIRST ACTION:** clear the Railway `lifegw` env var
`LIFED_ARCAN_SYSTEM_PROMPT` (currently `"You are a helpful AI assistant.
Keep responses concise."`) so the grounded default takes effect in
production — an explicit override still wins by design, so until it's
cleared the live agent stays ungrounded. (Then re-run the E2E probe
below; then start the harness arc.)

## State of the world (P15 snapshot 2026-06-10)

- **broomva/life** — `main` @ `447891bb` (= squash-merge of #1676,
  "feat(arcan-proxy): ship grounded default chat-agent persona"). Working
  tree clean. The conductor workspace branch `medan` is a clean ancestor
  1 commit behind `main` (does NOT carry the change; `main` does).
- **PR #1676** — **MERGED** (`447891bb`). Post-merge `main` CI green:
  `Test (Linux)` ✅ + `Test (macOS)` ✅ (full `cargo test --workspace`),
  Lint/MSRV/Format ✅.
- **PR #1674** — still **OPEN** (`docs/chat-agent-grounding-handoff`, the
  Stage-0 handoff doc). Cross-linked with a comment pointing at #1676.
  Mergeable; it's docs-only. Decide: merge or supersede with this doc.
- **Railway `Life / production / lifegw`** — LIVE, unchanged since
  Stage 0. `LIFED_ARCAN_BACKEND=vercel_ai_gateway`,
  `OPENAI_MODEL=openai/gpt-5-mini`, `OPENAI_BASE_URL=https://ai-gateway.vercel.sh/v1`,
  key `vck_…` (free tier). **`LIFED_ARCAN_SYSTEM_PROMPT` is still set to a
  generic value** → the grounded default is NOT yet live (see FIRST ACTION).
- **Vercel `broomva-tech` prod** — LIVE; chat works end-to-end (outage
  fixed in Stage 0).
- **No local daemons running** in this code workspace; "live" = the
  Railway + Vercel deploys above.

## What Stage 1 delivered (don't redo it)

| PR | Merge SHA | Crate(s) / files | What it gave |
|----|-----------|------------------|--------------|
| life #1676 | `447891bb` | `crates/life-runtime/arcan-proxy/src/grounding.rs` (new) + `assets/chat_agent_persona.md` (new) | Embedded, source-controlled default grounding persona via `include_str!`; `resolve_system_prompt()` precedence (env override wins, else grounded default; never `None`). Dependency-clean (static string, zero new crate deps). |
| life #1676 | `447891bb` | `arcan-proxy/src/vercel_ai_gateway.rs`, `anthropic.rs`, `lib.rs` | Both `from_env()` backends now resolve through `grounding::resolve_system_prompt()`. Anthropic backend emits `system` as a **cacheable** content block (`cache_control: ephemeral`). +6 tests (4 resolver-precedence, 1 Vercel flow-through, 1 Anthropic). |

**Persona grounding source** (verifiable, in-repo only): `llms.txt`,
`.well-known/agent.json`, `README.md`. Carlos described with verifiable
maintainer facts only (`carlos@broomva.tech` per CODE_OF_CONDUCT, org
"Broomva" per agent.json) — the P20 reviewer caught and we removed the
unverifiable "creator/lead developer" superlative. Persona honestly
discloses that tool/sandbox use is NOT yet wired into the chat surface.

## The corrected architecture (critical — the Stage-0 handoff was wrong here)

The Stage-0 handoff (#1674) said *"make `lifed` depend on `arcan-praxis`"*
and use the `arcan-core` context compiler. **This is forbidden by
`scripts/verify_dependencies_lifed.sh`** (a CI lane): `lifed` and the
`*-proxy` crates MUST NOT depend on `arcan-core`, `arcan-lago`,
`lago-knowledge`, or `arcan-praxis` (substrate runtime crates). Verified
via 3 parallel Explore agents. Therefore:

- **Grounding** (Stage 1, done) = a dependency-clean static string in
  `arcan-proxy`. ✅
- **Tool harness + retrieval grounding** (the next arc) must live
  **behind the `arcand` gRPC boundary**, NOT in lifed. `arcand` already
  exposes `AgentSubstrate.DispatchMessage` (streaming →
  `KernelRuntime::tick_on_branch`, in `crates/arcan/arcand/src/substrate.rs`),
  but it is **Phase-1** (only TOKEN/FINISH/ERROR; no TOOL_CALL events).
  The full public-plane proto already has the kinds:
  `proto/life/v1/agent.proto` defines `AGENT_EVENT_KIND_TOOL_CALL_PENDING`,
  `_TOOL_RESULT`, `_APPROVAL_REQUIRED` (the substrate proto
  `proto/arcan/v1/substrate.proto` does not yet).

## E2E proof (re-runnable once the FIRST ACTION is done)

```bash
# Grounding probe — after clearing the Railway LIFED_ARCAN_SYSTEM_PROMPT
# override and redeploying lifegw:
CID=$(uuidgen); MID=$(uuidgen)
curl -s -N -X POST https://broomva.tech/api/chat -H 'Content-Type: application/json' -H 'Origin: https://broomva.tech' \
  --data "{\"id\":\"$CID\",\"message\":{\"id\":\"$MID\",\"role\":\"user\",\"parts\":[{\"type\":\"text\",\"text\":\"What is broomva.tech and who maintains it?\"}],\"metadata\":{\"createdAt\":\"2026-06-10T00:00:00Z\",\"parentMessageId\":null,\"selectedModel\":\"openai/gpt-5-mini\",\"activeStreamId\":null}},\"prevMessages\":[]}" --max-time 45
# Expected AFTER env cleared: a factual answer naming the Life Agent OS,
# Broomva, and Carlos (carlos@broomva.tech) — NOT "I don't have information".

# Unit-level proof (always passes, no deploy needed):
cargo test -p arcan-proxy        # 41 tests incl. grounding::* + default_grounding_flows_*
```

## First action

**Clear the production override so the grounded default goes live:**

```bash
# Railway (owner-only): project Life / env production / service lifegw
railway variables --service lifegw unset LIFED_ARCAN_SYSTEM_PROMPT
# then redeploy lifegw; then run the E2E grounding probe above.
```

If you'd rather keep an explicit override, set it to a *grounded* value
instead of the generic one — but the embedded default
(`arcan-proxy/assets/chat_agent_persona.md`) is now the maintained
source of truth, so unsetting is preferred.

## Pickup state (open threads ≤5)

- [ ] **Ship grounding to prod** — clear/replace the Railway
  `LIFED_ARCAN_SYSTEM_PROMPT` override (FIRST ACTION) + verify via probe.
- [ ] **Harness Phase-2 (next arc)** — extend `arcand`
  `AgentSubstrate.DispatchMessage` + `proto/arcan/v1/substrate.proto` to
  emit `TOOL_CALL_PENDING` / `TOOL_RESULT`; have `ArcanProxy` (UDS) pass
  them through. Tools execute via `arcan-praxis` *inside arcand*, never
  in lifed. Requires an `arcand` daemon deployed alongside lifed (ops).
- [ ] **Scopes** — broaden the chat session cap beyond `agent:dispatch`
  (`crates/life-runtime/lifegw/src/auth/jwks.rs`) to add `tool:` /
  `fs:` / pillar scopes + wire the `APPROVAL_REQUIRED` UI (broomva.tech
  already decodes it).
- [ ] **Per-query retrieval grounding** — once arcand publishes the
  tool-capable service, ground via `lago-knowledge` search instead of (or
  in addition to) the static persona.
- [ ] **Free-tier gateway key** (ops, owner-only, carried from Stage 0) —
  premium models 403, accessible ones 429 under load; top up Vercel AI
  Gateway credits / add BYOK.

## Related context

- **Prior handoff (Stage 0):** `docs/handoffs/2026-06-09-chat-agent-grounding-and-harness.md` (PR #1674, OPEN).
- **Decision record (in-repo, canonical):** the `grounding.rs` module
  rustdoc explains *why* a static string and *why* the harness belongs
  behind the arcand gRPC boundary (the dep-rule constraint).
- **Dep-rule gate:** `scripts/verify_dependencies_lifed.sh` (the CI lane
  that forbids substrate-runtime deps in lifed/`*-proxy`).
- **Harness surface (for the next arc):** `crates/arcan/arcand/src/substrate.rs`
  (`AgentSubstrate.DispatchMessage`), `crates/arcan/arcan-praxis/src/sandbox_runner.rs`
  (sandboxed tool exec), `crates/ergon/ergon/src/runtime.rs` (reusable
  Provider/ToolRegistry agent-loop traits), `crates/arcan/arcan-provider/src/anthropic.rs`
  (tool-calling provider).
- **Linear:** not created this session (Linear MCP unauthenticated, carried
  from Stage 0). Link BRO- IDs to #1676 when re-authed.
- **P20 review verdict:** APPROVE-WITH-NITS; MAJOR (unverifiable Carlos
  claim) fixed pre-merge; Anthropic prompt-caching + Anthropic test folded in.
