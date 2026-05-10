# Lumen — Warp on Life Agent OS

**Date**: 2026-05-10
**Status**: Draft (locked decisions inline; Phase α approved for immediate implementation)
**Sibling of**: Spec C₃ M7 (lifegw edge gateway) — adds the Warp-compatible GraphQL surface as a new sub-phase **M7-W**.
**Composes with**: Spec D (Anima Custody, 100% shipped), Spec E (Agent-Loop Compute Contract, E-Sub-A shipped 2026-05-08).
**Owner**: lifegw + new `apps/lumen/` (β phase).
**Linear umbrella**: TBD (created post-spec-approval via MCP Linear, NOT linear CLI per workspace rule).

## Problem

Warp open-sourced its client on 2026-04-28 under AGPLv3 (UI framework crates `warpui_core` + `warpui` are MIT). The repository is `github.com/warpdotdev/warp` — 63 Rust workspace crates, 4,134-line GraphQL schema (`crates/warp_graphql_schema/api/schema.graphql`), GPU-rendered terminal on `wgpu`, MCP support, ~56k stars at announcement. OpenAI is the founding sponsor of the open repo.

What Warp **kept proprietary** is **Oz** — the cloud orchestration platform: agent scheduling, memory, handoff, verification loops, model routing, billing, settings sync, conversation state, authentication. The Warp client speaks GraphQL to `app.warp.dev`; Oz lives behind that endpoint. The client has no fallback path that would let it run without Oz.

Three structural costs of leaving the integration unaddressed:

1. **Life Agent OS has no end-user surface.** Life ships nine substrates (arcan, lago, autonomic, haima, praxis, spaces, anima, inference) plus 13 bstack primitives, but most users don't run a daemon stack — they use a terminal. Without a developer-facing client, Life remains an architectural project even as its primitives mature.

2. **Spec E (Agent-Loop Compute Contract) has no hero consumer.** E-Sub-A shipped on 2026-05-08, with E-Sub-B (MLX), E-Sub-C (vLLM), E-Sub-D (Vigil), E-Sub-E (Lago-backed KvCache), and E-Sub-F (conformance) fanning out. The "POSIX of agent silicon" thesis needs a real public consumer for the contract to mean anything. Without one, Spec E is a memo, not a moat.

3. **The substitution is uncommonly clean.** Every Oz responsibility maps 1:1 to a Life primitive that already exists or is in flight: arcan replaces Oz's agent loop; lago replaces conversation state and settings; AnimaCustody replaces auth; haima replaces billing; nous + autonomic replace verification loops; Spec E InferenceBackend replaces model routing. The boundary Oz hides behind — `app.warp.dev` GraphQL — is one URL change.

Lumen exists to close this gap: replace Oz with Life, present Life through Warp's UX, and give Spec E its hero consumer.

## Solution

A new sub-phase of Spec C₃ M7 — **M7-W: Warp GraphQL adapter** — extends lifegw with a wire-compatible mirror of Warp's GraphQL schema. Resolvers translate to lifed gRPC. Identity, billing, inference, memory, and agent invocation route through Life's existing substrates with no new orchestration code.

The integration ships in two phases:

- **Phase α (≤ 3 weeks)**: Official, unmodified Warp build is pointed at lifegw via endpoint configuration. Validates the integration end-to-end. No fork. Acceptance gate: an unmodified Warp client drives an arcan-backed agent task to completion against lifegw.

- **Phase β (≤ 6 weeks after α)**: Soft fork of `github.com/warpdotdev/warp` lands at `apps/lumen/`. Default endpoint, branding, first-launch flow, and bstack-aware skill registry baked in. Acceptance gate: a third-party developer downloads Lumen from broomva.tech, logs in, and drives an agent task — no Broomva engineer in the loop.

The same lifegw GraphQL adapter serves both phases. The Phase β fork is a UX and defaults layer (~1 KLOC delta + assets). All heavy engineering ships in Phase α and is reused.

The frontier vision — **Life as orchestrator of Warp** (arcan opens Lumen sessions, drives commands, observes blocks, plans next steps) — is acknowledged but explicitly deferred. Three v1 design constraints preserve the path:

1. The GraphQL surface is symmetric (callable from Lumen-as-client and from Life-as-client).
2. arcan's invocation channel accepts both reactive (Warp asks) and proactive (Life decides) triggers.
3. Lumen is treated as one display surface among peers (chatOS, mission-control, prosopon), not as Life's home.

## Locked Decisions

### L6-D1 — Project name is **Lumen**

Lumen pairs with Lago: lago is the event-sourced ground truth, Lumen is the surface that shines through it. Both names are short, latinate, and unambiguous in technical writing. The fork distribution is `apps/lumen/`; the lifegw sub-phase is `M7-W` (Warp), not `M7-L`, because the sub-phase is a wire-protocol adapter and would be reused even if a future client weren't called Lumen.

Rejected alternatives: *Vita* (already coupled to Anima naming), *Pneuma* (already in `core/prosopon/`), *Broomva ADE* (less memorable, conflates company and product).

### L6-D2 — Life is the default agent; Warp's agent picker is preserved

