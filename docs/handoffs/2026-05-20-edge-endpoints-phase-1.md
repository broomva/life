---
title: Handoff — Edge Endpoints Phase 1 (Anthropic Messages + OpenAI Chat Completions)
type: handoff
created: 2026-05-20
status: ready-for-next-session
spec: docs/superpowers/specs/2026-05-20-anthropic-openai-edge-endpoints.md
prior_session_artifacts:
  - https://github.com/broomva/alpine-cabin/commit/d0b5526   # terrain + rocks + dock + 3D full-screen
  - https://github.com/broomva/alpine-cabin/commit/35b94a2   # realtime procedural cabin (sliders → 3D)
  - https://github.com/broomva/broomva.tech/pull/187          # CORS middleware (MERGED)
  - https://github.com/broomva/life/pull/1399                 # edge-endpoints spec (MERGED)
linear: TBD (create on Phase 1 start)
---

# Handoff — Edge Endpoints Phase 1

## TL;DR for the next session

You're picking up Phase 1 of the [edge-endpoints spec](../superpowers/specs/2026-05-20-anthropic-openai-edge-endpoints.md): build `POST /v1/messages` (Anthropic shape) and `POST /v1/chat/completions` (OpenAI shape) on `broomva.tech`, both routed through lifegw → lifed → Anthropic provider so we get the full bstack observability story (anima identity, lago events, vigil traces, haima billing) on chat traffic.

The unblocker work — CORS for browser callers, the spec itself — is already shipped. Two PRs landed today; both MERGED.

The deliverable is two new Next.js route handlers + a contract test suite that golden-tests the SSE wire-byte output against `@anthropic-ai/sdk` and `openai` reference fixtures.

**Before writing code:** lock decision points D1-D6 from the spec (defaults are reasonable; user input needed on D2, D4, D5).

## State snapshot (P15)

| Repo | Branch | Ahead/Behind | Working tree | Deploy state |
|---|---|---|---|---|
| `broomva/alpine-cabin` | `main` | 0/0 origin | clean | live @ <https://broomva.github.io/alpine-cabin/> serving `35b94a2` |
| `broomva/broomva.tech` | `main` | (per-clone) | varies per machine | Vercel auto-deploys on merge — PR #187 (CORS) landed today |
| `broomva/life` | `main` | (per-clone) | varies per machine | spec doc landed via PR #1399 |

Open PRs across the three repos: none related to this work after #187 + #1399 merged.

## What shipped today

Four commits / two PRs over two architecture layers:

