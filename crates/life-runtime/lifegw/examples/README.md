# lifegw — examples

Dev-tooling examples for the lifegw edge gateway. Currently:

| Example | Purpose | Linear |
|---|---|---|
| `local_smoke` | Boot lifegw locally against an in-process mock lifed for ~30 s interactive smoke. Zero deploy, zero Railway slot. | [BRO-1165](https://linear.app/broomva/issue/BRO-1165) |

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

Spec J Phase 1 ships three escalating test surfaces:

| Path | Command / artefact | What it certifies |
|---|---|---|
| 1 | `cargo test -p lifegw --test spec_j_e2e_smoke` | In-process E2E — codec, route, auth, recording haima, mock lifed; 5 scenarios. |
| 2 | This example (`cargo run -p lifegw --example local_smoke`) | Real TCP socket, interactive — operator-driven probe of the edge wire over HTTP. |
| 3 | Railway staging deploy per `docs/conformance/2026-05-18-claude-code-smoke-runbook.md` | Live evidence — Loom + Vigil traces + lago replay + haima ledger. |

Path 1 is automated; path 3 is the Phase 1 conformance gate; **path 2 is
the iterating-engineer's loop** between the two.
