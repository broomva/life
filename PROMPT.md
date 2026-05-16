# Lumen Phase α — continuation PROMPT.md (for `persist iterate`)

**Branch**: `feat/lumen-phase-alpha-m7-w` @ `36617935` (13 commits ahead of `main`)
**Architecture spec**: `docs/lumen/architecture/2026-05-15-life-native-wiring.md`
**Last session output**: Phase α₁ shipped end-to-end (auth bootstrap + dev shortcut + Lumen-audience scope synthesis), 5 unit tests green, empirical chain proven through HTTP+TLS+TraceLayer.

## Single-sentence goal

Ship Phases α₂ → α₄ on this branch, gated by the architecture doc's
done-when tripwire ("Lumen runs 30 min with zero outbound to *.warp.dev,
*.googleapis.com, *.firebaseapp.com, *.sentry.io, *.rudderstack.com").

## P15 state snapshot (verify before doing anything)

```bash
cd /Users/broomva/broomva/core/life/.worktrees/lumen-phase-alpha-m7-w
git log --oneline -5
# Expected:
#   36617935 fix(lumen): Phase α₁ dev-shortcut path + audience-aware scope synthesis
#   3568c9e2 feat(lumen): Phase α₁ — `/life/v1/auth/bootstrap` + launcher injection (no Firebase)
#   a20a5af6 docs(lumen): architecture decision — Life-native wiring (no Warp backend ties)
#   f77ae8a9 docs(lumen): empirical Lumen→lifegw round-trip captured — Layer 2-GUI fully closed
#   abbec08f fix(lifegw): advertise ALPN h2 + http/1.1 on TLS listener
git status -s   # expect: clean
./scripts/lumen-smoke.sh status                  # lifegw + lifed up
./scripts/lumen-app.sh pid                       # Lumen process (or empty)
```

## What's already shipped

- `crates/life-runtime/lifegw/src/services/auth_bootstrap.rs` — POST /life/v1/auth/bootstrap mints Tier-User cap (dev-shortcut in dev mode, real JWS in prod)
- `crates/life-runtime/lifegw/src/auth/jwks.rs` — dev-shortcut path now synthesizes audience-appropriate scopes
- `scripts/lumen-app.sh` — pre-launch curl to bootstrap, injects via `WARP_USER_SECRET`
- `crates/life-runtime/lifegw/src/services/auth_bootstrap.rs` — 5 unit tests green

## Phase α₂ — what's left (concrete file paths + fix pattern)

The remaining work is implementing 5 GraphQL queries + 1 REST endpoint
end-to-end. The substrate revealed during α₁ testing: each resolver
needs to mint a Tier-2 from the user's Tier-User claims + attach to
the upstream lifed call.

### α₂.1 — Wire `resolve_get_user` to attach Tier-2 on upstream call

**File**: `crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier1.rs:73`
(`resolve_get_user`)

**Symptom**: today returns `UPSTREAM_ERROR: missing Tier-2 capability
token`. Reason: `dataloaders.identity.me(IdentityEmpty{})` is called
without minting a Tier-2 and attaching as gRPC metadata.

**Fix pattern**: see how `resolve_create_agent_task` does it (likely
mints via `ctx.tier2_minter` and wraps the call with `with_token` or
sets metadata directly). Apply the same pattern to `resolve_get_user`.
Update `WarpRequestCtx` to carry a Tier-2 minter (probably already
there — check `state.minter`).

**Validation**: same curl as Phase α₁ end-of-session test but expect
HTTP 200 + valid UserOutput shape (NOT `UPSTREAM_ERROR`).

### α₂.2 — Cover GetUser + GetUserSettings + GetWorkspacesMetadataForUser + GetRequestLimitInfo

All four operations hit the same field `user(requestContext: $rc): UserOutput`.
One resolver fix (α₂.1) covers all four operations. Validate each:

```bash
TOKEN=$(curl -sS --cacert crates/life-runtime/lifegw/dev-tls/dev-ca.pem \
  -X POST https://127.0.0.1:8443/life/v1/auth/bootstrap \
  -H "Content-Type: application/json" -d '{"user_id":"lumen-local"}' \
  | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["access_token"])')

# Each of these should return HTTP 200 + data:
for query in \
  'user(requestContext: $rc) { ... on UserOutput { user { isOnboarded profile { displayName email } } } }' \
  'user(requestContext: $rc) { ... on UserOutput { user { settings { isCloudConversationStorageEnabled isCrashReportingEnabled isTelemetryEnabled } } } }' \
  'user(requestContext: $rc) { ... on UserOutput { user { workspaces { uid name } } } }' \
  'user(requestContext: $rc) { ... on UserOutput { user { workspaces { uid bonusGrantsInfo { grants { createdAt costCents } } } } } }'
do
  curl -sS --cacert crates/life-runtime/lifegw/dev-tls/dev-ca.pem \
    -X POST https://127.0.0.1:8443/graphql/v2 \
    -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
    -d "{\"query\":\"query M(\$rc: RequestContext!) { $query }\",\"variables\":{\"rc\":{\"clientContext\":{\"version\":\"x\"},\"osContext\":{\"name\":\"darwin\",\"category\":\"darwin\",\"version\":\"0\"}}}}"
done
```

The resolver returns a minimal but **valid-shape** UserOutput. The
sub-fields that don't have Life-side data return null/empty values
(GraphQL nullability handles this).

### α₂.3 — Verify `freeAvailableModels` still works

Already wired (smoke client uses it). Curl-test post-α₂.1 to ensure
the Tier-2 wiring didn't regress it:

```bash
curl ... -d '{"query":"{ freeAvailableModels { __typename } }"}'
```

### α₂.4 — Implement REST `GET /api/v1/agent/identities`

**Mapping**: list authored agents from `core/life/agents/*.md`.
**File**: extend `crates/life-runtime/lifegw/src/services/agent_http.rs`
to add a new route handler.
**Behavior**: scan `agents/` dir, parse frontmatter for name + display
name, return a list shaped like Warp's `AgentIdentity` response (see
`apps/warp-oss-patch/upstream/crates/warp_server_client/src/auth.rs:652`
for the consumer shape).

