# Anima — The Self Layer for the Life Agent OS

> *Anima* (Latin: soul, inner self) — the crate that answers **who the agent is**,
> while every other crate answers what the agent does.

**Spec ground truth**: `docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md` (Spec D — production custody design; sibling of Spec C₃ §5 lifegw KMS). The current `AnimaKeystore` + `Ed25519Identity` is the dev/in-process backend; Spec D defines the trait abstraction and the six production backends (Vault, WebCrypto+passkey, TPM, soma, hardware wallet, remote anima).

## Architecture

```
anima/
├── crates/
│   ├── anima-core/         # Pure types: Soul, Identity, Belief, Self, Policy, Events, IdentityDocument
│   ├── anima-identity/     # Cryptographic operations: seed, Ed25519, secp256k1, JWT, DID
│   └── anima-lago/         # Persistence bridge: genesis events, belief projection
```

### Core Types

| Type | Mutability | Purpose |
|------|-----------|---------|
| `AgentSoul` | **Immutable** | Origin, lineage, values, cryptographic root. Created once. |
| `AgentIdentity` | Lifecycle-mutable | Ed25519 (auth) + secp256k1 (economics) dual keypair + DID |
| `AgentBelief` | **Mutable** | Capabilities, trust scores, reputation, economic state |
| `AgentSelf` | Composite | Soul + Identity + Belief. The entry point for all crates. |
| `PolicyManifest` | **Immutable** (in soul) | Safety constraints, capability ceiling, economic limits |
| `AgentIdentityDocument` | Derived | KYA (Know Your Agent) document: DID, capabilities, trust, attestations |
| `AgentType` | Value | Autonomous, Delegated, or Hosted |
| `TrustTier` | Value | Unverified, Provisional, Trusted, or Certified |

### Key Derivation

```
MasterSeed (32 bytes, random)
  ├── HKDF-SHA256(seed, "anima/ed25519/v1")   → Ed25519 (Agent Auth Protocol)
  └── HKDF-SHA256(seed, "anima/secp256k1/v1") → secp256k1 (Haima/web3)
```

Single seed → dual keypair. Encrypted at rest with ChaCha20-Poly1305.

### Event Namespace

All events use `EventKind::Custom` with prefix `"anima."`:
- `anima.soul_genesis` — first event in an agent's journal
- `anima.identity_created` — keypair created
- `anima.capability_granted` / `capability_revoked`
- `anima.trust_updated` — peer trust score change
- `anima.economic_belief_updated` — from Haima/Autonomic
- `anima.belief_snapshot` — periodic checkpoint
- `anima.policy_violation_detected` — blocked action
- `anima.identity_attested` — attestation received (KYA)
- `anima.identity_verified` — identity verified by external party (KYA)

### Persistence Model

- Soul → Lago genesis event (first event, never overwritten)
- Belief → Pure projection (fold over event stream), like Haima's `FinancialState`
- Identity → Event-sourced lifecycle transitions
- Self → Reconstructed from journal replay

## Dependencies

```
anima-core → aios-protocol, haima-core, bs58
anima-identity → anima-core, haima-wallet, ed25519-dalek, k256, hkdf, chacha20poly1305, bs58
anima-lago → anima-core, lago-core, lago-journal
```

## Conventions

- **Edition**: 2024 (Rust 1.85)
- **No unsafe**: `#[forbid(unsafe_code)]`
- **Errors**: `thiserror` (not `anyhow`)
- **Testing**: Every module has unit tests
- **Soul immutability**: No `&mut self` methods on `AgentSoul`
- **Belief constrained by Soul**: `PolicyManifest` is the hard ceiling

## Commands

```bash
cargo check --workspace     # Type check
cargo test --workspace      # Run all 111 tests
cargo clippy --workspace    # Lint
cargo fmt --all             # Format
```

## KYA (Know Your Agent)

KYA is the agent-era equivalent of KYC. It provides:

