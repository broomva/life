# Anima

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/tests-passing-green.svg)](#)
[![docs](https://img.shields.io/badge/docs-broomva.tech-purple.svg)](https://docs.broomva.tech/docs/life/anima)

**Agent identity, beliefs, and self-model for the Life Agent OS** -- the crate that answers *who the agent is*, while every other crate answers what the agent does.

Anima (Latin: soul, inner self) provides the foundational identity primitives: an immutable soul, a cryptographic identity with dual keypairs, mutable beliefs constrained by policy, and a composite self that ties them together.

## Architecture

```
anima/
  crates/
    anima-core/         Pure types: Soul, Identity, Belief, Self, Policy, Events
    anima-identity/     Cryptographic operations: seed, Ed25519, secp256k1, JWT, DID
    anima-lago/         Persistence bridge: genesis events, belief projection
```

### Core Types

| Type | Mutability | Purpose |
|------|-----------|---------|
| `AgentSoul` | Immutable | Origin, lineage, values, cryptographic root. Created once. |
| `AgentIdentity` | Lifecycle-mutable | Ed25519 (auth) + secp256k1 (economics) dual keypair + DID |
| `AgentBelief` | Mutable | Capabilities, trust scores, reputation, economic state |
| `AgentSelf` | Composite | Soul + Identity + Belief. The entry point for all consumers. |
| `PolicyManifest` | Immutable (in soul) | Safety constraints, capability ceiling, economic limits |
| `AgentIdentityDocument` | Derived | KYA (Know Your Agent) document: DID, capabilities, trust, attestations |

### Key Derivation

```
MasterSeed (32 bytes, random)
  +-- HKDF-SHA256(seed, "anima/ed25519/v1")   --> Ed25519 (Agent Auth Protocol)
  +-- HKDF-SHA256(seed, "anima/secp256k1/v1") --> secp256k1 (Haima / web3)
```

A single seed derives a dual keypair. The seed is encrypted at rest with ChaCha20-Poly1305 and zeroized on drop.

### DID Generation

Anima generates `did:key` identifiers from Ed25519 public keys:

```
did:key:z6Mk...  (multicodec Ed25519 prefix 0xed01 + public key, base58-btc encoded)
```

Functions: `generate_did_key()`, `resolve_did_key()`, `verify_did_key()`.

### PolicyManifest (Constitutional Law)

The `PolicyManifest` is embedded in the `AgentSoul` and acts as an immutable ceiling on agent behavior:

- **Safety constraints**: Hard limits on what the agent may do
- **Capability ceiling**: Maximum capabilities that beliefs can never exceed
- **Economic limits**: Spending caps, revenue thresholds

Beliefs are always constrained by the soul's policy -- the agent can learn and adapt, but never violate its constitutional law.

## Persistence Model

| Layer | Storage | Pattern |
|-------|---------|---------|
| Soul | Lago genesis event | First event, never overwritten |
| Identity | Event-sourced lifecycle | Transitions via `anima.*` events |
| Belief | Pure projection | Deterministic fold over event stream |
| Self | Reconstructed | Journal replay on session start |

### Event Namespace

All events use `EventKind::Custom` with prefix `"anima."`:

- `anima.soul_genesis` -- first event in an agent's journal
- `anima.identity_created` -- keypair created
- `anima.capability_granted` / `capability_revoked`
- `anima.trust_updated` -- peer trust score change
- `anima.identity_attested` -- attestation received (KYA)

## Integration Points

| System | How Anima Integrates |
|--------|---------------------|
| **Arcan** | Reconstructs `AgentSelf` from Lago on session start |
| **Lago** | Soul = genesis event; Belief = projection fold |
| **Autonomic** | Beliefs feed into homeostasis regulation |
| **Haima** | secp256k1 identity unifies with wallet |
| **Spaces** | Ed25519 key signs messages, presence includes identity |

## Build and Test

```bash
# Full verification
cargo fmt && cargo clippy --workspace && cargo test --workspace

# Run all tests (111)
cargo test --workspace

# Type check
cargo check --workspace

# Lint
cargo clippy --workspace
```

## Dependency Order

```
aios-protocol (canonical contract)
    |
anima-core (types + traits, depends on aios-protocol + haima-core)
    |                \
anima-identity        anima-lago (+ lago-core, lago-journal)
  (ed25519-dalek,
   k256, hkdf,
   chacha20poly1305)
```

## Documentation

Full documentation: [docs.broomva.tech/docs/life/anima](https://docs.broomva.tech/docs/life/anima)

## License

[MIT](LICENSE)
