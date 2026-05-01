# Spec D — Anima Production Custody

**Date**: 2026-04-29
**Status**: Draft (locked decisions inline; phasing approved)
**Sibling of**: Spec C₃ §5 (lifegw Tier-2 KMS) — this is the user-scoped analogue
**Owner**: identity layer (`crates/anima/`)
**Linear (pending MCP re-auth)**: umbrella ticket "Spec D — Anima Production Custody" with D-Sub-A through D-Sub-F as children.

## Problem

The platform side of the trust boundary is being closed by Spec C₃ §5 + M7 Sub-phase E: the Tier-2 capability-token signing key never sits in lifegw process memory, lives behind a `KmsSigner` trait, and has Vault/AWS/GCP backends. A pwned lifegw can mint tokens while it lives but can't exfiltrate the key.

The user side has the same threat model and a worse blast radius. Anima today (`crates/anima/anima-identity/src/keystore.rs::AnimaKeystore`) holds three secrets in process memory:

| Secret | Authority claim | Sign frequency | Today's home |
|---|---|---|---|
| Ed25519 private key | "agent X is alice's agent" via JWT; "alice asserted Z" via identity/belief events | high (every event) | `Ed25519Identity` in process memory |
| secp256k1 private key | "alice authorises this transfer" via EVM tx / x402 | low (per payment) | `Zeroizing<Vec<u8>>` in process memory |
| Master seed | derives both of the above | n/a (cold) | `MasterSeed` in process memory; `EncryptedSeed` (ChaCha20-Poly1305) at rest |

A compromise of an anima process loses **all three** simultaneously. Unlike Tier-2, you can't rotate a DID and expire the old one — the DID and wallet address are user-public artifacts that downstream verifiers cache forever. **Identity compromise is structurally durable.**

This is Spec D: the anima-side custody design that makes user-scoped identity production-grade.

## Solution

A custody trait abstraction (`AnimaCustody`) with multiple backends, mirroring lifegw's `KmsSigner` shape but wider in two ways:

1. **Multi-curve.** Auth (P-256 ECDSA) and wallet (secp256k1 ECDSA) are separate signing operations through the same trait.
2. **Browser-deployable.** Some backends (`WebCryptoAnima`) must work where Vault and TPM are not options — passkey-managed non-extractable keys.

Browser deployments use a **split-custody / Tier-User pattern**: the browser holds the auth keypair only; wallet operations route to a non-browser custody backend (Vault, TPM, hardware wallet, or a remote server-side anima). This is the user-scoped analogue of Spec C's Tier-1/Tier-2 separation.

## Locked Decisions

### L4-D5 — Split custody for browser deployments
Browser holds the auth keypair only (passkey-managed, P-256, non-extractable). Wallet operations route to one of: (a) the user's server-side anima (`RemoteAnima` backend), (b) a hardware wallet (`HardwareWalletAnima`), or (c) the soma admin signing oracle (`SomaCustody`). Holding secp256k1 in the JS heap is explicitly rejected — it defeats the non-extractable WebCrypto guarantee.

### L4-D6 — Anima auth keypair migrates from Ed25519 to ECDSA P-256
Reasons: passkey-native (browser non-extractable for free); curve unification with lifegw Tier-2 (already ES256/P-256); standard JWT alg end-to-end. Cost: anima's `Ed25519Identity` → `EcdsaP256Identity`; DID multicodec changes from `0xed01` to `0x1200` (`did:key:z6Mk…` → `did:key:zDn…`); broomva.tech Agent Auth Protocol verifier swaps from EdDSA to ES256. Migration is D-Sub-A's primary work.

### L4-D7 — Wallet keypair stays secp256k1
Haima/x402/Base assume secp256k1 EOA + EIP-712 + EIP-155 throughout. Re-architecting to P-256 would require ERC-4337 smart accounts on every chain (gas overhead, ecosystem lock-in to chains with EIP-7212), break x402's EOA assumption, and provide no security benefit since the secp256k1 key never sits in browser memory under L4-D5.

