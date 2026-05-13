# Spec F — Life Onboarding & Capability UX

**Date**: 2026-05-13
**Status**: Draft (Phase F-Sub-A in progress via [#1242](https://github.com/broomva/life/pull/1242); F-Sub-B…O queued)
**Sibling of**: Spec D §"Trait shape" (custody backends) and Spec C₃ §6.5 (lifegw close codes)
**Owner**: developer-facing surface (`crates/cli/`, `crates/anima/`, `crates/haima/`, `crates/lago/`, new `crates/capability-registry/`)
**Linear**: umbrella issue (Urgent, 0 points) with 15 sub-phase children F-Sub-A…O (priorities + estimates inline); filed against the **Life** project on the broomva team after user approval of this spec.

## Problem

A user landing on the Life Agent OS today goes through three disconnected onboarding hops:

1. `life init` — writes a project-local `.life/config.toml` and `.life/control/policy.yaml`. Stops there.
2. `life setup` — interactive wizard for an LLM provider; writes `~/.life/config.toml` (global), stores the API key in keychain.
3. `arcan chat` — actually starts the agent. By this point the user has typed three commands and **does not yet have an agent identity, a wallet, or a way to discover external capabilities**.

Compared to the bar set by `zero init` — one command, you have a usable wallet, you're ready to call paid APIs — Life today is friction-heavy at first run. The substrate is dramatically deeper (Anima Spec D ships six production custody backends, Haima ships x402 client + EIP-3009 signing, Lago is the event-sourced journal) but the **first-run UX does not exercise any of it**.

The friction is the gap between the substrate primitives and the developer-facing surface. Identity creation is async-`Arc<dyn AnimaCustody>` plumbing in `crates/anima/anima-identity/src/in_process.rs:82`. Wallet derivation is a `CustodyWalletAdapter` behind a feature flag in `haima-x402`. Capability discovery is a `Bazaar` HTTP client in `haima-x402/src/bazaar.rs`. Each works in isolation; **nothing composes them into a single `life init` story**.

The deep-research analysis of Zero (zero.xyz) and Coinbase Agentic.market that produced this spec [^1] surfaced a second-order observation: the substrate side of Life is *ahead* of every named competitor — Anima's six-backend custody is genuinely deeper than Zero's flat private key. The first-run UX side is *behind* — Zero compresses to one command what Life today spreads across three. Spec F closes that gap and then extends past it.

This is Spec F: the developer-facing surface that turns Life's substrate advantages into a one-command onboarding story and a Life-native capability discovery layer.

[^1]: `~/Documents/Zero_xyz_Architecture_Research_20260513/research_report_20260513_zero_xyz_architecture.md` — 10,099-word architectural deep-dive across 27 cited sources, completed 2026-05-13.

## Solution

Four phases, each independently shippable, each compounding with the previous.

**Phase F.A — Bootstrap** extends `life init` so a single command produces a working agent identity (DID + wallet), a local event journal, a status query path, and a one-time welcome credit. The Zero-`zero init` UX, built on AnimaCustody so production deploys upgrade seamlessly.

**Phase F.B — Custody flag** exposes the five non-InProcess Spec D backends as `life init --custody=vault|tpm|webcrypto|hardware|soma`, adds at-rest seed encryption via the keychain credential cascade, and auto-detects the runtime context (Claude Code / Cursor / VSCode / chatOS browser / mission-control desktop) to pick the right default backend per environment.

**Phase F.C — Capability substrate** introduces the missing layer that both Zero and Agentic.market built independently: a Life-native `capability-registry` crate emitting Anima-signed `capability.advertised` events, a `ReviewLedger` port emitting `capability.reviewed` events paired to `finance.payment_settled`, a `PaidCapability` trait in `aios-protocol` that unifies Praxis tools and Haima merchants under one call shape, and an opt-in Spaces gossip topic so discovery can be distributed rather than centralised. This is the architectural piece Life is currently missing.

**Phase F.D — Operator UX** is the polish layer: `life upgrade check`, `life identity attest`, `life policy show`, `life trust show`, agent skill installation (the pattern that lets Claude Code / Cursor / Codex pick up `life`'s capability surface automatically), and multi-agent wallet hierarchies via Anima lineage.

The four phases are sequenced so each one closes a specific class of friction and the next one builds on data already produced.

## Locked Decisions

### L6-F1 — `life init` is the single entry point, not a wrapper around multiple commands
Adding a `life bootstrap`, `life identity new`, `life wallet generate` would split the conceptual model. Zero's success comes from one command doing everything; Life mirrors that. New behaviour goes into `init`. Custody backend choice, journal mode, welcome credit are all `init` flags. Re-running `init` is idempotent (Phase F-Sub-A shipped this for identity).

### L6-F2 — `.life/identity/soul.json` is the public agent descriptor; committable
The DID, wallet address, soul hash, and full `AgentSoul` JSON live in `.life/identity/soul.json` with the project's git history. Two collaborators on the same project see the same agent identity — this is by design. Reproducible builds, deterministic test fixtures, and shared CI all depend on the soul being committable. The seed file (`seed.local.bin`) is always gitignored; production deployments don't write it at all (custody backend points elsewhere).

### L6-F3 — InProcess is the default custody backend, but never the only build target
Every backend ships behind a Cargo feature flag (`kms-vault`, `kms-tpm`, `kms-soma`, `kms-remote`, `kms-webcrypto`, `hw-wallet`) per Spec D. `life init` defaults to InProcess (no extra deps), and `life init --custody=vault` toggles the corresponding feature on the binary. The slim `life-cli` build only pulls InProcess; opt-in builds add the rest. This keeps install-size proportional to deployment shape.

### L6-F4 — Capability discovery is event-sourced via Lago, not state-stored
The `capability-registry` crate is a Lago projection over `capability.advertised` events. There is no separate "registry table." A query is a fold over the projection at a given timestamp, which means: (a) rankings are replayable, (b) BM25/graph indexing piggybacks on existing Lago infrastructure (`lago-knowledge`), (c) Spaces-distributed discovery is a natural extension because Lago already has gossip via the `arcan-spaces` bridge.

### L6-F5 — Reviews are deferred-write events paired to payment events
A capability review is an `capability.reviewed { payment_id, accuracy, value, reliability, content }` event. The `payment_id` references a prior `finance.payment_settled` event by ULID, so reviews and payments compose without coupling. Unreviewed payments surface in a `unreviewed_runs` projection — same UX as `zero runs --unreviewed`. Numeric ratings feed search ranking; content text feeds human-and-agent decision-making on the capability's public page.

### L6-F6 — `PaidCapability` is a sibling of `Tool`, not a wrapper
Both live in `aios-protocol`. Praxis tools implement `Tool` and are free; Haima-fronted endpoints implement `PaidCapability` and produce payment events. From the agent's call site, a `PaidCapability` invocation is *visually identical* to a `Tool` invocation — the payment plumbing is below the trait line. This is the AWS Bedrock AgentCore pattern [^2] but one layer lower (kernel contract, not vendor SDK).

[^2]: AWS Bedrock AgentCore Payments — agent code does not distinguish free local tool calls from paid remote API calls; payment authentication is transparent to the agent author. https://aws.amazon.com/blogs/machine-learning/agents-that-transact-introducing-amazon-bedrock-agentcore-payments-built-with-coinbase-and-stripe/

### L6-F7 — Welcome credit is a Haima merchant, not a magic top-up
Phase F-Sub-D's welcome credit is implemented as a Haima merchant the user pays *to* (zero USDC, with the merchant settling a promotional credit back). This keeps the financial state projection consistent (every credit has an `EventKind::Custom("finance.revenue_received")`), avoids a special-case code path, and makes the welcome credit auditable in `lago replay --tree` exactly like every other payment.

### L6-F8 — Skill installation is opt-in, never automatic
Phase F-Sub-N writes a `life` skill to `~/.claude/skills/life/SKILL.md` (and equivalents for Cursor/Codex/Windsurf) **only on explicit `life init --install-skills` or interactive `life setup --skills`**. Auto-installing skills into a user's agent runtime without consent is a footgun; Zero's `zero init` postinstall script does this and the skill catches some users by surprise. Life is explicit.

## Architecture

### Bootstrap data flow (Phase F.A)

```
$ life init
   │
   ├── crates/cli/life-cli/src/init.rs::run_in(&root)
   │     │
   │     ├─→ create_life_dir(.life/, .life/control/, .life/identity/)
   │     ├─→ write_config(.life/config.toml)     ← idempotent
   │     ├─→ write_policy(.life/control/policy.yaml)
   │     ├─→ bootstrap_anima_identity(life_dir)
   │     │     │
   │     │     ├─→ MasterSeed::generate()
   │     │     ├─→ InProcessAnima::from_seed_arc(seed)
   │     │     ├─→ SoulBuilder::new(name, mission, auth_pubkey).build()
   │     │     ├─→ write_seed(.life/identity/seed.local.bin, 0o600)
   │     │     └─→ write_soul_document(.life/identity/soul.json)
   │     ├─→ bootstrap_lago_journal(life_dir)          ← F-Sub-B
   │     │     │
   │     │     ├─→ open redb at .life/journal/events.redb
   │     │     ├─→ create_genesis_event(&soul)         ← anima-lago
   │     │     └─→ append AnimaEventKind::SoulGenesis
   │     ├─→ claim_welcome_credit(wallet_address)?     ← F-Sub-D (opt-in)
   │     │     │
   │     │     └─→ POST https://merchants.broomva.tech/welcome/claim
   │     │             body: { wallet, walletSignature }
   │     │             returns: { credit_micro_credits, tx_hash }
   │     └─→ update_gitignore(root)
   │
   └─→ exits with: did:key:zDn..., 0x..., $X welcome credit applied
```

### Capability substrate (Phase F.C)

```
                   ┌─────────────────────────────────────────────────┐
                   │           capability-registry (NEW)              │
                   │                                                 │
                   │   ┌────────────┐   ┌────────────┐  ┌──────────┐│
                   │   │ Projection │←──│ BM25 index │←─│ Graph    ││
                   │   │  (in-mem)  │   │            │  │  walker  ││
                   │   └─────▲──────┘   └────────────┘  └──────────┘│
                   └─────────┼───────────────────────────────────────┘
                             │
              ┌──────────────┼─────────────────────────┐
              │ Lago folds: capability.advertised,     │
              │             capability.reviewed,       │
              │             capability.deprecated      │
              └──────────────┬─────────────────────────┘
                             │
            ┌────────────────┴────────────────┐
            │                                 │
       ┌────▼──────┐               ┌──────────▼──────┐
       │ Praxis    │               │ Haima merchant  │
       │ Tool      │               │ (PaidCapability)│
       └───────────┘               └─────────────────┘
            │                                 │
            └──── both signed by AnimaCustody ┘
```

A capability advertisement is a Lago event:

```rust
// crates/aios/aios-protocol/src/capability.rs (NEW — F-Sub-J)
EventKind::Custom("capability.advertised") {
    advertiser_did: String,         // anima DID
    capability_id: String,          // ULID
    schema_version: u32,
    name: String,
    description: String,
    invocation: InvocationDescriptor,  // tool or paid
    pricing: Option<PricingDescriptor>,// None for free Praxis tools
    trust_tier: TrustTier,             // from anima
    expires_at: Option<DateTime<Utc>>,
    signature: String,                 // JWS over canonical form by advertiser
}
```

The registry projection folds the event stream and exposes `search(query) -> Vec<CapabilityListing>` via BM25 + graph walk. Distribution happens by either centralising at one Lago (single-host) or fanning out via Spaces gossip (F-Sub-K — agents publish advertisements to a shared `#capabilities` channel; other agents fold the channel into their local projection).

### Operator surface (Phase F.D)

```
$ life identity attest --claim "owner of github.com/broomva/life"
   →  emits anima.identity_attested event signed by user_did
   →  prints JWS for sharing

$ life policy show
   →  reads .life/identity/soul.json → soul.values (PolicyManifest)
   →  prints capability ceiling, safety constraints, economic limits

$ life trust show
   →  resolves DID via lago-auth::verify_jwt
   →  prints current TrustTier (Unverified/Provisional/Trusted/Certified)
   →  prints attestations from peers

$ life upgrade check
   →  reads ~/.life/update_check.json (last check timestamp)
   →  fetches https://broomva.tech/api/life/versions
   →  prints binary update status for life-cli, arcan, haimad, etc.

$ life init --install-skills
   →  writes ~/.claude/skills/life/SKILL.md (Claude Code)
   →  writes ~/.cursor/skills/life/skill.md (Cursor)
   →  writes ~/.codex/skills/life/SKILL.md (Codex)
   →  user's agent can now self-discover `life` commands
```

## Phasing

The 15 sub-phases below are each independently shippable. Each lists: **scope** (what changes), **deliverables** (concrete artifacts), **acceptance** (machine-checkable conditions), **dependencies** (which earlier sub-phases must land first), and **estimate** (Linear story points, 1≈half-day, 2≈one day, 3≈two days, 5≈one week, 8≈two weeks).

### F-Sub-A — Anima identity bootstrap on `life init` ✅ SHIPPED

**Scope**: Extend `life init` to generate an `InProcessAnima` identity, derive the secp256k1 EVM wallet, persist `.life/identity/soul.json` (committable) + `.life/identity/seed.local.bin` (0o600, gitignored), and print the DID + wallet address.

**Deliverables**:
- `crates/cli/life-cli/src/init.rs::bootstrap_anima_identity`
- `crates/cli/life-cli/Cargo.toml` — `anima-identity`, `anima-core` path deps
- 6 new tests on top of the existing 3 (12 total in life-cli)

**Acceptance**: ✅ all met in PR [#1242](https://github.com/broomva/life/pull/1242).

**Dependencies**: none — depends on shipped Spec D D-Sub-A.

**Estimate**: 2 points. **Status**: DONE in PR #1242.

### F-Sub-B — Lago journal init at `.life/journal/`

**Scope**: After identity bootstrap, open a project-local Lago journal at `.life/journal/events.redb` and append `AnimaEventKind::SoulGenesis` as the first event. Lago becomes the durable persistence layer for the agent's identity and all future events from this project.

**Deliverables**:
- `crates/cli/life-cli/src/init.rs::bootstrap_lago_journal`
- `Cargo.toml` deps: `lago-journal`, `anima-lago`
- Idempotency: if `.life/journal/events.redb` exists with a genesis event, reuse; never write a second genesis
- 4 new tests (genesis written, genesis hash matches soul hash, idempotent, branch-by-default)

**Acceptance**:
- `life init` exits with `.life/journal/events.redb` present and `lago log --data-dir .life/journal/ --limit 1` shows the `anima.soul_genesis` event with the same `soul_hash` as `.life/identity/soul.json`
- Re-running `life init` does not duplicate the genesis event
- All existing life-cli tests still pass

**Dependencies**: F-Sub-A.

**Estimate**: 2 points. **Priority**: High (unblocks F-Sub-C and F-Sub-D).

### F-Sub-C — `life status` shows identity, wallet, journal state

**Scope**: Currently `life status` requires `--agent` and queries a deployed agent. Add a new mode invoked without args that surfaces the *local* agent state: DID, wallet address, wallet balance (via Haima query if available), journal event count, last event, custody backend, and a "next step" hint.

**Deliverables**:
- `crates/cli/life-cli/src/status.rs` extended with a `LocalStatusArgs` variant
- Reads `.life/identity/soul.json` and `.life/journal/events.redb`
- Optional Haima HTTP query for wallet balance (gracefully degrades if `haimad` not running)
- Pretty-prints to stderr, JSON to stdout under `--format=json`
- 3 new tests (no .life/, .life/ with identity but no journal, full state)

**Acceptance**:
- `life status` in a fresh `life init`-ed dir prints DID + wallet + "journal: 1 event, anima.soul_genesis"
- `life status --format=json` emits a stable JSON shape suitable for scripts
- Errors out cleanly with a `life init` hint if `.life/` doesn't exist

**Dependencies**: F-Sub-A, F-Sub-B.

**Estimate**: 1 point. **Priority**: High (foundational debugging UX).

### F-Sub-D — `life init --claim-welcome` via a Haima merchant

**Scope**: Optional flag on `life init` (and standalone `life wallet claim-welcome`) that signs a welcome-claim message with the new wallet, POSTs to a Haima merchant at `https://merchants.broomva.tech/welcome/claim`, and receives back a USDC credit settlement. Recorded as `finance.revenue_received` event on the project Lago journal. Implements L6-F7.

**Deliverables**:
- Server side: a small Haima merchant in `apps/haima-merchants/welcome` (or under broomva.tech) that verifies the signature, checks the wallet isn't already claimed, and settles 1 USDC on Base
- Client side: `crates/cli/life-cli/src/wallet.rs::claim_welcome` (new module) — signs via `AnimaCustody::sign_digest`, POSTs, persists the receipt
- `life init --claim-welcome` flag default to `false`; interactive `life setup` prompts the user
- Tests: signature roundtrip, server-side double-claim rejection, event persistence

**Acceptance**:
- A fresh `life init --claim-welcome` produces a wallet with a non-zero USDC balance on Base testnet initially (mainnet behind a `--mainnet` flag once the merchant is funded)
- Re-running with the same wallet rejects (server-side dedupe)
- `lago log` shows the `finance.revenue_received` event with the welcome merchant as sender
- The flag defaults to false; no surprise calls to broomva.tech

**Dependencies**: F-Sub-A, F-Sub-B (for the event record). Server side can ship in parallel.

**Estimate**: 3 points. **Priority**: Medium. Server-side adds 2 more for the merchant + dedupe DB.

### F-Sub-E — `--custody=` flag on `life init`

**Scope**: Add `--custody=in_process|vault|tpm|webcrypto|hardware|soma` to `life init`. Each backend has its own config requirements (Vault URL, TPM PKCS#11 module path, etc.) gathered interactively or via additional flags. Builds gated by Cargo features per L6-F3.

**Deliverables**:
- `crates/cli/life-cli/src/init.rs` — dispatch on `--custody` to the right `AnimaCustody` constructor
- New `init` subcommand args (`--vault-url`, `--vault-token`, `--tpm-module`, etc.) — only flag visibility (no behaviour) when the corresponding Cargo feature is off
- Per-backend feature flags propagated: `life-cli`'s `[features]` declares `kms-vault`, `kms-tpm`, etc.; default is empty (InProcess only)
- Tests: feature-gated tests per backend (skipped when feature off); error path for "feature not enabled"

**Acceptance**:
- `life init --custody=vault --vault-url=... --vault-token=...` (when built with `--features kms-vault`) produces a soul.json with `custody.kind: "vault"`, no seed file on disk
- `life init --custody=vault` on default build errors with "kms-vault feature not enabled; rebuild with `cargo install life-cli --features kms-vault`"
- All Spec D backends supported by Phase F.B end-state

**Dependencies**: F-Sub-A. (Other backends are already shipped in Spec D D-Sub-B…F.)

**Estimate**: 5 points (2 for the flag plumbing, 1 per backend for interactive-config UX, gated by 5 backends × 0.5 ≈ 3 points).

### F-Sub-F — `EncryptedSeed` at rest + keychain passphrase

**Scope**: For InProcess deployments where `seed.local.bin` lives on disk, replace the raw 32-byte bytes with an `anima_identity::EncryptedSeed` (already exists, ChaCha20-Poly1305) keyed by a passphrase stored via `life-paths::credentials` (keychain → `.env` fallback). Phase A's `0o600` was the bootstrap; this is the harder protection.

**Deliverables**:
- `crates/cli/life-cli/src/init.rs::write_seed` → `write_encrypted_seed`
- New helper that mints a random per-install passphrase, stores it via `life-paths::credentials::store_credential("LIFE_IDENTITY_SEED_KEY", "life-identity-seed", &phrase)`, encrypts the seed
- Reload path reads the passphrase, decrypts the seed, constructs the InProcessAnima
- Migration: if `seed.local.bin` exists in the old raw format, transparently upgrade on next `life init` (or `life identity upgrade`)
- Tests: encrypt/decrypt roundtrip, wrong-passphrase fails, migration of raw → encrypted

**Acceptance**:
- A fresh `life init` produces `.life/identity/seed.local.bin` whose contents are 28+ bytes of ciphertext (not 32 bytes of seed)
- `cat seed.local.bin` reveals no plaintext seed material
- Deleting the keychain entry makes the seed unrecoverable (test verifies decryption fails)
- Existing raw-seed installs auto-migrate

**Dependencies**: F-Sub-A.

**Estimate**: 2 points.

### F-Sub-G — Runtime context auto-detection for default custody

**Scope**: When the user doesn't pass `--custody`, choose a sensible default based on the runtime: `WebCrypto` in a browser (chatOS), `Soma` if `/run/life/soma-admin.sock` exists, `Tpm` if a PKCS#11 module is configured via env, otherwise `InProcess`. Mirrors Zero's `CLAUDECODE` / `CURSOR_TRACE_ID` env detection but for substrate selection.

**Deliverables**:
- `crates/cli/life-cli/src/init.rs::detect_default_custody() -> BackendKind`
- Detection rules documented in a new `crates/cli/life-cli/src/init/context.rs`
- `life init --custody=auto` is the default; explicit `--custody=...` always wins
- Tests cover each detection branch with env-var manipulation

**Acceptance**:
- Running `life init` inside chatOS browser context (mocked via env) picks `WebCrypto`
- Running on a host with `/run/life/soma-admin.sock` picks `Soma`
- Running with no signals picks `InProcess`
- Detection is explainable: `life init --explain-custody` prints why each backend was/wasn't chosen

**Dependencies**: F-Sub-E (for the secret).

**Estimate**: 1 point.

### F-Sub-H — `capability-registry` crate

**Scope**: New `crates/capability-registry/` (sibling of `lago`). Maintains a Lago projection of `capability.advertised`, `capability.reviewed`, and `capability.deprecated` events. Exposes `pub fn search(query: &str, opts: SearchOpts) -> Vec<CapabilityListing>` over a BM25 index built from the projection. Reads from the local project Lago by default; optionally federates with Spaces channels via F-Sub-K.

**Deliverables**:
- `crates/capability-registry/Cargo.toml` (new)
- `crates/capability-registry/src/lib.rs` — `Registry`, `CapabilityListing`, `SearchOpts`
- `crates/capability-registry/src/projection.rs` — fold over Lago events
- `crates/capability-registry/src/search.rs` — BM25 over capability text + graph walk for related capabilities
- Cargo workspace member registered
- Tests: 8+ covering event fold, search ranking, trust-tier filtering, expiration, deprecation

**Acceptance**:
- A test that publishes 3 capabilities to a Lago journal and `Registry::search("translation")` returns them ranked by review-weighted score
- Trust-tier filter excludes Unverified advertisers when `min_tier = Provisional`
- Expired advertisements (`expires_at < now`) disappear from search results

**Dependencies**: F-Sub-B (Lago journal exists), F-Sub-J (`PaidCapability` trait — for the listing shape).

**Estimate**: 8 points (~2 weeks). Largest single sub-phase.

### F-Sub-I — `ReviewLedger` port + `capability.reviewed` events

**Scope**: New trait `ReviewLedger` in `aios-protocol::review`. Default impl writes `capability.reviewed { payment_id, accuracy, value, reliability, content }` events on Lago. Each event references a prior `finance.payment_settled` event by ULID. Adds a `unreviewed_payments()` projection so `life review list` surfaces unreviewed runs (mirroring `zero runs --unreviewed`).

**Deliverables**:
- `crates/aios/aios-protocol/src/review.rs` — `ReviewLedger`, `Review`, `UnreviewedPayment`
- `crates/haima/haima-lago/src/review.rs` (or new `haima-review`) — Lago-backed impl
- `crates/cli/life-cli/src/review.rs` — `life review submit` / `life review list` subcommands
- 6+ tests covering: submit, list-unreviewed, query-by-capability, idempotency (review same payment twice → updates instead of duplicating)

**Acceptance**:
- After a (mocked) `finance.payment_settled`, `life review list` shows the payment as unreviewed
- `life review submit <payment_id> --accuracy 5 --value 4 --reliability 5 --content "..."` writes a `capability.reviewed` event
- `lago replay --tree` shows the review event linked to the payment
- The same payment can't be reviewed twice without `--force` (updates the existing review)

**Dependencies**: F-Sub-B.

**Estimate**: 3 points.

### F-Sub-J — `PaidCapability` trait in `aios-protocol`

**Scope**: Define a typed Rust trait for paid HTTP capabilities alongside the existing `Tool` trait. Implement adapter from `haima-x402` so any Bazaar-discovered or advertiser-supplied capability is a `PaidCapability`. Bridge in Praxis so paid tools and free tools share a single call shape. Implements L6-F6.

**Deliverables**:
- `crates/aios/aios-protocol/src/capability.rs` (new) — `PaidCapability`, `InvocationDescriptor`, `PricingDescriptor`
- `crates/haima/haima-x402/src/paid_capability.rs` — adapter from bazaar listings
- `crates/praxis/praxis-core/src/paid_capability.rs` — wrapper letting Praxis surface `PaidCapability` alongside `Tool`
- Praxis MCP server exposes both Tools and PaidCapabilities transparently
- 5+ tests covering: bazaar listing → PaidCapability, call shape, payment plumbing transparency

**Acceptance**:
- A bazaar capability and a Praxis tool can both be invoked through `dyn PaidCapability` (paid path settles payment automatically; free path no-ops)
- Praxis MCP server lists paid capabilities to MCP clients
- An agent calling a `PaidCapability` does not have to know the price ahead of time — `PaymentPolicy` evaluates inline

**Dependencies**: none — independent of F-Sub-H (but F-Sub-H consumes this).

**Estimate**: 3 points.

### F-Sub-K — Spaces gossip channel for capabilities

**Scope**: Opt-in Spaces channel `#capabilities` where agents publish their `capability.advertised` events. Subscribers fold the channel into their local registry projection. Implements decentralized discovery per L6-F4.

**Deliverables**:
- New table in the Spaces module: `CapabilityAdvertisement` (signed Lago event payload)
- Spaces reducer: `publish_capability_advertisement(advertisement, signature)` — verifies signature against advertiser's DID
- `crates/capability-registry/src/spaces_bridge.rs` — subscribes to Spaces channel and folds messages into local registry
- `life publish --capability <id>` CLI command that emits the advertisement event AND publishes to subscribed Spaces channels
- 4+ tests: signature verification, subscription fold, conflict resolution (multiple advertisers for the same name)

**Acceptance**:
- Two test agents in two separate Lago journals, both subscribed to the same Spaces `#capabilities` channel, see each other's advertisements
- Signature verification rejects malformed or stale advertisements
- Local registry projection reflects channel state within 1 second of publish

**Dependencies**: F-Sub-H, F-Sub-J. Requires `spaces/` to be running.

**Estimate**: 5 points.

### F-Sub-L — `life upgrade check`

**Scope**: Mirror Zero's `~/.zero/update_check.json` pattern: a daily check against `https://broomva.tech/api/life/versions` for new versions of `life-cli`, `arcan`, `haimad`, etc. Non-blocking, runs on `life init` and `life setup` (and any command after >24h since last check). Prints a single-line "Update available: …" nudge.

**Deliverables**:
- `crates/cli/life-cli/src/upgrade.rs` (new) — `check_for_updates()` + `print_upgrade_hint()`
- `~/.life/update_check.json` cache with `last_check_at` + `latest_version` per binary
- Server side: `apps/broomva.tech/app/api/life/versions/route.ts` returns `{ life-cli: "0.4.0", arcan: "...", haimad: "..." }`
- 3 tests: cache hit, cache miss, server error degrades silently

**Acceptance**:
- Stale cache (>24h) triggers a fresh fetch
- Network error doesn't block any command — silent fallback to cached value
- Server-side endpoint has a deterministic JSON shape

**Dependencies**: none.

**Estimate**: 1 point.

### F-Sub-M — `life identity attest` + `life policy show` + `life trust show`

**Scope**: Three short CLI commands exposing the identity + trust + policy surface to users (and to other agents reading the soul as data).

- `life identity attest --claim "<text>"` — emits a JWS signed by the user's auth key over `{ claim, did, timestamp }`. Useful for proving ownership of external resources without rolling a custom signing flow.
- `life policy show` — reads `.life/identity/soul.json::soul.values` (PolicyManifest) and pretty-prints the capability ceiling, safety constraints, economic limits.
- `life trust show` — calls `lago-auth::verify_jwt` against the agent's own DID, prints the current trust tier and any attestations from peers.

**Deliverables**:
- `crates/cli/life-cli/src/identity.rs` (new) — three subcommands
- 4+ tests per subcommand

**Acceptance**:
- The JWS from `life identity attest` verifies against the soul's `auth_pubkey` using any third-party ES256 verifier
- `life policy show` matches the soul's `values` byte-for-byte
- `life trust show` reflects the current trust tier from Lago

**Dependencies**: F-Sub-A.

**Estimate**: 2 points.

### F-Sub-N — Agent skill installation (opt-in)

**Scope**: On explicit `life init --install-skills` or `life setup` prompt, write a `life` skill (a SKILL.md frontmatter file) to the relevant agent runtime directories so Claude Code, Cursor, Codex, and Windsurf can pick up the `life` command surface without manual configuration. Implements L6-F8.

**Deliverables**:
- `crates/cli/life-cli/skills/life.md` — the canonical skill content (commands, examples, when-to-use guidance)
- `crates/cli/life-cli/src/skill_install.rs` — writes the skill to the right path per runtime: `~/.claude/skills/life/SKILL.md`, `~/.cursor/skills/life/skill.md`, `~/.codex/skills/life/SKILL.md`, etc.
- Detection: skip runtimes that aren't installed (no auto-discovery surprise)
- 4 tests: install path correct per runtime, idempotent re-install, refuses to overwrite user-customized skill, --uninstall-skills cleans up

**Acceptance**:
- After `life init --install-skills` on a machine with Claude Code, `~/.claude/skills/life/SKILL.md` exists with the canonical content
- Re-running is a no-op unless `--force-install-skills`
- A user-modified skill file is preserved (warns instead of overwriting)
- `life init --uninstall-skills` removes all skill files

**Dependencies**: none.

**Estimate**: 2 points.

### F-Sub-O — Multi-agent wallet hierarchy via Anima lineage

**Scope**: When an agent spawns a child agent (already supported via `SoulBuilder::lineage_entry`), the child receives its own DID + wallet AND a parent-side spend cap. Child payments emit `finance.payment_settled` events parented to the parent via `lineage_entry.parent_payment_id`. Allows autonomous parent agents to delegate budget to children with audit trail.

**Deliverables**:
- `crates/anima/anima-core/src/soul.rs` — extend `SoulBuilder` with `parent_wallet_authority(parent_did, spend_cap)`
- `crates/haima/haima-core/src/policy.rs` — extend `PaymentPolicy` to enforce parent-set caps on children
- New event: `finance.child_spend_authorized { parent_did, child_did, cap_micro_credits, expires_at }`
- `life agent spawn <name> --spend-cap=10000` CLI subcommand that creates a child agent with the cap
- 6+ tests: spawn, cap enforcement, audit chain in `lago replay --tree`

**Acceptance**:
- `life agent spawn child --spend-cap=10000μc` creates `.life/agents/child/identity/soul.json` with a parent reference
- Child's payment within cap succeeds; over-cap is rejected with "exceeded parent-set spend cap"
- `lago replay --tree` shows the parent → child lineage clearly

**Dependencies**: F-Sub-A, F-Sub-E. Touches Spec D's lineage event shape — re-use rather than fork.

**Estimate**: 5 points.

## Sequencing Against Life Roadmap

The four phases group the 15 sub-phases as follows:

| Phase | Sub-phases | Theme | Target | Blocks | Blocked by |
|---|---|---|---|---|---|
| **F.A** Bootstrap | F-Sub-A (✅), B, C, D | One-command-runnable agent | Q2 2026 (in progress) | F.B, F.C, F.D | Spec D D-Sub-A (✅) |
| **F.B** Custody | F-Sub-E, F, G | Production-grade key handling | Q3 2026 | F.D (skill install reads context) | F.A |
| **F.C** Capability | F-Sub-H, I, J, K | Discovery + reputation substrate | Q3-Q4 2026 | downstream apps that need cross-substrate discovery | F.A, F.B |
| **F.D** Operator UX | F-Sub-L, M, N, O | CLI polish + multi-agent | Continuous from Q3 2026 | — | F.B (for G context detection), F.A (for identity surface) |

Concretely, F-Sub-A ships in PR #1242. F-Sub-B and F-Sub-C are the next logical follow-ups (small, well-bounded). F-Sub-D, F-Sub-E, F-Sub-F can land in parallel — different files, no shared state. F-Sub-G depends on F-Sub-E (the secret-store path).

Phase F.C is the largest single block of work (F-Sub-H alone is 8 points). It can begin as soon as F-Sub-B lands (Lago journal in place), and F-Sub-J can run in parallel with F-Sub-H since they're separate trait surfaces. F-Sub-I (review ledger) can run independently. F-Sub-K (Spaces gossip) requires F-Sub-H + F-Sub-J.

Phase F.D is hygiene polish; safe to ship continuously alongside F.B and F.C. F-Sub-L is the smallest (1 point), F-Sub-N is the trickiest only because it touches multiple agent runtime layouts.

Total estimate: **45 points** across 15 sub-phases. At the current Life velocity (Spec D shipped 6 sub-phases × ~3-5 points each in 5 days; Spec C M5 shipped 5 sub-phases over ~2 weeks), Spec F is roughly **5-7 weeks of focused engineering** with two engineers, **8-10 weeks** with one.

## Critical Path

The path that matters most for product impact:

```
F-Sub-A ✅ → F-Sub-B → F-Sub-C → F-Sub-J → F-Sub-H → F-Sub-I
        ↓
        F-Sub-E (for production deployments)
```

This unblocks:
1. A user can `life init` and have a working agent with identity + wallet + journal (F.A complete)
2. A user can `life init --custody=vault` and deploy to production safely (F-Sub-E)
3. The agent can discover both free Praxis tools and paid Haima capabilities through one trait (F-Sub-J)
4. There's a registry to search over (F-Sub-H)
5. Reviews accumulate to feed ranking (F-Sub-I)

F-Sub-D (welcome credit), F-Sub-K (Spaces gossip), F-Sub-N (skill installation), and F-Sub-O (lineage wallets) are all valuable but not on the critical path for "Life is usable end-to-end."

## Open Questions

1. **Welcome merchant operator**: Phase F-Sub-D's merchant runs *somewhere*. Broomva.tech is the natural host, but the merchant needs a dedicated wallet, a server-side double-claim DB, and gas budget. Worth a dedicated micro-spec before F-Sub-D ships.

2. **Spaces channel governance for F-Sub-K**: Who can create the `#capabilities` channel? Who can moderate? Who pays for the SpacetimeDB hosting? The federated discovery story requires a trust anchor that doesn't exist yet — maybe the broomva.tech-operated channel is the default, with self-hosted alternatives.

3. **`PaidCapability` price-discovery before invocation**: Phase F-Sub-J's trait surfaces price *at* invocation (via 402). For agents that need to budget across multiple capabilities, a pre-invocation `quote()` method would help. Worth adding as a v2 trait method, gated behind a feature flag.

4. **Skill content versioning for F-Sub-N**: When `life-cli` updates, the canonical skill file may need to change. Should re-installing detect a version mismatch? How does this interact with user-customized skills? Probably needs a `skill.toml` lockfile alongside.

5. **Trust tier escalation events**: F-Sub-M's `life trust show` reads the current tier, but how does a tier escalation actually happen? Via `anima.identity_attested` events from trusted issuers? Via a Haima payment threshold reached? This is a Spec G topic but worth flagging.

6. **Lineage budget enforcement vs Spec C lifegw rate limits**: F-Sub-O parent-set spend caps are an anima-side concept; lifegw also has rate limits. Are these the same primitive or composed? Probably composed — anima checks intent, lifegw enforces network-side — but worth a clear write-up.

## References

- `crates/cli/life-cli/src/init.rs` — current `life init` (extended by F-Sub-A in PR #1242)
- `crates/cli/life-cli/src/setup.rs` — interactive wizard (interacts with F-Sub-N skill install)
- `crates/anima/anima-identity/src/in_process.rs` — Spec D D-Sub-A backend used by F-Sub-A
- `crates/anima/anima-identity/src/{vault,tpm,webcrypto,hardware_wallet,soma,remote}.rs` — Spec D D-Sub-B…F backends consumed by F-Sub-E
- `crates/anima/anima-identity/src/seed.rs::EncryptedSeed` — used by F-Sub-F
- `crates/anima/anima-core/src/soul.rs::SoulBuilder` — used by F-Sub-A and extended by F-Sub-O
- `crates/anima/anima-lago/src/genesis.rs::create_genesis_event` — F-Sub-B genesis event source
- `crates/haima/haima-x402/src/bazaar.rs` — current `agentic.market` Bazaar client (parallel approach to F-Sub-H)
- `crates/haima/haima-wallet/src/backend.rs::WalletBackend` — used by F-Sub-D welcome flow
- `crates/lago/lago-knowledge/` — BM25 substrate reused by F-Sub-H
- `crates/spaces/life-spaces/` — networking substrate reused by F-Sub-K
- `docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md` — sibling spec providing custody backends
- `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md` — sibling spec for lifegw close codes
- `~/Documents/Zero_xyz_Architecture_Research_20260513/research_report_20260513_zero_xyz_architecture.md` — Zero architectural deep-dive that motivated this spec
- AWS Bedrock AgentCore Payments — https://aws.amazon.com/blogs/machine-learning/agents-that-transact-introducing-amazon-bedrock-agentcore-payments-built-with-coinbase-and-stripe/ — same trait-line-transparency pattern at vendor-SDK layer
- Coinbase Bazaar / Agentic.market — https://cryptobriefing.com/agentic-market-ai-agents-hub/ — parallel discovery substrate
- Zero CLI (`@zeroxyz/cli`) — https://www.zero.xyz/welcome — UX inspiration
