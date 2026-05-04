# Vercel handoff — `LIFED_GATEWAY_URL`

After `lifegw-stack` is healthy on Railway, broomva.tech needs one Vercel
env-var flip to route agent traffic through the canonical wire.

## 1. Verify lifegw is healthy

```bash
curl -sf https://lifegw-production.up.railway.app/healthz
# expected: HTTP 200 (lifegw replies cleartext through Caddy)

curl -sf https://lifegw-production.up.railway.app/caddy/healthz
# expected: HTTP 200 ("ok")

# WS upgrade smoke test (uses websocat):
websocat -H "Sec-WebSocket-Protocol: bearer.dev-token-for-smoke" \
  "wss://lifegw-production.up.railway.app/v1/agent/stream?sid=smoke"
# expected: connects, server replies with one or more frames before
# closing on a missing session
```

## 2. Set the Vercel production env var

```bash
cd ~/broomva/broomva.tech/apps/chat
vercel env add LIFED_GATEWAY_URL production
# paste:  https://lifegw-production.up.railway.app
```

Or, if a custom domain is later wired (`gw.broomva.tech` etc.), use that
domain instead — broomva.tech doesn't care which DNS name resolves the
gateway, only that `LIFED_GATEWAY_URL` is reachable.

## 3. What flips on the broomva.tech side

```
apps/chat/lib/life-runtime/agent-session/factory.ts:
  if (env.LIFED_GATEWAY_URL) {
    return new LifedWsAgentSessionClient({ baseUrl: env.LIFED_GATEWAY_URL });
  }
  return new InProcessAgentSessionClient(...);
```

So with the env set:
- `/api/life/run/[project]/prosopon` opens a WS to lifegw instead of running
  `RealAgentRunner` inline.
- `/api/life/health` reports `lifed: live` and `arcan: live (via lifegw)`.
- The Dock badges flip from SIM to LIVE on next refresh (data-driven from
  `/api/life/health`).

## 4. Tier-User cap shape

The `LifedWsAgentSessionClient` opens the WS with:

```
Sec-WebSocket-Protocol: bearer.<jwt>
```

Stage 1 (this deploy) accepts the dev shortcut: `dev-token-for-{user_id}`.
broomva.tech's session client mints this synthetic token by default in
non-production environments. Production cap minting (real ES256 JWS via
the lifegw KMS, audience `anima.user-cap`, 15-min TTL) is tracked in the
spec at `apps/chat/docs/superpowers/specs/2026-05-03-life-runtime-canonical.md`
§F (Future Stage).

## 5. Rollback path

```bash
cd ~/broomva/broomva.tech/apps/chat
vercel env rm LIFED_GATEWAY_URL production
```

The factory falls back to `InProcessAgentSessionClient` — the old in-process
agent loop — without redeploying anything else.

## 6. Known Stage-1 limitation: Tier-2 audience chain

When the WS handshake completes and lifegw forwards a freshly-minted Tier-2
JWS to lifed, lifed currently rejects it with
`{"kind":"closing","reason":"policy_violation:token_expired"}`. Root cause:

- The entrypoint starts `lifed` before `lifegw` (lifegw needs lifed's UDS
  to start, so the order is forced).
- lifed's `auth.jwks_path = /run/life/lifegw-jwks.json` does not exist at
  the moment lifed boots.
- lifed falls back to `JwksCache::dev_only()` per `bootstrap.rs:97-105`,
  which **only** accepts the literal `Bearer test-token-for-{user_id}`
  shortcut — it does not verify real ES256 JWS.
- A few hundred ms later lifegw publishes the JWKS to that exact path, but
  lifed's cache is already pinned to dev-only.

Two clean fixes — pick one before declaring Stage 2:

a. **Pre-publish a shared JWKS**: in `entrypoint.sh`, generate one
   ES256 key, write its public half to `/run/life/lifegw-jwks.json`
   *before* `lifed` starts, and pass the full PEM to lifegw via
   `LIFEGW_TIER2_KEY_PEM`-style env (lifegw would need a config knob to
   adopt that key instead of `StaticKeystore::generate_dev()`).

b. **Hot-reload the JwksCache in lifed**: add a periodic reload (e.g. on
   verify miss, refetch from `cfg.auth.jwks_path` if the mtime moved).
   This is closer to the production-shape eventually needed for KMS key
   rotation.

The deploy is otherwise functional: TLS, WS upgrade, Tier-1 dev-signer,
gRPC-web fallback, `/anima/custody/*` axum routes, `/healthz` — all
green from the public domain.
