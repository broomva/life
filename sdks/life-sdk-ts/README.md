# @broomva/life-sdk

> **⚠️ v0.1.0-pre — pre-release foundation.** Two known wire-protocol
> gaps prevent this SDK from completing real calls against a production
> lifegw deployment. See [KNOWN_LIMITATIONS.md](./KNOWN_LIMITATIONS.md)
> for B1 (Connect-vs-grpc-web wire mismatch) and B2 (browser WS auth
> via subprotocol). The structural foundation (services, typed errors,
> WS state machine, proto types, codec helpers, 50 unit tests) is
> stable and reusable; v0.2.0 ships the transport rework.

Public TypeScript SDK for the **Life Agent OS**. Talks to `lifegw` (the
edge gateway) — see KNOWN_LIMITATIONS.md for the current transport
state — exposing the four user-facing services from `life.v1.*`:

| Service | Purpose |
|---|---|
| `Agent` | Session lifecycle, chat, tool dispatch, catalog reads |
| `Events` | Event tail, content-addressed blob fetch |
| `Wallet` | Balance, debit (idempotent), transfer, statement |
| `Identity` | `whoami`, profile, session list / revoke |

The admin plane (`life.admin.*`) is intentionally NOT exposed — it is
UDS-only on the gateway and never reachable from the public SDK.

> **Spec ground truth**
>
> - Spec C₂ (lifed facade) — `docs/superpowers/specs/2026-04-26-spec-c2-lifed-facade.md`
> - Spec C₃ (lifegw edge gateway) — Linear BRO-922 + amendments in `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md`
> - Master spec — `docs/superpowers/specs/2026-04-25-life-runtime-architecture-spec.md`

---

## Install

```bash
# Bun
bun add @broomva/life-sdk

# pnpm / npm / yarn
pnpm add @broomva/life-sdk
npm  install @broomva/life-sdk
yarn add @broomva/life-sdk
```

In Node hosts older than 22, also install `ws`:

```bash
pnpm add ws
```

---

## 5-minute quickstart

```ts
import { LifeClient } from "@broomva/life-sdk";

const life = new LifeClient({
  baseUrl: "https://api.life.dev",         // your lifegw HTTPS endpoint
  getAuthToken: async () => myAuthToken(), // any Vercel JWT producer (Clerk, Auth.js, …)
});

// 1. Resolve who I am.
const me = await life.identity.whoami();

// 2. Open a session.
const session = await life.agent.createSession({
  userId: me.userId,
  projectId: "p-default",
  label: "first-session",
});

// 3. Chat — the server streams tokens + tool events back.
for await (const event of life.agent.sendMessage({
  sid: session.sid,
  content: "Hello, Life",
})) {
  console.log(event.kind, event.record.sequence);
}

// 4. Read the wallet.
const balance = await life.wallet.getBalance({
  userId: me.userId,
  projectId: "p-default",
});
console.log(balance.micros, balance.currency);
```

Run the example end-to-end against a local lifegw (from inside this
package directory):

```bash
cd sdks/life-sdk-ts
pnpm install
LIFE_BASE_URL=https://localhost:443 \
LIFE_USER_ID=u-quickstart \
pnpm run example:quickstart
```

> The repo doesn't currently have a top-level pnpm workspace
> declaration, so `pnpm --filter` won't resolve the package name from
> the repo root. Run from `sdks/life-sdk-ts/` directly.

---

## Long-lived streaming over WebSocket

`Agent.SendMessage` and `Agent.StreamSession` are server-streaming
gRPC calls — usable directly when you have a fresh in-memory token
and a short response. For browsers / apps that need
**reconnect-on-drop** semantics, prefer the WebSocket helper:

```ts
const session = await life.streamSession("sid-123", {
  onAgentEvent: (e) => console.log(e.seqNo, e.agentKind),
  onError: (err) => console.error(err.code, err.message),
  onClose: () => console.log("disconnected"),
});

session.sendMessage("hello over WS");

// …later…
session.close("done");
```

The session tracks the highest `seq_no` it has seen and uses it as
`last_seq_no` on reconnect (Spec C₃ §11.4) so the stream resumes
from `seq + 1`. Auto-reconnect is enabled by default for transient
close codes (4002, 4004, 1011); permanent codes (1008, 4001, 4003,
4005) surface via `onError` immediately.

In Node 20 / older hosts, wire the `ws` package:

```ts
import WebSocket from "ws";

const life = new LifeClient({
  baseUrl: "https://api.life.dev",
  getAuthToken: async () => token,
  webSocketFactory: (url, protocols) => new WebSocket(url, protocols),
});
```

The browser path uses the standard `WebSocket` global; the bearer
token is forwarded as a `bearer.<token>` subprotocol because browsers
cannot set custom request headers on WS handshakes.

---

## Error handling

Every error thrown by the SDK extends `LifeSdkError`:

```ts
import {
  AuthError,
  RateLimitError,
  BackpressureError,
  IpBlockedError,
  LifedUnavailableError,
  SequenceRetiredError,
  InternalServerError,
  GoingAwayError,
  TlsNegotiationError,
  TransportError,
  GrpcError,
} from "@broomva/life-sdk";
```

The full close-code → error mapping (Spec C₃ §6.5):

| Close code | Error class | Reason prefix |
|---:|---|---|
| 1000 | _(graceful, no error)_ | `normal` |
| 1001 | `GoingAwayError` | `going_away` |
| 1008 | `AuthError` | `policy_violation:token_expired` |
| 1011 | `InternalServerError` | `internal_error` |
| 4001 | `RateLimitError` | `rate_limit:per_user` |
| 4002 | `BackpressureError` | `backpressure:slow_consumer` |
| 4003 | `IpBlockedError` | `ip_blocked` |
| 4004 | `LifedUnavailableError` | `lifed_circuit_open` |
| 4005 | `SequenceRetiredError` | `sequence_retired` |

For unary RPCs the same classes are produced from gRPC status codes:

| gRPC code | Error class |
|---|---|
| `UNAUTHENTICATED` / `PERMISSION_DENIED` | `AuthError` |
| `RESOURCE_EXHAUSTED` | `RateLimitError` |
| `UNAVAILABLE` | `LifedUnavailableError` |
| `OUT_OF_RANGE` | `SequenceRetiredError` |
| `INTERNAL` | `InternalServerError` |
| _other_ | `GrpcError` |

---

## TLS 1.3 requirement

`lifegw` mandates TLS 1.3 on its public listener (Spec C₃ §5.1).
Clients that cannot negotiate 1.3 surface a `TlsNegotiationError`
on the first call. In practice this means:

- All evergreen browsers — supported.
- Node ≥ 18 — supported.
- Old curl, ancient Java, IE 11 — rejected.

The browser path can't always introspect the underlying TLS error;
the SDK's heuristic remaps obvious markers (`SSL`, `TLS`, `handshake`,
`EPROTO`, `ERR_SSL_*`) to `TlsNegotiationError`. Other transport
errors surface as `TransportError`.

---

## API reference

### `LifeClient`

```ts
new LifeClient({
  baseUrl: string,
  getAuthToken?: () => Promise<string | undefined>,
  fetch?: typeof fetch,
  webSocketFactory?: WebSocketFactory,
})
```

Aggregates the four service clients on a shared `Transport`:

- `life.agent: AgentClient`
- `life.events: EventsClient`
- `life.wallet: WalletClient`
- `life.identity: IdentityClient`

Plus `life.streamSession(sid, handlers?, extraOpts?): Promise<WsAgentSession>` for WS sessions.

### Per-call options

Every method takes an optional `TransportCallOptions` argument:

```ts
{ signal?: AbortSignal, timeoutMs?: number, headers?: Record<string,string> }
```

### Currency precision

All amounts are `bigint` micros (μUSDC) — proto3 `uint64` → JSON
string → SDK `bigint`. Convert to USDC by dividing by `1_000_000n`:

```ts
const usdc = Number(balance.micros) / 1_000_000;
```

### Idempotency

- `Wallet.Debit` is idempotent on `(wallet, sid)` — replays return the original receipt.
- `Wallet.Transfer` idempotency lands in a follow-up; pass a stable `memo` for now.

---

## Troubleshooting

**`TlsNegotiationError: TLS 1.3 negotiation failed`** — your client
can't speak TLS 1.3. Upgrade Node, browser, or test fixture; in CI
make sure your reverse proxy or fixture also runs TLS 1.3.

**`AuthError: auth token rejected`** — the bearer JWT failed Tier-1
validation. Confirm:
1. `getAuthToken()` returns a fresh token.
2. The token is signed by a key in the gateway's JWKS (Vercel /
   Auth.js / Clerk).
3. The clock skew between client and server is < 5 minutes.

**`RateLimitError: rate_limit:per_user`** — the per-user token
bucket is exhausted (Sub-phase D D1). Back off + retry. The reason
prefix tells you whether it's per-user or per-IP.

**`SequenceRetiredError`** — your `from_sequence` cursor is older
than what lifed retains. Drop the cursor and reconnect with `0n` to
resume from the live tail.

**`InternalServerError` on a WS session** — most often a heartbeat
timeout. The server pings every 30 s and closes if no pong arrives
in 60 s. Check the network path doesn't strip WS pings (some
load-balancers rewrite them).

---

## Development

```bash
cd sdks/life-sdk-ts

bun install                       # or pnpm / npm install
bun run typecheck                 # TS strict-mode
bun run test                      # vitest run
bun run build                     # tsc → dist/
```

The SDK is hand-curated — proto types in `src/proto/` mirror
`proto/life/v1/*.proto`. When the proto changes, update the matching
file by hand. A future revision can switch to `buf generate +
protoc-gen-es`; the public surface is structured so the codegen swap
won't break consumers.

---

## License

MIT © Broomva
