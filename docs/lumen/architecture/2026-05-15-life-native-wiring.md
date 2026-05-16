# Lumen ↔ Life-OS native wiring — architecture decision

**Date**: 2026-05-15
**Branch**: `feat/lumen-phase-alpha-m7-w`
**Status**: Proposed (no code yet)
**Predecessors**:
- `docs/lumen/agent-driven-feedback-loop.md` (substrate)
- `docs/lumen/validation/2026-05-15/README.md` (Layer 2-GUI closure evidence)
- Empirical wire round-trip proven at commit `f77ae8a9`

## Problem statement

Lumen (our patched warp-oss fork) currently inherits warp-oss's
complete cloud-coupling: Firebase auth, `app.warp.dev` API surface,
`rtc.app.warp.dev` WSS, `oz.warp.dev` browser links, `wss://sessions.app.warp.dev`
session sharing, plus the dormant-in-OSS Rudderstack / Sentry /
releases.warp.dev endpoints. Layer 2-GUI proved Lumen can *transport*
to lifegw end-to-end (TLS 1.3 + ALPN h2 + HTTP/2 to a decoded HTTP
request in 22 ms). The next question is **what wire shape** should
flow over that transport.

The constraint:

> **No ties to Warp's backend. Life agents replace Oz.**

We want Lumen to be a Life-OS terminal — using Warp's open source
as the UI substrate, with Life-OS as the agent runtime + persistence
+ capability + identity layer. Firebase, Oz, Warp Drive cloud, and
the GraphQL contract pointing at `app.warp.dev` are all
non-negotiable removals.

## Surface inventory (from the research pass)

A research-agent pass produced a complete inventory. Summary table; full
report lives in this session's transcript with file:line citations.

### First-launch traffic (blocking)

| Layer | Surface | Count |
|---|---|---|
| Firebase REST (auth-default path) | `securetoken.googleapis.com/v1/token`, `identitytoolkit.googleapis.com/v1/accounts:signInWithCustomToken` | 2 endpoints |
| `/api/v1/*` REST (post-login) | `/api/v1/agent/identities` | 1 endpoint |
| GraphQL queries | `GetUser`, `GetUserSettings`, `GetWorkspacesMetadataForUser`, `GetRequestLimitInfo`, `FreeAvailableModels` | 5 ops |
| GraphQL subscription | `GetWarpDriveUpdates` over `wss://rtc.app.warp.dev/graphql/v2` w/ `graphql-transport-ws` | 1 subscription |

Skip-login (anonymous) path replaces the Firebase token-exchange with:
`CreateAnonymousUser` mutation + `MintCustomToken` mutation +
`signInWithCustomToken` REST → still Firebase-coupled by default.

### User-action-gated traffic (NOT first-launch)

- 40+ other `/api/v1/agent/*` and `/api/v1/harness-support/*` endpoints
- 60+ GraphQL mutations (the entire `crates/graphql/src/api/mutations/` tree)
- 30+ GraphQL queries
- WSS `wss://sessions.app.warp.dev` (only when user shares a session)
- SSE `/api/v1/agent/events/stream` (only when an agent run is active)
- `oz.warp.dev/*` (browser opens, not server-to-server)
- 6 `/oauth/connect/{slug}` browser flows (GitHub / GCP / etc. third-party connect)

### Off-by-default in OSS build

- Rudderstack analytics (`telemetry_config: None`)
- Sentry crash reporting (`crash_reporting_config: None`)
- Autoupdate (`autoupdate_config: None`)
- MCP static config (`mcp_static_config: None`)

These need **zero work** to be silent.

### Hardcoded URLs (per the inventory)

~30 hits, classified:

| Class | Count | Action |
|---|---|---|
| Channel-config defaults (`config.rs:46-68`) | 4 | already overridable via env (proven) |
| Firebase REST direct (`credentials.rs:194,197`) | 2 | replaceable by avoiding Firebase credential path |
| Oz browser links (`oz_launch.rs`, `cloud_setup_guide_view.rs`, etc.) | ~6 | redirect or stub-handle URL scheme |
| Static legal/marketing URLs (`util/links.rs`, `auth_view_body.rs`, etc.) | ~5 | low-stakes, can rewrite or leave |
| Linux installer scripts (`releases.warp.dev/linux/...`) | ~5 | installer-time only, not in-app |
| Build-time / docs-only refs | rest | non-blocking |