1. **alpine-cabin: terrain + rocks + dock + 3D full-screen** (commit [`d0b5526`](https://github.com/broomva/alpine-cabin/commit/d0b5526))
   - `cad/cabin.py`: column tops aligned at platform_z, `build_terrain()` sloped polyhedron, `build_rocks()` Box-rotated boulders (lightweight: 311 KB GLB vs 6.7 MB with spheres), envelope-only GLB sidecar for validator.
   - `web/`: tab nav reduced from 7 → 6 (Parámetros removed), sliders inlined in Overview, dock panel in 3D tab, 3D fills viewport edge-to-edge.

2. **alpine-cabin: realtime procedural cabin** (commit [`35b94a2`](https://github.com/broomva/alpine-cabin/commit/35b94a2))
   - `web/js/cabin-builder.js` (NEW, 280 LOC): Three.js mirror of `cad/cabin.py` + `cad/envelope.py`. `CabinBuilder.rebuild(params)` constructs columns, platform, A-frame (rafters/ties/ridge/purlins), envelope (roof panels, glass, rear wall, deck), terrain wedge, box-boulders. Shared materials cached; geometries disposed on rebuild.
   - `web/js/viewer.js`: removed GLB loader, mounts builder + returns `update(newParams)` handle.
   - `web/js/app.js`: `scheduleViewerRebuild()` rAF-batched, called from `onSliderChange` + `resetAll`. Slider sync across Overview ↔ Dock via `data-path` match.

3. **broomva.tech: dynamic CORS middleware** (PR [#187](https://github.com/broomva/broomva.tech/pull/187), MERGED)
   - `apps/broomva/middleware.ts` (NEW): allowlist `{broomva.tech, www.broomva.tech, broomva.github.io, localhost:*}` plus `BROOMVA_CORS_EXTRA_ORIGINS` env override. Preflight 204 + echo Origin on match. `Vary: Origin` prevents cache poisoning.
   - `apps/broomva/next.config.ts`: removed static single-origin CORS block.

4. **life: edge-endpoints spec** (PR [#1399](https://github.com/broomva/life/pull/1399), MERGED)
   - `docs/superpowers/specs/2026-05-20-anthropic-openai-edge-endpoints.md`. Read this first in the next session.

## What's queued — Phase 1

From the spec §Migration plan §Phase 1:

1. Create `apps/broomva/app/api/v1/messages/route.ts` (Anthropic Messages shape).
2. Create `apps/broomva/app/api/v1/chat/completions/route.ts` (OpenAI Chat Completions shape).
3. Each route:
   - Validates auth (Tier-1 JWT via `Authorization: Bearer`, or browser session via existing helper).
   - Opens a lifegw session via `apps/broomva/lib/life-runtime/agent-session/lifed-ws-client.ts` (already exists, tested at `…/lifed-ws-client.test.ts` and `__tests__/lifed-ws-contract.test.ts`).
   - Translates inbound request → internal `WireOutbound::SendMessage` envelope (one envelope per user message in `messages[]`).
   - Streams `WireInbound::AgentEvent` frames out as the appropriate SSE shape per the spec wire-byte mapping table.
   - Handles `stream: false` (buffer + single JSON) and `stream: true` (SSE) consistently.
4. Contract test suite:
   - Anthropic golden fixtures: load expected SSE bytes for `message_start | content_block_start | content_block_delta | content_block_stop | message_delta | message_stop`. The `@anthropic-ai/sdk` package's test fixtures are a good source.
   - OpenAI golden fixtures: load expected `data: {...}\n\n` chunk format. `openai` package tests have these.
   - Use the in-process lifed test harness so contract tests don't depend on a deployed lifegw.
5. Update `docs.broomva.tech/docs/sdk/typescript` with curl + Anthropic SDK + OpenAI SDK + Vercel AI SDK examples for both routes.

Estimated: ~4 days for the two routes + tests, ~2 days for SDK/docs. Concurrent with Phase 2 if you fan out.

## Decision points to lock before code (D1–D6 from spec)

| # | Question | Recommended default | Action needed |
|---|---|---|---|
| **D1** | Streaming session reuse strategy (multi-turn conversations) | (c) hash-based sticky session — derive session ID from `messages[:N]` hash | Confirm with user. If they prefer (a) `X-Life-Session-Id` header, more flexible but requires SDK changes. |
| **D2** | OpenAI `gpt-*` model alias resolution | Accept `gpt-4o` etc. as aliases → resolve to Claude Sonnet in the model registry | **User input needed**: is this brand-honest (we say "OpenAI compat" but route to Claude), or should `gpt-*` aliases 400 until a real GPT backend lands? |
| **D3** | Tool-result round-trip for OpenAI `messages[]` with `role: "tool"` | Translate to Anthropic `tool_result` content block | Lock by default unless we hit a fixture-mismatch in tests. |
| **D4** | Deprecation window for `broomva.tech /api/chat` (Arcan-direct) | 90 days from announce date | **User input needed**: is 90 days right for the user base, or 30/180? |
| **D5** | Auth modes — keep `BROOMVA_TOKEN` env var? | Keep — SDK/CLI ergonomics demand it | **User input needed**: confirm. |
| **D6** | Routes live on `broomva.tech` Next.js app or directly on lifegw | Next.js — closer to existing auth + CORS + Vercel cold-start path | Lock by default. |

**Hard rule from CLAUDE.md §Ritual vs Substance:** if the spec recommends a default and you can't articulate a concrete reason to deviate, lock the default and move. Don't burn cycles on D1/D3/D6.

## Files the next session must read

In this order:

1. `docs/superpowers/specs/2026-05-20-anthropic-openai-edge-endpoints.md` (the spec — this handoff is a complement, not a replacement)
2. `crates/life-runtime/lifegw/src/services/agent_http.rs` (lifegw `/v1/agent/create_session` handler — this is what the new routes will proxy to via `lifed-ws-client.ts`)
3. `crates/life-runtime/lifegw/src/services/ws.rs` (`WireOutbound`/`WireInbound` enum definitions — wire envelope ground truth)
4. `proto/life/v1/events.proto` (proto definition for `agent_kind` numeric enum + `EventRecord` shape)
5. `apps/broomva/lib/life-runtime/agent-session/lifed-ws-client.ts` (existing TS client wrapping the WS — your new routes will use this)
6. `apps/broomva/app/(chat)/api/chat/route.ts` (the route being deprecated — read for context, not for emulation)
7. `apps/broomva/middleware.ts` (CORS middleware — verify the routes inherit it via the `/api/:path*` matcher; should be automatic)

## Validation plan template (P11)

Before any commit lands:
- Local: `bun run typecheck` + `bun run test` in `apps/broomva/` (CI runs same).
- Contract tests: run the new contract suite locally with the in-process lifed harness.
- Manual smoke: `curl -X POST http://localhost:3000/api/v1/messages -H "Authorization: Bearer $BROOMVA_TOKEN" -H "Content-Type: application/json" -d '{...}'` returns valid Anthropic SSE.
- Compare with reference: `npx tsx scripts/diff-anthropic-fixture.ts` (Phase 1 deliverable — write this) reads our SSE output and a fixture from `@anthropic-ai/sdk` test data, asserts byte-equivalence on the event-name + payload shape (not exact whitespace).
- After deploy: same curl against `https://broomva.tech/api/v1/messages` from a `https://broomva.github.io` Origin to verify the CORS middleware lets it through.

## Anti-patterns to avoid

- **Do not** re-implement the lifegw WebSocket protocol from scratch. `lifed-ws-client.ts` already exists and is tested. Wrap it; don't replace it.
- **Do not** add CORS to lifegw itself in this work unit. The Next.js middleware shipped in PR #187 is sufficient for browser callers via `broomva.tech/api/*`. Direct browser → `life.broomva.tech` is a separate spec (would need Caddy CORS or a `CorsLayer` in lifegw).
- **Do not** leave the Vercel-AI-SDK SSE format mixed into the new routes. Anthropic SSE and OpenAI SSE are distinct shapes; emit one or the other strictly per route. The custom Vercel-AI shape stays in `/api/chat` until that route is removed in Phase 5.
- **Do not** introduce a new auth scheme. Three modes (Tier-1 JWT, browser session, `BROOMVA_TOKEN` env) are enough.
- **Do not** start a new chat session per inbound HTTP request unless you've implemented D1's sticky-session strategy. A naive 1:1 mapping breaks multi-turn conversations.

## What I'd ask the user first thing next session

1. "Lock D2: should OpenAI `gpt-*` model IDs alias to Claude, or 400 until a real GPT backend lands?"
2. "Lock D4: 90-day deprecation window for `/api/chat` — too short, too long, or right?"
3. "Lock D5: confirm we keep `BROOMVA_TOKEN` env var as a first-class auth mode in the SDK."
4. "Should I create the Linear ticket for Phase 1 now? If yes, project/cycle?"

Once those four are locked, you can dispatch the two routes in parallel via Fanout (P5) — one subagent per route, fresh worktree each, contract tests in a third parallel worktree.

## Prompt for fresh session

A self-contained prompt is in the response body of the message that produced this handoff. Paste that into a new Claude Code session in any of the three repos (`broomva.tech` is the natural starting point since that's where the code lands).
