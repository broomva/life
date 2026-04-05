//! Seed management — the single secret from which all keys derive.
//!
//! A 32-byte cryptographically random seed is the root of all identity.
//! From it, we derive:
//!
//! - Ed25519 private key (for Agent Auth Protocol)
//! - secp256k1 private key (for Haima/web3)
//!
//! Derivation uses HKDF-SHA256 with domain-separated info strings,
//! ensuring the derived keys are cryptographically independent.
//!
//! The seed is encrypted at rest using ChaCha20-Poly1305, consistent
//! with Haima's existing wallet encryption pattern.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use anima_core::error::{AnimaError, AnimaResult};

/// Domain separation strings for HKDF key derivation.
const ED25519_DOMAIN: &[u8] = b"anima/ed25519/v1";
const SECP256K1_DOMAIN: &[u8] = b"anima/secp256k1/v1";

/// A 32-byte master seed from which all identity keys are derived.
///
/// The seed is zeroized on drop to prevent key material from
/// lingering in memory.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct MasterSeed {
    bytes: [u8; 32],
}

impl MasterSeed {
    /// Generate a new random seed.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self { bytes }
    }

    /// Create from existing bytes (e.g., after decryption).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    /// Derive the Ed25519 private key bytes from this seed.
    ///
    /// Uses HKDF-SHA256 with domain separation to ensure independence
    /// from the secp256k1 key.
    pub fn derive_ed25519_key(&self) -> Zeroizing<[u8; 32]> {
        let hk = Hkdf::<Sha256>::new(None, &self.bytes);
        let mut okm = Zeroizing::new([0u8; 32]);
        hk.expand(ED25519_DOMAIN, okm.as_mut())
            .expect("HKDF expand should not fail for 32-byte output");
        okm
    }

    /// Derive the secp256k1 private key bytes from this seed.
    ///
    /// Uses HKDF-SHA256 with domain separation to ensure independence
    /// from the Ed25519 key.
    pub fn derive_secp256k1_key(&self) -> Zeroizing<[u8; 32]> {
        let hk = Hkdf::<Sha256>::new(None, &self.bytes);
        let mut okm = Zeroizing::new([0u8; 32]);
        hk.expand(SECP256K1_DOMAIN, okm.as_mut())
            .expect("HKDF expand should not fail for 32-byte output");
        okm
    }

    /// Encrypt the seed for storage.
    ///
    /// Returns (nonce, ciphertext). Uses ChaCha20-Poly1305 with a
    /// random 12-byte nonce, consistent with Haima's wallet encryption.
    pub fn encrypt(&self, encryption_key: &[u8; 32]) -> AnimaResult<EncryptedSeed> {
        let cipher = ChaCha20Poly1305::new(encryption_key.into());

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, self.bytes.as_ref())
            .map_err(|e| AnimaError::Crypto(format!("seed encryption failed: {e}")))?;

        Ok(EncryptedSeed {
            nonce: nonce_bytes.to_vec(),
            ciphertext,
        })
    }

    /// Decrypt a seed from storage.
    pub fn decrypt(encrypted: &EncryptedSeed, encryption_key: &[u8; 32]) -> AnimaResult<Self> {
        let cipher = ChaCha20Poly1305::new(encryption_key.into());
        let nonce = Nonce::from_slice(&encrypted.nonce);

        let plaintext = cipher
            .decrypt(nonce, encrypted.ciphertext.as_ref())
            .map_err(|e| AnimaError::Crypto(format!("seed decryption failed: {e}")))?;

        if plaintext.len() != 32 {
            return Err(AnimaError::Crypto(format!(
                "decrypted seed has wrong length: {} (expected 32)",
                plaintext.len()
            )));
        }

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&plaintext);

        Ok(Self { bytes })
    }

    /// Raw access to seed bytes (for blob storage).
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }
}

/// An encrypted seed, safe to persist.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EncryptedSeed {
    /// The 12-byte nonce used for ChaCha20-Poly1305.
    pub nonce: Vec<u8>,
    /// The encrypted seed bytes + authentication tag.
    pub ciphertext: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_keys_are_different() {
        let seed = MasterSeed::generate();
        let ed25519_key = seed.derive_ed25519_key();
        let secp256k1_key = seed.derive_secp256k1_key();

        // Keys derived with different domain separators must differ
        assert_ne!(ed25519_key.as_ref(), secp256k1_key.as_ref());
    }

    #[test]
    fn derivation_is_deterministic() {
        let bytes = [42u8; 32];
        let seed1 = MasterSeed::from_bytes(bytes);
        let seed2 = MasterSeed::from_bytes(bytes);

        assert_eq!(
            seed1.derive_ed25519_key().as_ref(),
            seed2.derive_ed25519_key().as_ref()
        );
        assert_eq!(
            seed1.derive_secp256k1_key().as_ref(),
            seed2.derive_secp256k1_key().as_ref()
        );
    }

    #[test]
    fn different_seeds_produce_different_keys() {
        let seed1 = MasterSeed::from_bytes([1u8; 32]);
        let seed2 = MasterSeed::from_bytes([2u8; 32]);

        assert_ne!(
            seed1.derive_ed25519_key().as_ref(),
            seed2.derive_ed25519_key().as_ref()
        );
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let seed = MasterSeed::generate();
        let original_ed25519 = seed.derive_ed25519_key().to_vec();

        let encryption_key = [99u8; 32];
        let encrypted = seed.encrypt(&encryption_key).unwrap();

        let decrypted = MasterSeed::decrypt(&encrypted, &encryption_key).unwrap();
        let recovered_ed25519 = decrypted.derive_ed25519_key().to_vec();

        assert_eq!(original_ed25519, recovered_ed25519);
    }

    #[test]
    fn wrong_key_fails_decryption() {
        let seed = MasterSeed::generate();
        let encryption_key = [99u8; 32];
        let encrypted = seed.encrypt(&encryption_key).unwrap();

        let wrong_key = [100u8; 32];
        assert!(MasterSeed::decrypt(&encrypted, &wrong_key).is_err());
    }
}