## Warp ↔ Life primitive mapping

| Warp concept | Life equivalent | Wiring notes |
|---|---|---|
| `AgentTask` (server-side cloud agent) | Authored Agent (`agents/<name>.md` + Arcan tick loop) | New authored agent `agents/warp-task.md` translates Warp's prompt + context into Arcan's `spawn_agent` |
| `createAgentTask(prompt, env, …)` mutation | `lifed.Agent.CreateSession` over UDS | lifegw adapter: takes Warp's GraphQL input, calls `spawn_agent("warp-task", ctx)`, returns synthesized `taskId` |
| `warpDriveUpdates` subscription | lago session tail (gRPC `Events.Subscribe`) | lifegw bridges WSS frames ← lago event stream; per-event format translation |
| SSE `/api/v1/agent/events/stream` | same lago tail | second transport over the same event source |
| Firebase user identity | Anima identity (DID, P-256 auth + secp256k1 wallet) | lifegw mints a Life Tier-2 cap token from anima local-user state, returns it as a Bearer token Lumen treats as `Credentials::Bearer` |
| `/login/remote` browser handoff | local URL-scheme redirect to a Lumen-built-in "you're already signed in" page, OR pre-injection via `WARP_USER_SECRET` | bypasses the OAuth round-trip entirely |
| `oz.warp.dev/runs/{id}`, `oz.warp.dev/agents/...` (browser links) | new minimal Life-OS web UI OR `arcan` CLI invocations | not strictly required for first-launch; defer |
| Warp Drive cloud objects | lago objects (filesystem manifests + content-addressed blobs) | direct mapping, only need translation layer |
| Conversation history | lago session journal | direct mapping |
| Sandboxed agent env | Praxis sandbox + Soma-anchored capabilities | already exists |
| Wallet / billing (`overages`, `purchase_addon_credits`) | Haima (x402 + per-task billing) | already exists |
| Model picker / `FreeAvailableModels` | `inference-core::BackendRegistry` (Spec E) | Spec E lands inference backends; lifegw exposes them via `freeAvailableModels` resolver |
| Cloud env / Remote Dev | Spaces (planned) + Praxis | partial; defer |
| Session sharing (RTC) | Spaces (text/voice channels + agent log) | already exists; bridge later |

The 1:1 mapping is remarkably clean — Warp built their own version of
Life's architecture. Every primitive has a Life counterpart already
shipped or specced.

## Option space

Five options, in order of patch-surface (smallest → largest):

### Option α — Compatibility layer: lifegw mimics Warp's API

lifegw adds a `warp-compat` GraphQL/REST surface that translates every
Warp call into the underlying Life primitives. Lumen sees what looks
like a Warp backend but it's Life under the hood.

**Pros**
- We've already built ~80% of this (the GraphQL SDL walker, 11 wired T1 ops, the warp_graphql route module, the Cynic schema mirror in lifegw, Patch E TraceLayer).
- Lumen source patches stay minimal: only the four already applied (`channel-oss-override`, `skip-metal-when-wgpu`, `skip-oss-login`, `rename-to-lumen`).
- Fast to first-light. The transport already works as of `f77ae8a9`.
- Upstream-syncable: pulling new warp-oss versions doesn't require re-patching the entire networking layer.

**Cons**
- We're permanently coupled to Warp's API *shape* even if not their backend. Every Warp upgrade is a potential breakage.
- Inherits Warp's data model (their AgentTask shape, their `WarpDriveUpdates` event variants) — translation cost grows as we expose more Life surface.

**Verdict**: best for Phase α (immediate unblock); the compatibility layer is also a clean integration point we'd build *anyway* if we ever wanted other Warp-compatible clients to use Life.

### Option β — Replace `warp_server_client` wholesale

Fork the entire `crates/warp_server_client/` and rebuild it as
`lumen_life_client` speaking lifed's native gRPC over loopback /
HTTPS.

