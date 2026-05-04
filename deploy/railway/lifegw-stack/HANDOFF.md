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

## 6. Stage 1.5 — Tier-1 verification is REAL now (May 2026)

The Tier-1 chain runs against broomva.tech's real Better Auth-bridged
ES256 keypair:

```
broomva.tech (Vercel)
  ─▶ /api/auth/jwks.json publishes the public ES256 key
     (private half lives in `LIFEGW_TIER1_SIGNING_JWK` Vercel env)
  ─▶ /api/life/run/.../prosopon mints a fresh Tier-1 JWT per turn
     (audience=lifegw, issuer=https://broomva.tech, exp = now + 15min)
     ─▶ wss://lifegw.../v1/agent/stream  (Bearer / subprotocol)
        ─▶ lifegw fetches the JWKS, verifies the JWS via the fetched key
           ─▶ mints Tier-2 cap, opens upstream lifed Agent.StreamSession
```

`lifegw.toml` keeps `dev_signer_enabled = true` so the dev shortcut
(`Bearer dev-token-for-{user_id}`) **also** still works — lifegw's
`JwksCache::new_with_dev_shortcut(cfg)` makes the two paths additive
(real JWS preferred, dev shortcut as fallback). Production deploys
flip `dev_signer_enabled = false` once the KMS provider also moves off
`Dev` (Spec C₃ Sub-phase E).

## 7. Stage 2 — boot-order race fixed (May 2026)

Both fixes from §6's earlier draft (a + b) are now shipped together —
because either alone would have been incomplete:

**Fix A — operator-provided Tier-2 key** (`KmsProvider::StaticPem`)

The container no longer calls `StaticKeystore::generate_dev()` on every
boot. Instead, `entrypoint.sh` either:

- reads `LIFEGW_TIER2_SIGNING_KEY_PEM` from env (operator-injected via
  Railway secrets / external KMS shim), or
- reuses the persistent file at `/var/life-state/tier2-signing.pkcs8.pem`
  (Railway volume mount — survives image redeploys), or
- generates a fresh PKCS#8 PEM via `openssl genpkey -algorithm EC
  -pkeyopt ec_paramgen_curve:P-256` on first boot and writes it to the
  same path.

That PEM is exported as `LIFEGW_TIER2_SIGNING_KEY_PEM` and lifegw's
`KmsProvider::StaticPem` arm reads it via `Keystore::from_pem`, with
the operator-pinned `kid` from `[auth.static_pem]`. **The kid stays
stable across container reboots and image redeploys** — clients never
see a kid roll except during an explicit operator rotation.

**Fix B — lazy file-backed JwksCache in lifed** (`JwksCache::new_lazy_file`)

lifed no longer makes a one-shot boot-time decision about its verifier
identity. The new cache:

- holds a path, not a key set
- reads `auth.jwks_path` lazily on first `validate()` call
- re-stats the file on each subsequent verify; if mtime advanced (key
  rotation) **or** the TTL window expired, the keys are reloaded
- serialises concurrent miss-driven loads behind a `parking_lot::Mutex`
  so a thundering herd of 100 verifiers produces at most one file read
- accepts the `Bearer test-token-for-{user_id}` dev shortcut additively
  when `auth.dev_signer_enabled = true`

Both fixes mirror lifegw's own production patterns (Spec C₃ §5).

### Verification

```
$ TOKEN=$(node mint-tier1.mjs)            # broomva.tech-keyed JWS
$ wss://lifegw.../v1/agent/stream + Authorization: Bearer $TOKEN
WS OK: subprotocol='life.v1.agent'
first frame: {"kind":"closing","reason":"internal_error"}
```

The handshake completes, lifegw verifies the Tier-1 JWS via
broomva.tech's `/api/auth/jwks.json`, mints a Tier-2 cap with the
operator-pinned kid, and lifed accepts it. The trailing
`internal_error` is a separate downstream issue (lifed's
`Agent.StreamSession` against `MockSubstrates` can't run a real agent
turn) — that's the §8 Stage 3 work, not auth.

Volume persistence verified: a `railway redeploy` after first boot
shows `[entrypoint] reusing existing Tier-2 key at
/var/life-state/tier2-signing.pkcs8.pem` instead of regenerating, and
the published JWKS keeps the same kid across deploys.

## 8. Open work (Stage 3) — real substrates instead of mocks

`internal_error` from lifed's `Agent.StreamSession` is the next thing
to fix. Two paths:

a. **Single-container substrate fan-out**: ship `arcand`, `lagod`,
   `haimad`, `animad`, `soma` into the same Railway container (more
   `runuser -- /usr/local/bin/<daemon> &` blocks in `entrypoint.sh`).
   All UDS sockets stay on the shared `/run/life/` tmpfs. Heaviest
   single-image bloat but keeps the existing UDS-only wire.

b. **Multi-container topology with substrate-side TCP transport**: each
   substrate becomes its own Railway service, lifed's substrate clients
   speak TCP+TLS over Railway's private DNS. Requires touching each
   substrate's listener (currently UDS-only) but separates concerns
   cleanly.

(b) is the production-grade answer; (a) is the pragmatic one for
keeping the demo end-to-end stack inside one container. We'll pick one
based on the next user-facing milestone.

The dev `Bearer dev-token-for-X` and `Bearer test-token-for-X` paths
both still work as integration-test fallbacks. Production posture
flips `dev_signer_enabled = false` in both daemons once every caller
mints real JWS — at that point the dev shortcut is rejected outright.