### L4-D8 — `AnimaCustody` is per-`AgentSelf`, not per-process
Each `AgentSelf` (whether a user or an agent) gets its own custody backend resolved at construction time. The user-vs-agent distinction is encoded in the DID delegation chain via `anima.identity_attested` events signed by the parent's custody — not in the trait shape. Agents inherit their parent's custody backend by default unless overridden in the agent's birth event.

### L4-D9 — Custody migration is a first-class event
`anima.custody_migrated { from_backend, to_backend, attestation }` is a self-issued event documenting that custody moved (e.g., user upgraded `InProcessAnima` → `TpmAnima`, or chatOS user moved their wallet from `RemoteAnima` to `HardwareWalletAnima`). Doesn't change the keys; documents the move so verifiers can audit lineage.

### L4-D10 — Rotation is documented in the journal, not implicit
`anima.identity_rotated { old_did, new_did, rotation_proof_jws }` where `rotation_proof_jws` is a signature by the *old* key over the *new* key. Verifiers seeing the old DID fetch the rotation event from Lago and re-resolve. Without this, rotation breaks every cached DID downstream.

## Architecture

### Trait shape

```rust
// crates/anima/anima-identity/src/custody.rs (new)

pub trait AnimaCustody: Send + Sync + 'static {
    fn user_did(&self) -> &str;                          // did:key:zDn…
    fn auth_pubkey(&self) -> [u8; 33];                   // P-256 SEC1 compressed
    fn wallet_address(&self) -> Option<&WalletAddress>;  // None if no wallet half resolved

    /// Sign a JWS over the supplied claims using the auth (P-256) key.
    /// Implementations build the header (alg=ES256, kid=DID).
    fn sign_jws(&self, claims: &serde_json::Value) -> AnimaResult<String>;

    /// Sign a raw 32-byte digest with the auth key (for non-JWT identity events).
    fn sign_digest(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]>;

    /// Sign an EVM transaction with the wallet (secp256k1) key.
    /// Implementations may delegate to a remote backend; this is the slow path.
    fn sign_evm_tx(&self, tx: &TxRequest) -> AnimaResult<EvmSignature>;

    /// Sign an EIP-712 typed-data payload (used by haima for x402 + USDC).
    fn sign_eip712(&self, domain: &Eip712Domain, types: &Value, message: &Value)
        -> AnimaResult<EvmSignature>;

    /// Publish a rotation event into the journal, returning the event payload.
    /// Each backend implements this differently — TPM/Vault rotate the underlying
    /// key reference; in-process backends generate a fresh seed.
    fn rotate(&self) -> AnimaResult<DidRotationEvent>;

    /// User-scoped JWKS-equivalent: emits the AgentIdentityDocument with the
    /// rotation chain so any verifier can resolve old DIDs.
    fn export_identity_document(&self) -> AnimaResult<AgentIdentityDocument>;
}
```

Every call site (Arcan session reconstruction, Haima x402 path, Spaces presence, broomva.tech AAP) holds an `Arc<dyn AnimaCustody>`. Backend resolution happens once at `AgentSelf` construction.

### Backend matrix

| Backend | Auth key (P-256) | Wallet key (secp256k1) | Browser | Self-host | Status |
|---|---|---|---|---|---|
| `InProcessAnima` | in process | in process (zeroizing) | n/a | yes | shipped (refactor target for D-Sub-A) |
| `WebCryptoAnima` | passkey non-extractable `CryptoKey` | **delegated** (no native curve) | ✅ | n/a | new (D-Sub-C) |
| `TpmAnima` | TPM via PKCS#11 | TPM via PKCS#11 (where supported) or hardware wallet escalation | ❌ | yes | new (D-Sub-D) |
| `VaultTransitAnima` | Vault Transit P-256 key per-user | Vault Transit secp256k1 key per-user | ❌ | yes | new (D-Sub-B) |
| `SomaCustody` | soma admin RPC `SignAuth` | soma admin RPC `SignWallet` | ❌ | yes | new (D-Sub-E) |
| `HardwareWalletAnima` | wraps another backend's auth half | Ledger/Trezor (secp256k1) | partial (WebHID/WebUSB) | n/a | new (D-Sub-F) |
| `RemoteAnima` | local passkey | proxy to user's server-side anima | ✅ paired with `WebCryptoAnima` | yes | new (D-Sub-C, paired) |