Out of the box, a Lumen session opens straight into arcan-as-agent: ConsciousnessActor → nous verification → autonomic homeostasis → lago memory. No agent picker required for the default path. Warp's existing custom-agent surface (Claude Code, Codex, Gemini CLI) remains selectable. Lumen does not amputate UX it inherits.

Rejected alternative: making Lumen agent-pickable but defaulting to Warp's built-in agent. This forfeits the strategic point of the fork — that Life is the brain — and forces every user through an extra step to use the integration the product exists to demonstrate.

### L6-D3 — Wire protocol is verbatim Warp GraphQL

lifegw's `warp_graphql` module copies `crates/warp_graphql_schema/api/schema.graphql` (4,134 lines) byte-identically into `crates/lifegw/src/warp_graphql/schema_mirror/schema.graphql`. The adapter uses async-graphql in **SDL-first mode** so the schema file *is* the contract; resolvers register against type names from the SDL.

Rejected alternative: hand-curating a subset schema. This would force forking the Warp client to match whatever subset we picked, eliminating Phase α's value (run the integration against unmodified Warp). SDL-first means schema introspection from the client matches byte-for-byte.

### L6-D4 — Tiered resolver implementation: ~30 wired (T1+T2), ~90 stubbed (T3)

Of the ~122 operations in Warp's schema (22 queries + 65 mutations + 1 subscription), Lumen v1 wires ~30 and stubs the rest. Stubs return safe defaults (`null`, empty list, "feature not enabled in this build") that the Warp client treats as feature-flag-off without erroring.

| Tier | Count | Purpose | Examples |
|---|---|---|---|
| **T1 — must implement (working agent)** | ~15 | Minimum surface for an agent session to work end-to-end | `generateDialogue`, `createAgentTask`, `updateAgentTask`, `getUser`, `createAnonymousUser`, `mintCustomToken`, `getUserSettings`, `updateUserSettings`, `warpDriveUpdates`, `getFreeAvailableModels`, `setUserIsOnboarded`, `generateApiKey`, `expireApiKey`, `issueTaskIdentityToken` |
| **T2 — production UX** | ~15 | Conversation history, code search, billing visibility, MCP discovery | `listAIConversations`, `getAIConversationFormat`, `generateCommands`, `getRequestLimitInfo`, `getConversationUsage`, `getRelevantFragments`, `rerankFragments`, `generateCodeEmbeddings`, `getSimpleIntegrations`, `codebaseContextConfig`, `getAIOveragesForWorkspace` |
| **T3 — stub** | ~90 | Warp.dev SaaS scaffolding Lumen does not reproduce | All `*Team*` mutations, `cloudObject` family, `*MerkleTree`, `purchaseAddonCredits`, `stripeBillingPortal`, `*FileArtifact*`, `*ManagedSecret*` (deferred to lago wiring later) |

Rejected alternative: implementing the full surface. ~70% of Warp's mutations are SaaS-collaboration features (teams, sharing, permissions) that Lumen v1 does not reproduce because they are Warp.dev product features, not platform requirements. They become T3 stubs and graduate to T1/T2 only if a clear Lumen use case emerges.

### L6-D5 — Agent invocation is `createAgentTask` + multiplexed `warpDriveUpdates` subscription

Warp does not have a separate `Agent.StreamSession` mutation. Agent invocation is two operations:

1. Client calls `createAgentTask(input)` — returns `task_id`.
2. Client subscribes (or has previously subscribed) to the *single* `warpDriveUpdates` subscription, filtered to `AmbientTaskUpdated` events for that `task_id`.

`warpDriveUpdates` is a multiplexed channel carrying seven event types (ObjectActionOccurred, ObjectContentUpdated, ObjectDeleted, ObjectMetadataUpdated, ObjectPermissionsUpdated, TeamMembershipsChanged, AmbientTaskUpdated). All concurrent task lifecycles for a user share the same subscription.

The lifegw adapter bridges to `lifed.EventService.Subscribe` (already shipped in Spec C M5 sub-phase A). lifed multiplexes events from arcan (per-task agent output), lago (conversation state changes), vigil (telemetry), and autonomic (homeostatic signals) into the unified `AmbientTaskUpdated` frame format Warp expects.

Rejected alternative: adding a separate `agentStream` mutation that returns a per-task stream. This would diverge from Warp's wire protocol, breaking Phase α's "unmodified Warp client" gate.

### L6-D6 — Authentication: broomva.tech OAuth-PKCE → JWT → AnimaCustody

First-launch auth flows through broomva.tech's existing Better Auth via standard OAuth-PKCE:

1. Lumen opens browser to `https://broomva.tech/lumen/auth?redirect=http://127.0.0.1:<port>/cb&challenge=<PKCE>`.
2. User authenticates against Better Auth (email/passkey/whatever broomva.tech supports).
3. Browser 302s to `localhost:<port>/cb?code=<one-time>`.
4. Lumen calls `mintCustomToken { code, verifier }` against lifegw.
5. lifegw resolver:
   - Validates code with broomva.tech `/api/auth/verify`.
   - `AnimaCustody.bind_oauth_session` wraps the broomva session in an Anima-signed claim (binds the broomva user_id to an anima DID).
   - Mints JWT `{ sub: user_id, aud: "lumen", scope: ["agent:read", "agent:write"], iat, exp: 24h }`.
