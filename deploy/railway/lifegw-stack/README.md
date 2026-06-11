# lifegw-stack — Railway deploy

A multi-process container that ships `lifegw` + `lifed` together with the
real `arcan` and `lagod` substrate daemons, fronted by Caddy, behind
Railway's TLS-terminating edge. This is the deploy profile referenced by
the canonical Life runtime spec at
`apps/chat/docs/superpowers/specs/2026-05-03-life-runtime-canonical.md` §M.
It has advanced incrementally (#1695): Stage 5 added real arcan, **Stage 6
adds real lago** — `arcan=real lago=real haima=mock anima=mock`.

## Topology

```
broomva.tech (Vercel)
       │  wss://<railway-domain>/v1/agent/stream  (TLS at Railway edge)
       ▼
+──────────────────── Railway service "lifegw" ──────────────────────+
│                                                                    │
│   :$PORT (cleartext)                                               │
│      │                                                             │
│      ▼                                                             │
│   caddy ── reverse_proxy ──► https://127.0.0.1:8443 (self-signed)  │
│                                       │                            │
│                                       ▼                            │
│                                    lifegw                          │
│                                       │                            │
│                                       ▼ /run/life/life.sock        │
│                                    lifed (per-substrate selection) │
│                                       │                            │
│                  ┌────────────────────┼────────────────────┐       │
│                  ▼ arcan.sock         ▼ lago.sock           │       │
│               arcan (REAL)         lagod (REAL)      haima/anima    │
│            substrate-plane gRPC   lago.v1.Lago-      (MOCK — no     │
│            (arcan.v1.Agent-       Substrate over     socket bound)  │
│             Substrate over UDS)    UDS               │             │
│                                                                    │
+────────────────────────────────────────────────────────────────────+
```

## What runs in this image

| Process | Role | Listens on |
|---|---|---|
| `caddy` | reverse-proxy / TLS-terminated bridge | `:$PORT` (cleartext) |
| `lifegw` | edge gateway — Tier-1/Tier-2 mint, WS bidi pump, `/anima/custody/*` | `127.0.0.1:8443` (self-signed TLS) |
| `lifed` | facade aggregator — `life.v1.{Agent,Events,Wallet,Identity}` | `/run/life/life.sock` (UDS) |
| `arcan` | REAL arcan substrate — `arcan.v1.AgentSubstrate` (agent loop) | `/run/life/arcan.sock` (UDS) + `:3000` HTTP (internal) |
| `lagod` | REAL lago substrate — `lago.v1.LagoSubstrate` (event journal + blobs) | `/run/life/lago.sock` (UDS) + `:50051` gRPC / `:8077` HTTP (internal) |

`lifed` boots with `LIFED_ALLOW_MOCK_FALLBACK=1` and per-substrate real/mock
selection: it dials each substrate's UDS once at boot. `arcan.sock` and
`lago.sock` are bound by `entrypoint.sh` **before** lifed starts, so lifed
selects the **real** arcan + lago substrates; `haima.sock` / `anima.sock`
are absent, so haima/anima degrade to in-process mocks (gated by the
fallback flag). Boot log prints the selection:
`substrates: arcan=real lago=real haima=mock anima=mock`.

> **Why real arcan + lago but mock haima/anima?** Stage 5/6 ship the two
> substrates the core chat path exercises — the agent loop (arcan) and its
> durable event journal (lago). haima (finance) and anima (custody) are not
> on the chat hot path; their daemons join the container in later stages
> (drop `LIFED_ALLOW_MOCK_FALLBACK` once all four sockets are bound).
>
> Stage 6 also required a lagod-side change: `lagod --uds-socket <PATH>`
> binds `lago.v1.LagoSubstrate` over a UDS. BRO-1017 had mounted that
> service on the TCP gRPC port only, so lifed's lago-proxy (which dials a
> Unix socket) had nothing to connect to — lago could never be selected as
> real until the substrate-plane UDS server existed.