`RemoteAnima` is the deployment glue that makes `WebCryptoAnima` work for users without hardware wallets: the browser holds the auth half via passkey; wallet ops are forwarded to the user's server-side anima (running on broomva.tech / Sentinel / Life-Module-tenant infra) which holds secp256k1 in `VaultTransitAnima`. From the chain's perspective the signature is still EOA-flavored — the question is only which custody path produced it.

### Browser path (D-Sub-C in detail)

The hard part of Spec D. Three sub-questions resolved:

1. **Auth keypair generation.** On first browser launch, anima calls `navigator.credentials.create({publicKey: {alg: -7 /* ES256 */, …}})` to mint a passkey. The `CryptoKey` returned is non-extractable. Anima caches the `credentialId` in `IndexedDB`; signing uses `navigator.credentials.get({allowCredentials: [{id: credentialId, type: 'public-key'}]})`.

2. **DID derivation.** The passkey returns the SEC1 compressed public key bytes via the attestation object (`authenticatorData.attestedCredentialData.credentialPublicKey` decoded as COSE_Key). Multicodec-prefix with `0x1200` (P-256) and base58btc-encode → `did:key:zDn…`.

3. **Signing flow.** Anima frontend calls `navigator.credentials.get(...)` with the JWS signing input as `clientDataJSON.challenge`. The browser shows the OS auth prompt (Touch ID / Windows Hello / etc.). Returned signature is DER-encoded ECDSA; anima converts to JOSE compact form for JWS.

4. **Recovery.** Passkey portability is the OS's responsibility (iCloud Keychain, Google Password Manager, BitWarden). Anima does NOT escrow seeds, encrypted or otherwise, in the browser. If the user loses every device with the passkey, they lose the identity — same threat model as a stolen YubiKey. Migration to a new device requires the user to attest an `anima.identity_rotated` event from a still-trusted device.

5. **Session continuity.** One passkey-mediated handshake per browser session mints a short-lived in-memory `Tier-User` capability that authorizes subsequent signing requests against an in-tab signing oracle, so the user isn't prompted on every JWT mint. Default TTL: 15 minutes. Tracks the lifegw Tier-2 pattern.

### Event additions

Three new event types in the `anima.*` namespace:

```
anima.identity_rotated     { old_did, new_did, rotation_proof_jws, rotated_at }
anima.custody_migrated     { from_backend, to_backend, attestation, migrated_at }
anima.identity_revoked     { did, reason, revoked_at }
```

Verifier semantics:
- A DID is **resolvable** if no `identity_revoked` event for it exists in the journal.
- An **old DID** in a rotation chain remains resolvable for verification of historical events but MUST NOT mint new signatures (enforced at custody-trait level: `rotate()` returns a custody handle whose `user_did()` returns the new DID).
- `AgentIdentityDocument.rotation_chain: Vec<DidRotation>` is now mandatory and replayed from the Lago journal at session start.

### Substrate integration changes (call-site delta)

| Crate | Today | Spec D |
|---|---|---|
| `arcan` | reconstructs `AgentSelf` then holds `AnimaKeystore` | reconstructs `AgentSelf` then holds `Arc<dyn AnimaCustody>` resolved from config |
| `haima-x402` | takes `LocalSigner` (raw secp256k1 private key) | takes `Arc<dyn AnimaCustody>`; calls `sign_eip712` and `sign_evm_tx` |
| `spaces` SDK | signs presence with `Ed25519Identity` | signs presence via custody trait (P-256 now) |
| `broomva.tech` Agent Auth Protocol | verifies EdDSA JWT against published Ed25519 pubkey | verifies ES256 JWT; resolves DID via `rotation_chain` from Lago journal |
| `lago-auth` | issues per-user vault sessions against in-process auth pubkey | resolves user DID via journal-replayed `rotation_chain`; verifies P-256 sig |