6. Lumen stores JWT in OS keychain; uses `Authorization: Bearer <JWT>` header thereafter.

TpmAnima enrollment is offered as an **optional graduation** for users who want hardware-backed identity for high-stakes operations (haima settlements, multi-tenant agent supervision). Default users stay on broomva.tech-issued JWTs.

Rejected alternative: pure-TpmAnima first-launch flow. broomva.tech is the canonical Broomva identity surface and reusing it eliminates net new auth infrastructure. The OAuth-PKCE pattern works on every desktop OS without per-platform special-casing.

### L6-D7 — Code search via Spec E embeddings + lago graph index

Warp's `getRelevantFragments`, `rerankFragments`, `generateCodeEmbeddings` operations route as follows:

```
generateCodeEmbeddings:
  Lumen → mutation { workspace_id, files[] }
  lifegw → lifed.InferenceService.Embed (Spec E InferenceBackend, embedding model)
       → lago appends EmbeddingComputed events keyed by (workspace_id, file_path, sha)
       → bookkeeping P6 scores high-novelty fragments; promotes them to L3 entities

getRelevantFragments / rerankFragments:
  Lumen → query { workspace_id, query }
  lifegw → lifed.LagoQueryService.GraphSearch
       → lago graph index returns ranked event refs
       → resolver hydrates and returns fragments
```

Strategic side-effect: every code search becomes a knowledge-graph query. Bookkeeping P6 fires on Lumen indexing automatically; high-novelty code patterns become Layer 3 entity pages. The Layer 2 → Layer 4 pipeline now runs on developer code, not just conversation transcripts.

Performance caveat: lago graph index is event-sourced, not a vector index. For workspaces > ~100k LOC similarity search may be slow. Acceptable for v1 (most relevant Lumen workspaces are smaller); HNSW/FAISS layered as a v1.1 optimization tracked as a follow-up ticket.

Rejected alternative: shipping a parallel FAISS/HNSW index for code embeddings. This forks the data path — code search uses one substrate, agent memory uses another — and forfeits the bookkeeping integration. v1 prefers architectural consistency; v1.1 adds the optimization where it's needed.

### L6-D8 — Skill exposure leverages **Praxis**, not a new lifed service

Warp's `getSimpleIntegrations` and `mcp_gallery_template` types are the natural attachment point for a skill catalog. Praxis already provides this surface as the canonical tool execution and skill engine:

- `praxis-skills` does SKILL.md discovery + parsing + registry.
- `praxis-mcp` exposes any `ToolRegistry` as an MCP server (stdio for Claude Desktop, Streamable HTTP via axum).

The lifegw adapter delegates to Praxis rather than re-implementing skill discovery in lifed:

- `getSimpleIntegrations` → `lifed.RoutingCacheService.ListMcpServers` (already shipped in M5 sub-phase C) → enumerates Praxis-registered MCPs + user-configured external MCPs.
- `mcp_gallery_template` lookups → praxis-skills registry.

The bstack 27-skill catalog (P1–P13 primitives + the user-installed `~/.claude/plugins/cache/.../skills/`) is registered into praxis-skills at lifed startup; it then surfaces in Lumen via Warp's existing MCP UX without new code paths.

Rejected alternative: a new `lifed.SkillsService` parallel to `praxis-skills`. This would reinvent canonical Praxis primitives for no architectural gain and create two skill catalogs to keep in sync.

### L6-D9 — Cloud Agents are deferred; sandbox routes through Praxis + Soma when needed

Warp's Oz includes containerized cloud agents triggered by webhook/schedule/chat. Lumen v1 does not reproduce this. When required:

- Sandbox isolation routes through `praxis-core::SandboxPolicy` + future OS-level isolation (Aegis project, currently planned).
- Cloud-agent containerization routes through `soma` (kernel daemon, custody-oracle UDS, group authn — Spec D D-Sub-E shipped 2026-05-02) — soma already has the right primitives for sandboxed agent execution.
- Scheduling routes through future `chronos` project (currently planned).

Rejected alternative: building a parallel cloud-agent runtime in v1. This would compete for the architectural slot Aegis + Chronos already own, fragmenting the Life roadmap.

### L6-D10 — Frontier vision (Life-orchestrates-Warp) shapes v1 design but is not built

Three v1 design constraints preserve the frontier path without building it:

1. **Symmetric GraphQL surface.** lifegw's Warp-compatible API is callable both *from* Lumen (today) and *by* Life as a client (tomorrow). No client-only assumptions in resolver signatures.
2. **Bidirectional arcan invocation.** ConsciousnessActor's input channel accepts both Warp-originated triggers (reactive) and arcan-originated triggers (proactive). Same loop, two trigger sources.
3. **Lumen as a peer surface, not the surface.** Branding and telemetry treat Lumen as one display surface among peers (chatOS, mission-control, prosopon).

Rejected alternative: building Life-orchestrates-Warp in v1. The UX surface for "Life decides to open a terminal session" is non-trivial and would expand v1 scope by months. v1 ships the substrate; v2+ adds orchestration UX.

### L6-D11 — License: AGPLv3 for the Lumen fork; lifegw retains its existing license

