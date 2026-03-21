//! # Anima Identity — Cryptographic Identity for Life Agents
//!
//! This crate provides the cryptographic operations for agent identity:
//!
//! - **Seed management**: Single 32-byte master seed → dual keypair derivation
//! - **Ed25519**: Agent Auth Protocol compatible authentication + JWT signing
//! - **secp256k1**: Haima-compatible wallet identity for on-chain economics
//! - **Keystore**: Unified interface for creating and managing agent identity
//!
//! ## Key Derivation
//!
//! ```text
//! MasterSeed (32 bytes, random)
//!   ├── HKDF(seed, "anima/ed25519/v1")   → Ed25519 private key
//!   └── HKDF(seed, "anima/secp256k1/v1") → secp256k1 private key
//! ```
//!
//! Both keys are cryptographically independent despite sharing a seed.
//! The seed is encrypted at rest using ChaCha20-Poly1305.

pub mod ed25519;
pub mod keystore;
pub mod seed;

pub use keystore::AnimaKeystore;
pub use seed::{EncryptedSeed, MasterSeed};
