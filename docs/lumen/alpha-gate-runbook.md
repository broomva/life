# Lumen Phase α — Gate Exercise Runbook

**Status**: code-complete @ commit `9ce9374b` on branch `feat/lumen-phase-alpha-m7-w`.

This runbook walks through the manual α-gate exercise: bring up the lifegw + lifed stack locally, point a patched `warp-oss` binary at it, type a prompt, observe the agent-task round-trip. It is the Layer 3 ("manual interactive smoke") of the validation pyramid we agreed in the Lumen design conversation.

The runbook is **layered** — each step is independently verifiable and produces evidence. If the bottom layer doesn't work, we know not to bother with the higher layers.

---

## Layer 0 — One-time prerequisites

These are environment installs you only do once.

### 0.1  macOS Metal Toolchain

The patched `warp-oss` binary compiles GPU shaders via `metal`. The toolchain ships as an opt-in Xcode component.

```bash
xcodebuild -downloadComponent MetalToolchain
```

This is interactive and ~GB-scale. After it completes, verify:

```bash
xcrun metal --version 2>&1 | head -1
# Expected output: Apple metal version 31.x.x (...)
```

(If you don't see `Apple metal version ...`, the toolchain didn't install correctly — try Xcode → Preferences → Components.)

### 0.2  Build the patched `warp-oss` binary

```bash
cd /Users/broomva/broomva/core/life/.worktrees/lumen-phase-alpha-m7-w
./apps/warp-oss-patch/apply-patch.sh
```

This script:
- Clones `github.com/warpdotdev/warp` into `apps/warp-oss-patch/upstream/` (~5 min cold clone)
- Applies the one-line OSS-channel override patch
- Builds `apps/warp-oss-patch/upstream/target/release/warp-oss` (~20-30 min cold compile)

Verify:

```bash
ls -la apps/warp-oss-patch/upstream/target/release/warp-oss
# Expected: -rwxr-xr-x ... warp-oss
```

---

## Layer 1 — Server-side smoke (no warp-oss needed)

This layer verifies the lifegw + lifed stack works in isolation. Run it BEFORE worrying about warp-oss to catch any server-side regressions independently.

### 1.1  Bring up the stack

```bash
cd /Users/broomva/broomva/core/life/.worktrees/lumen-phase-alpha-m7-w
./scripts/lumen-smoke.sh up
```

The script:
- Generates a self-signed dev-tls cert at `crates/life-runtime/lifegw/dev-tls/` if missing
- Writes lifed + lifegw configs to `/tmp/lumen-smoke/` (override with `WORKDIR=...`)
- Builds + starts `lifed` (with `--allow-mock-fallback` so substrate calls mock)
- Builds + starts `lifegw` (in `dev-mode` so `Bearer dev-token-for-{user_id}` works)
- Tails both daemons' logs in your shell (Ctrl-C detaches; daemons keep running)

Expected end-state:
- `lifegw HTTPS  → https://127.0.0.1:8443`
- `GraphQL POST  → https://127.0.0.1:8443/graphql/v2`
- `GraphQL WS    → wss://127.0.0.1:8443/graphql-ws`

### 1.2  Run the curl-based smoke

In a second shell:

```bash
cd /Users/broomva/broomva/core/life/.worktrees/lumen-phase-alpha-m7-w
./scripts/lumen-smoke.sh smoke-curl
```

This runs three checks against the live stack:
1. **Schema introspection** — `__schema { queryType }` returns `RootQuery`.
2. **Tier-1 op presence** — RootMutation exposes `createAgentTask`, `mintCustomToken`, `updateUserSettings`, `generateDialogue`.
3. **createAgentTask dispatch** — Real resolver fires, response shape is one of `CreateAgentTaskOutput | UserFacingError | UPSTREAM_ERROR | UNAUTHENTICATED`.

If smoke-curl passes, the server side is healthy.

### 1.3  Manual curl explorations (optional)

```bash
# Introspection of the full schema
curl --insecure -X POST https://127.0.0.1:8443/graphql/v2 \
  -H "Authorization: Bearer $LUMEN_DEV_TOKEN" \
  -H "Content-Type: application/json" \
  --data '{"query": "{ __schema { types { name kind } } }"}' | jq '.data.__schema.types | length'
# Expected: 200+ types (whole upstream Warp surface)

# Anonymous mode (no Bearer):
curl --insecure -X POST https://127.0.0.1:8443/graphql/v2 \
  -H "Content-Type: application/json" \
  --data '{"query": "{ user { __typename } }"}'
# Expected: typed UNAUTHENTICATED extension code

# Tier-User mint
curl --insecure -X POST https://127.0.0.1:8443/graphql/v2 \
  -H "Authorization: Bearer $LUMEN_DEV_TOKEN" \
  -H "Content-Type: application/json" \
  --data '{"query": "mutation { mintCustomToken { __typename ... on MintCustomTokenOutput { token expiresAt } } }"}'
# Expected: real ES256-signed token or UPSTREAM_ERROR (depending on mock fallback path)
```

---

## Layer 2 — Manual warp-oss smoke

Requires Layer 0 (Metal Toolchain + binary built) AND Layer 1 (stack up).

### 2.1  Launch warp-oss against lifegw