The Lumen client is a soft-fork of AGPLv3 Warp; the fork inherits AGPLv3. The lifegw GraphQL adapter does not link against Warp source — Lumen client speaks HTTP — so no AGPL contagion to lifegw. lifegw retains whatever license it currently carries (Apache-2.0 / MIT dual, per the workspace default).

This aligns with Broomva's open-source-first stance documented in repository conventions.

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│ Phase α (validate, ≤ 3 weeks)                                    │
│   Official warpdotdev/warp build, unmodified                     │
│   $ WARP_API_ENDPOINT=https://lifegw.broomva.tech/graphql warp  │
└────────────────┬─────────────────────────────────────────────────┘
                 │
┌────────────────▼─────────────────────────────────────────────────┐
│ Phase β (productize, ≤ 6 weeks after α)                          │
│   Lumen client at apps/lumen/                                    │
│   Soft fork of warpdotdev/warp, AGPLv3                           │
│   Default endpoint = lifegw.broomva.tech, Broomva branding       │
└────────────────┬─────────────────────────────────────────────────┘
                 │  same wire protocol both phases
                 │  GraphQL/HTTPS + graphql-transport-ws (WS)
                 │  Authorization: Bearer <JWT>
                 │
┌────────────────▼─────────────────────────────────────────────────┐
│ lifegw — Spec C₃ (existing) + M7-W (NEW)                         │
│ Existing: TLS 1.3, JWT/JWKS, KMS, rate-limit, admin plane,       │
│           cert hot-swap, WS heartbeat, Spec C₃ §6.5 close codes  │
│ NEW (M7-W): warp_graphql adapter                                 │
│   ├── schema_mirror/schema.graphql (verbatim 4,134-line copy)    │
│   ├── adapter.rs — async-graphql SDL loader + resolver registry  │
│   ├── auth.rs — Bearer JWT → AnimaCustody-issued claims          │
│   ├── dataloader.rs — batched lifed gRPC (N+1 prevention)        │
│   ├── subscriptions/warp_drive_updates.rs — multiplexed bridge   │
│   ├── resolvers/{tier1, tier2, tier3_stubs}/                     │
│   └── conformance/ — schema introspection + golden response tests│
└────────────────┬─────────────────────────────────────────────────┘
                 │  internal gRPC (Tokio + tonic)
                 │
┌────────────────▼─────────────────────────────────────────────────┐
│ lifed facade — Spec C M5 (production-shipped 2026-04-29)         │
│ • Agent.StreamSession  ◄── arcan ConsciousnessActor (default)    │
│ • Events               ◄── lago + vigil multiplex                │
│ • Identity proxy       ◄── AnimaCustody                          │
│ • Wallet proxy         ◄── haima x402                             │
│ • RoutingCache         ◄── praxis-mcp + bstack 27-skill catalog  │
│ • Saga proxy           ◄── multi-step tool approvals             │
└────────────────┬─────────────────────────────────────────────────┘
                 │
                 ├── inference  → Spec E InferenceBackend
                 │                (E-Sub-A InProcess ✓; B/C/D fanning out)
                 ├── arcan      → THE default agent (ConsciousnessActor loop)
                 ├── nous       → verification (replaces Oz "verification loops")
                 ├── autonomic  → homeostasis (RCS L1)
                 ├── lago       → memory, settings, history, code embeddings
                 ├── haima      → x402 per-task billing
                 ├── anima      → AnimaCustody (TpmAnima graduation)
                 ├── praxis     → tool execution + MCP server registry
                 └── (deferred) soma + future Aegis/Chronos for Cloud Agents

[Frontier path, deferred — v1 design preserves it]
arcan ──── outbound calls ───→ lifegw GraphQL ───→ Lumen sessions
   (Life drives Warp, not just serves it)
