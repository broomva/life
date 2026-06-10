# Chat Agent — Grounding & Harness — Stage 0 (post-outage)

**TL;DR.** The broomva.tech chat *outage* is fully fixed, deployed, and dogfooded (it streams, multi-turns, and handles complex prompts). But the live agent is a **bare single-completion** — no harness, no tools, no FS, generic "helpful assistant" prompt, zero broomva.tech/KG grounding. The next arc lives in **this (life) repo**: turn `lifed`'s chat backend into a *grounded, tool-capable agent*. **FIRST ACTION:** in `crates/life-runtime/lifed/src/bootstrap.rs:305`, scope a new `ArcanBackendChoice` that runs a real agent loop over `arcan-praxis` instead of `VercelAiGatewayArcan`'s one-shot completion (read `crates/arcan/arcan-praxis/src/sandbox_runner.rs` + `crates/ergon/ergon/src/runtime.rs` first).

## State of the world (P15 snapshot 2026-06-09)

- **broomva/life** — `main` @ `87991f50` (= merge of #1673). `docs/handoffs/` exists. ~14 stale worktrees under `.worktrees/` + `../life-worktrees/` (P8 janitor fodder; mine already cleaned).
- **broomva.tech** — `main` @ `9abd839` (= merge of #245). PRs #243/#244/#245 merged.
- **Railway `Life / production / lifegw`** — LIVE. `LIFED_ARCAN_BACKEND=vercel_ai_gateway`, `OPENAI_MODEL=openai/gpt-5-mini`, `OPENAI_BASE_URL=https://ai-gateway.vercel.sh/v1`, key `vck_…` (**free tier**). Boots with `mock-fallback=true` (peripheral substrate UDS sockets missing — NOT the arcan backend; arcan is real `VercelAiGatewayArcan`).
- **Vercel `broomva-tech` prod** — LIVE, serving `9abd839`. Chat works end-to-end.
- **Debugging access** — `railway logs` (service `lifegw`) shows real arcan dispatch + provider status. `/api/life/health` "live" only checks config/reachability, NOT that the agent streams.

## What the outage arc delivered (don't redo it)

| PR | Merge SHA | Crate(s) / files | What it gave |
|----|-----------|------------------|--------------|
| broomva.tech #243 | `1d740a2c4` | `lib/life-runtime/agent-session/lifed-ws-client.ts` | `decodeAgentEvent` accepts BOTH short (`TOKEN`) and canonical (`AGENT_EVENT_KIND_TOKEN`) `agent_kind` — was dropping every frame. |
| broomva.tech #244 | `82e405ef2` | same file | Per-turn `stream()` loop `break`s on FINISH — lifed doesn't close the WS post-FINISH, so client must self-terminate (else 30s frame-deadline AFTER a complete answer). |
| broomva.tech #245 | `9abd839e3` | `app/(chat)/api/chat/route.ts` (`buildLifegwUserText`) | Multi-turn: prepends a role-labelled transcript of recent `previousMessages` to the content sent to lifegw (lifed sends NO history). |
| life #1673 | `87991f50e` | `crates/life-runtime/lifed/src/services/agent.rs` | Fan-out pump broadcasts `ERROR`+`FINISH` on arcan dispatch failure instead of a silent hang (so a 403/429 surfaces inline, not as a 30s timeout). |
| (env) | — | Railway `OPENAI_MODEL` | `anthropic/claude-sonnet-4-6` (403 on free tier) → `openai/gpt-5-mini`. **The single change that made tokens flow at all.** |

## The gap to close (this arc) — verified findings

The live path is `broomva.tech /api/chat → mint Tier-cap → Caddy → lifegw → lifed (UDS) → arcan-proxy → Vercel AI Gateway → model`. At the lifed boundary:

1. **No harness.** `VercelAiGatewayArcan` (`crates/life-runtime/arcan-proxy/src/vercel_ai_gateway.rs`) = `reqwest` + a custom SSE parser. `build_request_body` emits `{model, messages:[system,user], stream:true}` — **no `tools` array, no agent loop**. The `loop` at `:409` is SSE token parsing, not reason→act. Backends are only `mock` / `vercel_ai_gateway` / `anthropic` (`bootstrap.rs:305-328`); none tool-capable.
2. **No grounding.** `LIFED_ARCAN_SYSTEM_PROMPT` = literally *"You are a helpful AI assistant. Keep responses concise."* No KG, blogs, notes, or persona. Live probe: "Who is Carlos Escobar-Valbuena?" → "I don't have enough context…". It cannot answer FAQs factually.
3. **Capabilities exist but are NOT wired to chat.** `crates/arcan/arcan-praxis/src/sandbox_runner.rs` defines ephemeral sandboxes (Docker/nsjail/bubblewrap/Vercel Sandbox) with `SandboxCapabilitySet::FILESYSTEM_READ|WRITE` + command exec under `SandboxPolicy`. `crates/ergon` is a hand-rolled workflow runtime. `crates/arcan/arcan-provider/src/rig_bridge.rs` uses `rig-core`. The proto already has `TOOL_CALL_PENDING`/`TOOL_RESULT`/`APPROVAL_REQUIRED`/`ListTools`. **But `lifed` does NOT depend on `arcan-praxis`** — all of this is consumed only by the standalone `arcan` crate, not the chat dispatch.
4. **Scopes are minimal.** Chat session cap carries only `["agent:dispatch"]` (`crates/life-runtime/lifegw/src/auth/jwks.rs:513`). The Tier-User scope system (`auth/tier_user.rs`) supports fine-grained grants (e.g. `anima.user.sign_auth`, wallet scopes) but chat doesn't request them.
5. **Orphaned reference impl.** broomva.tech's old in-app engine `apps/broomva/lib/ai/core-chat-agent.ts` is a working AI-SDK harness (`streamText` + `stepCountIs(5)` + tools incl. `codeExecution` sandbox + KG tools `lib/ai/tools/knowledge-graph.ts` reading `scripts/generate-agent-knowledge.ts` output). It's the blueprint for what the lifed agent should do — bypassed by the lifegw migration (`ce3f619`).

## E2E proof (re-runnable)

```bash
# Outage fix — single turn streams clean, no error chunk:
CID=$(uuidgen); MID=$(uuidgen)
curl -s -N -X POST https://broomva.tech/api/chat -H 'Content-Type: application/json' -H 'Origin: https://broomva.tech' \
  --data "{\"id\":\"$CID\",\"message\":{\"id\":\"$MID\",\"role\":\"user\",\"parts\":[{\"type\":\"text\",\"text\":\"Say PONG\"}],\"metadata\":{\"createdAt\":\"2026-06-09T00:00:00Z\",\"parentMessageId\":null,\"selectedModel\":\"google/gemini-2.5-flash-lite\",\"activeStreamId\":null}},\"prevMessages\":[]}" --max-time 45
# Expected: text-start → text-delta(PONG) → text-end → [DONE], and NO {"type":"error"} chunk.

# Grounding gap (current failing FAQ — should pass after this arc):
#   ask "What is broomva.tech?" → today: "I don't have information about broomva.tech."
```

## First action

In `crates/life-runtime/lifed/src/bootstrap.rs` (the `ArcanBackendChoice` enum @ ~line 305 + `arcan_backend_from_env()` @ ~315), design a **tool-capable backend** that runs an agent loop (model → emit `TOOL_CALL_PENDING` → execute tool via `arcan-praxis` → feed `TOOL_RESULT` back → repeat until FINISH), gated by `APPROVAL_REQUIRED` for sensitive ops. Before writing, read `crates/arcan/arcan-praxis/src/sandbox_runner.rs` and `crates/ergon/ergon/src/runtime.rs` to decide: **reuse ergon's loop** vs. **a thin loop in `arcan-proxy`**. Keep `VercelAiGatewayArcan` as the no-tools fast path; select the new one via `LIFED_ARCAN_BACKEND`.

**If you want grounding shipped first (smaller, unblocks FAQs today):** set a rich `LIFED_ARCAN_SYSTEM_PROMPT` (Carlos bio + broomva.tech overview + top FAQ facts) as an immediate baseline, and/or inject the `agent-knowledge.json` bundle into the dispatched content (broomva.tech-side, mirrors `buildLifegwUserText` in `route.ts`). This needs no harness and is a config/route change.

## Pickup state (open threads)

- [ ] **Grounding** — KG/persona context into the dispatch (quick: system prompt; proper: per-query retrieval over `agent-knowledge.json` / `searchKnowledge`).
- [ ] **Harness + tools** — tool-capable arcan backend driving `arcan-praxis` (sandbox FS/exec) + the pillars (haima/anima/ergon) as tools; make `lifed` depend on `arcan-praxis`.
- [ ] **Scopes** — broaden the chat cap beyond `agent:dispatch` (`fs:`,`exec:`,`tool:`,pillar scopes) + wire the `APPROVAL_REQUIRED` UI (broomva.tech already decodes it).
- [ ] **Free-tier gateway key** (ops, owner-only) — premium models 403, accessible ones 429 under load. Top up Vercel AI Gateway credits / add BYOK keys.
- [ ] **Structured multi-turn history** — replace #245's transcript-prepend stopgap with proper role-structured history through the lifegw WS protocol (lifed-side).

## Related context

- Architecture + gotchas memory: `~/.claude/projects/-Users-broomva-broomva-broomva-tech/memory/chat-lifegw-architecture.md`, `chat-outage-2026-06-09-rootcause.md`
- Reference harness to port: `apps/broomva/lib/ai/core-chat-agent.ts` (broomva.tech) — AI-SDK `streamText` + tools.
- Linear: tickets NOT created this session (Linear MCP unauthenticated) — link BRO- IDs to #243/#244/#245/life#1673.
- Companion PR explainer for the outage fix lives in the PR bodies of #243/#244/#245 + life#1673.
