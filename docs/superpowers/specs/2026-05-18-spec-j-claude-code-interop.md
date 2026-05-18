# Spec J — Claude Code Interoperability (Anthropic Messages ↔ lifegw)

**Date**: 2026-05-18
**Status**: Draft (Phase 0 — spec artifact only; implementation gated on user approval)
**Sibling of**: Spec C₃ (lifegw edge gateway) — same wire-shape-at-the-edge pattern, applied to Anthropic Messages instead of native lifed gRPC/WS.
**Owner**: `crates/life-runtime/lifegw/` + new `crates/life-runtime/lifegw-anthropic-codec/`
**Linear umbrella**: [BRO-1140](https://linear.app/broomva/issue/BRO-1140/spec-j-claude-code-interoperability-anthropic-messages-lifegw)
**Reference impl** (MIT, Python FastAPI, ~25k★): [Alishahryar1/free-claude-code](https://github.com/Alishahryar1/free-claude-code) — the "I'm pointing Claude Code at a proxy" UX is exactly what we replicate; the SSE block policy + tool-use handling is the wire-shape reference.

## Problem

Coding agents — Claude Code, Cursor, Cline, Aider, OpenHands, JetBrains ACP — have already converged on one client protocol: **Anthropic's Messages API**. Whoever owns that surface owns every coding agent's session, regardless of which model actually answers. Today every Claude Code conversation goes to `api.anthropic.com` directly. Three costs follow:

1. **No identity binding.** The conversation is keyless to Life's perspective — no AnimaCustody session, no anima DID, no rotation safety. Cross-device handoff is impossible.
2. **No event sourcing.** The conversation thread is not in lago. `lago replay --tree` can't reconstruct it; no compensating saga can revert a bad agent-edit; no Vigil span exists for the call.
3. **No metered substrate.** The user pays Anthropic directly. Haima never sees the call. There is no per-task billing, no x402 settlement, no usage receipt.

`free-claude-code` proves the user surface (point Claude Code at a proxy, never touch the agent again) works at scale — 25k★, daily-pushed, used by enough engineers that the project is its own attack surface for new providers. What it doesn't do is land the conversation in a real agent OS. **Spec J does both: replicates the proxy UX natively at the lifegw edge, and routes the request through the full Spec C/D/E stack.**

Strategically, this is the mirror of Spec E. Spec E is the bid for *POSIX of agent silicon* — own the runtime contract between inference engines and the agent loop. Spec J is the bid for *POSIX of agent client protocol* — own the wire between coding agents and the runtime. Both are leverage plays on existing-incumbent surface: NVIDIA's CUDA, Anthropic's Messages API.

## Solution

A single new axum router mounted in `lifegw` that speaks Anthropic Messages SSE inbound, and translates each request into the **existing** `lifed.Agent.{CreateSession, SendMessage, StreamSession}` machinery. No new substrate. No new fork of the abstraction stack. A small new codec crate (`lifegw-anthropic-codec`) owns the SSE encoding because the wire shape is finicky enough (block policy, thinking blocks, tool_use boundaries, message_delta usage counters) to deserve a unit-testable home.

Five distinguishing claims:

1. **Edge-only, substrate-free.** Phase 1 adds one router file + one codec crate. Zero changes to lifed, arcan, lago, anima, haima, autonomic. The infrastructure that already terminates `Agent.StreamSession` (currently `arcan-proxy::AnthropicArcan` outbound to `api.anthropic.com`) is reused unchanged. Ship cost: weeks, not months.

2. **Stateless sid synthesis.** Claude Code's HTTP protocol has no conversation ID — every request carries the whole `messages: [...]` array. lifegw derives a stable sid from `sha256(anima_did + canonical(first_user_message))[:16]` so resume-of-conversation maps to the same Life session. No client cookies, no session header convention required.

3. **Tool-use bridge via HTTP semantics.** Anthropic Messages tool calls aren't WebSocket — they're a single HTTP request that emits `tool_use` content blocks and then closes. Spec E §6.5 close-code `4010 ToolAwait` already names this pattern at the inference layer. The HTTP equivalent: emit the tool_use blocks in this response, close it cleanly, expect the *next* HTTP request to carry the `tool_result` (re-resumed via sid). Multi-turn tool flows are stateful at the lago event-sourced layer, not at the HTTP socket.

4. **Free composition with Spec C/D/E/F.** AnimaCustody session binding fires inside `Agent.CreateSession`. Tier-2 cap minting fires in lifegw's `AuthLayer`. Lago events emit from inside `lifed.Agent.StreamSession`. Vigil GenAI semconv spans wrap the `/v1/messages` handler. Haima per-call billing fires on the autonomic-budget gate. *All of this exists* — Spec J only adds the wire adapter.

5. **Anthropic-protocol is the substrate-of-coding-agents.** Cursor, Cline, Aider, OpenHands all speak it. One lifegw `/v1/messages` route → every coding-agent ecosystem becomes a Life client. Compare the alternative (write an integration plugin for each agent): N integrations vs 1 protocol.

## Locked Decisions

### L10-D1 — One router file, one codec crate. No new substrate.

The Anthropic surface lives at `crates/life-runtime/lifegw/src/services/anthropic_messages.rs` (sibling of `agent_http.rs`, `anima_custody.rs`, `ws.rs`). SSE encoding/decoding lives at `crates/life-runtime/lifegw-anthropic-codec/` (sibling of `life-runtime-pool`, `life-runtime-proto`).

**Rejected alternative**: new `crates/inference/inference-anthropic` Spec E backend. That conflates two abstractions — Spec E's `InferenceBackend` is an outbound silicon contract (host → model), while this is an inbound protocol adapter (client → runtime). Folding them creates a hairy bidirectional crate. Phase 2 *promotes* the existing `arcan-proxy::AnthropicArcan` to also `impl InferenceBackend` so the *outbound* path unifies — but the inbound edge path stays separate from Spec E.

### L10-D2 — Stateless sid synthesis with anima DID + first-message hash

Anthropic Messages has no `conversation_id`. Every request is a full `messages: [...]` array. lifegw synthesizes:

```rust
sid = format!("claude-code:{}", &hex::encode(sha256(format!(
    "{did}::{first_user_message_canonicalized}"
)))[..16]);
```

where:
- `did` is the user's Anima DID, recovered from the Tier-1 JWS claims at edge.
- `first_user_message_canonicalized` is the bytes of `messages[0].content` after stripping system-prompt prefix injections that Claude Code does for tool-result re-injection (specific bytes documented in §[Streaming + Reconnect](#streaming--reconnect)).

**Rejected alternative**: ephemeral sid per request. That loses KV cache reuse, breaks `lago replay --tree`, and turns every Claude Code message into a new Life agent. The 16-hex-char prefix gives `2^64` collision space per-anima — sufficient for the lifetime of any single user.

### L10-D3 — Tool-use exits across HTTP boundaries, conversation state lives in lago

Claude Code's tool-use pattern over Anthropic Messages is:
1. Request 1: user message → response with `tool_use` content block → SSE closes with `message_stop`.
2. Claude Code executes the tool locally.
3. Request 2: `messages: [..., assistant_with_tool_use, user_with_tool_result]` → response continues the assistant turn.

This maps perfectly to Spec E §6.5 `CloseCode::ToolAwait (4010)` *if* the conversation state is in lago, not held on the HTTP socket. lifegw closes the stream with `message_stop` (no anomaly — this is Anthropic's protocol); the *next* HTTP request from Claude Code re-arrives, sid is re-derived, the same Life session resumes from the lago event tail.

**Rejected alternative**: WebSocket upgrade for tool calls. Anthropic Messages clients don't speak WebSocket. The right WebSocket abstraction already exists (`/v1/agent/stream`, Spec C₃ M7-C); we're not Claude Code's transport designer, we're Claude Code's protocol target.

### L10-D4 — Codec crate is workspace-internal, not published

`lifegw-anthropic-codec` lives in the `core/life` Rust workspace and is **not** intended for crates.io. Reason: every Anthropic Messages quirk we encode against is **observed behavior of Claude Code v0.x**, not a stable spec. The same way Anthropic's own SDK encodes "what the API actually does" rather than "what the docs say it does," this crate encodes "what Claude Code actually sends." Drift is inevitable; pinning to workspace velocity beats pinning to a publish cadence.

If we later want to publish, the right shape is to extract a *subset* into a `claude-code-protocol` crate with explicit version compat — but only after Phase 2 stabilizes.

### L10-D5 — Anthropic `anthropic-version` header is honored; ours is `2023-06-01`

Claude Code sends `anthropic-version: 2023-06-01` (current as of Claude Code v0.x). lifegw rejects requests with unsupported versions via `400 Bad Request` so future Claude Code releases that bump the version surface a clear error before we silently regress on protocol drift.

### L10-D6 — `/v1/models` returns Spec E's discoverable backend list, not Anthropic's fixed list

The model picker (`CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`) calls `GET /v1/models`. lifegw returns:
- The set of `claude-*` model IDs Anthropic exposes (pinned list, ~7 entries — keeps `/model` autocomplete working for Claude Code defaults).
- The set of Spec E backend-advertised models (whatever the `InferenceRouter` knows about — MLX models, vLLM models, NVIDIA NIM models, etc.) prefixed with `life/<backend>/<model>`.
- For each Spec E model that supports thinking, both the model ID and a `<model>-no-thinking` variant — matching free-claude-code's `gateway_model_id` / `no_thinking_gateway_model_id` pattern. Routing maps `*-no-thinking` to the same backend with thinking disabled.

**Rejected alternative**: return only Anthropic's seven IDs. That loses Spec E composability — the whole point of Life as silicon-fanout is that any backend reaches Claude Code via the same protocol surface.

### L10-D7 — `count_tokens` runs at the edge, no upstream RPC

`POST /v1/messages/count_tokens` returns an estimate from `tiktoken`-equivalent (Rust: `tiktoken-rs`) without an upstream call. Claude Code uses it for compact-window budgeting. Round-tripping to lifed adds latency for what is fundamentally a tokenizer probe.

**Rejected alternative**: dispatch as a `lifed.Agent.CountTokens` RPC. That RPC doesn't exist and would require a new proto, a new lifed handler, and a new ArcanCall trait method for ~0 user-visible benefit. Edge-resolve.

### L10-D8 — `CLAUDE_CODE_AUTO_COMPACT_WINDOW=190000` is the documented client quirk; we trust the client

Free-claude-code sets `CLAUDE_CODE_AUTO_COMPACT_WINDOW=190000` when launching `fcc-claude` because Claude Code defaults to a smaller compact window and that interacts badly with backend-provided model size variance. lifegw does **not** force this — Claude Code launchers can set it themselves. We document the recommendation in the Phase 1 README.

**Rejected alternative**: lifegw enforces window size via a response header. Anthropic's protocol has no such header; injecting one is a protocol break.

## Architecture

### Wire shape mapping (1 of 4 spec axes — Section 1 of user brief)

```
Claude Code (CLI / VS Code / JetBrains ACP)
  │  POST /v1/messages
  │  Authorization: Bearer <ANTHROPIC_AUTH_TOKEN — Tier-1 JWS>
  │  anthropic-version: 2023-06-01
  │  body: {model, messages[], tools?, max_tokens, stream: true, thinking?}
  ▼
lifegw  (TLS 1.3 + Tier-1 verify + rate-limit + Tier-2 mint)
  │
  │  axum router → services/anthropic_messages.rs
  │
  │  1. verify Tier-1 JWS → AnimaDid
  │  2. synthesize sid = "claude-code:{sha256(did + first_msg)[:16]}"
  │  3. mint Tier-2 cap (scopes: ["agent:write", "agent:read"], aud=lifed, sub=did, sid)
  │  4. open lifed.Agent.StreamSession (resume by sid; create if absent)
  │  5. translate {messages[]} → lifed.Agent.SendMessage(sid, content=last_user_msg)
  │  6. stream lifed AgentEvents → encode each as Anthropic SSE chunks
  │  7. return HTTP 200, Content-Type: text/event-stream, chunked body
  ▼
lifed.Agent.StreamSession
  │  (existing — Spec C M5 sub-phase E)
  │  saga: create_session if absent → arcan create_agent + lago open_namespace
  │                                 + haima bind_wallet + anima register_session
  │  dispatch: arcan-proxy::ArcanCall → upstream backend
  ▼
upstream backend
  - Phase 1: arcan-proxy::AnthropicArcan → api.anthropic.com (today, uncommitted but real)
  - Phase 2: inference-core::InferenceBackend (Spec E router picks per call)
```

#### Anthropic SSE wire vs lifed `AgentEvent` wire — translation table

| Anthropic SSE event | `pb::AgentEvent` kind | Notes |
|---|---|---|
| `message_start` | `EventKind::SessionStart` (synthesized at edge if not in upstream stream) | Contains `id`, `model`, `role: "assistant"`, `usage: {input_tokens, output_tokens: 0, cache_*: 0}` |
| `content_block_start` (type=text) | first `EventKind::Token` of a text run | lifegw tracks "active block index" state |
| `content_block_delta` (text_delta) | each subsequent `EventKind::Token` for the same text run | Free-claude-code's `NativeSseBlockPolicyState` is the reference algorithm |
| `content_block_start` (type=thinking) | `EventKind::Thinking` (open) | Requires Spec C₂ extending `AgentEventKind` if not present |
| `content_block_delta` (thinking_delta) | `EventKind::Thinking` (delta) | |
| `content_block_start` (type=tool_use) | `EventKind::ToolCallEmit` | Carries tool_use_id, name, input |
| `content_block_delta` (input_json_delta) | `EventKind::ToolCallEmit` (with `partial_json` payload field) | Tool input is streamed JSON |
| `content_block_stop` | block boundary (synthesized at edge) | Marks block as closed; codec state machine drives the index |
| `message_delta` | `EventKind::Usage` (synthesized at message end) | Carries final `usage.output_tokens` + `stop_reason` |
| `message_stop` | `EventKind::Finish` | Equivalent of stream EOF |
| `ping` (Anthropic heartbeat) | synthesized at edge | Sent every 15s if upstream is quiet, to keep the HTTP socket alive |
| Error / abnormal close | `EventKind::Error` → Anthropic `error` event + HTTP 200 stream close | Anthropic's error events use SSE format, not HTTP status code |

**Critical**: The Anthropic stream encodes per-content-block lifecycle (start → delta* → stop), while `pb::AgentEvent` is a flat token stream. The codec crate's `BlockPolicyState` tracks block boundaries by inspecting `EventKind` transitions and emits the synthesized `content_block_start` / `content_block_stop` framing.

### Auth shape (Section 2 of user brief)

```
Claude Code env:
  ANTHROPIC_BASE_URL=https://lifegw.broomva.dev
  ANTHROPIC_AUTH_TOKEN=<Tier-1 JWS issued by broomva.tech>

Claude Code request:
  Authorization: Bearer <Tier-1 JWS>
  (Claude Code uses ANTHROPIC_AUTH_TOKEN as the Bearer credential — see fcc-server's behavior)

lifegw AuthLayer (existing M7-B):
  1. verify Tier-1 JWS against Vercel JWKS (kid rotation, single-flight cache)
  2. extract user_id, anima_did from claims
  3. apply rate-limit (per-user + per-IP token bucket, M7-D)
  4. Tier-2Minter.mint({sub: did, aud: "lifed", sid: <synthesized>, scopes: ["agent:write", "agent:read"]})
  5. forward Tier-2 cap on outbound tonic call to lifed
```

**No new auth machinery.** The same AuthLayer that gates `/v1/agent/create_session` (HTTP/JSON) and `/v1/agent/stream` (WS) gates `/v1/messages`. The Tier-1 → Tier-2 mint pattern is M7-B's bedrock contract.

**ANTHROPIC_AUTH_TOKEN format**: Tier-1 JWS issued by `broomva.tech/api/auth/tier1`. Users obtain it via the existing M9 onboarding flow (Spec H — also in flight as PR #1243). For developer convenience, `dev_signer_enabled=true` lifegw deployments accept `Bearer dev-token-for-<user_id>` as a Tier-1 short-circuit.

**Anthropic's `x-api-key` header**: Some Claude Code launchers send `x-api-key` instead of `Authorization: Bearer`. lifegw accepts both — `x-api-key: <Tier-1 JWS>` is treated as equivalent to `Authorization: Bearer <Tier-1 JWS>`. This is a Claude Code-specific quirk.

### Anima binding (Section 3 of user brief)

```
sid_synthesis(req: AnthropicMessagesRequest, did: AnimaDid) -> Sid {
    let first_user = req.messages.iter()
        .find(|m| m.role == "user")
        .expect("messages must contain a user turn");  // 400 if absent
    let canon = canonicalize(&first_user.content);     // strip tool_result re-injection
    let hash = sha256(format!("{did}::{canon}").as_bytes());
    format!("claude-code:{}", hex::encode(&hash[..8]))  // 16 hex chars
}
```

**Anima session lifecycle**:
- First request for an sid → `lifed.Agent.CreateSession{user_id, project_id="claude-code-default", label="cc:<sid_short>", resume_sid: None}` → saga binds anima session, opens lago namespace, binds haima wallet, registers anima session.
- Subsequent requests with same sid → `lifed.Agent.CreateSession{resume_sid: Some(sid)}` → cache hit on lifed routing cache → re-attached.

**Rotation safety**: anima identity rotation (Spec D) invalidates the cached lifed session. Next request for the same sid surfaces `Status::not_found` → lifegw re-runs the saga (transparent to Claude Code; one extra ~10ms saga cost at the rotation boundary).

**Project-id default**: free-claude-code has no concept of "projects" — all conversations are flat. Spec J defaults `project_id="claude-code-default"`. A future header (`X-Life-Project-Id`) can override; not in Phase 1 scope.

### Model picker (Section 4 of user brief)

```
GET /v1/models  →  {
  "data": [
    // Anthropic-named pinned set (lets Claude Code's hardcoded defaults work):
    {"id": "claude-opus-4-20250514",   "display_name": "Claude Opus 4",   ...},
    {"id": "claude-sonnet-4-20250514", "display_name": "Claude Sonnet 4", ...},
    {"id": "claude-haiku-4-20250514",  "display_name": "Claude Haiku 4",  ...},

    // Spec E backend-discoverable set (lifegw queries lifed.Agent.ListBackends OR
    // its own cache; Phase 1 starts with a static list, Phase 2 wires to Spec E):
    {"id": "life/mlx/llama-3.1-70b", "display_name": "Llama 3.1 70B (MLX)", ...},
    {"id": "life/vllm/qwen-2.5-72b", "display_name": "Qwen 2.5 72B (vLLM)", ...},

    // No-thinking variants for thinking-capable backends:
    {"id": "life/mlx/llama-3.1-70b-no-thinking", "display_name": "Llama 3.1 70B (MLX, no thinking)", ...},
  ],
  "first_id": "claude-opus-4-20250514",
  "has_more": false,
  "last_id": "life/mlx/llama-3.1-70b-no-thinking"
}
```

**Model selection inside `/v1/messages`**:
1. Claude Code sends `model: <id>` in the request.
2. lifegw resolves `<id>` → backend descriptor:
   - `claude-*` → AnthropicArcan (Phase 1) or any Spec E backend tagged as Anthropic-compatible (Phase 2).
   - `life/<backend>/<model>` → Spec E `InferenceRouter` dispatched to that backend.
   - `*-no-thinking` suffix → strip suffix, set `thinking_enabled=false` on the upstream call.
3. lifegw passes the resolved model identity through to `lifed.Agent.SendMessage` via the `metadata` field on the proto request (extension: add `optional string model_hint = 4;` to `SendMessageRequest` if absent).
4. Phase 1: if model isn't in the static list, fall back to AnthropicArcan with the literal model string (so a new Claude model works without lifegw updates).

### Tool use (Section 5 of user brief)

Anthropic tool-use wire shape (request → response → next request):

```
Request 1: messages=[user("read foo.txt")]
                      tools=[{name: "read_file", input_schema: {...}}]

Response 1:
  event: message_start
  data: {... role: "assistant", model: "claude-sonnet-4-20250514"}

  event: content_block_start
  data: {type: "text", text: "I'll read that file."}

  event: content_block_delta
  data: {type: "text_delta", text: "..."}

  event: content_block_stop

  event: content_block_start
  data: {index: 1, type: "tool_use", id: "toolu_01abc", name: "read_file", input: {}}

  event: content_block_delta
  data: {index: 1, type: "input_json_delta", partial_json: "{\"path\":"}

  event: content_block_delta
  data: {index: 1, type: "input_json_delta", partial_json: " \"foo.txt\"}"}

  event: content_block_stop
  data: {index: 1}

  event: message_delta
  data: {stop_reason: "tool_use", usage: {output_tokens: 47}}

  event: message_stop

(Claude Code executes read_file locally, gets "hello world")

Request 2: messages=[
  user("read foo.txt"),
  assistant([
    {type: "text", text: "I'll read that file."},
    {type: "tool_use", id: "toolu_01abc", name: "read_file", input: {path: "foo.txt"}}
  ]),
  user([
    {type: "tool_result", tool_use_id: "toolu_01abc", content: "hello world"}
  ])
]
```

**Mapping to Spec E §6.5 `CloseCode::ToolAwait (4010)`**:
- Inside lifed/arcan-proxy, the upstream stream closes with `CloseCode::ToolAwait` at the moment `tool_use` is emitted.
- lifegw catches this and emits Anthropic's `message_delta{stop_reason: "tool_use"} → message_stop`. HTTP stream closes cleanly (200 OK).
- The tool_use content block is included in the closing stream via the codec's block-policy state.
- Tool execution happens on Claude Code's side (Claude Code is the tool host; Praxis is **not** involved for Anthropic-protocol clients — they bring their own filesystem).
- Request 2 arrives; sid is re-derived from `first_user_msg = "read foo.txt"` (same as Request 1) so we hit the same Life session.
- The `tool_result` in Request 2's last user message gets forwarded as `lifed.Agent.SendMessage{content: <serialized tool_result>}`. The lifed session resumes from the tool-await state.

**Subtle case — Praxis-side tools**: Some Life sessions want to use Praxis tools (Spec C₁) for sandbox-isolated filesystem access. Claude Code can't see Praxis. Phase 1 punt: if Claude Code sends an empty `tools: []` array AND the upstream lifed session has Praxis tools defined, lifed's tool-call gets *executed inline* by Praxis and the result is folded into the response before `message_stop` (no Claude-Code-side tool round-trip). This is the inverse of the L10-D3 ToolAwait pattern. Mode selection: based on which tools the upstream session declares vs which Claude Code sent.

**Phase 1 simplification**: support **only** Claude-Code-host tools (the bring-your-own-tool path). Praxis-side tools are Phase 2 (J-Sub-D extension).

### count_tokens (Section 6 of user brief)

```
POST /v1/messages/count_tokens  →  {"input_tokens": <usize>}
```

Edge-resolved via `tiktoken-rs` (`cl100k_base` encoding, matching Anthropic's tokenizer family closely enough for budgeting). Free-claude-code uses Python's `tiktoken`; we use the Rust port.

**Accuracy caveat**: this returns an *estimate*, not the exact token count Anthropic would charge. Claude Code uses it for compact-window logic, where ±5% error is fine. Phase 2 can wire to a per-backend probe if needed.

### Streaming + reconnect (Section 7 of user brief)

**Anthropic SSE does not reconnect.** If the client loses the HTTP connection, Claude Code re-issues the request with the same `messages: [...]` array. From lifegw's perspective:

1. Request 1 starts streaming, sid synthesized.
2. Connection dies mid-stream (network blip, client cancel, deadline).
3. Request 2 arrives with the *same* `messages: [...]` array (Claude Code didn't see the assistant's response, so it doesn't include it).
4. sid re-derives identically (deterministic synthesis).
5. lifegw calls `Agent.StreamSession{sid, from_sequence: <last_seen>}` — uses the Spec C₃ M7-D `from_sequence` proto extension.
6. lifed streams from the last_sequence position. lifegw replays the missed portion of the assistant turn (or just streams from current head if message_stop already happened upstream).

**Important divergence from Anthropic's real API**: Anthropic doesn't replay. If you lose the connection mid-response, you get a *new* response from scratch. Life *does* replay because we have lago. This is a **safer** behavior — but it's an extension, and Phase 1 ships it as default. Phase 2 can add a header (`X-Life-Replay: never`) to match Anthropic's real semantics if any client breaks.

**Heartbeat**: free-claude-code emits a synthetic `ping` event every 15s during silence to keep the HTTP connection alive through L7 proxies that cut idle streams. lifegw does the same.

**Timeout**: Anthropic's API has a 10-minute hard cap on a single response stream. lifegw enforces 600s via tokio timer; on timeout, emit `message_delta{stop_reason: "stop_sequence"}` + `message_stop` + close. Claude Code interprets this as a stopped response and the user re-prompts.

### CLAUDE_CODE_AUTO_COMPACT_WINDOW (Section 8 of user brief)

Documented in Phase 1 README; no lifegw-side enforcement. Claude Code launchers (including `fcc-claude` and our future `life-claude` launcher) set this. The recommended value depends on the model's actual context window:
- Anthropic Claude 4 family: 190000 (matches free-claude-code default)
- Long-context backends (1M+ context): 950000
- 32K-context backends: 28000

A future `GET /v1/config` endpoint could surface the recommended value, but that's Phase 3+.

### Cost gate (Section 9 of user brief)

```
[anthropic_messages handler]
  ↓
  haima_check(user_did, estimated_cost) → Ok | Err(InsufficientCredits)
  ↓ (Err → 402 Payment Required with x402 challenge)
  ↓ (Ok → continue)
  ↓
[lifed.Agent.SendMessage]
  ↓
[arcan-proxy backend dispatches]
  ↓
[on response complete]
  haima_settle(user_did, actual_usage, backend_price) → Lago event "haima.charged"
```

**Estimation**: `estimated_cost = (input_tokens * backend.input_price + max_tokens * backend.output_price)`. Settlement is on actual usage from the `message_delta.usage` field.

**Per-backend pricing**: Spec E's `BackendCapabilities` will carry `price_per_input_token_usd_micros` and `price_per_output_token_usd_micros`. Phase 1 wires this to a static config table (one entry per known backend); Phase 2 reads from Spec E.

**Free tier**: deployments can disable haima gating via `cfg.billing.enforce = false`. The Vigil span still records the usage for telemetry; the haima settlement is a no-op.

**x402 challenge**: on Insufficient Credits, lifegw returns:
```
HTTP/1.1 402 Payment Required
X-Payment: {"chain": "base", "token": "USDC", "amount": "0.10",
            "facilitator": "https://haima.broomva.dev/x402"}
{"type": "error", "error": {"type": "billing_error", "message": "Insufficient credits"}}
```
Claude Code doesn't speak x402 natively (yet), but a wrapping launcher (`life-claude`) can intercept the 402 and trigger a top-up flow.

### Vigil span emission (Section 10 of user brief)

Each `/v1/messages` request creates one root span `life.anthropic.messages` with:
- `gen_ai.system` = `"life"`
- `gen_ai.operation.name` = `"chat"`
- `gen_ai.request.model` = resolved model ID
- `gen_ai.request.max_tokens` = from request
- `gen_ai.request.temperature` = from request (if set)
- `gen_ai.usage.input_tokens` = on stream complete
- `gen_ai.usage.output_tokens` = on stream complete
- `life.session.id` = synthesized sid
- `life.anima.did` = user DID
- `life.haima.cost_usd_micros` = on settlement
- `life.backend.id` = which upstream backend served the call
- `life.backend.kind` = `"anthropic-arcan"` / `"mlx"` / `"vllm"` etc.

Children:
- `life.anthropic.sid_synthesis` (~µs)
- `life.anthropic.auth_verify` (~ms, includes JWKS lookup)
- `life.anthropic.haima_check` (~ms, includes wallet read)
- `lifed.agent.stream_session` (~10ms saga + ongoing stream)
- `life.anthropic.codec_encode` (per-event ~µs, aggregated)

Spans propagate W3C `traceparent` to the upstream lifed call (already wired in M5 Sub-phase E).

### Rust port surface of `core/anthropic/*` (Section 11 of user brief)

free-claude-code's `core/anthropic/` Python module maps to Rust thus:

| free-claude-code file | Purpose | Rust home in Spec J |
|---|---|---|
| `core/anthropic/sse.py` | SSE event builder, `format_sse_event`, `ContentBlockManager`, tool-call state | `lifegw-anthropic-codec/src/encoder.rs` |
| `core/anthropic/native_sse_block_policy.py` | Per-upstream-block index remapping + overlap repair | `lifegw-anthropic-codec/src/block_policy.rs` |
| `core/anthropic/stream_contracts.py` | Wire-shape assertions for streamed events | `lifegw-anthropic-codec/src/contracts.rs` (test-only) |
| `core/anthropic/thinking.py` | Thinking-block lifecycle (open/delta/close, redaction policy) | `lifegw-anthropic-codec/src/thinking.rs` |
| `core/anthropic/tools.py` | tool_use block construction, tool_result reduction | `lifegw-anthropic-codec/src/tools.rs` |
| `core/anthropic/server_tool_sse.py` | Server-tool events (web_search, etc.) | `lifegw-anthropic-codec/src/server_tools.rs` (deferred to Phase 2; Claude Code's built-in tools are client-side) |
| `core/anthropic/tokens.py` | tiktoken wrapping for usage counters | `lifegw-anthropic-codec/src/tokens.rs` (uses `tiktoken-rs`) |
| `core/anthropic/conversion.py` | Anthropic ↔ OpenAI message format conversion | **NOT PORTED** — lifegw speaks Anthropic only; OpenAI shape is out of scope for Spec J (separate spec if/when needed) |
| `core/anthropic/native_messages_request.py` | Request validation (model required, tools format, etc.) | `lifegw/src/services/anthropic_messages.rs::AnthropicMessagesBody` |
| `core/anthropic/emitted_sse_tracker.py` | De-dup SSE events to prevent double-emission on retry | `lifegw-anthropic-codec/src/state.rs::EmittedTracker` |
| `core/anthropic/errors.py` | Anthropic error event format | `lifegw-anthropic-codec/src/errors.rs` |
| `core/anthropic/provider_stream_error.py` | Upstream error → Anthropic error event mapping | merged into `lifegw-anthropic-codec/src/errors.rs` |
| `core/anthropic/utils.py` | Misc helpers | inline / `lifegw-anthropic-codec/src/util.rs` |

**Lines of code rough estimate**: ~1.7 MB of Python → ~3-5 KLOC of Rust (most of the Python is in `providers/`, which we don't need; `core/anthropic/` itself is the meat and is much smaller).

### CI lane (Section 12 of user brief)

```bash
#!/usr/bin/env bash
# scripts/verify_dependencies_lifegw_anthropic_codec.sh
# Enforces Spec J L10-D1: codec crate is edge-only and substrate-free.

set -euo pipefail
cd "$(dirname "$0")/.."

forbidden_deps=(
  arcand
  lago-runtime
  lago-journal
  haima-runtime
  anima-runtime
  arcan-core
  arcan-harness
  arcan-aios-adapters
  inference-core   # codec MUST NOT pull Spec E; only AgentEvent proto
)

for dep in "${forbidden_deps[@]}"; do
  if cargo tree -p lifegw-anthropic-codec -e features 2>/dev/null | grep -q "^[│ ]*$dep "; then
    echo "FAIL: lifegw-anthropic-codec depends on $dep (forbidden by Spec J L10-D1)"
    exit 1
  fi
done

echo "OK: lifegw-anthropic-codec dependencies are clean"
```

Mounted as a CI lane mirroring `scripts/verify_dependencies_lifegw.sh` and `scripts/verify_dependencies_lifed.sh`.

`lifegw` itself depends on the codec; the codec's dependency rules are:
- MAY depend on: `life-runtime-proto`, `serde`, `serde_json`, `tokio`, `futures`, `bytes`, `tiktoken-rs`, `sha2`, `hex`, `thiserror`, `tracing`.
- MUST NOT depend on: anything in §11.2 of Spec C₂ (substrate runtimes). Codec is pure wire-shape translation.

### Conformance test plan (Section 13 of user brief)

`crates/life-runtime/lifegw-anthropic-codec/tests/` (≥ 40 tests for Phase 1):

| Suite | Cases | What it checks |
|---|---|---|
| `encoder_simple` | 6 | Single-block text streams encode to correct `message_start` → `content_block_*` → `message_stop` shape |
| `encoder_thinking` | 5 | Thinking blocks open/delta/close properly; `thinking_signature` propagated; redaction-mode dropped |
| `encoder_tools` | 8 | tool_use block start/delta/stop; tool input JSON streaming via `input_json_delta`; multi-tool sequences |
| `encoder_multi_block` | 6 | Text → tool_use → text re-open; correct block index allocation; block_policy_state correctness |
| `encoder_usage` | 4 | `message_delta` carries `usage`; `cache_creation_input_tokens` / `cache_read_input_tokens` propagated |
| `encoder_errors` | 5 | Upstream `EventKind::Error` → SSE `event: error\ndata: {...}` (NOT HTTP error); `overloaded_error`, `api_error`, `invalid_request_error` mapping |
| `decoder_request` | 6 | `messages: [...]` parsing; `tools: [...]` parsing; `thinking: {type: enabled}`; system prompt; `anthropic-version` validation |
| `sid_synthesis` | 4 | Deterministic for same input; differs for different `did`; differs for different `first_user_message`; stable across re-canonicalization noise |
| `count_tokens` | 3 | Token-count probe for simple text, multi-turn, tool-bearing inputs (±5% of anthropic.com reference) |
| `block_policy_state` | 6 | Up-stream block re-mapping; dropped-block accounting; overlap repair |

Plus `crates/life-runtime/lifegw/tests/anthropic_messages_integration.rs` (≥ 10 integration cases):

| Case | What it exercises |
|---|---|
| `simple_chat_completion` | One-shot user msg → assistant reply; sid synthesized; lago event tail visible |
| `multi_turn_no_tools` | Three-turn conversation; sid stable; lifed session resumed |
| `tool_use_round_trip` | Request 1 emits tool_use → Request 2 carries tool_result → conversation resumes |
| `auth_missing_returns_401` | No bearer → 401 |
| `auth_invalid_returns_401` | Garbage bearer → 401 (JWKS verify fails) |
| `rate_limit_engaged` | Burst beyond per-user cap → 429 `rate_limit_error` |
| `count_tokens_endpoint` | POST `/v1/messages/count_tokens` returns plausible int |
| `models_endpoint` | GET `/v1/models` returns Anthropic + Spec E list |
| `connection_drop_resume` | Drop mid-stream, re-request, lifegw replays from `from_sequence` |
| `large_request_body` | 100K-token input doesn't blow memory (streaming body parser) |

Plus a **live conformance smoke** (manual, not in CI): point Claude Code CLI at a local lifegw and run a real coding session for ≥ 15 minutes including ≥ 3 tool calls. Capture session transcript + screenshots into `docs/conformance/2026-05-XX-claude-code-smoke.md`.

### Risk matrix + failure modes (Section 14 of user brief)

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Anthropic Messages protocol drift (new content-block type added) | Medium (every 6mo) | Drop messages, Claude Code errors | Strict `anthropic-version` header; reject unknown content_block types with `400 Bad Request`; track upstream changes in `core/anthropic/` reference |
| Claude Code version drift (sends new headers / fields) | Medium | Silently breaks one client version | Test matrix against ≥ 3 Claude Code versions; smoke test on each Claude Code minor release |
| sid collision (two distinct conversations hash to same sid) | Very low (2^64 space per anima) | Wrong session reused; user sees stale tool_result | Cryptographic hash; if collision ever observed, switch to 24-hex prefix |
| Tool-use boundaries break across HTTP requests | Low | Tool execution loop hangs | Strong contract: lifegw closes stream with `message_stop` only after `content_block_stop` for every open block; integration test for partial-block crash recovery |
| Replay-from-`from_sequence` returns events Claude Code already saw | Medium | Claude Code sees a duplicate token run | `EmittedTracker` de-dup at codec level; if upstream `from_sequence` isn't honored (Phase 1 lifed wiring may be partial), fall back to "always replay full assistant turn after disconnect" |
| Haima cost-gate latency adds visible delay | Low (haima wallet check is ~1ms) | Slow first-token latency | Edge-cache wallet status with 5s TTL; surface to Vigil for monitoring |
| Spec E backend selection picks an unsuitable backend for Anthropic-shape request | Medium (Phase 2 only) | Wrong model behavior | Conservative routing default: `claude-*` requests prefer AnthropicArcan; `life/*` requests use the explicit backend; never auto-substitute |
| Free-claude-code's block-policy edge cases not covered in unit tests | Medium | Stream corruption on long tool_use inputs | Port their `tests/core/anthropic/test_native_sse_block_policy.py` cases 1:1 into Rust; add fuzz testing under `tests/encoder_fuzz.rs` |
| `tiktoken-rs` differs from Anthropic's actual tokenizer | High (known | ±5% count error | Document as "estimate"; future J-Sub-F2 adds per-backend `CountTokens` probe |
| WebSocket-wanting tool host (advanced agent) hits HTTP-only ToolAwait limit | Low | Multi-step tool dance with intermediate streaming impossible | Document the limit; advanced flows use `/v1/agent/stream` WS directly |
| ANTHROPIC_AUTH_TOKEN gets cached by VS Code extension and outlives Tier-1 expiry | High | Sudden 401 mid-session | Tier-1 default TTL is 24h; document; future J-Sub-H2 adds long-lived "Claude Code app password" Tier-0 surface |

### Sub-phase decomposition (Section 15 of user brief)

Phase 1 (target: 2-3 weeks via worktree fan-out):

```
J-Sub-A — codec crate scaffold + Anthropic SSE encoder
  ├── new crate crates/life-runtime/lifegw-anthropic-codec/
  ├── BlockPolicyState + EncoderState + EmittedTracker
  ├── pb::AgentEvent → SSE chunk translation
  ├── 25+ unit tests, fuzz harness
  └── duration: ~5 days
  blocked_by: nothing
  blocks: J-Sub-B (codec is a build dep)

J-Sub-B — lifegw /v1/messages route + Tier-1↔Tier-2 wiring
  ├── crates/life-runtime/lifegw/src/services/anthropic_messages.rs
  ├── axum router mounted in bootstrap.rs
  ├── request validation (#[serde(deny_unknown_fields)])
  ├── Tier-2 mint via existing minter
  ├── upstream lifed.Agent.{CreateSession, StreamSession} wiring
  ├── 8+ integration tests
  └── duration: ~5 days
  blocked_by: J-Sub-A
  blocks: J-Sub-D, J-Sub-E, J-Sub-F

J-Sub-C — sid synthesis + stateless conversation mapping
  ├── crates/life-runtime/lifegw-anthropic-codec/src/sid.rs
  ├── deterministic hashing with anima_did + canonical first user message
  ├── canonicalization (strip tool_result re-injection, normalize whitespace)
  ├── 4+ unit tests
  └── duration: ~2 days
  blocked_by: J-Sub-A
  blocks: J-Sub-B (B needs sid synth function)

J-Sub-D — tool-use bridge (CloseCode::ToolAwait HTTP semantics)
  ├── translation tool_use ↔ pb::AgentEvent
  ├── ToolAwait close-code handling in handler
  ├── tool_result re-injection on subsequent request
  ├── 6+ integration tests
  └── duration: ~4 days
  blocked_by: J-Sub-B
  blocks: J-Sub-G (E2E needs tools)

J-Sub-E — Vigil GenAI semconv spans + haima billing
  ├── span structure per §[Vigil span emission]
  ├── haima.check + haima.settle hook
  ├── x402 challenge response on InsufficientCredits
  ├── 4+ integration tests
  └── duration: ~3 days
  blocked_by: J-Sub-B
  blocks: J-Sub-G

J-Sub-F — /v1/models + /v1/messages/count_tokens
  ├── static model list (Phase 1) with extension hook for Spec E backends
  ├── tiktoken-rs probe
  ├── 4+ unit + 2 integration tests
  └── duration: ~2 days
  blocked_by: J-Sub-B
  blocks: nothing (parallelizable with D/E)

J-Sub-G — E2E smoke (Claude Code CLI ↔ deployed lifegw)
  ├── deploy lifegw to staging
  ├── point real Claude Code CLI at it
  ├── 15+ minute coding session, ≥ 3 tool calls
  ├── transcript + screenshot capture into docs/conformance/
  └── duration: ~2 days
  blocked_by: J-Sub-D, J-Sub-E, J-Sub-F
  blocks: Phase 1 merge

Critical path: J-Sub-A (5d) → J-Sub-B (5d) → [parallel: J-Sub-D (4d) || J-Sub-E (3d) || J-Sub-F (2d)] → J-Sub-G (2d) = ~16 working days
```

Phase 2 (deferred, separate spec amendment):

```
J-Sub-H — AnthropicArcan → InferenceBackend promotion
  ├── impl InferenceBackend for arcan-proxy::AnthropicArcan
  ├── E-Sub-F conformance battery extension
  ├── duration: ~3 days
  blocked_by: Spec E E-Sub-F completion

J-Sub-I — Praxis-side tool execution (inverse of L10-D3)
  ├── handler-side detection: empty client tools, populated lifed tools
  ├── inline tool execution before message_stop
  ├── duration: ~4 days
  blocked_by: J-Sub-G

J-Sub-J — life-claude launcher (`fcc-claude` analog)
  ├── apps/life-claude — Bun/Rust CLI that sets env + invokes claude
  ├── x402 challenge interception + topup flow
  ├── duration: ~3 days
  blocked_by: J-Sub-G
```

## Dependencies

### Upstream dependencies (must exist for this spec to work)

| Dependency | Location | Status |
|---|---|---|
| `lifegw` AuthLayer (Tier-1 verify + Tier-2 mint) | `crates/life-runtime/lifegw/src/auth/` | ✅ Shipped (M7-B / M7-D) |
| `lifegw` rate-limiter | `crates/life-runtime/lifegw/src/services/rate_limit.rs` | ✅ Shipped (M7-D) |
| `lifegw` HTTP/JSON wrapper template | `crates/life-runtime/lifegw/src/services/agent_http.rs` | ✅ Shipped (Stage 3a, May 2026) |
| `lifed.Agent.{CreateSession, SendMessage, StreamSession}` | `crates/life-runtime/lifed/src/services/agent.rs` | ✅ Shipped (M5 100%) |
| `lifed.Agent.StreamSession` `from_sequence` proto field | `proto/life/v1/agent.proto` | ✅ Shipped (M7-D D6) |
| `arcan-proxy::AnthropicArcan` outbound adapter | `crates/life-runtime/arcan-proxy/src/anthropic.rs` | ⚠️ Uncommitted on main (modified working tree) — Phase 1 depends on it landing first |
| `pb::AgentEventKind::ToolCallEmit` | `crates/life-runtime-proto/proto/life/v1/agent.proto` | ❓ Verify; may need new variant for Phase 1 |
| `pb::AgentEventKind::Thinking` | as above | ❓ Verify; may need new variant for thinking block support |
| Spec C₃ §6.5 close codes (incl. ToolAwait 4010) | `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md` | ✅ Shipped |
| Vigil GenAI semconv attributes | `crates/life-vigil/` | ✅ Shipped (M5 Sub-phase E OTLP exporter) |
| Haima per-call billing | `crates/haima/` | ✅ Shipped (Phase F0) |

**Critical pre-condition for J-Sub-B kickoff**: `arcan-proxy::AnthropicArcan` must be committed to main. The uncommitted state on the local tree is a blocker. **Action item before Phase 1 dispatch**: file a precursor ticket to commit/merge that file.

### Downstream dependencies (what depends on this spec)

| Consumer | Why it depends |
|---|---|
| Future `apps/life-claude` launcher | Wraps `claude` with `ANTHROPIC_BASE_URL=lifegw.broomva.dev` |
| Cursor / Cline / Aider / OpenHands integrations | Same protocol, same surface; one env-var flip per agent |
| Spec H (Onboarding & Capability UX) | Onboarding flow ends with "point your Claude Code at <gateway>" — Spec J is the technical realization |
| Spec E Phase 2+ public spec | Spec J's `/v1/models` surface is the demo of Spec E backend fan-out reaching real coding-agent clients |

### Conflicts / coordination

- **PR #1243 (Spec H — Onboarding)** is in flight; the onboarding "point Claude Code at us" CTA depends on Spec J being shippable. Coordination: Spec J Phase 1 should land before or alongside Spec H's CTA goes live.
- **`feat/spec-i-khora-substrate` worktree** is unrelated; no conflict.
- **`crates/life-runtime/arcan-proxy/src/anthropic.rs`** modification (uncommitted) is on main's local tree. Spec J depends on it; ideally it merges to main as a precursor PR.

## Validation surfaces (P11)

When implementation lands, the following surfaces produce evidence:

1. **Unit + integration tests** — `cargo test -p lifegw-anthropic-codec -p lifegw -- anthropic_messages` ≥ 50 cases green.
2. **CI dependency lane** — `bash scripts/verify_dependencies_lifegw_anthropic_codec.sh` exits 0.
3. **Live smoke** — Claude Code CLI session of ≥ 15 minutes, transcripted into `docs/conformance/2026-05-XX-claude-code-smoke.md`. At least 3 tool calls, at least 1 connection-drop recovery.
4. **Vigil traces** — Each test session produces a trace in Jaeger / Tempo / Langfuse showing `life.anthropic.messages` root → `lifed.agent.stream_session` child → backend span. Screenshot captured.
5. **Lago replay** — `lago replay --tree <synthesized_sid>` reconstructs the test conversation as durable events.
6. **Haima settlement** — `haima ledger show <user_did>` shows N entries (one per request) with non-zero amounts.
7. **Cross-model adversarial review (P20)** — `cross-review pre-push --diff-base origin/main` ≥ 7/10 anti-slop score; verdict logged in PR.

## Cross-references

- Spec C — Life Runtime Architecture: `docs/superpowers/specs/2026-04-25-life-runtime-architecture-spec.md` §L0–§L14
- Spec C₂ — lifed facade: `docs/superpowers/specs/2026-04-26-spec-c2-lifed-facade.md`
- Spec C₃ — lifegw edge gateway: `docs/superpowers/specs/2026-04-27-spec-c3-lifegw-design.md`
- Spec C₃ — close codes: `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md`
- Spec D — Anima production custody: `docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md`
- Spec E — Agent-Loop Compute Contract: `docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md`
- Spec H — Onboarding & Capability UX: PR [#1243](https://github.com/broomva/life/pull/1243) (in flight)
- free-claude-code reference: <https://github.com/Alishahryar1/free-claude-code> (MIT, ~25k★, daily-pushed)
- Reference impl files of high interest:
  - `core/anthropic/sse.py`, `native_sse_block_policy.py`, `thinking.py`, `tools.py`, `emitted_sse_tracker.py`
  - `api/routes.py` (POST /v1/messages handler)
  - `api/model_router.py` (tier-to-backend resolution)
  - `messaging/session.py` (tree-queued session model — *not* ported in Phase 1; reference for Phase 3+ Discord/Telegram ingress)

## Open questions (for user review before Phase 1 dispatch)

1. **Spec letter**: J. F=auth-tier-1, G=external-trigger-ingress, H=onboarding (PR #1243), I=Khora-agent-environment (worktree in flight). Confirm Spec J is the right letter, or assign different.
2. **arcan-proxy::AnthropicArcan**: uncommitted on local main. Action: file precursor PR to commit, or fold into J-Sub-B?
3. **Praxis-side tools (J-Sub-I)**: defer to Phase 2 as outlined, or worth including in Phase 1?
4. **Codec crate naming**: `lifegw-anthropic-codec` — alternatives: `life-anthropic-protocol`, `lifegw-protocol-anthropic`, `claude-code-protocol`. Naming locks the publishing-to-crates.io decision (L10-D4).
5. **life-claude launcher (J-Sub-J)**: ship in Phase 1 (so onboarding demo works end-to-end) or defer to Phase 2?
6. **Anthropic-version header policy**: hard-reject unknown versions (L10-D5) or warn-and-passthrough?
7. **Replay-on-disconnect default**: Life-replay (current draft) or Anthropic-no-replay (matches real API behavior)?
8. **`tiktoken-rs` accuracy vs per-backend probe**: tolerate ±5% estimation error in Phase 1, or invest in per-backend CountTokens RPC from the start?

## Phase 0 deliverables (this artifact)

- [x] This spec at `docs/superpowers/specs/2026-05-18-spec-j-claude-code-interop.md`
- [ ] Phase 1 plan at `docs/superpowers/plans/2026-05-18-spec-j-phase-1-lifegw-edge.md`
- [ ] Knowledge entity at `~/broomva/research/entities/concept/claude-code-interop.md`
- [ ] Linear umbrella ticket [BRO-1140](https://linear.app/broomva/issue/BRO-1140) created
- [ ] PR opened with this spec + plan + entity
- [ ] User review + sign-off → Phase 1 sub-tickets filed + worktree fan-out dispatched

---

*This spec is itself an instance of the **Crystallize (P16)** primitive: free-claude-code recurred enough in the user's attention (a one-week-old observation that became the conversation seed) to warrant a Life-side specification. The Phase 1 plan is the rule-of-three follow-through — three independent sub-streams (codec, route, conformance) executing in parallel.*