```

### Code-magnitude estimate (new code only)

| Component | Where | Approximate size |
|---|---|---|
| `warp_graphql` adapter (schema mirror + resolvers + auth + dataloader + subscriptions) | `crates/lifegw/src/warp_graphql/` | ~3 KLOC Rust |
| Conformance battery | `crates/lifegw/tests/warp_conformance.rs` + golden fixtures | ~50 tests + ~30 JSON goldens |
| arcan Warp-task adapter (createAgentTask → ConsciousnessActor) | `arcan/crates/arcan-core/src/warp_agent.rs` | ~1 KLOC |
| OAuth-PKCE first-launch flow (β only) | `apps/lumen/src/auth.rs` + browser handoff | ~500 LOC |
| Lumen fork patches (β only) | `apps/lumen/` (defaults, branding, assets) | ~1 KLOC + assets |
| TS SDK bindings (mirror D-Sub-C wiremock pattern) | `sdks/life-sdk-ts/src/lumen/` | ~400 LOC + 30 tests |

All Rust crates use **edition = "2024"**, **rust-version = "1.85"**, per workspace conventions.

## Schema Strategy

The Warp schema lives at `crates/warp_graphql_schema/api/schema.graphql` in the upstream repo. The adapter copies this file byte-identically into `crates/lifegw/src/warp_graphql/schema_mirror/schema.graphql`. async-graphql's SDL-first mode loads it at startup; resolvers register against type names declared in the SDL.

Three implementation rules:

1. **The schema mirror is pinned to a specific upstream commit.** Schema changes upstream do not silently propagate. Conformance tests catch drift; rebase is a quarterly explicit operation.

2. **Tier 3 stubs use a single fallback resolver.** Any operation without an explicit resolver registration falls through to a generic stub that returns the SDL's nullable default (`null`, empty list, empty object). The Warp client treats these as "feature flag off."

3. **Federation directives (`@key`, `@extends`) in the schema are accepted but not federated.** lifegw is a single gateway; we don't actually federate to subgraphs. async-graphql 7.x parses the directives and lets us serve a single composed schema.

## Tiered Operation Rollout

### Tier 1 — must implement (working agent)

| Operation | Maps to |
|---|---|
| `generateDialogue` | `lifed.AgentService.OpenSession` → arcan ConsciousnessActor |
| `createAgentTask` | `lifed.AgentService.CreateTask` → arcan task spawn |
| `updateAgentTask` | `lifed.AgentService.UpdateTask` → arcan task control (cancel/approve) |
| `getUser` | `lifed.IdentityService.GetUser` → broomva.tech-bound user |
| `createAnonymousUser` | `lifed.IdentityService.CreateAnonymous` → temp user_id (upgraded on OAuth) |
| `setUserIsOnboarded` | `lifed.IdentityService.SetOnboardingFlag` |
| `mintCustomToken` | lifegw resolver: OAuth-PKCE code exchange → AnimaCustody-bound JWT |
| `issueTaskIdentityToken` | lifegw resolver: short-lived task-scoped JWT |
| `generateApiKey` / `expireApiKey` | `lifed.IdentityService.{IssueApiKey, RevokeApiKey}` |
| `getUserSettings` / `updateUserSettings` | `lifed.SettingsService.{Get, Update}` → lago |
| `getFreeAvailableModels` | `lifed.InferenceService.ListAvailableModels` → Spec E backend registry |
| `warpDriveUpdates` (subscription) | `lifed.EventService.Subscribe` (multiplexed) |

### Tier 2 — production UX

| Operation | Maps to |
|---|---|
| `listAIConversations` | `lifed.SettingsService.ListConversations` → lago query |
| `getAIConversationFormat` | `lifed.SettingsService.GetConversationFormat` |
| `generateCommands` | `lifed.InferenceService.Complete` → Spec E (small model) |
| `getRequestLimitInfo` | `lifed.WalletService.GetLimits` → haima |
| `getConversationUsage` | `lifed.WalletService.GetUsage` → haima |
| `getAIOveragesForWorkspace` | `lifed.WalletService.GetOverages` → haima |
| `getRelevantFragments` | `lifed.LagoQueryService.GraphSearch` (code embeddings) |
| `rerankFragments` | `lifed.InferenceService.Rerank` → Spec E |
| `generateCodeEmbeddings` | `lifed.InferenceService.Embed` → Spec E + lago events |
| `getSimpleIntegrations` | `lifed.RoutingCacheService.ListMcpServers` → praxis-mcp |
| `codebaseContextConfig` | `lifed.SettingsService.GetCodebaseConfig` |

### Tier 3 — stub

All `*Team*` mutations, `cloudObject` family, `*MerkleTree`, `purchaseAddonCredits`, `stripeBillingPortal`, `*FileArtifact*`, `*ManagedSecret*` (deferred to lago wiring), `getCloudEnvironments`, `listWarpDevImages`, `getAvailableHarnesses`, integration OAuth flows (`getOAuthConnectTxStatus`).

Stub returns: `null` for nullable single fields, `[]` for list fields, `{ success: false, reason: "feature not enabled in this build" }` for mutation results where the schema requires non-null structured response.

## Agent Invocation Flow

```
1. User types in Lumen → client mutates createAgentTask
       ▼
   lifegw.warp_graphql.resolvers.tier1::create_agent_task
       ▼
   gRPC: lifed.AgentService.OpenSession {
     user_jwt_claims: anima-bound,
     prompt: <user input>,
     model: <from getFreeAvailableModels selection>,
     workspace_id: <current>,
   }
       ▼
   lifed dispatches to arcan via internal channel.
   arcan spawns ConsciousnessActor for this task.
       ▼
   Returns { task_id, status: "running" }

2. Lumen client opens (or reuses) WS subscription warpDriveUpdates
       ▼
   lifegw.warp_graphql.subscriptions::warp_drive_updates
       ▼
   bridge to lifed.EventService.Subscribe (already shipped in M5 sub-phase A)
       ▼
   lifed multiplexes events from arcan + lago + vigil + autonomic

3. arcan emits per-step events (token deltas, tool calls, status transitions)
       ▼
   lifed.EventService publishes AmbientTaskUpdated frames keyed by task_id
       ▼
   lifegw filters by user_id + task_id, re-emits as graphql-transport-ws messages
       ▼
   Lumen client renders into block-by-block scrollback

4. Tool calls (when arcan needs Praxis)
       ▼
   ConsciousnessActor yields ToolCall to Praxis runtime
   Praxis executes (sandbox-policed via SandboxPolicy)
   Result returns to ConsciousnessActor; loop continues
       ▼
   AmbientTaskUpdated frames carry ToolCall + ToolResult deltas