### α₂ done-when

- All 5 GraphQL queries return HTTP 200 with non-error responses
- `/api/v1/agent/identities` returns a list (possibly empty, but valid JSON)
- lifegw TraceLayer log shows decoded HTTP requests for each
- Lumen launch (post-Group Container prompt dismissal) shows NO
  `Failed to fetch` warnings in `~/Library/Logs/lumen.log`

## Phase α₃ — authored agent + dispatch bridge

### α₃.1 — Author `core/life/agents/warp-task.md`

YAML frontmatter:

```yaml
---
name: warp-task
display_name: Warp Task
description: Adapts Warp's AgentTask invocation into a Life authored-agent run.
inputs:
  prompt: string
  environment_uid?: string
  conversation_id?: string
  attached_files?: array
---
```

Body: prompt template that instructs the agent to execute the Warp
task as a Life agent run, respecting the existing 8-phase tick.

### α₃.2 — Wire `resolve_create_agent_task` to spawn the authored agent

**File**: `crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier1.rs:643`

**Today**: returns typed `UNAUTHENTICATED` (gates on Tier-1 scope).
**Needs**: call `lifed.Agent.CreateSession` with
`authored_agent: "warp-task"` + the input context. Return a
synthesized `taskId` (use the lifed session_id, base64-encoded as
needed to match Warp's `ID!` scalar). Update the
`UpdateAgentTaskInput` resolver pair similarly.

### α₃.3 — Bridge `warpDriveUpdates` subscription to lago tail

**File**: `crates/life-runtime/lifegw/src/warp_graphql/resolvers/`
(currently no subscriptions module; create one — `subscriptions.rs`)

**Today**: smoke client receives `AmbientTaskUpdated` (stub).
**Needs**: subscribe to lago events for the task_id session, translate
each `EventEnvelope` to Warp's `AmbientTaskUpdated` variants
(`AmbientCommandStarted`, `AmbientCommandFinished`, etc.),
yield to the WSS client.

### α₃.4 — Optionally: SSE `/api/v1/agent/events/stream`

Lower-priority — same lago tail, different transport. Defer if
α₃.1-3 take longer than expected.

### α₃ done-when

- Typing a prompt into Lumen's agent panel produces a real Life
  authored-agent invocation visible in `lago log`
- `lago replay --tree <session_id>` shows the agent's 8-phase tick
- Lumen renders the streamed events as Warp blocks (visual proof
  via screenshot)

## Phase α₄ — Oz browser-link redirect

**Files**: ~6 sites listed in
`docs/lumen/architecture/2026-05-15-life-native-wiring.md` §"Hardcoded URLs".

For each, replace `https://oz.warp.dev/{path}` with one of:
- `localhost:3001/runs/{id}` (if a Life-OS web UI is set up)
- `arcan run show {id}` (separate CLI)
- A stub modal saying "Coming in Life-OS UI"

Lowest-priority because these are user-clickable links, not
blocking paths. Defer if α₂/α₃ take longer.

## Tripwire validation (done-when for the whole arc)

```bash
# 1. Bring up stack + Lumen
./scripts/lumen-smoke.sh up
./scripts/lumen-app.sh launch

# 2. (Operator) Dismiss the macOS Group Container prompt

# 3. Open a Lumen tab, type a command, open the agent panel,
#    interact for ~30 min

# 4. Tripwire: ZERO outbound traffic to:
sudo tcpdump -i lo0 -nn 'port 443 and dst not 127.0.0.1' \
  | grep -iE 'warp.dev|googleapis|firebaseapp|sentry|rudderstack' \
  | head -1
# Expect: nothing for 30 min.

# 5. lifegw TraceLayer log: every Lumen GraphQL/REST call visible
grep -cE 'tower_http::trace::on_request' /tmp/lumen-smoke/lifegw.log
# Expect: > 20 (many request spans logged)

# 6. Zero TLS errors / fetch errors in Lumen log
grep -cE 'certificate is not trusted|Failed to fetch' ~/Library/Logs/lumen.log
# Expect: 0
```

## Bstack reflexes the next session must apply

- **P14** dep-chain trace before each phase's first write
- **P15** snapshot at session start (commands above)
- **P10** stay in this worktree
- **P11** restart stack + curl-test after every code change
- **P7** if pushing to CI, `p9 watch --background` immediately
- **P6** bookkeeping after substantive phases (lago events from α₃
  test runs are graph-relevant)
- **P5** consider parallel agents for α₂.1 + α₂.4 (independent
  resolvers / REST endpoints)
- **P20** cross-review gate before any PR (substantive code)

## Open questions (from architecture doc, surfaced during α₁)

1. Anima identity shape — should bootstrap mint an ephemeral anima
   identity per launch, or require an established one?
2. Workspace concept — single-workspace stub vs Spaces channels?
3. Model registry — hardcoded list vs `~/.config/lumen/models.toml`
   vs `inference-core::BackendRegistry`?
4. Authored-agent variations — `warp-task-fast.md` + `warp-task-deep.md`?
5. Event-shape translation home — `subscriptions.rs` module location?

## Acceptance gate before opening a PR to main

Per the user's standing instruction: **do NOT open a PR to main until
the user explicitly asks**. Stay on `feat/lumen-phase-alpha-m7-w`
through α₂/α₃/α₄. When all four phases are done + the tripwire is
green, surface to the user with the full evidence package and let
them decide on PR timing.
