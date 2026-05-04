# lifegw-stack — Railway deploy

A multi-process container that ships `lifegw` + `lifed` together, fronted by
Caddy, behind Railway's TLS-terminating edge. This is the **Stage-1** deploy
profile referenced by the canonical Life runtime spec at
`apps/chat/docs/superpowers/specs/2026-05-03-life-runtime-canonical.md` §M.

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
│                                    lifed (mock-fallback)           │
│                                                                    │
+────────────────────────────────────────────────────────────────────+
```

## What runs in this image

| Process | Role | Listens on |
|---|---|---|
| `caddy` | reverse-proxy / TLS-terminated bridge | `:$PORT` (cleartext) |
| `lifegw` | edge gateway — Tier-1/Tier-2 mint, WS bidi pump, `/anima/custody/*` | `127.0.0.1:8443` (self-signed TLS) |
| `lifed` | facade aggregator — `life.v1.{Agent,Events,Wallet,Identity}` | `/run/life/life.sock` (UDS) |

`lifed` boots with `LIFED_ALLOW_MOCK_FALLBACK=1`, which substitutes mock substrates
for arcand / lagod / haimad / animad / soma. The wire path through Caddy → lifegw →
lifed runs against real code; substrate behaviour is mocked.

## Stage 1 vs production

| | Stage 1 (this image) | Production |
|---|---|---|
| Tier-1 verification | dev signer (`Bearer dev-token-for-{user_id}`) | real Vercel JWKS |
| Tier-2 / Tier-User mint | in-process keystore | Vault / AWS / GCP KMS |
| Substrates | mock fallback | arcand + lagod + haimad + animad + soma sharing `/run/life/` |
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
docker run --rm -p 8080:8080 \
  -e PORT=8080 \
  -e LIFED_ALLOW_MOCK_FALLBACK=1 \
  -e RUST_LOG=info,lifegw=debug,lifed=debug \
  lifegw-stack

# In another shell:
curl -sf http://127.0.0.1:8080/healthz
curl -sf http://127.0.0.1:8080/caddy/healthz   # caddy-side liveness
```

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
| `Dockerfile` | multi-stage builder + runtime image |
| `Caddyfile` | reverse-proxy config (`:$PORT` → `https://127.0.0.1:8443`) |
| `lifegw.toml` | lifegw config — dev signer, dev KMS, loopback bind |
| `lifed.toml` | lifed config — dev signer, mock substrate fallback |
| `entrypoint.sh` | fan-out script (cert gen → lifed → lifegw → caddy fg) |
