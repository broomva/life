# M9 Decomposition — apps/broomva onto SDK + edge endpoints

**Date**: 2026-05-20
**Status**: PLANNING (no implementation yet)
**Owner**: Spec C M9 (Wave 4 — public-launch gate)
**Parent ticket**: BRO-924
**Critical-path successor**: M10 (Spec C public launch)
**Author**: Stream B1 (planning agent)

## What this document is

A consolidated execution plan for **Spec C M9** that unifies two pre-existing threads:

1. The **2026-05-02 M9 Anima-custody plan** (`docs/superpowers/plans/2026-05-02-m9-anima-custody-apps-migration.md`) — passkey enrollment, USDC e2e on Base testnet, mission-control desktop pairing.
2. The **2026-05-20 BRO-1208 edge-endpoints Phase 1 plan** — `/api/v1/messages`, `/api/v1/chat/completions`, and `/api/chat` lifegw rewire (Urgent, just filed, scoped, references already-merged PRs #1399 / #1400 / #187).

The two threads share the same Vercel app (`broomva.tech/apps/broomva`) and the same gateway (`lifegw`), so they merge naturally into a single decomposed work plan. M9 ships when **all 7 sub-phases land** and the **acceptance criteria** in §M9 done are satisfied.

> **Path correction.** BRO-924 description says `broomva.tech/apps/chat/lib/*`. The actual path is `broomva.tech/apps/broomva/lib/*` (verified 2026-05-20). The chat surface lives at `apps/broomva/app/(chat)/` + `apps/broomva/app/api/(agent|chat-model|life|life-proxy)/`. The decomposition uses the verified path; BRO-924 description has been patched.

## P14 dep-chain (M9 as a whole)

**Upstream (M9 consumes):**

- Spec D 100% (PRs #1070-#1084, 2026-05-01..02) — `AnimaCustody` trait + 6 backends + `/anima/custody/*` routes + `TierUserMinter` + WS bearer subprotocol
- M8 SDK v0.1.0-pre (PR #1072, 2026-05-02) at `sdks/life-sdk-ts/` — services, typed errors, WS state machine, `WebCryptoAnima`, `RemoteAnimaClient`, `SessionCap`
- M7-FINAL lifegw production (PR #1071, 2026-05-01) — production KMS, JWKS, chaos battery
- M5 lifed production-shipped (PR #1063, 2026-04-29) — admin plane, pool, breaker, replay
- Spec C M0-M8 ship history (`docs/STATUS.md`)
- BRO-1208 prerequisites already merged 2026-05-20: spec PR #1399 (life), handoff PR #1400 (life), CORS middleware PR #187 (broomva.tech)
- **M8.1 (BRO-1137)** — Connect-vs-grpc-web wire-protocol reconciliation (Backlog) — *blocks SDK-direct browser unary/server-streaming calls; sidestepped for custody by D-Sub-C's `/anima/custody/*` HTTP/JSON routes; sidestepped for chat by BRO-1208's edge-endpoint shim*
- **M8.2 (BRO-1138)** — Browser WS auth via `Sec-WebSocket-Protocol: bearer.<jwt>` for `Agent.StreamSession` (Backlog) — *RESOLVED for custody by D-Sub-C; still open for streaming chat over WS from the browser*

**Downstream (M9 unlocks):**

- M10 — public `broomva.tech/life` launch
- Sentinel + materiales-intel.v1 freelance tenants migrate to lifed
- broomva.tech AAP verifier coordination (M9-G, cross-repo)
- The two SDK wire-protocol limitations (M8.1, M8.2) become non-blocking — but should land in M11 cleanup before SDK GA

## P15 state (2026-05-20)

- `apps/broomva/lib/*` has ~1,884 LOC of bespoke client code in top-level files alone (counted), plus 7 sub-modules (`arcan/`, `lago/`, `life-runtime/`, `relay/`, `credits/`, `limits/`, `graph/`) — `life-runtime/` alone is ~40 files including the `agent-session/` WS-client suite and `kernel/` proxy
- `apps/broomva/app/api/agent/chat` exists; sibling routes include `agent/`, `life/`, `life-proxy/`, `chat-model/`
- `chat.config.ts` (~5 KB), `proxy.ts` (~9 KB) live at app root
- BRO-924 is **Backlog**; BRO-1208 is **Todo, Urgent**; BRO-1137 + BRO-1138 are **Backlog**
- broomva.tech repo branch state: `feat/llm-endpoints-docs-sync` is the most recent dev branch; main is the deploy target; CORS PR #187 already merged

## Sub-phase decomposition

The seven executable sub-phases below mirror the **2026-05-02 M9.1-M9.7** numbering where possible, but **reorder by critical path**: BRO-1208's edge endpoints (M9-A here, ex-M9.0) now lead because they unblock the rest of the chat-surface migration. Anima-custody work (ex-M9.1..M9.7) follows since the edge shim makes downstream wallet operations addressable without browser-direct lifegw calls.

| Sub-phase | Scope | LOC (est) | Ticket | Parallel-safe? | Blocked-by | Effort |
|---|---|---|---|---|---|---|
| **M9-A — Edge endpoints Phase 1** | `/api/v1/messages` + `/api/v1/chat/completions` + `/api/chat` rewire through lifegw. Drop Arcan-direct + streamText fallback. 4-PR sequence per BRO-1208. | ~800 TS net + tests | BRO-1208 (exists, Urgent) | NO — gates everything downstream | — | 4-PR sequence; ~5d |
| **M9-B — apps/broomva/lib/ Phase 1 cleanup** | Delete `lib/arcan/`, `lib/lago-client.ts`, `lib/lago-assets.ts`, `lib/lago/`, `lib/skills-data.ts`. Update imports. Add SDK as dep. | -800 net, +200 SDK wiring | BRO-1212 | YES with M9-C/D/E once M9-A merged | M9-A | ~3d |
| **M9-C — Passkey enrollment UI** | `/account/security/passkey` settings page, WebCryptoAnima + PasskeyOracle factory in `packages/auth` or `lib/anima/`. Better-Auth post-signin hook. | ~400 TSX + 200 lib | BRO-1213 | YES (frontend) | M9-A | ~4d |
| **M9-D — Tier-User cap sign-in flow** | After passkey enrollment, mint Tier-User cap on every sign-in via `mint_session_cap`. IndexedDB persistence (PasskeyOracle handles). `Authorization: Bearer` on `/anima/custody/*` calls. | ~250 TS | BRO-1214 | YES (auth) | M9-C | ~3d |
| **M9-E — Server-side anima daemon + USDC e2e** | Production lifegw with `anima_custody` config, production soma admin plane, Vault secp256k1 sidecar. Playwright e2e: chat → sign EIP-3009 → broadcast → Base testnet receipt. | ~300 config + 400 test | BRO-1215 | YES (infra) — runs against staging while M9-C/D land | M9-A | ~5d (infra) + 3d (e2e) |
| **M9-F — apps/broomva/lib/ Phase 2 cleanup** | Delete `lib/agent-auth.ts`, `lib/agent-auth-client.ts`, `lib/anonymous-session-{client,server}.ts`, `lib/life-runtime/`. Migrate `lib/relay/`, `lib/credits/`, `lib/limits/`, `lib/tier-access.ts`, `lib/stripe.ts`, `lib/graph.ts`, `lib/tenant-context.ts` to SDK. | -2000 net, +400 SDK wiring | BRO-1216 | NO — depends on M9-D auth and M9-E wallet | M9-A, M9-D, M9-E | ~5d |
| **M9-G — broomva.tech AAP verifier coordination** | broomva.tech adopts `lago-auth::verify_jwt` shape. Multi-curve verifier (ES256 primary, EdDSA legacy). Acceptance: rotation-aware JWT verification. | ~300 TS | BRO-1217 | YES (cross-repo) | M9-E | ~2d |

**Mission-control desktop pairing (ex-M9.6)** is **DEFERRED to M11** (post-public-launch). It is not on the M10 public-launch critical path: chat is the public surface, mission-control is internal tooling. The deferral is a scope-shrinkage decision logged here to keep M9 finite.

**Estimated total**: ~24 calendar days if fully parallel, ~32 days if M9-B + M9-C + M9-D + M9-E + M9-G run sequentially. Wave-dispatch (P19 `bstack wave dispatch`) is the recommended mechanism for M9-B/C/D/E/G after M9-A merges.

## Critical-path diagram

```
                 M9-A (BRO-1208 — gate)
                       │
       ┌───────────────┼───────────────┬──────────────┐
       ▼               ▼               ▼              ▼
     M9-B           M9-C            M9-E          (M9-G starts
   (BRO-1212)     (BRO-1213)     (BRO-1215)        after M9-E
                     │                  │            staging)
                     ▼                  │
                   M9-D                  │
                 (BRO-1214)              │
                     │                   │
                     └────────┬──────────┘
                              ▼
                            M9-F (BRO-1216)
                              │
                              ▼
                          M9 done → M10 public launch
```

**Cycle check**: no cycles. M9-B is parallel-safe with M9-C through M9-E (different file regions). M9-G can start as soon as M9-E reaches a stable staging deployment.

## Linear ticket map

| Sub-phase | Ticket | Existed before this pass? | Status | Blocked-by |
|---|---|---|---|---|
| M9-A | BRO-1208 | Yes (filed 2026-05-20 by user) | Todo (Urgent) | — |
| M9-B | BRO-1212 | NEW (this pass) | Todo (High) | BRO-1208 |
| M9-C | BRO-1213 | NEW (this pass) | Todo (High) | BRO-1208 |
| M9-D | BRO-1214 | NEW (this pass) | Todo (High) | BRO-1213 |
| M9-E | BRO-1215 | NEW (this pass) | Todo (High) | BRO-1208 |
| M9-F | BRO-1216 | NEW (this pass) | Todo (High) | BRO-1208, BRO-1214, BRO-1215 |
| M9-G | BRO-1217 | NEW (this pass) | Todo (Medium) | BRO-1215 |

All 6 new tickets are children of BRO-924 (via `parentId`). BRO-924 stays the M9 umbrella; its description has been patched with this decomposition table.

BRO-1137 + BRO-1138 remain Backlog; they are *not* M9 blockers because the BRO-1208 edge-endpoint shim sidesteps them for chat (server-side gateway speaks both protocols; SDK only sees its own framing inside the edge route). They become cleanup tickets for v0.2.0 SDK GA, scheduled post-M10.

## Risk register

### R1 — Browser passkey UX flow gate (HIGH)

**Risk**: A user signs in on a fresh device with no passkey enrolled. Re-enrollment on every fresh device is bad UX; full account recovery (email + KYC) is a hard wall. Cross-device passkey sync (FIDO2 conditional UI / iCloud Keychain / Google Password Manager) is partial — not universal across browsers.

**Manifestation**: M9 ships, a user opens broomva.tech on a phone with no synced passkey, gets stuck.

**Mitigation**:
- Default UX: detect missing passkey → prompt "enroll new passkey on this device" → user enrolls → new device gets its own DID, rotation event published.
- Fallback: existing-device-issued rotation cap. The first device mints a rotation cap (15-min TTL, single-use) and presents a QR/manual code; the new device redeems it for an enrollment session.
- Last resort: email-based recovery is *deferred to M11*. M9 doesn't ship account-recovery; it ships "passkey or no service". Document this explicitly in the M9-C settings UI.
- **Owner**: M9-C handoff includes UX wireframes for these three states.

### R2 — USDC e2e on Base testnet (MEDIUM)

**Risk**: Vault secp256k1 sidecar (D-Sub-B follow-up) isn't production-ready. HashiCorp Vault v1.15 doesn't natively support secp256k1; options are (a) Vault Enterprise w/ pluggable transit ($$$), (b) HSM sidecar via PKCS#11 (complexity), (c) wait for upstream. Without a real signer, USDC e2e can't actually broadcast.

**Manifestation**: M9-E lands the infra wiring but the signature path returns a stubbed signature; broadcast fails on Base testnet.

**Mitigation**:
- Pre-M9-E spike: stand up softhsm in dev, validate PKCS#11 secp256k1 signature against a known test vector (~half day).
- Production deployment uses softhsm-in-Docker as a stopgap; document the upgrade path to Vault Enterprise or real HSM as M10-launch follow-up.
- e2e test gracefully degrades: if Vault returns 503, the test stubs the broadcast and asserts the signature shape only, with a clear "live-broadcast disabled" log line. Real-broadcast variant runs nightly, not per-PR.
- **Owner**: M9-E handoff has explicit pre-flight checklist for Vault.

### R3 — M8.1 / M8.2 wire-protocol mismatch (LOW for chat surface, HIGH for SDK GA)

**Risk**: `@broomva/life-sdk@0.1.0-pre` cannot directly call lifegw from a browser (Connect-JSON vs tonic-web/grpc-web codec mismatch — BRO-1137; and `Sec-WebSocket-Protocol: bearer.*` not yet honored on `Agent.StreamSession` upgrades — BRO-1138). The 2026-05-02 M9 plan assumed direct SDK calls in M9.1.

**Manifestation**: M9 ships but the SDK is still effectively unusable outside of Node hosts; v0.2.0 SDK GA is blocked.

**Mitigation**:
- M9 ships via the **edge-endpoint shim pattern** (BRO-1208 + M9-B/F). The Next.js edge routes in `apps/broomva/app/api/v1/*` accept Anthropic-shaped + OpenAI-shaped JSON, do server-side translation, and call lifegw with the correct on-the-wire format. The SDK is *not on the critical path* for M9 — it's a follow-up cleanup.
- Track BRO-1137 + BRO-1138 as M10 cleanup tickets, not M9 blockers. Resolution path: BRO-1137 → switch SDK to gRPC-Web binary framing via `@bufbuild/protobuf` (Option A in KNOWN_LIMITATIONS.md). BRO-1138 → extend lifegw WS auth middleware to honor `Sec-WebSocket-Protocol: bearer.*` for `Agent.StreamSession` (D-Sub-C precedent in PR #1084).
- **Owner**: deferred; M11 candidates if not absorbed by M10 final cleanup.

### R4 — Vercel-side bundle constraints (LOW)

**Risk**: WebCryptoAnima + RemoteAnimaClient + Connect-ES + tonic-web codecs together push the client bundle over Vercel Hobby plan limits (1 MB compressed) or balloon the cold-start of edge functions.

**Manifestation**: M9-C / M9-D land, deploy preview cold-starts hit ~3-5s; Vercel free tier OOM warnings.

**Mitigation**:
- Lazy-load `WebCryptoAnima` only on `/account/security/passkey` and wallet-flow pages; not on home or chat.
- Code-split the WebAuthn passkey-prompt flow behind a dynamic import (`next/dynamic`).
- Server-side custody calls (`/anima/custody/*`) go through edge routes, so the browser doesn't need to bundle the codec stack — only `fetch` JSON.
- Set a hard bundle-size budget in `apps/broomva/next.config.ts` (`bundleAnalyzer` + `experimental.bundlePagesRouterDependencies`).
- **Owner**: M9-C handoff includes a pre-merge bundle-size check (P11 step).

### R5 — Rotation chain + multi-curve verifier complexity (MEDIUM)

**Risk**: M9-G's multi-curve (ES256 + EdDSA) rotation-aware verifier is the trickiest piece. broomva.tech's existing JWT verification uses Better Auth's defaults (HS256 today; possibly RS256). Migrating to the canonical `lago-auth::verify_jwt` shape across all routes is a wide blast radius.

**Manifestation**: A subtle verification regression locks legitimate users out, or worse, accepts a token that should have been rejected post-rotation.

**Mitigation**:
- M9-G ships **side-by-side**: new verifier mounted on `/api/v1/*` only; existing routes keep Better Auth's verifier. Migration to global is a separate M10 ticket.
- Acceptance suite ports the rotation-chain test from `crates/anima/anima-identity/src/journal.rs` to a Vercel-Edge-compatible TS test (Vitest) with timestamp-keyed JWTs.
- 24h pre-launch chaos drill: rotate a test user, assert old tokens fail post-rotation and new tokens succeed within 30s.
- **Owner**: M9-G handoff specifies the side-by-side mount as a hard constraint, not an option.

## Cross-cutting validation (per sub-phase)

Every sub-phase handoff includes these P11 gates:

- `bun run typecheck` + `bun run test` + `bun run biome check` (broomva.tech repo)
- For Rust changes (M9-E, M9-G server side): `cargo test --workspace` + `cargo clippy -- -D warnings` + `cargo fmt --check`
- Contract test against in-process lifed harness (where applicable)
- Manual smoke on Vercel preview deploy from `https://broomva.github.io` Origin (CORS path)
- P9 watcher (`p9 watch --background`) after every push; auto-merge `--merge` (broomva.tech doesn't allow `--squash`)

## Out of scope for M9 (deferred to M10/M11)

- Mission-control desktop TPM + Ledger pairing (ex-M9.6 → M11)
- Full FIDO2 attestation chain verification (D-Sub-C R-2 follow-up → M11)
- Generic EIP-712 encoder shared across backends (D-Sub-A..F follow-up → M11)
- Live softhsm CI fixture for D-Sub-D (→ M11)
- Live Ledger e2e for D-Sub-F (→ M11)
- BRO-1137 (SDK Connect-vs-grpc-web reconciliation) → M11 (sidestepped by edge-endpoint shim for M9)
- BRO-1138 (SDK browser WS auth) → M11 (sidestepped by Tier-User HTTP cap path for custody; still blocks browser-direct `Agent.StreamSession`)
- Sentinel + materiales-intel.v1 tenant migration → M10 work post-public-launch
- Global migration of broomva.tech routes off Better Auth verifier → M11+ (M9-G is `/api/v1/*` side-by-side only)

## Sub-handoff index

Each sub-phase has a dedicated handoff doc in `~/broomva/conductor/workspaces/broomva/kolkata/.context/handoffs/`:

- `2026-05-20-spec-c-m9-sub-a.md` — Edge endpoints Phase 1 (BRO-1208)
- `2026-05-20-spec-c-m9-sub-b.md` — apps/broomva/lib Phase 1 cleanup (BRO-1212)
- `2026-05-20-spec-c-m9-sub-c.md` — Passkey enrollment UI (BRO-1213)
- `2026-05-20-spec-c-m9-sub-d.md` — Tier-User cap sign-in flow (BRO-1214)
- `2026-05-20-spec-c-m9-sub-e.md` — Production anima infra + USDC e2e (BRO-1215)
- `2026-05-20-spec-c-m9-sub-f.md` — apps/broomva/lib Phase 2 cleanup (BRO-1216)
- `2026-05-20-spec-c-m9-sub-g.md` — broomva.tech AAP verifier (BRO-1217)

## Recommended first dispatch

**M9-A (BRO-1208)**. It is Urgent, scoped, has decisions locked (D1-D6 in BRO-1208 description), references already-merged prerequisites (PRs #1399/#1400/#187), and gates every other sub-phase. The first PR in BRO-1208's 4-PR sequence (`/api/v1/messages` + shared infra) is small, well-bounded, and a textbook subagent-driven-development dispatch target.

## Acceptance criteria (M9 done)

1. Edge endpoints `/api/v1/messages` + `/api/v1/chat/completions` accept and forward Anthropic-shaped + OpenAI-shaped JSON to lifegw; `/api/chat` re-routes internally to lifegw; Arcan-direct + streamText fallback paths removed (BRO-1208 closed)
2. `apps/broomva/lib/` shrinks by ≥40% (BRO-1212 + BRO-1216 closed)
3. chatOS browser user can enroll a passkey via `/account/security/passkey` and see their DID + wallet address in settings (BRO-1213 closed)
4. chatOS browser user can sign in via Tier-User cap, with cap re-mint on tab open if cached cap expired (BRO-1214 closed)
5. Production lifegw + soma + Vault stand up; live USDC EIP-3009 e2e on Base testnet returns a valid signature + broadcast receipt (BRO-1215 closed)
6. broomva.tech accepts P-256 / ES256 JWTs via the canonical `lago-auth::verify_jwt` verifier path; rotation-aware (BRO-1217 closed)
7. All sub-phase CIs green; risk register updated with any new findings; STATUS.md M9 entry written

## Tracking

- Linear epic: `BRO-924` (parent, description patched by this pass)
- Sub-tickets: M9-B through M9-G created by the planning agent (BRO-1212 .. BRO-1217); M9-A is BRO-1208 (pre-existing)
- Status tracked in `docs/STATUS.md` § "M9 progress" (new section added per sub-phase ship)

## See also

- 2026-05-02 Anima-custody apps plan: `docs/superpowers/plans/2026-05-02-m9-anima-custody-apps-migration.md` (superseded by this plan for sub-phase numbering; M9.6 deferred)
- Spec D anima-custody: `docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md`
- Spec C₃ close-codes: `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md`
- SDK known limitations: `sdks/life-sdk-ts/KNOWN_LIMITATIONS.md`
- BRO-1208 sub-spec: linked in ticket body (PR #1399 merged 2026-05-20)