**Pros**
- Clean Life-native wire. No compatibility shim. No translation layer.
- Lumen becomes a first-class Life client; the same wire could serve other terminals or apps.

**Cons**
- Massive engineering effort — `warp_server_client` is ~80 endpoints + heavy retry/auth/telemetry plumbing. Months of work.
- Upstream sync becomes nightmare: every warp-oss change in `warp_server_client` requires re-evaluation of our fork.
- Higher-level callers (`server_api/ai.rs`, `server_api/team.rs`, etc.) have their own shape expectations; we'd be touching ~30 crates downstream.

**Verdict**: too expensive for Phase α; viable as Phase γ after the compatibility layer has shipped + been validated.

### Option γ — Hybrid translator (Phase α extended into β)

Keep Option α's compatibility layer, but expose a *parallel* Life-native
RPC surface on lifegw (e.g., `/life/v1/*`). Migrate call sites in
Lumen's source one-by-one as upstream-divergence becomes safe to take
on. The compatibility layer ages out gradually.

**Pros**
- Combines α's fast-time-to-light with β's clean long-term destination.
- Incremental migration: pick high-leverage call sites first.
- The compatibility layer survives indefinitely for any client that wants to use Life with the Warp API shape.

**Cons**
- Two API surfaces to maintain.
- Migration path takes longer to commit to "fully Life-native".

**Verdict**: best long-term architecture. **Recommended path**.

### Option δ — Credential bypass: skip Firebase, keep everything else

Lumen has a built-in escape hatch: `Credentials::ApiKey` and
`Credentials::Bearer` variants in `auth/credentials.rs:17-33` bypass
Firebase entirely. Setting `WARP_USER_SECRET` at launch time
(`auth_state.rs:133-138`) injects a credential. So we can:

- Mint a Life Tier-2 cap token at lifegw
- Inject it into Lumen via `WARP_USER_SECRET=<life-token>` at launch
- Lumen attaches it as `Authorization: Bearer …` on every call
- lifegw recognizes the token + routes accordingly

**Pros**
- Zero source patches to Lumen beyond what's already applied.
- Cleanest possible split: auth is *fully* Life's, transport is unchanged.
- Independent of Option α/β/γ — composes with any of them.

**Cons**
- This is just the auth-channel part of α; needs to be combined with at least the read-side endpoints to be useful for first-launch.

**Verdict**: this is **Phase α₁**, the immediate-unblock sub-option of α. Land it first.

### Option ε — Greenfield Life-native terminal

Skip warp-oss entirely. Build a new Rust terminal (on wezterm /
zellij / alacritty substrate) with Life as the day-one backend.

**Pros**
- Zero Warp coupling.
- No upstream-fragility.

**Cons**
- Months of engineering. Throws away Patch A–E and all empirical evidence we just gathered.
- We'd be re-implementing the parts of warp-oss that are best-in-class (terminal kernel, block model, GPU rendering).

**Verdict**: not for this year. A long-term aspirational target if the Lumen fork ever becomes unmaintainable.

## Recommended path — phased

**Phase α₁ — Credential bypass (1 day, the immediate unblock)**

1. Add a `lifegw /life/v1/auth/bootstrap` endpoint that returns a Life
   Tier-2 cap token for the current local identity (Anima-backed if
   available, otherwise a dev-mode stub).
2. Update `scripts/lumen-app.sh launch` to call that endpoint at
   start, capture the token, and pass it via `WARP_USER_SECRET` to
   the Lumen process.
3. Lumen reads it (`auth_state.rs:133-138`), wraps as
   `Credentials::Bearer`, attaches to every call.
4. **Validation**: lifegw TraceLayer log shows authenticated requests
   (`Authorization: Bearer life_…`) arriving from Lumen. The
   `/api/v1/oauth/device/auth` content-type-mismatch from `f77ae8a9`
   goes away because Lumen never enters the login flow.

**Phase α₂ — First-launch read endpoints (1-2 days)**

Implement these on lifegw, each mapped to a Life primitive:

