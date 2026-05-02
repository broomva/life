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
| D-Sub-A | `InProcessAnima` | shipped this PR |
| D-Sub-B | `VaultTransitAnima` | planned (~5 days) |
| D-Sub-C | `WebCryptoAnima` + `RemoteAnima` | M9-blocking (~7 days) |
| D-Sub-D | `TpmAnima` | planned (~3 days) |
| D-Sub-E | `SomaCustody` + rotation/revocation flow | planned (~5 days) |
| D-Sub-F | `HardwareWalletAnima` | optional (~2 days) |