### DID Generation (`anima-identity/src/did.rs`)
- `generate_did_key(public_key)` — Creates `did:key:z6Mk...` from Ed25519 public key
- `resolve_did_key(did)` — Extracts public key from a `did:key` DID
- `verify_did_key(did, public_key)` — Verifies DID matches a public key
- Format: multicodec Ed25519 prefix (0xed01) + public key, base58-btc encoded

### Identity Document (`anima-core/src/identity_document.rs`)
- `AgentIdentityDocument` — Complete KYA document (DID, capabilities, trust, attestations)
- `AgentType` — Autonomous, Delegated, or Hosted
- `TrustTier` — Unverified (<0.4), Provisional (0.4-0.7), Trusted (0.7-0.9), Certified (>=0.9)
- `Attestation` — Verifiable claims from issuers with expiry
- `IdentityDocumentBuilder` — Builder pattern for document construction

### AgentSelf Integration
- `AgentSelf::did()` — Access the agent's DID
- `AgentSelf::identity_document(agent_type, trust_score)` — Generate a KYA document

### Lago Events
- `anima.identity_attested` — Attestation received
- `anima.identity_verified` — Identity verified by external party

## Integration Points

| Crate | How Anima Integrates |
|-------|---------------------|
| **Arcan** | Reconstructs `AgentSelf` from Lago on session start (D-Sub-A: via `Arc<dyn AnimaCustody>`) |
| **Lago** | Soul = genesis event; Belief = projection fold |
| **Autonomic** | Beliefs feed into homeostasis regulation |
| **Haima** | secp256k1 wallet half via `CustodyWalletAdapter` (haima-x402, feature `custody-adapter`) |
| **Spaces** | Ed25519 key signs messages, presence includes identity (signed-presence not yet wired) |
| **Vigil** | OTel spans carry `agent.id` + `agent.soul_hash` |
| **broomva.tech** | Agent Auth Protocol via ES256 JWT (Spec D L4-D6 — was EdDSA) |
| **lago-auth** | `agent_jwt::detect_alg` dispatches EdDSA / ES256 verification |

## Spec D — Production Custody (D-Sub-A shipped)

`docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md` defines the
trait abstraction + 6 production backends. D-Sub-A (this PR) ships:

- `AnimaCustody` trait (`anima-identity/src/custody.rs`) + 6 backend
  variants in `BackendKind`. Only `InProcessAnima` is implemented.
- `EcdsaP256Identity` (`anima-identity/src/p256.rs`) — ES256 / P-256 via
  the `p256` crate. Mirrors `Ed25519Identity` API surface for mechanical
  swap.
- `did:key` extends to multicodec `0x1200` (P-256 → `did:key:zDn…`).
  Legacy `0xed01` (Ed25519 → `did:key:z6Mk…`) preserved for verifying
  historical events.
- New events: `anima.identity_rotated`, `anima.custody_migrated`,
  `anima.identity_revoked`. Plus `BackendKind` enum on `anima-core::event`.
- `AgentIdentityDocument.rotation_chain` (`Vec<DidRotation>`) +
  `published_at_seq: u64`. Both `#[serde(default)]` for backwards compat.

### D-Sub-A coordination items (cross-repo / cross-substrate)

- **broomva.tech AAP verifier** (separate repo): swap `EdDSA` →
  `ES256` (or accept both during the transition window). Track via Linear
  ticket: `BRO-XXX: broomva.tech AAP P-256 verifier swap` (placeholder;
  user files when MCP re-auths). **Non-blocking** because there are no
  production users yet — the cutover is fresh-deploy.
- **Spaces presence signing**: spec mentions "spaces SDK signs presence
  with `Ed25519Identity`". The `crates/spaces/life-spaces` crate today
  uses SpacetimeDB tables for presence-state, not cryptographic
  presence-beacons. When signed presence ships, route through
  `AnimaCustody::sign_digest`. Filed as a follow-up.
- **lifegw / broomva.tech ES256 verification cohesion**: lifegw's
  Tier-2 KMS (Spec C₃ §5) and anima's auth (Spec D L4-D6) are both ES256
  / P-256 now. Verifiers can share JWKS publish/cache plumbing; no
  immediate dependency, but worth tracking for a future M8 SDK pass.