| Warp endpoint | Life mapping | Status today |
|---|---|---|
| GraphQL `GetUser` | anima identity → synthesized Warp user shape | needs new resolver |
| GraphQL `GetUserSettings` | local settings file or default | trivial |
| GraphQL `GetWorkspacesMetadataForUser` | single-workspace stub OR future Spaces channels | trivial stub for now |
| GraphQL `GetRequestLimitInfo` | constant "unlimited" stub OR Haima budget | trivial stub for now |
| GraphQL `FreeAvailableModels` | `inference-core::BackendRegistry` (when Spec E ships) OR a hardcoded list | trivial stub |
| REST `GET /api/v1/agent/identities` | list of named agents from `agents/*.md` | needs file-system scan |

**Validation**: Lumen launches, populates user picker + model picker +
workspace UI without any 401s or fetch errors in
`~/Library/Logs/lumen.log`. lifegw log shows ~6 decoded HTTP requests
per launch.

**Phase α₃ — Agent dispatch (2-3 days)**

The heart of the integration.

1. Author a new authored agent: `core/life/agents/warp-task.md`
   - Frontmatter: name, description, inputs (prompt, environment_uid,
     attached_files, conversation_id)
   - Body: prompt template that takes Warp's task envelope and
     instructs the agent to run as a Warp-style cloud agent
2. lifegw `createAgentTask` mutation:
   - Validates input
   - Calls `lifed.Agent.CreateSession` with
     `authored_agent: "warp-task", context: {prompt, env, ...}`
   - Returns synthesized `taskId` (the Life session_id, opaquely)
3. lifegw `warpDriveUpdates` subscription:
   - Subscribes to lago event tail for the session
   - Translates Life event variants → Warp's `AmbientTaskUpdated` etc.
   - Streams over the existing `graphql-transport-ws` route (already
     proven working)
4. lifegw `/api/v1/agent/events/stream` SSE bridge:
   - Same lago tail, SSE transport per `Accept: text/event-stream`

**Validation**: typing a prompt into Lumen's agent panel produces a
real Life authored-agent invocation. Arcan's tick loop runs, lago
records every event, Lumen renders the stream as Warp blocks. The
agent has access to Praxis tools + anima identity + Haima budget — all
the Life primitives — without any Warp coupling.

**Phase α₄ — Oz browser-link redirect (1 day)**

Replace the ~6 `oz.warp.dev` URL hits with one of:
- Open `arcan run show <id>` in a separate terminal tab
- Open a tiny Life-OS web UI at `http://localhost:3001/runs/<id>`
- Stub-handle the URL scheme + display a "Coming in Life-OS UI" notice

Lowest priority — these are user-clickable UI links, not blocking
paths.

**Phase β — incremental call-site migration (ongoing)**

Once Phase α is stable and Lumen runs end-to-end:
- Identify Warp endpoints where the Life-native wire would be
  significantly faster, simpler, or richer
- Add `/life/v1/*` equivalents on lifegw with native shapes
- Patch the corresponding warp-oss call sites to use the new endpoint
- Compatibility layer ages out organically

**Phase γ — fork completion (12+ months out)**

If/when Lumen's divergence from warp-oss is large enough that
upstream-sync no longer makes sense, formalize the fork. Replace
`warp_server_client` with `lumen_life_client`. By this point most
call sites have already migrated via Phase β.

## Phase α₁ in detail (the next concrete commit set)

The smallest credible unblock:

### Deliverables

1. **`crates/life-runtime/lifegw/src/services/auth_bootstrap.rs`** —
   new axum route `POST /life/v1/auth/bootstrap` returning
   `{ access_token: "life_eyJ…", user_id: "anima:did:peer:…",
   expires_in: 3600 }`. Uses the existing `Tier2Minter` to mint a cap
   token scoped to "lumen-client-access".

2. **`scripts/lumen-app.sh launch`** — pre-launch curl to
   `https://127.0.0.1:8443/life/v1/auth/bootstrap`, capture token,
   pass to Lumen process via `WARP_USER_SECRET` env var.

3. **No Lumen source patches** — `Credentials::ApiKey` /
   `WARP_USER_SECRET` consumption already exists upstream
   (`auth_state.rs:133-138`).

### Validation

