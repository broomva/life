# lifegw — examples

Dev-tooling examples for the lifegw edge gateway. Currently:

| Example | Purpose | Linear |
|---|---|---|
| `local_smoke` | Boot lifegw locally against an in-process mock lifed for ~30 s interactive smoke. Zero deploy, zero Railway slot. | [BRO-1165](https://linear.app/broomva/issue/BRO-1165) |
| `local_smoke_anthropic` | Real-Anthropic local smoke — daily dogfooding, no Railway slot. Same shape as `local_smoke`, but the upstream forwards to `api.anthropic.com` via `arcan_proxy::AnthropicArcan`. Requires `ANTHROPIC_API_KEY`. | [BRO-1185](https://linear.app/broomva/issue/BRO-1185) |

## `local_smoke`

Boots the real lifegw Anthropic Messages router (`/v1/messages`,
`/v1/models`, `/v1/messages/count_tokens`) on a kernel-picked `127.0.0.1`
port, wired against a mock `lifed.Agent` service running in-process over a
tempdir UDS. Same code paths as production (codec, auth, rate-limit, vigil
spans, stub haima) — only the substrate is mocked.

### Run

```sh
cargo run -p lifegw --example local_smoke
```

On boot you get:

```
lifegw local smoke ready:
  URL:    http://127.0.0.1:<port>
  Bearer: dev-token-for-broomva

Try:
  curl http://127.0.0.1:<port>/v1/models | jq .

  curl -N -H "Authorization: Bearer dev-token-for-broomva" \
       -H "anthropic-version: 2023-06-01" \
       -H "Content-Type: application/json" \
       -d '{"model":"claude-sonnet-4-20250514","messages":[{"role":"user","content":"hello"}],"max_tokens":100,"stream":true}' \
       http://127.0.0.1:<port>/v1/messages

Press Ctrl-C to exit.
```

The port the kernel picks varies per run; copy the printed value.

### Three curl recipes

#### 1. `GET /v1/models` — Anthropic-pinned static catalogue (unauthenticated)

```sh
curl http://127.0.0.1:<port>/v1/models | jq .
```

Expected shape (truncated):

```json
{
  "data": [
    { "id": "claude-opus-4-20250514",     "type": "model", ... },
    { "id": "claude-sonnet-4-20250514",   "type": "model", ... },
    { "id": "claude-haiku-4-20250514",    "type": "model", ... },
    { "id": "claude-sonnet-4-5-20250929", "type": "model", ... },
    { "id": "claude-haiku-4-5-20251001",  "type": "model", ... }
  ],
  "first_id": "claude-opus-4-20250514",
  "last_id":  "claude-haiku-4-5-20251001",
  "has_more": false
}
```

#### 2. `POST /v1/messages/count_tokens` — edge token-count probe

```sh
curl -s -H "Authorization: Bearer dev-token-for-broomva" \
     -H "Content-Type: application/json" \
     -d '{"model":"claude-sonnet-4-20250514","messages":[{"role":"user","content":"Please write a unit test for the new module."}]}' \
     http://127.0.0.1:<port>/v1/messages/count_tokens \
     -i
```

Expected:

```
HTTP/1.1 200 OK
content-type: application/json
x-life-cost-estimate-usd-micros: <positive integer>
...

{"input_tokens": <small integer>}
```

The `X-Life-Cost-Estimate-Usd-Micros` header is the prefetch hint Claude
Code uses to budget its context window; it's positive whenever the model
is in the Phase 1 pricing snapshot.

#### 3. `POST /v1/messages` — streaming chat (SSE)

```sh
curl -N -H "Authorization: Bearer dev-token-for-broomva" \
     -H "anthropic-version: 2023-06-01" \
     -H "Content-Type: application/json" \
     -d '{"model":"claude-sonnet-4-20250514","messages":[{"role":"user","content":"hello"}],"max_tokens":100,"stream":true}' \
     http://127.0.0.1:<port>/v1/messages
```

Expected SSE order (`-N` disables curl's buffering):

```
event: message_start
data: {...}

event: content_block_start
data: {...}

event: content_block_delta
data: {... "text": "Hello"}

event: content_block_delta
data: {... "text": " from"}

event: content_block_delta
data: {... "text": " lifegw"}

event: content_block_delta
data: {... "text": " local"}

event: content_block_delta
data: {... "text": " smoke!"}

event: content_block_stop
data: {...}

event: message_delta
data: {... "stop_reason": "end_turn"}

event: message_stop
data: {...}
```

Mock lifed emits the fixed token sequence
`Hello`, ` from`, ` lifegw`, ` local`, ` smoke!` then a `stop` Finish. The
real codec runs over it, producing the canonical Anthropic SSE wire shape.

### Pointing Claude Code at the example

```sh
export ANTHROPIC_BASE_URL=http://127.0.0.1:<port>
export ANTHROPIC_AUTH_TOKEN=dev-token-for-broomva
claude
```

Claude Code's `/model` picker queries `/v1/models` at bootstrap (matches
`api.anthropic.com`'s posture). Conversations route through `/v1/messages`
and exercise the same codec + auth + rate-limit + Vigil span paths the
production gateway uses.

### Known limits

- **Text-only mock.** Tool-use round-trips, drop+resume sid stability, and
  haima cost-gate failures all live in `tests/spec_j_e2e_smoke.rs`. The
  example is the *single happy-path* surface — if you need tool-use you
  drive the codec via the real `/v1/messages` route and `tool_use` payload
  shapes; this example's mock won't synthesise `tool_use` blocks.
- **No real substrate.** No lago durability, no anima identity, no haima
  ledger. The `StubHaimaClient` is wired (`billing_enforce = false`) so
  `/v1/messages` and `/v1/messages/count_tokens` pass the cost gate
  unconditionally. Production runs against haimad once that daemon
  exposes a `check`/`settle` RPC (Phase 2+).
- **Mocked `from_sequence` replay.** The mock returns an empty
  `StreamSession` reply on resume; lago-side cursor replay is the Railway
  staging path's responsibility (`docs/conformance/...claude-code-smoke-runbook.md`).
- **No TLS.** Plain HTTP on `127.0.0.1`. Production lifegw terminates TLS
  1.3 via rustls; the example skips the bind dance because operators are
  not testing TLS posture here.
- **Single mock session.** Each request creates a fresh `mock-sid-N`; the
  mock doesn't persist session state between calls. Use the in-process
  E2E test or Railway deploy to exercise resume / multi-turn semantics.

### Relation to the other test paths

Spec J Phase 1 ships four escalating test surfaces:

| Path | Command / artefact | Upstream | What it certifies |
|---|---|---|---|
| 1 | `cargo test -p lifegw --test spec_j_e2e_smoke` | mock | In-process E2E — codec, route, auth, recording haima, mock lifed; 5 scenarios. |
| 2 | This example (`cargo run -p lifegw --example local_smoke`) | mock | Real TCP socket, interactive — operator-driven probe of the edge wire over HTTP. |
| 3 | Railway staging deploy per `docs/conformance/2026-05-18-claude-code-smoke-runbook.md` | real (full saga) | Live conformance evidence — Loom + Vigil traces + lago replay + haima ledger. |
| 4 | `cargo run -p lifegw --example local_smoke_anthropic` | real (no saga) | Daily dogfooding — real Claude responses on a `127.0.0.1` socket without burning a Railway slot. |

Path 1 is automated; path 3 is the Phase 1 conformance gate;
**path 2 is the iterating-engineer's loop on the gateway with no
upstream cost**, and **path 4 is the iterating-engineer's loop with
real Claude responses** (every call hits `api.anthropic.com` and costs
real money). See the `local_smoke_anthropic` section below for the
cost warning and recipes.

## `local_smoke_anthropic`

Same shape as `local_smoke`, with **one substantive change**: the
upstream tonic `Agent` service forwards to
`arcan_proxy::anthropic::AnthropicArcan` instead of returning canned
events. Result: a real Claude Code ↔ lifegw ↔ `api.anthropic.com`
round-trip on `127.0.0.1:<port>` with only `ANTHROPIC_API_KEY` set —
no Railway deploy, no Vercel JWKS, no lifed saga, no haima ledger.

### Cost warning — read this first

Every `/v1/messages` call hits `api.anthropic.com` and is billed to
your `ANTHROPIC_API_KEY`. The active model defaults to **Claude Sonnet
4.5** (`claude-sonnet-4-5-20250929`); set `ANTHROPIC_MODEL` to
override. Approximate pricing (live numbers vary — check
[the Anthropic pricing page](https://www.anthropic.com/pricing) before
extended runs):

| Model | Tier | Cost posture |
|---|---|---|
| `claude-haiku-4-5-20251001` | small / fast | Cheapest — recommended default for iteration. |
| `claude-sonnet-4-5-20250929` | balanced | Moderate. The example's default if `ANTHROPIC_MODEL` is unset. |
| `claude-opus-4-20250514` | high-end | Expensive. Avoid for routine dogfooding. |

The 600 s `HARD_STREAM_TIMEOUT` cap inside the production
`anthropic_messages` router still applies — long thinking turns are
bounded by the same backstop the production gateway uses.

### Run

```sh
export ANTHROPIC_API_KEY=sk-...
# Recommended — Haiku is the cheapest model for iteration.
export ANTHROPIC_MODEL=claude-haiku-4-5-20251001
cargo run -p lifegw --example local_smoke_anthropic
```

Optional env knobs (consumed by `AnthropicArcanConfig::from_env()`):

| Var | Default | Purpose |
|---|---|---|
| `ANTHROPIC_API_KEY` | *(required)* | Your Anthropic API key. The example bails with a clean error if unset. |
| `ANTHROPIC_MODEL` | `claude-sonnet-4-5-20250929` | Active model. Set to Haiku for cheap iteration. |
| `ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | Override for proxy / VPC endpoints. |
| `ANTHROPIC_MAX_TOKENS` | `4096` | Per-turn cap. Lower for tighter cost control. |

On boot you get:

```
lifegw local smoke (real Anthropic upstream) ready:
  URL:    http://127.0.0.1:<port>
  Bearer: dev-token-for-broomva
  Model:  claude-haiku-4-5-20251001

WARNING: this binds to api.anthropic.com using your ANTHROPIC_API_KEY.
         Every /v1/messages call costs real money on the active model.
         Haiku (claude-haiku-4-5-20251001) is the cheapest; switch to
         it for iteration via ANTHROPIC_MODEL=...

Try:
  curl http://127.0.0.1:<port>/v1/models | jq .

  curl -N -H "Authorization: Bearer dev-token-for-broomva" \
       -H "anthropic-version: 2023-06-01" \
       -H "Content-Type: application/json" \
       -d '{"model":"claude-haiku-4-5-20251001","messages":[{"role":"user","content":"reply with the single word OK"}],"max_tokens":20,"stream":true}' \
       http://127.0.0.1:<port>/v1/messages

Press Ctrl-C to exit.
```

### Three curl recipes

#### 1. `GET /v1/models` — Anthropic-pinned static catalogue (unauthenticated)

```sh
curl http://127.0.0.1:<port>/v1/models | jq .
```

Identical posture to the mock-upstream example — the `/v1/models`
route is served by lifegw's static catalogue and does not hit
`api.anthropic.com`.

#### 2. `POST /v1/messages/count_tokens` — edge token-count probe

```sh
curl -s -H "Authorization: Bearer dev-token-for-broomva" \
     -H "Content-Type: application/json" \
     -d '{"model":"claude-haiku-4-5-20251001","messages":[{"role":"user","content":"Please write a unit test for the new module."}]}' \
     http://127.0.0.1:<port>/v1/messages/count_tokens \
     -i
```

Like the catalogue route, count_tokens runs at the edge and does not
hit `api.anthropic.com` — it estimates tokens via the encoder's local
heuristic and stamps the `X-Life-Cost-Estimate-Usd-Micros` header.

#### 3. `POST /v1/messages` — streaming chat (SSE) — **real Claude response**

```sh
curl -N -H "Authorization: Bearer dev-token-for-broomva" \
     -H "anthropic-version: 2023-06-01" \
     -H "Content-Type: application/json" \
     -d '{"model":"claude-haiku-4-5-20251001","messages":[{"role":"user","content":"reply with the single word OK"}],"max_tokens":20,"stream":true}' \
     http://127.0.0.1:<port>/v1/messages
```

The SSE frame shape is canonical Anthropic (`message_start`,
`content_block_start`, `content_block_delta`, `content_block_stop`,
`message_delta`, `message_stop`); the content is whatever the active
model actually generates. `max_tokens: 20` plus the "reply with the
single word OK" prompt keeps the cost negligible for a smoke check.

### Pointing Claude Code at the example

```sh
export ANTHROPIC_BASE_URL=http://127.0.0.1:<port>
export ANTHROPIC_AUTH_TOKEN=dev-token-for-broomva
claude
```

Claude Code's `/model` picker queries `/v1/models` at bootstrap
against the static catalogue. Conversations route through
`/v1/messages` and hit `api.anthropic.com` via lifegw, exercising the
same codec + auth + rate-limit + Vigil span paths the production
gateway uses. Set `ANTHROPIC_MODEL=claude-haiku-4-5-20251001` in
*Claude Code*'s environment (or pick Haiku in the `/model` picker) to
keep iteration cheap.

### Honest divergence from production

Production lifegw goes through:

```
lifegw → tonic UDS → lifed.Agent.StreamSession → real saga
       → arcan-proxy → AnthropicArcan → api.anthropic.com
```

This example shortcuts the `lifed` layer entirely. The in-process
tonic `Agent` service dispatches **directly** to `AnthropicArcan`:

```
lifegw → tonic UDS → AnthropicProxyAgentService (this example)
       → AnthropicArcan → api.anthropic.com
```

What's exercised end-to-end (matches production):

- Anthropic Messages codec (`lifegw_anthropic_codec`)
- Auth (`JwksCache::dev_only()` + Tier-2 mint)
- Rate limit (`TokenBucketLimiter`)
- Vigil span emission (real spans on stderr, real GenAI semconv)
- `StubHaimaClient` cost gate (no-op, identical to current production)
- `AnthropicArcan` HTTP client + SSE parser
- 600 s `HARD_STREAM_TIMEOUT` wall-clock cap

What's NOT exercised (different from production):

- `lifed` saga (Tier-2 → Tier-3 derivation)
- `arcan-proxy` retry policy / circuit breaker
- Real `lago` event durability
- Real `haima` ledger settlement
- Real `anima` identity wire

For full-saga validation, use **Path 3** (the Railway operator
runbook at `docs/conformance/2026-05-18-claude-code-smoke-runbook.md`).

### Known limits

- **No `lifed` saga.** See "Honest divergence" above.
- **No tool-use round-trip.** The codec handles `tool_use` blocks, but
  Anthropic only emits them when the request body declares tools. The
  example doesn't inject a tool definition into the in-flight request
  — for tool-use end-to-end use `tests/spec_j_e2e_smoke.rs` (mocked
  upstream) or the Railway path (real saga). The `AnthropicArcan` SSE
  parser does carry tool-use payloads through faithfully when the
  upstream emits them.
- **No `from_sequence` replay.** The in-process Agent has no lago to
  replay against. Mid-stream drops surface as fresh requests.
- **No TLS.** Plain HTTP on `127.0.0.1` (production lifegw terminates
  TLS 1.3 via rustls; the example skips the bind dance because
  operators are not testing TLS posture here).
- **Per-sid history is `AnthropicArcan`'s in-memory map.** It survives
  across `SendMessage` / `StreamSession` calls within one process run
  but does NOT persist to lago.
- **Cost is on you.** Every `/v1/messages` call is billed.

### Substrate-free guarantee

`arcan-proxy` is a `[dev-dependencies]` entry in
`crates/life-runtime/lifegw/Cargo.toml`, not a production dep. The
`scripts/verify_dependencies_lifegw.sh` script (Spec C₃ §11.2 LOCKED
L4-D13 enforcement) uses `cargo tree --edges normal` which excludes
dev-deps, so the production graph is unchanged. Same carve-out
pattern as the `lifed` dev-dep used by
`tests/integration_proxy_passthrough.rs`.
