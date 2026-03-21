# Anima — The Self Layer for the Life Agent OS

> *Anima* (Latin: soul, inner self) — the crate that answers **who the agent is**,
> while every other crate answers what the agent does.

## Architecture

```
anima/
├── crates/
│   ├── anima-core/         # Pure types: Soul, Identity, Belief, Self, Policy, Events
│   ├── anima-identity/     # Cryptographic operations: seed, Ed25519, secp256k1, JWT, DID
│   └── anima-lago/         # Persistence bridge: genesis events, belief projection
```

### Core Types

| Type | Mutability | Purpose |
|------|-----------|---------|
| `AgentSoul` | **Immutable** | Origin, lineage, values, cryptographic root. Created once. |
| `AgentIdentity` | Lifecycle-mutable | Ed25519 (auth) + secp256k1 (economics) dual keypair |
| `AgentBelief` | **Mutable** | Capabilities, trust scores, reputation, economic state |
| `AgentSelf` | Composite | Soul + Identity + Belief. The entry point for all crates. |
| `PolicyManifest` | **Immutable** (in soul) | Safety constraints, capability ceiling, economic limits |

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

### Persistence Model

- Soul → Lago genesis event (first event, never overwritten)
- Belief → Pure projection (fold over event stream), like Haima's `FinancialState`
- Identity → Event-sourced lifecycle transitions
- Self → Reconstructed from journal replay

## Dependencies

```
anima-core → aios-protocol, haima-core
anima-identity → anima-core, haima-wallet, ed25519-dalek, k256, hkdf, chacha20poly1305
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
cargo test --workspace      # Run all 58 tests
cargo clippy --workspace    # Lint
cargo fmt --all             # Format
```

## Integration Points

| Crate | How Anima Integrates |
|-------|---------------------|
| **Arcan** | Reconstructs `AgentSelf` from Lago on session start |
| **Lago** | Soul = genesis event; Belief = projection fold |
| **Autonomic** | Beliefs feed into homeostasis regulation |
| **Haima** | secp256k1 identity unifies with wallet |
| **Spaces** | Ed25519 key signs messages, presence includes identity |
| **Vigil** | OTel spans carry `agent.id` + `agent.soul_hash` |
| **broomva.tech** | Agent Auth Protocol via Ed25519 JWT signing |
