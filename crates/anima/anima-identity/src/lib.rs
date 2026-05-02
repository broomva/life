//! # Anima Identity — Cryptographic Identity for Life Agents
//!
//! This crate provides the cryptographic operations for agent identity:
//!
//! - **Seed management**: Single 32-byte master seed → triple keypair derivation
//! - **P-256 (ES256)**: Spec D L4-D6 — current Agent Auth Protocol identity + JWT signing
//! - **Ed25519**: Legacy auth identity, retained for verifying historical events
//! - **secp256k1**: Haima-compatible wallet identity for on-chain economics
//! - **Custody trait**: [`AnimaCustody`] — production-grade trait abstraction (Spec D)
//! - **Keystore**: Backwards-compat unified interface (deprecated; prefer `AnimaCustody`)
//!
//! ## Key Derivation (Spec D L4-D6)
//!
//! ```text
//! MasterSeed (32 bytes, random)
//!   ├── HKDF(seed, "anima/p256/v1")      → P-256 private key (current auth)
//!   ├── HKDF(seed, "anima/ed25519/v1")   → Ed25519 private key (legacy)
//!   └── HKDF(seed, "anima/secp256k1/v1") → secp256k1 private key
//! ```
//!
//! All three keys are cryptographically independent despite sharing a seed.
//! The seed is encrypted at rest using ChaCha20-Poly1305.
//!
//! ## Custody Trait
//!
//! [`AnimaCustody`] is the production custody abstraction. Six backends are
//! planned (per Spec D §"Backend matrix") — only [`InProcessAnima`] ships in
//! D-Sub-A; other backends are filed as D-Sub-B…F.

pub mod custody;
pub mod did;
pub mod ed25519;
pub mod in_process;
pub mod keystore;
pub mod p256;
pub mod rlp;
pub mod seed;

#[cfg(feature = "kms-vault")]
pub mod vault;

pub use custody::{
    AnimaCustody, AnimaCustodyHandle, BackendKind, DidRotationEvent, Eip712Domain, EvmSignature,
    TxRequest,
};
pub use did::{
    AuthAlg, DidResolution, generate_did_key, generate_did_key_p256, resolve_did_key,
    resolve_did_key_ed25519, resolve_did_key_p256, verify_did_key, verify_did_key_p256,
};
pub use in_process::InProcessAnima;
pub use keystore::AnimaKeystore;
pub use p256::EcdsaP256Identity;
pub use seed::{EncryptedSeed, MasterSeed};

#[cfg(feature = "kms-vault")]
pub use vault::{VaultMtls, VaultTransitAnima};