```bash
./scripts/lumen-app.sh down && ./scripts/lumen-app.sh launch
sleep 8
grep -c "Authorization: Bearer" /tmp/lumen-smoke/lifegw.log    # ≥ 1
grep -c "401 Unauthorized" /tmp/lumen-smoke/lifegw.log         # 0
```

### Estimated effort

- lifegw endpoint: 30-60 LOC + 1 test
- launcher integration: 10 LOC of bash
- evidence capture + commit: standard

Total: ~half a day.

## Open questions for the next session

1. **Anima identity shape** — for Phase α₁'s bootstrap endpoint, do
   we mint a fresh ephemeral anima identity per Lumen launch, or
   require the user to have an established one? (Spec D D-Sub-C's
   InProcessAnima covers ephemeral; SomaCustody covers persistent.)
2. **Workspace concept** — Lumen expects at least one workspace.
   Map to Life's "personal workspace" (anima-scoped) or to Spaces
   channels?
3. **Model registry source-of-truth** — until Spec E (BackendRegistry)
   ships, do we hardcode a single-model list (claude-sonnet, gpt-4o)
   in `FreeAvailableModels`, or use a config file at
   `~/.config/lumen/models.toml`?
4. **Authored agent variations** — Phase α₃ ships `warp-task.md`.
   Do we want `warp-task-fast.md` (lower-cost) and `warp-task-deep.md`
   (longer context) as separate authored agents Lumen surfaces in the
   model picker?
5. **Event-shape translation** — `WarpDriveUpdates` → lago events
   needs a concrete schema mapping. Likely a small `event_translate.rs`
   module in lifegw. Where do its tests live?

## Done-when

- [ ] Phase α₁ shipped: `WARP_USER_SECRET` injection works, lifegw
      log shows authenticated traffic from Lumen, zero 401s.
- [ ] Phase α₂ shipped: first-launch read endpoints respond with
      meaningful (not error) bodies; Lumen UI populates without
      "Failed to fetch" warnings.
- [ ] Phase α₃ shipped: typing a prompt into Lumen's agent panel
      triggers an authored agent invocation visible in lago.
- [ ] Phase α₄ shipped: no `oz.warp.dev` browser opens fire.
- [ ] Lumen runs for 30 minutes with zero requests outbound to any
      `*.warp.dev`, `*.googleapis.com`, `*.firebaseapp.com`,
      `*.sentry.io`, `*.rudderstack.com`. Verified by tcpdump on
      `lo0` + a `pf` rule blocking those domains as a tripwire.

## Crosscheck against bstack primitives

- **P11 Empirical** — every phase has a concrete "validation" line
  with a log-grep / screenshot / count check.
- **P14 Dep-Chain** — each phase enumerates upstream files it touches
  + downstream consumers (covered in the per-phase tables).
- **P3 Tickets** — Phase α₁ → α₄ each becomes a Linear ticket on the
  Lumen Phase α umbrella once we start executing.
- **P10 Worktree** — all Phase α work continues in
  `.worktrees/lumen-phase-alpha-m7-w`; only when Phase β starts do
  we evaluate a new worktree.
- **P15 Snapshot** — current state: `feat/lumen-phase-alpha-m7-w` @
  `f77ae8a9`, 11 commits ahead of `main`, Layer 2-GUI 9/9 closed.

## References

- `docs/lumen/agent-driven-feedback-loop.md` — substrate architecture
- `docs/lumen/validation/2026-05-15/README.md` — Layer 2-GUI closure evidence
- `core/life/CLAUDE.md` "Where does new behavior live?" — authored agents decision tree
- `core/life/docs/superpowers/specs/2026-05-09-bro-1006-authored-agents-architecture.md` — authored agents spec
- `apps/warp-oss-patch/upstream/crates/warp_core/src/channel/config.rs` — Warp's channel-config (the hard-coded URLs we override)
- `apps/warp-oss-patch/upstream/crates/graphql/src/api/` — all 90+ GraphQL operations
- `apps/warp-oss-patch/upstream/app/src/auth/auth_state.rs:133-138` — `WARP_USER_SECRET` consumption (Phase α₁'s opening)