5. Token usage / billing
       ▼
   arcan calls Spec E InferenceBackend → returns usage tokens
   autonomic increments per-task counters in lago
   haima x402 settles at task close (per plan tier: per-task, per-block, or session-batched)

6. Task completion
       ▼
   ConsciousnessActor emits terminal AmbientTaskUpdated { status: "completed" | "failed" }
   lifed.AgentService.UpdateTask records final state
   WS subscription continues for other tasks; per-task channel closes
```

## Identity & Auth

(Per L6-D6.) Initial onboarding via broomva.tech OAuth-PKCE; downstream JWT verified at lifegw via existing JWKS infrastructure. Optional TpmAnima graduation for users who want hardware-backed identity.

```
First launch:
  Lumen → opens browser to https://broomva.tech/lumen/auth?
                              redirect=http://127.0.0.1:<port>/cb
                              &challenge=<PKCE-S256>
  User → logs in via Better Auth (broomva.tech existing flow)
  broomva.tech → 302 to localhost:<port>/cb?code=<one-time>
  Lumen → POST /graphql/v2?op=mintCustomToken { code, verifier }
  lifegw resolver:
    1. POST broomva.tech/api/auth/verify { code, verifier }  →  user_id, profile
    2. AnimaCustody.bind_oauth_session(user_id) → anima_did + signed claim
    3. Tier-User minter (per Spec C₃ §5) issues JWT:
       { sub: user_id,
         aud: "lumen",
         scope: ["agent:read", "agent:write"],
         iat: <now>,
         exp: <now + 24h>,
         anima_did: <DID> }
  Lumen → stores JWT in OS keychain
  Lumen → all subsequent calls: Authorization: Bearer <JWT>

Refresh:
  At exp - 1h, Lumen → POST mintCustomToken { refresh_token }
  lifegw → re-mints with same anima_did, fresh exp