```bash
cd /Users/broomva/broomva/core/life/.worktrees/lumen-phase-alpha-m7-w
WARP_SERVER_ROOT_URL=https://127.0.0.1:8443 \
WARP_WS_SERVER_URL=wss://127.0.0.1:8443/graphql-ws \
WARP_AUTH_TOKEN=dev-token-for-test-user-1 \
./apps/warp-oss-patch/upstream/target/release/warp-oss
```

**Note on TLS**: the dev-tls cert is self-signed. macOS may refuse the connection by default. Workarounds:
- Add the cert to the system keychain: `sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain crates/life-runtime/lifegw/dev-tls/cert.pem`
- OR re-bind lifegw on plain HTTP (set `https_addr` to a non-TLS path) — would require lifegw config change.

### 2.2  Observation rig (recommended)

While warp-oss is running, watch what's happening across the stack in parallel:

```bash
# Pane 1 — lifegw stdout
tail -F /tmp/lumen-smoke/lifegw.log

# Pane 2 — lifed stdout
tail -F /tmp/lumen-smoke/lifed.log

# Pane 3 — every GraphQL POST hitting lifegw
tail -F /tmp/lumen-smoke/lifegw.log | grep -E "POST|graphql"

# Pane 4 — vigil OTel traces (if Jaeger is running locally on :16686)
open http://localhost:16686
```

### 2.3  Drive the agent flow

In the warp-oss window:
1. Sign-in / onboarding — anonymous + `createAnonymousUser` will surface `FEATURE_NOT_ENABLED`. Acceptable for α; β will wire the real flow.
2. Type a prompt in the terminal (e.g. `list files`).
3. Watch panes 1+2 for `createAgentTask` POST + lifed `Agent.CreateSession` dispatch.
4. Watch the warp-oss window for streaming response (will surface as a placeholder frame from Task 20's stub — real streaming bridge lands in a follow-up integration ticket).

### 2.4  Expected outcomes by op

| Action | Expected (Phase α) |
|---|---|
| Connect | Schema introspection succeeds; warp-oss UI loads its model picker |
| `freeAvailableModels` query | Returns 2 hardcoded entries: `claude-sonnet-4`, `mock-default` |
| Type prompt → `createAgentTask` | Succeeds; lifegw logs show `Agent.CreateSession` call, returns session_id as task_id |
| Subscribe `warpDriveUpdates` | One placeholder `AmbientTaskUpdated` frame, then stream closes |
| `createAnonymousUser` | Typed `FEATURE_NOT_ENABLED` (acceptable α deviation) |
| Sign-in via mintCustomToken | Real ES256 token minted via TierUserMinter |

If warp-oss client rejects responses for any reason (missing nested fields on UserResult, etc.), that's the same nested-stub-returns-null concern flagged in the Task 14/15/17 reviews. The β fix is per-type field-resolvers; Phase α accepts degraded UX for these paths.

---

## Layer 3 — Evidence capture

For each successful Layer 2 run, capture under `docs/lumen/validation/2026-MM-DD/`:

- Screenshots at each step (warp-oss window + observation panes).
- Full session log archive: `cat /tmp/lumen-smoke/{lifed,lifegw}.log > docs/lumen/validation/2026-MM-DD/stack-logs.txt`
- Optional screencast (macOS Cmd-Shift-5).

This is the Phase α gate evidence package — commit + tag.

---

## Stopping the stack

```bash
./scripts/lumen-smoke.sh down
```

Kills both daemons + cleans up `/tmp/lumen-smoke/`. Use `status` to see what's currently running.

---

## Known issues & workarounds

| Symptom | Likely cause | Workaround |
|---|---|---|
| `metal: command not found` during `apply-patch.sh build` | Metal Toolchain not installed | Run `xcodebuild -downloadComponent MetalToolchain` |
| `lifegw didn't bind within 60s` | dev-tls cert missing or invalid | Delete `crates/life-runtime/lifegw/dev-tls/` and re-run `up` |
| `lifed didn't open UDS` | `$WORKDIR/run/` unwritable | `WORKDIR=$HOME/.lumen-smoke ./scripts/lumen-smoke.sh up` |
| `failed to connect` from warp-oss to https://127.0.0.1:8443 | macOS rejects self-signed cert | Trust the cert via keychain (see Step 2.1 note) |
| `createAnonymousUser` returns `FEATURE_NOT_ENABLED` | Known Phase α stub | Accept; β implements |
| Subscription emits only one frame then closes | Task 20 ships placeholder; real bridge pending | Accept; integration ticket |
| Nested `UserResult.settings` returns `null` on non-null field | Auto-stub doesn't synthesize objects | Accept; β per-type field-resolvers |

---

## What's NOT exercised by this runbook (out of α scope, β follow-ups)

- Real broomva.tech OAuth-PKCE flow.
- Real lifed.Agent.StreamSession bridge to `warpDriveUpdates`.
- Real arcan ConsciousnessActor invocation through `WarpTaskBackend` trait.
- Firebase API key neutralization on warp-oss.
- Lumen-branded build (currently still "Warp OSS" branding).
- Cloud Agents via soma + praxis + chronos.

All tracked as follow-up tickets in the Phase α acceptance summary.
