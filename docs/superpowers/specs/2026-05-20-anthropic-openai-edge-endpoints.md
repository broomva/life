---
title: Spec — Anthropic Messages + OpenAI Chat Completions as Life Edge Endpoints
status: draft
created: 2026-05-20
area: life-runtime/lifegw
related:
  - docs/superpowers/specs/2026-05-07-spec-j-claude-code-interop.md
  - docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md
  - docs/superpowers/plans/2026-05-18-spec-j-phase-1-lifegw-edge.md
linear: TBD
---

# Anthropic Messages + OpenAI Chat Completions as Life edge endpoints

## TL;DR

Today the production "chat" surface that any browser caller (alpine-cabin, future SaaS embeds, third-party apps) lands on is `https://broomva.tech/api/chat`. That route is **Arcan-direct** — it bypasses lifegw entirely, talks to the Arcan runtime over `ARCAN_URL`, falls back to a raw Anthropic SDK call, and emits a Vercel-AI-SDK-shaped SSE stream that is neither Anthropic-compatible nor OpenAI-compatible.

This spec proposes the migration that closes that gap:

1. **New canonical edge endpoints**, both routed through lifegw → lifed → Anthropic provider, both observability-complete (anima identity, lago events, vigil traces, haima billing):
   - `POST /v1/messages` — Anthropic Messages API shape.
   - `POST /v1/chat/completions` — OpenAI Chat Completions shape.
2. **Deprecation** of `broomva.tech/api/chat` (Arcan-direct) on a 90-day window. New consumers MUST use one of the standard endpoints.
3. **Single shared agent loop** — both endpoints translate to the same internal `WireOutbound::SendMessage` envelope on the lifed session bus; the wire-shape difference is purely in/out at the edge.

The spec is intentionally scoped to *the wire and the deprecation arc*. It does not redesign the lifed agent loop or the Anthropic provider adapter; both already exist (Spec J Phase 1).

## Why this is non-negotiable for the next session

Three independent facts forced this:

1. **alpine-cabin** (a CC-BY-SA OSS project at `https://broomva.github.io/alpine-cabin/`) needs to call a Life endpoint from the browser to wire chat-driven cabin parameter updates. The CORS gap is being closed in a separate PR ([broomva.tech#187](https://github.com/broomva/broomva.tech/pull/187)). But once CORS works, the next blocker is wire shape: every off-the-shelf JS SDK speaks Anthropic or OpenAI; nobody speaks `{id, message, prevMessages, projectId}`.
2. **Vendor portability**. The Vercel AI SDK already speaks Anthropic + OpenAI natively. The `@anthropic-ai/sdk` and `openai` packages do too. Forcing every consumer through the custom `/api/chat` envelope is friction that costs us adoption.
3. **bstack architecture honesty**. The four pillars of self-operation (CLAUDE.md §Four Pillars) demand that compute, auth, and audit converge through a single substrate. Today `/api/chat` is the *only* surface that bypasses lifegw — meaning anima identity, lago event sourcing, vigil traces, and haima billing all silently break for chat traffic. Every other Life service routes through lifegw. Chat must too.

## Architectural map (today vs target)

### Today (two parallel architectures)

| Surface | Backend | Wire shape | Observability |
|---|---|---|---|
| `broomva.tech/api/chat` | Arcan → Anthropic SDK | Vercel AI SDK SSE, custom envelope | **None through Life stack** |
| `broomva.tech/api/life-proxy/*` | lifegw `/v1/agent/*` → lifed | lifegw session + WebSocket | Full: anima + lago + vigil + haima |

### Target (single architecture, multiple wire surfaces)

```
[browser] ─POST /v1/messages────────────┐
[browser] ─POST /v1/chat/completions────┤
[browser] ─POST /v1/agent/create_session┼─→ broomva.tech /api/life-proxy/* (Next route)
[CLI]     ─POST /v1/messages────────────┤        │
[CLI]     ─POST /v1/chat/completions────┘        ▼
                                          lifegw (life.broomva.tech)
                                                  │
                                                  ▼
                                          lifed (gRPC over UDS)
                                                  │
                                                  ▼
                                          Anthropic provider adapter
                                                  │
                                                  ▼
                                          api.anthropic.com / native /v1/messages
```

The fork at the top of the funnel is the **wire-shape adapter layer** — it normalizes Anthropic / OpenAI / native Life-session shapes into the same internal `SendMessage` envelope. Nothing downstream of the adapter cares which dialect the caller used.

## Wire shape — Anthropic Messages API

`POST /v1/messages` accepts the standard Anthropic Messages API request and emits the standard SSE event stream.

### Request
- Body: standard Anthropic shape (`model`, `messages[]`, `system`, `max_tokens`, `temperature`, `tools`, `tool_choice`, `stream`).
- `model`: validated against an allowlist exposed by lifed (the canonical model registry). Unknown models → 400.
- Auth: `Authorization: Bearer <tier1-jwt>` (same as lifegw direct). Anonymous calls 401. Browser callers obtain JWT via `broomva.tech /api/auth/api-token` (24h TTL, 7d refresh).
- Tools: forwarded as-is to the underlying agent. The adapter does **not** intercept tool calls — they round-trip through the lifed agent loop, surfacing as Anthropic `tool_use` content blocks in the response.

### Response (`stream: true`)
- SSE format identical to Anthropic's: `event: message_start | content_block_start | content_block_delta | content_block_stop | message_delta | message_stop | ping | error` with `data: {...}` payloads matching Anthropic's documented schemas.
- Mapping from internal `WireInbound::AgentEvent { agent_kind }` to Anthropic events:
  - `agent_kind=1` (TOKEN) → `content_block_delta` with `delta: { type: "text_delta", text: "..." }`
  - `agent_kind=2` (TOOL_CALL_PENDING) → `content_block_start` with `content_block: { type: "tool_use", id, name, input: {} }` followed by `content_block_delta` (`input_json_delta`) frames
  - `agent_kind=3` (TOOL_RESULT) → handled inside the agent loop, not exposed at the edge unless caller chose to surface tool results (advanced mode)
  - `agent_kind=4` (APPROVAL_REQUIRED) → custom Anthropic `ping` event with metadata; client can reply via separate approval endpoint (out of scope for v1)
  - `agent_kind=5` (FINISH) → `message_delta` (`stop_reason: end_turn`) + `message_stop`
  - `agent_kind=6` (ERROR) → `error` event with the standard Anthropic shape
  - `agent_kind=7` (HIBERNATE) → upgrades to long-poll mode; out of scope for v1

### Response (`stream: false`)
- Buffer all `content_block_delta` frames server-side, emit a single JSON response in Anthropic's non-stream shape (`{ id, type: "message", role: "assistant", content: [...], model, stop_reason, usage }`).

## Wire shape — OpenAI Chat Completions

`POST /v1/chat/completions` accepts the standard OpenAI request and emits the standard SSE stream.

### Request
- Body: standard OpenAI shape (`model`, `messages[]`, `stream`, `tools`, `tool_choice`, `temperature`, `max_tokens`, `n=1` only).
- `model`: same allowlist as Anthropic surface. Aliases supported (e.g., `claude-sonnet-4-20250514` → `anthropic/claude-sonnet-4`).
- Auth: identical to Anthropic surface.
- Tool shape: OpenAI `function` tools translated into Anthropic `input_schema` tool shape internally. Tool-result messages in `messages[]` (role: `tool`) translated back into Anthropic `tool_result` content blocks.

### Response (`stream: true`)
- SSE format identical to OpenAI's: `data: {...}\n\n` chunks with `choices[].delta.content`, terminated by `data: [DONE]\n\n`.
- Mapping from internal frames to OpenAI deltas: every TOKEN frame yields a `data: { ..., choices: [{ delta: { content: "..." } }] }` line. Tool-call frames yield `delta: { tool_calls: [...] }`. FINISH yields `finish_reason: "stop"` followed by `[DONE]`.

### Response (`stream: false`)
- Single JSON in OpenAI's non-stream shape (`{ id, object: "chat.completion", choices: [...], usage }`).

## Auth — three modes the edge supports

1. **Tier-1 JWT (primary)** — `Authorization: Bearer <jwt>` issued by `broomva.tech /api/auth/api-token`. 24h TTL, 7d refresh, includes `user_id`, `project_id`, `scopes`, `tier`. Validated by lifegw `JwksCache` against `https://broomva.tech/api/auth/jwks.json`.
2. **Browser session cookie (browser only)** — same as today's `/api/chat`. The Next.js edge route reads the Neon Auth session, mints a tier-1 JWT internally, and forwards. Lets browser callers from `broomva.tech` use the endpoint without explicit JWT handling.
3. **`BROOMVA_TOKEN` env var (CLI/SDK)** — same JWT as Tier-1, just stored in the user's env. The `@broomva/sdk` and Vercel AI SDK examples both surface this pattern. CLI tools that run server-side (CI, ops scripts) use this.

Anonymous access is **not** supported. Rate limits per `claims.sub` and per IP apply (existing lifegw token bucket).

## CORS — what changes

The static CORS block in `apps/broomva/next.config.ts` was replaced with `apps/broomva/middleware.ts` (PR [broomva.tech#187](https://github.com/broomva/broomva.tech/pull/187)). The middleware echoes `Origin` against an allowlist that includes:

- `https://broomva.tech` (canonical)
- `https://www.broomva.tech`
- `https://broomva.github.io` (GitHub Pages — alpine-cabin and future OSS demos)
- `http://localhost:*` (dev only — `NODE_ENV !== "production"`)
- `BROOMVA_CORS_EXTRA_ORIGINS` env var (comma-separated, hot-swap without redeploy)

`/v1/messages` and `/v1/chat/completions` will live under `/api/v1/*` on the Next.js app, so they inherit the same middleware automatically.

## Tool integration — chat-driven param updates (alpine-cabin use case)

The motivating use case is alpine-cabin's chat: a visitor types "make the cabin a bit taller and 0.5m narrower" and the 3D render updates in real time. The wire for that:

1. Browser sends `POST /v1/messages` with the user message **plus a tool definition** for `update_cabin_params`:
   ```json
   {
     "name": "update_cabin_params",
     "description": "Update cabin design parameters and immediately re-render the 3D model.",
     "input_schema": {
       "type": "object",
       "properties": {
         "updates": {
           "type": "object",
           "additionalProperties": { "type": "number" },
           "description": "Slider path → new value. Allowed paths: platform.width_m, platform.depth_m, envelope.enclosed_depth_m, envelope.terrace_depth_m, aframe.apex_height_m, aframe.portal_count, aframe.purlin_rows_per_side, columns.anchors_per_column."
         },
         "explain": { "type": "string" }
       },
       "required": ["updates"]
     }
   }
   ```
2. Claude responds with `content_block_start` of type `tool_use`, streams the `input_json_delta`, then `content_block_stop`.
3. The browser parses the `tool_use.input.updates`, validates each path against the slider whitelist (min/max/step from `web/js/ui.js`), applies to `liveParams`, and the existing `scheduleViewerRebuild()` re-renders.
4. The browser sends back a `tool_result` message confirming the apply, and Claude (in a second turn) summarizes what changed in natural language.

The agent loop never executes the tool server-side — it's a *client-executed* tool, which is the standard Anthropic pattern. The Life stack just routes the bytes.

## Migration plan — deprecating `/api/chat` (Arcan-direct)

### Phase 0: pre-work (this spec)
- Land CORS middleware (PR #187). **Done in parallel with this doc.**
- Confirm Anthropic provider adapter in lifed (Spec J Phase 1, already shipped) supports tool definitions in the request payload. If not, add it before Phase 1.

### Phase 1: build the two new endpoints (4 days est.)
- Create `apps/broomva/app/api/v1/messages/route.ts` and `apps/broomva/app/api/v1/chat/completions/route.ts`.
- Each route opens a lifegw session via the existing `lib/life-runtime/agent-session/lifed-ws-client.ts`, translates the inbound request to `WireOutbound::SendMessage`, streams `WireInbound::AgentEvent` frames out as the appropriate SSE shape.
- Tool definitions in the request are passed verbatim to the lifed agent, which forwards them to the Anthropic provider. Tool calls in the response surface as Anthropic / OpenAI shape per the wire mapping above.
- Tests: contract suite that asserts wire-byte equivalence with reference Anthropic / OpenAI fixtures (the `@anthropic-ai/sdk` and `openai` packages have public SSE fixtures we can golden-test against).

### Phase 2: SDK + docs (2 days)
- Update `docs.broomva.tech/docs/sdk/typescript` to document both endpoints (replace the Vercel-AI-SDK-only example).
- Add curl + `@anthropic-ai/sdk` + `openai` + `@ai-sdk/anthropic` + `@ai-sdk/openai-compatible` examples to the docs.
- Publish `@broomva/sdk` v1.0 with both shapes exposed as first-class.

### Phase 3: alpine-cabin chat (1 day)
- Wire `web/js/cabin-chat.js` in alpine-cabin to call `/v1/messages` with the `update_cabin_params` tool.
- Settings pane: "paste your `BROOMVA_TOKEN`" until the JWT-redirect flow is built.

### Phase 4: announce deprecation of `/api/chat` (T+0)
- 90-day window. Add `Deprecation: true` + `Sunset: <T+90 day ISO>` + `Link: </docs/sdk/typescript>; rel="successor-version"` response headers to `/api/chat`. Emit a vigil counter every time `/api/chat` is hit so we can track migration progress.

### Phase 5: hard-remove `/api/chat` (T+90)
- Once the vigil counter shows <1% of pre-T+0 traffic, delete the route. Remove `executeViaArcan` and `createCoreChatAgent` from broomva.tech. The Arcan runtime stays — it's still useful for non-chat agent workloads — but the chat path no longer touches it.

## Open questions

1. **Tool registration for client-executed tools**. Today the Anthropic provider adapter in lifed accepts tools but I haven't verified end-to-end that *client-executed* tools (no server-side handler registered) round-trip cleanly. Confirm or add support in Phase 1.
2. **Streaming vs polling parity with the lifegw WebSocket flow**. The native `/v1/agent/stream` is a duplex WebSocket; the new endpoints are HTTP/2 SSE. The session abstraction in `lifed-ws-client.ts` opens a fresh session per request — re-using existing sessions (for the multi-turn conversation case) requires a `session_id` field in the request, which neither Anthropic nor OpenAI ship natively. Either (a) tunnel through a custom header `X-Life-Session-Id`, or (b) use the `conversation_id` extension Anthropic recently shipped, or (c) auto-derive session ID from a hash of the `messages[]` prefix (sticky session, no client cooperation needed). Recommend (c) for v1.
3. **Model alias surface**. Today the user passes `claude-sonnet-4-20250514` and it works. For OpenAI shape, do we accept `gpt-4o` as an alias for the same Claude model? Or only accept Claude IDs? Lean: accept both, the model registry resolves either to the same internal Claude binding. Adding a real GPT backend is a separate spec.
4. **Pricing / haima billing for Anthropic vs OpenAI shape**. Both shapes should bill identically per token, since the underlying inference is the same. Confirm haima's metering is on the lifed-side token count, not the wire-side.

## What this spec is NOT

- Not a rewrite of the lifed agent loop.
- Not a rewrite of the Anthropic provider adapter.
- Not a new auth scheme — uses existing Tier-1 JWT + Neon Auth session cookie + `BROOMVA_TOKEN` env var.
- Not a CORS spec — that's PR [broomva.tech#187](https://github.com/broomva/broomva.tech/pull/187).
- Not the alpine-cabin chat client itself — that's a downstream consumer.

## Decision points to close before Phase 1 starts

| # | Question | Default | Decision |
|---|---|---|---|
| D1 | Streaming session reuse strategy | (c) hash-based sticky session | TBD |
| D2 | OpenAI `gpt-*` model alias resolution | Accept aliases, map to Claude | TBD |
| D3 | Tool-result message round-trip for `messages[]` with `role: "tool"` | Translate to Anthropic `tool_result` content block | TBD |
| D4 | Deprecation window for `/api/chat` | 90 days | TBD |
| D5 | Auth modes — drop `BROOMVA_TOKEN` env var? | Keep (SDK/CLI ergonomics) | TBD |
| D6 | Should `/v1/messages` live on `broomva.tech` Next.js app or directly on lifegw? | Next.js (closer to existing auth + CORS) | TBD |

Lock decisions before code lands. The recommended path is the default column.

## References

- Spec J Phase 1 (Anthropic bridge in lifed): `docs/superpowers/specs/2026-05-07-spec-j-claude-code-interop.md`
- Spec E (agent-loop compute contract): `docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md`
- Lifegw smoke runbook: `docs/conformance/2026-05-18-claude-code-smoke-runbook.md`
- CORS middleware PR: [broomva.tech#187](https://github.com/broomva/broomva.tech/pull/187)
- Anthropic Messages API: <https://docs.claude.com/en/api/messages>
- OpenAI Chat Completions: <https://platform.openai.com/docs/api-reference/chat>
- alpine-cabin (motivating consumer): <https://github.com/broomva/alpine-cabin>
