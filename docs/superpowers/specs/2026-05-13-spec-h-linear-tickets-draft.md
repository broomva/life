# Spec H — Linear Tickets Draft

> **STATE: SUPERSEDED 2026-05-13 — TICKETS FILED.** All 16 tickets filed under [BRO-1111](https://linear.app/broomva/issue/BRO-1111) umbrella on the **Life** project. Mapping below for traceability:
>
> | Section in this draft | Filed as | Status | Estimate |
> |---|---|---|---|
> | Umbrella | [BRO-1111](https://linear.app/broomva/issue/BRO-1111) | Backlog | 0 |
> | H-Sub-A (links PR #1242) | [BRO-1112](https://linear.app/broomva/issue/BRO-1112) | **Done** | 2 |
> | H-Sub-B | [BRO-1113](https://linear.app/broomva/issue/BRO-1113) | Backlog | 2 |
> | H-Sub-C | [BRO-1117](https://linear.app/broomva/issue/BRO-1117) | Backlog | 1 |
> | H-Sub-D | [BRO-1118](https://linear.app/broomva/issue/BRO-1118) | Backlog | 3 |
> | H-Sub-E | [BRO-1119](https://linear.app/broomva/issue/BRO-1119) | Backlog | 5 |
> | H-Sub-F | [BRO-1120](https://linear.app/broomva/issue/BRO-1120) | Backlog | 2 |
> | H-Sub-G | [BRO-1122](https://linear.app/broomva/issue/BRO-1122) | Backlog | 1 |
> | H-Sub-H | [BRO-1125](https://linear.app/broomva/issue/BRO-1125) | Backlog | 8 |
> | H-Sub-I | [BRO-1124](https://linear.app/broomva/issue/BRO-1124) | Backlog | 3 |
> | H-Sub-J | [BRO-1114](https://linear.app/broomva/issue/BRO-1114) | Backlog | 3 |
> | H-Sub-K | [BRO-1126](https://linear.app/broomva/issue/BRO-1126) | Backlog | 5 |
> | H-Sub-L | [BRO-1115](https://linear.app/broomva/issue/BRO-1115) | Backlog | 1 |
> | H-Sub-M | [BRO-1121](https://linear.app/broomva/issue/BRO-1121) | Backlog | 2 |
> | H-Sub-N | [BRO-1116](https://linear.app/broomva/issue/BRO-1116) | Backlog | 2 |
> | H-Sub-O | [BRO-1123](https://linear.app/broomva/issue/BRO-1123) | Backlog | 5 |
>
> **Total estimate:** 45 points (matches the spec doc). All `blockedBy` edges + parent relations wired during filing. Linear is now the canonical source for ticket bodies; this draft is retained below for historical reference.

All tickets target the **Life** project on the **broomva** team.

## Umbrella

**Title:** Spec H — Life Onboarding & Capability UX

**Body:**
```
Spec: core/life/docs/superpowers/specs/2026-05-13-spec-h-onboarding-capability-ux.md

Closes the developer-facing UX gap surfaced by the 2026-05-13 zero.xyz
architectural deep-dive (~/Documents/Zero_xyz_Architecture_Research_20260513).
Life's substrate (Anima Spec D shipped, Haima x402 client, Lago journal)
is ahead of named competitors; the first-run UX is behind.

Four phases:
- H.A Bootstrap (H-Sub-A..D, ~8 pts) — life init → identity + journal + status + welcome
- H.B Custody (H-Sub-E..G, ~8 pts) — --custody flag + EncryptedSeed + auto-detection
- H.C Capability (H-Sub-H..K, ~19 pts) — capability-registry + ReviewLedger + PaidCapability + Spaces gossip
- H.D Operator UX (H-Sub-L..O, ~10 pts) — upgrade check + attest/policy/trust + skill install + lineage wallets

Total: ~45 points across 15 sub-phases.
Critical path: A → B → C → J → H → I (E in parallel for production).

H-Sub-A SHIPPED in #1242 (anima identity + derived wallet on life init).
```

**Labels:** `spec`, `onboarding`, `capability`, `phase-1`
**Priority:** Urgent
**Estimate:** 0 (umbrella)
**Project:** Life

---

## H-Sub-A — Anima identity bootstrap on `life init` ✅ DONE

**Title:** Spec H H-Sub-A — Anima identity bootstrap on `life init`

**Body:**
```
Spec ref: §H-Sub-A
Shipped in: #1242

Extended `life init` to generate an InProcessAnima identity, derive the
secp256k1 EVM wallet, persist .life/identity/{soul.json, seed.local.bin},
print DID + wallet. 6 new tests on top of existing 3.

Soul descriptor includes did:key:zDn… (P-256 multicodec 0x1200),
EVM address on Base (eip155:8453), Blake3 soul hash, full AgentSoul JSON.
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-a`, `bootstrap`
**Priority:** High
**Estimate:** 2
**Status:** Done
**PR:** #1242

---

## H-Sub-B — Lago journal init at `.life/journal/`

**Title:** Spec H H-Sub-B — Lago journal init at `.life/journal/` + soul-genesis event

**Body:**
```
Spec ref: §H-Sub-B
Depends on: H-Sub-A

Open project-local Lago journal at .life/journal/events.redb after
identity bootstrap. Append AnimaEventKind::SoulGenesis as first event.
Idempotent — never write a second genesis.

Deliverables:
- crates/cli/life-cli/src/init.rs::bootstrap_lago_journal
- Cargo deps: lago-journal, anima-lago
- 4 new tests (genesis written, hash matches, idempotent, branch default)

Acceptance:
- life init exits with .life/journal/events.redb present
- lago log --data-dir .life/journal/ --limit 1 shows anima.soul_genesis
- soul_hash in event matches .life/identity/soul.json
- Re-running life init doesn't duplicate the genesis
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-b`, `bootstrap`, `lago`
**Priority:** High
**Estimate:** 2
**Blocked by:** H-Sub-A

---

## H-Sub-C — `life status` shows identity, wallet, journal state

**Title:** Spec H H-Sub-C — `life status` (local mode) surfaces identity + wallet + journal

**Body:**
```
Spec ref: §H-Sub-C
Depends on: H-Sub-A, H-Sub-B

Extend life status to support a no-args local mode. Reads
.life/identity/soul.json and .life/journal/events.redb; optionally
queries haimad for wallet balance (degrades gracefully if not running).

Deliverables:
- crates/cli/life-cli/src/status.rs — LocalStatusArgs variant
- --format=json for scripts
- 3 new tests (no .life/, identity-only, full state)

Acceptance:
- life status in fresh life init dir prints DID + wallet +
  "journal: 1 event, anima.soul_genesis"
- --format=json emits stable JSON shape
- Errors cleanly with "run life init" hint when .life/ missing
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-c`, `bootstrap`, `cli`
**Priority:** High
**Estimate:** 1
**Blocked by:** H-Sub-A, H-Sub-B

---

## H-Sub-D — `life init --claim-welcome` via a Haima merchant

**Title:** Spec H H-Sub-D — `life init --claim-welcome` welcome credit flow

**Body:**
```
Spec ref: §H-Sub-D
Depends on: H-Sub-A, H-Sub-B

Optional flag (default false) that signs a welcome-claim message with the
new wallet, POSTs to a Haima merchant at merchants.broomva.tech/welcome/claim,
receives back USDC credit. Recorded as finance.revenue_received event.

Client side:
- crates/cli/life-cli/src/wallet.rs::claim_welcome (new module)
- Signs via AnimaCustody::sign_digest

Server side (parallel sub-spec):
- apps/haima-merchants/welcome (or under broomva.tech)
- Signature verification, double-claim dedupe, settles 1 USDC on Base testnet

Acceptance:
- Fresh life init --claim-welcome produces wallet with USDC balance
- Re-running with same wallet rejects (server-side dedupe)
- lago log shows finance.revenue_received event
- Flag defaults to false; no surprise calls to broomva.tech
- Mainnet behind --mainnet flag once merchant funded
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-d`, `bootstrap`, `haima`, `growth`
**Priority:** Medium
**Estimate:** 3 (+ 2 for server-side merchant)
**Blocked by:** H-Sub-A, H-Sub-B

---

## H-Sub-E — `--custody=` flag on `life init`

**Title:** Spec H H-Sub-E — `--custody=in_process|vault|tpm|webcrypto|hardware|soma` flag

**Body:**
```
Spec ref: §H-Sub-E
Depends on: H-Sub-A; Spec D D-Sub-B..F (all shipped)

Add --custody flag dispatching to the right AnimaCustody constructor.
Each backend gated by a Cargo feature on life-cli (kms-vault, kms-tpm,
kms-soma, kms-remote, kms-webcrypto, hw-wallet). Default build = InProcess
only. Opt-in: cargo install life-cli --features kms-vault, etc.

Deliverables:
- crates/cli/life-cli/src/init.rs — custody dispatch
- New init args: --vault-url, --vault-token, --tpm-module, etc.
- Per-backend feature flags; clear "feature not enabled" error
- Feature-gated tests per backend (skipped when feature off)

Acceptance:
- life init --custody=vault (with kms-vault feature) produces soul.json
  with custody.kind: "vault", no seed file on disk
- life init --custody=vault on default build errors with
  "kms-vault feature not enabled; rebuild with..."
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-e`, `custody`, `production`
**Priority:** High
**Estimate:** 5
**Blocked by:** H-Sub-A

---

## H-Sub-F — `EncryptedSeed` at rest + keychain passphrase

**Title:** Spec H H-Sub-F — Encrypt seed.local.bin via EncryptedSeed + keychain passphrase

**Body:**
```
Spec ref: §H-Sub-F
Depends on: H-Sub-A

For InProcess deploys, replace raw 32-byte seed with anima_identity::EncryptedSeed
(ChaCha20-Poly1305) keyed by a per-install passphrase stored via
life-paths::credentials (keychain → .env fallback). Phase A's 0o600
was the bootstrap; this is the harder protection.

Deliverables:
- crates/cli/life-cli/src/init.rs::write_encrypted_seed (replaces write_seed)
- Random per-install passphrase via life-paths::credentials::store_credential
- Reload path reads passphrase, decrypts seed
- Migration: raw seed.local.bin auto-upgrades on next life init
- Tests: encrypt/decrypt roundtrip, wrong passphrase fails, raw→encrypted migration

Acceptance:
- Fresh life init produces seed.local.bin with 28+ bytes of ciphertext (not raw 32)
- cat seed.local.bin reveals no plaintext seed
- Deleting keychain entry makes seed unrecoverable
- Existing raw-seed installs auto-migrate
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-f`, `custody`, `security`
**Priority:** High
**Estimate:** 2
**Blocked by:** H-Sub-A

---

## H-Sub-G — Runtime context auto-detection for default custody

**Title:** Spec H H-Sub-G — Auto-detect runtime context to pick default custody backend

**Body:**
```
Spec ref: §H-Sub-G
Depends on: H-Sub-E (for the secret-store path)

When --custody not passed, pick default based on runtime:
- chatOS browser → WebCrypto
- /run/life/soma-admin.sock present → Soma
- PKCS#11 module configured via env → Tpm
- otherwise → InProcess

Mirrors Zero's CLAUDECODE / CURSOR_TRACE_ID env detection pattern,
applied to substrate selection.

Deliverables:
- crates/cli/life-cli/src/init/context.rs (new) — detect_default_custody()
- life init --custody=auto is default; explicit --custody=... always wins
- life init --explain-custody prints which signals fired and why
- Tests cover each detection branch with env manipulation

Acceptance:
- Mocked chatOS context picks WebCrypto
- Host with /run/life/soma-admin.sock picks Soma
- No signals picks InProcess
- --explain-custody is human-readable
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-g`, `custody`, `ux`
**Priority:** Medium
**Estimate:** 1
**Blocked by:** H-Sub-E

---

## H-Sub-H — `capability-registry` crate

**Title:** Spec H H-Sub-H — New `capability-registry` crate (Lago-projection + BM25 search)

**Body:**
```
Spec ref: §H-Sub-H
Depends on: H-Sub-B, H-Sub-J

New crates/capability-registry/ (sibling of lago). Maintains Lago
projection of capability.advertised / capability.reviewed / capability.deprecated
events. Exposes Registry::search(query, opts) -> Vec<CapabilityListing>
over BM25 index built from the projection. Reads local project Lago by
default; federates via Spaces (H-Sub-K) optionally.

Deliverables:
- crates/capability-registry/Cargo.toml + src/{lib,projection,search}.rs
- Workspace member registered
- BM25 over capability text + graph walk for related
- Trust-tier filter + expiration handling + deprecation
- 8+ tests covering event fold, ranking, filter, expiration, deprecation

Acceptance:
- Test publishing 3 capabilities → Registry::search("translation")
  returns them ranked by review-weighted score
- Trust-tier filter (min_tier=Provisional) excludes Unverified advertisers
- Expired capabilities (expires_at < now) disappear from results

LARGEST SINGLE SUB-PHASE.
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-h`, `capability`, `new-crate`
**Priority:** High
**Estimate:** 8
**Blocked by:** H-Sub-B, H-Sub-J

---

## H-Sub-I — `ReviewLedger` port + `capability.reviewed` events

**Title:** Spec H H-Sub-I — `ReviewLedger` port + capability.reviewed events on Lago

**Body:**
```
Spec ref: §H-Sub-I
Depends on: H-Sub-B

New trait ReviewLedger in aios-protocol::review. Default impl writes
capability.reviewed { payment_id, accuracy, value, reliability, content }
events on Lago, referencing prior finance.payment_settled by ULID.
Adds unreviewed_payments() projection.

Deliverables:
- crates/aios/aios-protocol/src/review.rs — ReviewLedger, Review, UnreviewedPayment
- crates/haima/haima-lago/src/review.rs (or haima-review crate) — Lago impl
- crates/cli/life-cli/src/review.rs — life review submit / list subcommands
- 6+ tests: submit, list-unreviewed, query-by-capability, idempotent updates

Acceptance:
- After mocked finance.payment_settled, life review list shows it
- life review submit <payment_id> --accuracy 5 --value 4 --reliability 5
  --content "..." writes capability.reviewed event
- lago replay --tree shows review linked to payment
- Same payment can't be reviewed twice without --force (updates existing)
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-i`, `capability`, `reputation`
**Priority:** High
**Estimate:** 3
**Blocked by:** H-Sub-B

---

## H-Sub-J — `PaidCapability` trait in `aios-protocol`

**Title:** Spec H H-Sub-J — `PaidCapability` trait — sibling of `Tool`, payment plumbing transparent

**Body:**
```
Spec ref: §H-Sub-J
Depends on: none (independent of H-Sub-H)

Define typed Rust trait for paid HTTP capabilities alongside existing Tool
trait. Implement adapter from haima-x402 so bazaar-discovered/advertised
capabilities are PaidCapability. Bridge in Praxis so paid + free tools
share a single call shape.

Deliverables:
- crates/aios/aios-protocol/src/capability.rs (new) — PaidCapability,
  InvocationDescriptor, PricingDescriptor
- crates/haima/haima-x402/src/paid_capability.rs — bazaar→PaidCapability adapter
- crates/praxis/praxis-core/src/paid_capability.rs — wrapper
- Praxis MCP server exposes both Tools and PaidCapabilities
- 5+ tests covering bazaar listing → PaidCapability, call shape, payment transparency

Acceptance:
- Bazaar capability and Praxis tool both invokable through dyn PaidCapability
- Paid path settles payment automatically; free path no-ops
- Praxis MCP server lists paid capabilities to MCP clients
- Agent calling PaidCapability doesn't need price upfront; PaymentPolicy
  evaluates inline
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-j`, `capability`, `aios-protocol`
**Priority:** High
**Estimate:** 3

---

## H-Sub-K — Spaces gossip channel for capabilities

**Title:** Spec H H-Sub-K — Spaces `#capabilities` channel + bridge to capability-registry

**Body:**
```
Spec ref: §H-Sub-K
Depends on: H-Sub-H, H-Sub-J

Opt-in Spaces channel where agents publish capability.advertised events.
Subscribers fold into local registry projection. Implements decentralized
discovery per L6-H4.

Deliverables:
- Spaces module table: CapabilityAdvertisement (signed Lago event payload)
- Reducer: publish_capability_advertisement (verifies signature against
  advertiser DID)
- crates/capability-registry/src/spaces_bridge.rs — subscription + fold
- life publish --capability <id> CLI command
- 4+ tests: signature verification, subscription fold, conflict resolution

Acceptance:
- Two test agents in two separate Lago journals, both subscribed to
  same Spaces #capabilities channel, see each other's advertisements
- Signature verification rejects malformed/stale advertisements
- Local registry reflects channel state within 1s of publish
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-k`, `capability`, `spaces`, `distributed`
**Priority:** Medium
**Estimate:** 5
**Blocked by:** H-Sub-H, H-Sub-J

---

## H-Sub-L — `life upgrade check`

**Title:** Spec H H-Sub-L — `life upgrade check` (daily silent version check)

**Body:**
```
Spec ref: §H-Sub-L
Depends on: none

Mirror Zero's update_check.json pattern: daily check against
https://broomva.tech/api/life/versions for new versions of life-cli,
arcan, haimad, etc. Non-blocking, runs on life init / life setup
(and any command after >24h since last check). Prints single-line
"Update available: …" nudge.

Deliverables:
- crates/cli/life-cli/src/upgrade.rs (new)
- ~/.life/update_check.json cache with last_check_at + latest_version per binary
- Server: apps/broomva.tech/app/api/life/versions/route.ts (new endpoint)
- 3 tests: cache hit, cache miss, server error degrades silently

Acceptance:
- Stale cache (>24h) triggers fresh fetch
- Network error never blocks any command
- Server endpoint has deterministic JSON shape
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-l`, `ux`, `release`
**Priority:** Low
**Estimate:** 1

---

## H-Sub-M — `life identity attest` + `life policy show` + `life trust show`

**Title:** Spec H H-Sub-M — Identity surface CLI commands (attest / policy / trust)

**Body:**
```
Spec ref: §H-Sub-M
Depends on: H-Sub-A

Three short CLI commands exposing identity + trust + policy surface
to users and to other agents reading the soul as data.

- life identity attest --claim "<text>" → emits JWS signed by auth key
  over { claim, did, timestamp }. Verifiable with any ES256 verifier.
- life policy show → reads soul.json::values (PolicyManifest), prints
  capability ceiling + safety constraints + economic limits.
- life trust show → calls lago-auth::verify_jwt against agent's DID,
  prints current trust tier + peer attestations.

Deliverables:
- crates/cli/life-cli/src/identity.rs (new) — three subcommands
- 4+ tests per subcommand

Acceptance:
- JWS from life identity attest verifies against soul.auth_pubkey via
  any third-party ES256 verifier
- life policy show matches soul.values byte-for-byte
- life trust show reflects current trust tier from Lago
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-m`, `identity`, `cli`
**Priority:** Medium
**Estimate:** 2
**Blocked by:** H-Sub-A

---

## H-Sub-N — Agent skill installation (opt-in)

**Title:** Spec H H-Sub-N — `life init --install-skills` agent skill installation (opt-in)

**Body:**
```
Spec ref: §H-Sub-N
Depends on: none

On explicit life init --install-skills or life setup prompt, write a
"life" skill (SKILL.md frontmatter) to relevant agent runtime dirs so
Claude Code, Cursor, Codex, Windsurf pick up life command surface
without manual config. Implements L6-H8 (opt-in only).

Deliverables:
- crates/cli/life-cli/skills/life.md — canonical skill content
- crates/cli/life-cli/src/skill_install.rs — writer (per-runtime paths)
- Detection: skip uninstalled runtimes
- 4 tests: install path per runtime, idempotent re-install, refuses
  to overwrite user-customized skill, --uninstall-skills cleans up

Acceptance:
- After life init --install-skills on machine with Claude Code,
  ~/.claude/skills/life/SKILL.md exists with canonical content
- Re-running is no-op unless --force-install-skills
- User-modified skill file preserved (warns instead of overwriting)
- --uninstall-skills removes all installed skill files
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-n`, `ux`, `skills`
**Priority:** Medium
**Estimate:** 2

---

## H-Sub-O — Multi-agent wallet hierarchy via Anima lineage

**Title:** Spec H H-Sub-O — Multi-agent wallet hierarchy via Anima lineage + parent spend caps

**Body:**
```
Spec ref: §H-Sub-O
Depends on: H-Sub-A, H-Sub-F

When agent spawns child agent (already supported via SoulBuilder::
lineage_entry), child receives own DID + wallet AND parent-side spend cap.
Child payments emit finance.payment_settled parented via
lineage_entry.parent_payment_id. Allows autonomous parents to delegate
budget to children with audit trail.

Deliverables:
- crates/anima/anima-core/src/soul.rs — SoulBuilder::parent_wallet_authority(
  parent_did, spend_cap)
- crates/haima/haima-core/src/policy.rs — PaymentPolicy enforces parent caps
- New event: finance.child_spend_authorized { parent_did, child_did,
  cap_micro_credits, expires_at }
- life agent spawn <name> --spend-cap=10000 CLI subcommand
- 6+ tests: spawn, cap enforcement, audit chain in lago replay --tree

Acceptance:
- life agent spawn child --spend-cap=10000μc creates
  .life/agents/child/identity/soul.json with parent reference
- Child payment within cap succeeds; over-cap rejected with clear msg
- lago replay --tree shows parent → child lineage
```

**Parent:** umbrella
**Labels:** `spec-f`, `sub-o`, `multi-agent`, `lineage`
**Priority:** Medium
**Estimate:** 5
**Blocked by:** H-Sub-A, H-Sub-F

---

## Filing Order

Once Linear re-auth lands, file in this order so blocked-by edges resolve cleanly on first save:

1. Umbrella (must exist first; subsequent tickets parent to it)
2. H-Sub-A (mark Done immediately, link #1242)
3. H-Sub-B, H-Sub-J, H-Sub-L, H-Sub-N (no blocked-by dependencies)
4. H-Sub-C (blocks on A + B), H-Sub-D (A + B), H-Sub-E (A), H-Sub-F (A), H-Sub-M (A)
5. H-Sub-G (E), H-Sub-O (A + F)
6. H-Sub-I (B), H-Sub-H (B + J)
7. H-Sub-K (H + J)

Total: 1 umbrella + 15 children = 16 tickets.

## Cross-References

- Spec doc: `core/life/docs/superpowers/specs/2026-05-13-spec-h-onboarding-capability-ux.md`
- H-Sub-A implementation: [#1242](https://github.com/broomva/life/pull/1242)
- Spec doc PR: [#1243](https://github.com/broomva/life/pull/1243)
- Source motivation: `~/Documents/Zero_xyz_Architecture_Research_20260513/research_report_20260513_zero_xyz_architecture.md`