## Phasing

Mirrors Spec C₃ M7's sub-phase structure. Each is a separately mergeable PR with its own test surface.

### D-Sub-A — Trait + `InProcessAnima` adapter (refactor only) — ~3 days

- Add `AnimaCustody` trait at `crates/anima/anima-identity/src/custody.rs`.
- Add `EcdsaP256Identity` alongside `Ed25519Identity`; deprecate the latter behind `#[cfg(feature = "ed25519-legacy")]`.
- Add `InProcessAnima` impl wrapping the existing `AnimaKeystore` but with P-256 auth key.
- Refactor every call site to take `Arc<dyn AnimaCustody>` (arcan, haima, spaces, broomva.tech).
- Update DID multicodec (`0xed01` → `0x1200`).
- All existing tests pass against `InProcessAnima` with P-256.
- **Acceptance**: behaviorally identical to today on Linux/macOS; no production users affected because the cutover is fresh-deploy.

### D-Sub-B — `VaultTransitAnima` for server-side multi-user — ~5 days

- Per-user Vault namespace pattern: `transit/keys/anima-{user_id}-auth-v{n}` + `transit/keys/anima-{user_id}-wallet-v{n}`.
- Vault token renewal loop (mTLS to Vault).
- `sign_evm_tx` posts the EIP-155-encoded RLP transaction digest to `transit/sign/anima-{user_id}-wallet-v{n}` and reconstructs the signed transaction.
- The natural backend for broomva.tech (multi-tenant identity provider) and Life-Module tenants (Sentinel, the constructora).
- **Acceptance**: Vault-fixture integration test signs a USDC transfer end-to-end on a Base-fork local chain.

### D-Sub-C — `WebCryptoAnima` + `RemoteAnima` (browser path) — ~7 days

- Browser frontend: passkey enroll/sign, DID derivation, signing oracle, session-cap TTL.
- `RemoteAnima` backend: tonic client to a server-side anima daemon over the same `lifegw` edge gateway used by lifed (reuses Tier-2 token plumbing).
- Wallet operations from browser: `WebCryptoAnima.sign_evm_tx` → forwards to `RemoteAnima.sign_evm_tx` → server-side `VaultTransitAnima` signs.
- `apps/chatOS` integration: passkey enrollment on first launch, custody status in settings UI.
- `apps/mission-control` integration: optional browser path for the embedded webview.
- **Acceptance**: end-to-end USDC transfer initiated in chatOS browser, signed via server-side Vault, settled on Base testnet.

### D-Sub-D — `TpmAnima` for desktop single-user — ~3 days