## This image vs production

| | This image (Stage 6) | Production |
|---|---|---|
| Tier-1 verification | dev signer (`Bearer dev-token-for-{user_id}`) | real Vercel JWKS |
| Tier-2 / Tier-User mint | in-process keystore | Vault / AWS / GCP KMS |
| Substrates | **arcan + lago real**; haima/anima mock | arcand + lagod + haimad + animad + soma sharing `/run/life/` |
| Persistence | arcan → `/var/lib/arcan`; lago → `${LIFE_STATE_DIR}/lago` (volume) | volume-backed per substrate |
| TLS | self-signed (loopback only) | real cert via Vault PKI / Let's Encrypt |
| Rate limit | defaults (60/min/user, 10 WS/user) | tuned per tenant |

## Build

From the `core/life` workspace root:

```bash
docker build -f deploy/railway/lifegw-stack/Dockerfile -t lifegw-stack .
```

Railway picks the Dockerfile via the service's deploy config — see
the railway service variables for `RAILWAY_DOCKERFILE_PATH`.

## Local smoke test

```bash
# ANTHROPIC_API_KEY unlocks the REAL arcan substrate (else MockArcan);
# the life-state volume persists lagod's journal + the Tier-2 key.
# (Comments cannot ride backslash continuations — `\  #` is an escaped
# space, which would terminate the command mid-line.)
docker run --rm -p 8080:8080 \
  -e PORT=8080 \
  -e LIFED_ALLOW_MOCK_FALLBACK=1 \
  -e ANTHROPIC_API_KEY=... \
  -e RUST_LOG=info,lifegw=debug,lifed=debug,arcan=info,lagod=info \
  -v life-state:/var/life-state \
  lifegw-stack

# In another shell:
curl -sf http://127.0.0.1:8080/healthz
curl -sf http://127.0.0.1:8080/caddy/healthz   # caddy-side liveness
```

Expect these lines in the boot log (entrypoint ordering — arcan + lago
sockets bound before lifed samples them):

```
[entrypoint] arcan UDS accepting connections ...
[entrypoint] lagod UDS accepting connections (after N half-seconds)
... lifed: substrates: arcan=real lago=real haima=mock anima=mock
```

`lagod`'s journal + blobs live under `${LIFE_STATE_DIR}/lago` (default
`/var/life-state/lago`); mount a volume there to keep events across
redeploys. Its TCP planes (`:50051` gRPC, `:8077` HTTP) are container-
internal — lifed reaches lagod purely over `/run/life/lago.sock`. They are
deliberately off `:8080` so they never collide with Caddy's `$PORT`
(override via `LAGO_GRPC_PORT` / `LAGO_HTTP_PORT`).

## Vercel handoff

Once the Railway service is up and a public domain is wired:

```
LIFED_GATEWAY_URL=https://<railway-domain-or-custom>
```

Set this in the broomva.tech Vercel project's production env. The
`createAgentSessionClient()` factory at
`apps/chat/lib/life-runtime/agent-session/factory.ts` reads the env var and
flips the backend from `InProcessAgentSessionClient` to
`LifedWsAgentSessionClient`. The `/api/life/health` endpoint reflects the
flip via `health.ts`, and the Dock SIM/LIVE/COMING badges follow.

## Files

| Path | Role |
|---|---|
| `Dockerfile` | multi-stage builder (`lifegw`+`lifed`+`arcan`+`lagod`) + runtime image |
| `Caddyfile` | reverse-proxy config (`:$PORT` → `https://127.0.0.1:8443`) |
| `lifegw.toml` | lifegw config — dev signer, dev KMS, loopback bind |
| `lifed.toml` | lifed config — per-substrate selection (arcan + lago real, haima/anima mock) |
| `entrypoint.sh` | fan-out script (cert gen → arcan → lagod → lifed → lifegw → caddy fg) |