Optional graduation:
  User invokes "Enable hardware-backed identity" from Lumen settings.
  Lumen → invokes TpmAnima.enroll() (PKCS#11) — wallet half optional.
       → AnimaCustody persists keypair to TPM, emits enrollment event to lago.
  Future high-stakes operations require TPM-signed challenges (haima settlements,
  cross-tenant agent supervision).
```

The lifegw `mintCustomToken` and `issueTaskIdentityToken` resolvers are new (~200 LOC); all other auth machinery (JWKS verification, KMS, Tier-User minter, Spec D AnimaCustody binding) is already shipped.

## Code Search via Spec E + Lago

(Per L6-D7.)

```
Indexing path (generateCodeEmbeddings):
  Lumen ──(mutation)──▶ lifegw warp_graphql ──(gRPC)──▶ lifed.InferenceService.Embed
                                                            ▼
                                                  Spec E InferenceBackend
                                                  (embedding-capable backend, e.g.
                                                   E-Sub-B MLX or E-Sub-C vLLM)
                                                            ▼
                                                  EmbeddingComputed events to lago
                                                  keyed by (workspace_id, file_path, sha)
                                                            ▼
                                                  bookkeeping P6 scoring +
                                                  L3 entity promotion (async)

Query path (getRelevantFragments / rerankFragments):
  Lumen ──(query)──▶ lifegw warp_graphql ──(gRPC)──▶ lifed.LagoQueryService.GraphSearch
                                                          ▼
                                                lago graph index
                                                (event-sourced, BM25 + graph traversal)
                                                          ▼
                                                ranked event refs returned
                                                          ▼
                                                resolver hydrates fragment text + metadata
```

The integration is structurally honest about its v1 cost: lago graph index is event-sourced, not vectorized. For workspaces under ~100k LOC the latency is acceptable. Above that, v1.1 will add an HNSW index alongside as a Tier-2 optimization.

The strategic side-effect — every code search becomes a knowledge-graph query, with bookkeeping P6 scoring and Layer 3 entity promotion firing automatically — is intentional. It extends the bookkeeping pipeline from conversation transcripts to developer code with no additional integration work.

## Skills & MCP Integration

(Per L6-D8.) Praxis is the canonical skill and MCP engine; lifegw's `getSimpleIntegrations` and `mcp_gallery_template` resolvers delegate rather than reinvent.

```
Lumen startup:
  Lumen client ──(query getSimpleIntegrations)──▶ lifegw
        ▼
  lifegw resolver ──(gRPC)──▶ lifed.RoutingCacheService.ListMcpServers
        ▼
  Returns:
    - Praxis-registered internal MCPs (bstack 27-skill catalog)
    - User-configured external MCPs (from praxis-mcp client bridge)
    - Each entry: { name, description, schema_url, capabilities }

Skill invocation (when arcan needs to invoke a skill):
  arcan ConsciousnessActor ──▶ Praxis ToolRegistry.invoke(skill_name, args)
        ▼
  Praxis resolves: internal skill (SKILL.md) OR external MCP (subprocess bridge)
        ▼
  Returns ToolResult to ConsciousnessActor
        ▼
  Stream emission (AmbientTaskUpdated) with ToolCall + ToolResult frames

bstack catalog registration (one-time at lifed startup):
  lifed startup ──▶ scans ~/.claude/plugins/cache/.../skills/
        ▼
  Each SKILL.md → registers with praxis-skills::SkillRegistry
        ▼
  Now available via lifed.RoutingCacheService.ListMcpServers
```

The bstack 27-skill catalog (P1–P13 primitives + the user-installed skills) becomes first-class Lumen surface without new orchestration code. Lumen users get `/p9 watch`, `/persist iterate`, `/bookkeeping run`, `/haima pay` from day one.

## Lumen Fork Scope (β phase)

The fork delta is small because endpoint configuration in Warp is build-time configurable via `ChannelState::server_root_url()` (per schema-discovery findings). The Phase α path uses official Warp with an env var override (`WARP_API_ENDPOINT`); Phase β bakes the default in.

| Patch category | Approximate LOC delta |
|---|---|
| Default endpoint constant | < 10 LOC |
| Branding (icon, splash, "About" copy, color theme) | ~200 LOC + asset files |
| Default skill registry pointed at bstack | ~100 LOC |
| First-launch flow (broomva.tech OAuth-PKCE vs anonymous) | ~500 LOC |
| Telemetry endpoint redirect (vigil OTel collector) | ~50 LOC |
| **Total per-rebase delta** | **~1 KLOC + assets** |

The fork is small enough that maintaining sync with upstream is tractable. Cadence: weekly rebase, conformance battery validates against the latest upstream schema.

## Phasing

### Phase α (≤ 3 weeks): "validate"

| Sub-phase | Deliverable | Acceptance gate |
|---|---|---|
| **M7-W.1** | `crates/lifegw/src/warp_graphql/` scaffolding: schema mirror, async-graphql binding, JWT auth middleware, lifed gRPC dataloader, Tier-3 stub fallback | All T1 resolvers stubbed; schema introspection matches Warp's |
| **M7-W.2** | T1 resolvers wired (~15 ops including createAgentTask, generateDialogue, mintCustomToken, getUser/getUserSettings, getFreeAvailableModels) | All T1 ops return correct shape against golden tests |
| **M7-W.3** | `warpDriveUpdates` subscription bridged to `lifed.EventService`; arcan Warp-task adapter (`arcan-core/src/warp_agent.rs`); E2E smoke test | **Unmodified official Warp client, pointed at lifegw via env var, drives an arcan-backed task to completion** |

α completion gate = the E2E smoke. If unmodified Warp completes a task against our gateway, α is done.

### Phase β (≤ 6 weeks after α): "productize"

| Sub-phase | Deliverable | Acceptance gate |
|---|---|---|
| **L.1** | `apps/lumen/` soft-fork from `github.com/warpdotdev/warp@<pinned-commit>`: branding, default endpoint, assets | Builds + runs on macOS / Linux / Windows with Broomva branding |
| **L.2** | broomva.tech OAuth-PKCE first-launch flow + JWT keychain integration | First launch on clean machine → user reaches working agent in < 60 seconds |
| **L.3** | T2 resolvers wired (~15 ops: history, code search via lago + Spec E embeddings, billing visibility, MCP discovery via praxis-mcp) | All T2 ops return correct shape against golden tests |
| **L.4** | Skills registration (bstack 27-skill catalog → praxis-skills); first-class Lumen commands (`/p9`, `/persist`, `/bookkeeping`, `/haima`) | Skill commands work from Lumen prompt; first-class help system |
| **L.5** | Conformance battery + signed binary release pipeline + downloadable installers + autoupdate channel | Public download from `broomva.tech/lumen`; signed binaries for macOS / Linux / Windows |

β completion gate = a third-party developer downloads Lumen, logs in, drives an agent task. No Broomva engineer in the loop.

## Testing Strategy

| Layer | Where | What |
|---|---|---|
| Unit | `crates/lifegw/src/warp_graphql/**/tests.rs` | Resolver logic, JWT extraction, dataloader batching, schema mirror parsing |
| Integration | `crates/lifegw/tests/integration/` | gRPC roundtrips lifegw ↔ lifed for each T1 + T2 op |
| Conformance | `crates/lifegw/tests/warp_conformance.rs` | Schema introspection AST-match against `schema.graphql`; golden responses for ~30 wired ops; ~90 stubbed ops produce safe defaults |
| Multi-language | `sdks/life-sdk-ts/src/lumen/` (mirror Spec D D-Sub-C wiremock pattern) | TypeScript client bindings byte-equivalent to Rust auth flow |
| E2E smoke | `crates/lifegw/tests/e2e_smoke.rs` | docker-compose: lifegw + lifed + arcan + official Warp client; drive agent task; assert terminal status |

E2E is the α completion gate — same shape as the M5 e2e_smoke that ships in `crates/lifed`.

## Risk Register

| Risk | Severity | Mitigation |
|---|---|---|
| Warp upstream schema drift (open development = faster changes) | Medium-high | Pin to specific commit; conformance battery catches drift; quarterly rebase cadence with explicit migration ticket |
| `server_root_url` not env-overridable on official Warp build | Low (verified on day 1) | Confirm before kicking off α. Worst case: skip α, go straight to β fork (saves ~1 week of validation work) |
| AGPL contagion via lifegw GraphQL bindings | **None** | Lumen client speaks HTTP to lifegw — no linking, no contagion |
| Spec E backends not production-ready in time for L.3 (code embeddings) | Medium | E-Sub-A InProcess covers v1 baseline; E-Sub-B (MLX) and E-Sub-C (vLLM) mature in parallel; if neither lands by L.3, fall back to E-Sub-A's embedding model with reduced quality (acceptable for v1) |
| lago graph index slow on large codebases | Medium | Acceptable for v1 (< 100k LOC); HNSW optimization tracked as v1.1 follow-up ticket |
| Lumen fork rebase burden | Low | Delta is ~1 KLOC + assets; weekly rebase rhythm; CI validates conformance against latest upstream schema |
| Two-front war with M9 chatOS migration | Real | Phase α is 3 weeks of focused work; Phase β starts only after M9 ships |
| broomva.tech auth load increase | Low | Lumen sessions are JWT, not stateful; minimal new load on Better Auth |
| OpenAI sponsorship pressures upstream toward GPT-only model selection | Low for fork strategy | Lumen routes through Spec E InferenceBackend — model-agnostic by design; `getFreeAvailableModels` returns whatever Spec E backends are registered |
| Phase α reveals an unimplementable T1 resolver (e.g., `generateDialogue` semantics not satisfiable by current arcan) | Medium | Mitigation built into the M7-W.2 sub-phase: any T1 resolver that can't be honestly implemented is escalated immediately, not stubbed silently. Acceptable response is to refine the arcan-Warp adapter, not lie about completion |

## Linear Tickets (proposed; created post-spec-approval via MCP Linear)

```
BRO-XXXX (umbrella, Urgent): Lumen — Warp on Life Agent OS [Spec C₃ M7-W + Lumen fork]
├── BRO-XXXX (Urgent, 5pt):  M7-W.1 lifegw warp_graphql scaffolding + schema mirror + Tier-3 stubs
├── BRO-XXXX (High, 8pt):    M7-W.2 T1 resolvers (~15 ops)
├── BRO-XXXX (High, 5pt):    M7-W.3 warpDriveUpdates subscription + arcan Warp-task adapter + E2E smoke
├── BRO-XXXX (Med, 3pt):     L.1 Lumen fork + branding + endpoint defaults
├── BRO-XXXX (High, 5pt):    L.2 broomva.tech OAuth-PKCE flow + JWT keychain
├── BRO-XXXX (High, 8pt):    L.3 T2 resolvers (~15 ops, incl. lago code search)
├── BRO-XXXX (High, 3pt):    L.4 Skills registration (bstack catalog → praxis-skills)
└── BRO-XXXX (Med, 5pt):     L.5 conformance + release pipeline + signed installers

Total: 42 pts.
α = 18 pts (~3 weeks).
β = 24 pts (~6 weeks after α).
```

Tickets are created via MCP Linear (per workspace rule: never `linear` CLI — defaults to stimulus workspace).

## Open Questions for Follow-up Phases

These are explicitly out of v1 scope but acknowledged for follow-up planning:

1. **Cloud Agents parity (Oz feature gap).** Webhook/scheduled containerized agents. Routes through future `soma` + `praxis` + `chronos` (planned). Standalone spec to be written when scoped.

2. **Settings sync across devices.** Warp.dev's cross-device settings sync. Lumen v1 stores settings locally + lago. Cross-device requires lago sync (already designed as part of Lago platform strategy).

3. **Team / multi-tenant features.** All `*Team*` mutations are Tier-3 stubs in v1. If/when Lumen needs team features, they map to spaces (distributed agent networking) + lago multi-tenant scoping.

4. **HNSW / FAISS code embedding index.** v1.1 optimization for workspaces > ~100k LOC. Tracked as follow-up ticket when first user hits the perf cliff.

5. **Life-orchestrates-Warp (frontier vision).** v2+ scope. Requires new UX surfaces in arcan ("decide to open a Lumen session") and Lumen ("accept proactive invocations from Life"). Spec to be written when v1 ships and we have feedback on what proactive orchestration should look like.

6. **Native autoupdate channel for Lumen.** L.5 ships a basic release pipeline; full autoupdate (delta updates, signed manifest, rollback) is a follow-up.

7. **TpmAnima desktop integration UX.** Hardware-backed identity is offered as a graduation in v1 but the UX flow for "enroll your TPM" needs design work. Tracked separately.

## References

- Warp open-source announcement (2026-04-28): `https://www.warp.dev/blog/warp-is-now-open-source`
- Warp repository: `github.com/warpdotdev/warp`
- Warp GraphQL schema: `crates/warp_graphql_schema/api/schema.graphql` (4,134 lines, AGPLv3)
- Spec C₃ M7 (lifegw edge gateway): `core/life/docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md`
- Spec D (Anima Custody, 100% shipped): `core/life/docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md`
- Spec E (Agent-Loop Compute Contract, E-Sub-A shipped 2026-05-08): `core/life/docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md`
- Authored Agents architecture: `core/life/docs/superpowers/specs/2026-05-09-bro-1006-authored-agents-architecture.md`
- bstack primitive catalog: project-root `CLAUDE.md` (P1–P13)
- Praxis canonical tool engine: `core/life/praxis/CLAUDE.md`

---

**Next step**: spec self-review, then user review, then invoke `superpowers:writing-plans` to produce the implementation plan for Phase α.