- PKCS#11 client (likely `cryptoki` crate) against the host TPM.
- Both keypairs in TPM if the platform supports secp256k1 (some do via NIST P-256 emulation; most don't natively → escalate wallet half to `HardwareWalletAnima`).
- Default backend for `mission-control` desktop deployments.
- **Acceptance**: cold start mission-control on a TPM-equipped Linux box, mint a JWT, never see the private key in process memory.

### D-Sub-E — `SomaCustody` + rotation/revocation event flow — ~5 days

- New `kernel.SignAuth` + `kernel.SignWallet` admin RPCs on soma. Mirrors the platform-side proposal where soma owns the Tier-2 KMS — same trust-boundary unification at the user scope.
- `SomaCustody` calls soma's UDS, authenticated via `SO_PEERCRED` + `life-runtime` group membership.
- Rotation event flow: `AnimaCustody::rotate()` writes `anima.identity_rotated` to Lago, returns a new custody handle.
- Revocation event flow: `anima.identity_revoked` blocks all future signing for the revoked DID at the custody-trait level (in-process check + cached negative answer with TTL).
- Identity document `rotation_chain` extension: every backend's `export_identity_document()` walks the journal back to the genesis event and emits the full chain.
- **Acceptance**: rotate a user's identity end-to-end; verify that a downstream verifier (broomva.tech) accepts a signature by the new DID, rejects a signature by the old DID for a post-rotation timestamp, and accepts a signature by the old DID for a pre-rotation timestamp.

### D-Sub-F — `HardwareWalletAnima` (optional, wallet-only) — ~2 days

- Wraps any other backend's auth half; takes secp256k1 wallet signing to a Ledger/Trezor via WebHID (browser) or hidapi (desktop).
- Rejects auth signing entirely (auth half delegated to wrapped backend).
- High-stakes UX: every wallet operation is hardware-confirmed.
- **Acceptance**: USDC transfer signed by a Ledger Nano X over WebHID from chatOS browser.

**Total**: ~25 working days, parallelizable across browser (C) vs server (B) vs desktop (D) vs hardware (F) tracks once D-Sub-A lands.

## Sequencing Against Spec C

| Milestone | Spec C status | Spec D dependency |
|---|---|---|
| M5 | ✅ production-shipped 2026-04-29 | none |
| M6 (Spec C₃) | ✅ design done 2026-04-26 | none |
| M7 Sub-phases A–D | ✅ shipped through 2026-04-29 | none |
| **M7 Sub-phase E** | 🟡 next critical-path | D-Sub-A in parallel (refactor-only, no Linear blocker) |
| **M8 SDK** | ⏳ blocked on M7-E | D-Sub-B before M8 ships, so SDK consumers see the production custody story |
| **M9 apps/chat migration** | ⏳ blocked on M7-E + M8 | D-Sub-C is M9-blocking — chatOS browser deployments need passkey custody before launch |
| **M10 launch** | ⏳ blocked on M9 | D-Sub-D + E + F can land post-M9 if the trait abstraction is in place |

The hard gate: **D-Sub-C must ship before chatOS goes public**. A chatOS that ships with file-based key custody for browser users would have a known-bad threat model that ships into every Sentinel/Life-Module tenant deployment too.

## Open Questions (deliberately deferred)

1. **EIP-7212 adoption strategy.** Even with L4-D7 (secp256k1 stays for wallet), Base shipped EIP-7212 (P-256 precompile) which would let smart accounts on Base authenticate via passkey directly. Worth tracking but not Spec D scope.
2. **Cross-device passkey sync edge cases.** What happens when iCloud Keychain syncs a passkey across devices — does the `credentialId` stay the same? (Yes, per spec, but worth a fixture test.)
3. **Hardware wallet UX for autonomous agents.** If an agent owns its own wallet but every tx requires user-confirmation on a Ledger, autonomous agent payments are blocked. The answer is probably "agents have hot wallets in `RemoteAnima`; users have cold wallets in `HardwareWalletAnima`; there's a `wallet_authorisation_chain` event linking them" — but designing this well is a Spec E topic.
4. **Soul migration tooling.** No production users today, so the Ed25519 → P-256 cutover is fresh-deploy. If users land on a pre-D-Sub-A build, we need a one-shot migration tool. Captured as a D-Sub-A nice-to-have.

## References

- Spec C₃ §5 (lifegw Tier-2 KMS) — sibling spec; same pattern at the platform layer.
- `crates/anima/anima-identity/src/keystore.rs` — current `AnimaKeystore` shape (refactor target).
- `crates/anima/anima-identity/src/ed25519.rs` — current Ed25519 impl (deprecated by L4-D6).
- `crates/anima/anima-identity/src/did.rs` — DID multicodec logic (extends to `0x1200` in D-Sub-A).
- `crates/life-runtime/lifegw/src/auth/kms.rs` — `KmsSigner` trait that this spec mirrors.
- W3C WebAuthn Level 3 — passkey spec.
- DID Method Specification: did:key — multicodec table for `0x1200` (P-256).
- EIP-712, EIP-155, EIP-7212 — wallet half references.