### Backend phasing reminder

| Sub-phase | Backend | Status |
|---|---|---|
| D-Sub-A | `InProcessAnima` | shipped 2026-04-29 (PR #1070) |
| D-Sub-B | `VaultTransitAnima` | shipped 2026-05-01 (PR #1073) |
| D-Sub-C | `WebCryptoAnima` + `RemoteAnima` | M9-blocking (~7 days) |
| D-Sub-D | `TpmAnima` | shipped this PR; PKCS#11 auth half + wallet-half delegation |
| D-Sub-E | `SomaCustody` + rotation/revocation flow | planned (~5 days) |
| D-Sub-F | `HardwareWalletAnima` | optional (~2 days) |

### D-Sub-B (`VaultTransitAnima`) handoff state

`VaultTransitAnima` ships under feature flag `kms-vault` (default off
to keep the slim build slim). Production deployments (broomva.tech +
Sentinel + Life-Module tenants) enable the feature; the `kms-vault`
build pulls reqwest + tokio for the renewal task.

- Per-user namespace pattern: `transit/keys/anima-{user_id}-{auth,wallet}-v{n}`.
- Auth half: P-256 (ECDSA) — Vault transit `ecdsa-p256`. Signs JWS
  via `transit/sign/<auth_key>` with `marshaling_algorithm: "jws"`.
- Wallet half: secp256k1 — Vault transit `ecdsa-secp256k1`. Signs
  prehash via `transit/sign/<wallet_key>` with `prehashed: true`.
  EIP-1559 RLP digest computed via `crate::rlp` (shared with
  `InProcessAnima`).
- `sign_evm_tx` produces broadcast-ready 65-byte `r||s||v`
  signatures. Recovery byte computed via ecrecover loop (Vault doesn't
  return `v`). Two scalar multiplications per tx — negligible.
- Rotation: `transit/keys/<auth_key>/rotate` bumps version. Wallet
  half preserved per L4-D7. Rotation proof JWS signed by the
  PRE-rotation key version using Vault's `key_version:` parameter.
- mTLS: parameter accepted for forward compat but workspace's reqwest
  pin doesn't enable a TLS feature → operators run a localhost mTLS
  sidecar (envoy/consul-template). Same caveat as lifegw's
  `VaultTransit`. Documented inline.

### D-Sub-B follow-ups

- **Vault secp256k1 native support** — Vault v1.15 does NOT support
  secp256k1 transit keys natively. Live `vault server -dev`
  integration test (`live_vault_dev_server` in `integration_vault.rs`)
  is currently `#[ignore]`-gated and exercises only the auth half.
  Full USDC-transfer + Base-fork end-to-end test is achievable when
  Vault-secp256k1 patches land or a secp256k1-capable HSM sidecar is
  introduced. Track via Linear (pending MCP re-auth).
- **Generic EIP-712 encoder** — D-Sub-B retains the D-Sub-A
  limitation: only EIP-3009 `TransferWithAuthorization` is supported
  through `sign_eip712`. Generic encoder deferred to a follow-up
  sub-phase (likely D-Sub-E when SomaCustody adds typed-data signing
  of arbitrary payloads).
- **mTLS feature plumbing** — when the workspace's reqwest pin gains
  an optional TLS feature (likely a Sub-phase F refinement after
  M7-E ships), revisit `with_explicit_keys`'s mTLS handling so
  populated configs aren't silently ignored.

### D-Sub-D (`TpmAnima`) handoff state

`TpmAnima` ships under feature flag `kms-tpm` (default off; mirrors
the `kms-vault` gating). Desktop deployments (mission-control on
Linux/macOS) enable the feature. Default builds of anima-identity do
NOT pull `cryptoki` so the slim binary stays slim.

- **Auth half: P-256 in TPM.** Bootstrap finds the keypair by
  `CKA_LABEL` + validates `CKA_EC_PARAMS` matches the prime256v1
  OID (DER `06 08 2A 86 48 CE 3D 03 01 07`). `sign_jws` /
  `sign_digest` go through PKCS#11 `C_Sign` with mechanism
  `CKM_ECDSA` over a SHA-256 prehash; the TPM never reveals the
  scalar. Pubkey is read once at construction via
  `get_attributes(EC_POINT)` and cached.
- **Wallet half: delegated.** Per the SPEC-D-DEVIATION block at
  the top of `tpm.rs`, the wallet keypair lives OUTSIDE the TPM.
  Two reasons: (1) TPM 2.0 secp256k1 support is OPTIONAL and rare;
  (2) wallet-key compromise has unrecoverable blast radius
  (drains funds), so it deserves stricter custody. Constructor
  takes `Option<Arc<dyn AnimaCustody>>` for the delegate.
  - `wallet_address()` forwards to delegate, or returns `None`.
  - `sign_evm_tx` / `sign_eip712` forward to delegate, or return
    `Crypto("tpm: no wallet_delegate configured...")` error.
  - Canonical mission-control deployment: TPM-auth +
    `HardwareWalletAnima` (Ledger) for wallet half (D-Sub-F).
  - Auth-only deployment: TPM-auth + `wallet_delegate=None`
    (agent can authenticate but cannot move funds).
- **Rotation: operator-driven.** `rotate()` generates a fresh
  P-256 keypair on the TPM via `C_GenerateKeyPair` with mechanism
  `CKM_EC_KEY_PAIR_GEN`. New label is `{auth_label}-rot-{ulid}`
  to avoid collision; old key remains on TPM under original
  label until operator wipes it via `pkcs11-tool --delete-object`.
  Rotation proof JWS is signed by the OLD key over the NEW DID
  per Spec D L4-D10. The simpler atomic-rotate semantics live in
  `VaultTransitAnima` — production operators needing automated
  rotation should run Vault, not TPM.
- **`with_explicit_session` constructor** for tests with a
  pre-opened session + resolved key handles. Lets the integration
  test suite exercise composition rules without requiring a live
  PKCS#11 fixture in CI.
- **AnimaError::Crypto on bootstrap failures.** Constructor errors
  bubble cleanly — no fallback to `InProcessAnima` would silently
  downgrade the security guarantee. Mission-control surfaces them
  to the operator and halts.

### D-Sub-D follow-ups

- **Live softhsm fixture in CI.** `tests/integration_tpm.rs` ships
  with two `#[ignore]`-gated live tests (`live_tpm_softhsm_smoke`
  + `live_tpm_with_wallet_delegate`) that require a real PKCS#11
  module path + provisioned key. The fixture-setup is documented
  in the file-level docstring; CI does not run them by default to
  avoid binding to softhsm/TPM availability. Track as a follow-up
  to wire a CI runner with softhsm pre-installed.
- **Live rotation against softhsm.** Rotation flow is implemented
  but not exercised by the live tests yet (requires careful
  cleanup of generated PKCS#11 objects across test runs). Filed
  as a follow-up — most operators use the static-label workflow
  (operator generates a new key and restarts mission-control)
  rather than in-band `rotate()`.
- **`HardwareWalletAnima` composition test.** D-Sub-F will ship
  the secp256k1 hardware wallet backend and add a composition
  test (`TpmAnima` auth + `HardwareWalletAnima` wallet) that
  verifies the production mission-control shape end-to-end. The
  D-Sub-D integration tests use `InProcessAnima` as a delegate
  proxy in the meantime.
- **PKCS#11 `Send + Sync` audit.** `cryptoki::session::Session`
  is not `Send` because PKCS#11 sessions are stateful per-thread.
  We wrap the `TpmSession` in `Mutex` and add `unsafe impl Send
  + Sync` (justified inline). If we move to per-call PKCS#11
  sessions (open-sign-close per signature) the unsafe impls
  would become unnecessary; keeping them for the long-lived
  session pattern.
