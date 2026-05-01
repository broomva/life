//! Keystore — unified identity creation from a single seed.
//!
//! **Deprecation note (Spec D L4-D6):** The `AnimaKeystore` API was the
//! original Ed25519-era surface. D-Sub-A migrates anima auth to ECDSA P-256
//! and introduces the [`crate::custody::AnimaCustody`] trait. New code should
//! prefer `AnimaCustody` (typically via [`crate::in_process::InProcessAnima`]
//! for dev / single-user host deployments). `AnimaKeystore` is retained for
//! the existing test suite + the Ed25519 helpers needed to verify historical
//! events; it forwards auth signing to the P-256 path internally.

use anima_core::error::{AnimaError, AnimaResult};
use anima_core::identity::{AgentIdentity, LifecycleState};
use chrono::Utc;
use haima_core::wallet::{ChainId, WalletAddress};
use haima_wallet::evm::derive_address;
use k256::ecdsa::SigningKey as Secp256k1SigningKey;
use zeroize::Zeroizing;

use crate::ed25519::Ed25519Identity;
use crate::p256::EcdsaP256Identity;
use crate::seed::{EncryptedSeed, MasterSeed};

/// Unified identity keystore that manages both authentication
/// and economic keypairs from a single master seed.
///
/// Holds BOTH a P-256 (current, Spec D L4-D6) and an Ed25519 (legacy)
/// identity derived from the same seed. The Ed25519 path remains usable
/// for verifying historical events; new identity events use P-256.
pub struct AnimaKeystore {
    seed: MasterSeed,
    /// Spec D L4-D6 — the current auth identity (ECDSA P-256).
    p256: EcdsaP256Identity,
    /// Legacy auth identity (Ed25519). Kept for historical-event
    /// verification only; do not mint new signatures via this.
    ed25519: Ed25519Identity,
    secp256k1_key: Zeroizing<Vec<u8>>,
    wallet_address: WalletAddress,
}

impl AnimaKeystore {
    /// Create a new keystore with a fresh random seed.
    pub fn generate() -> AnimaResult<Self> {
        let seed = MasterSeed::generate();
        Self::from_seed(seed)
    }

    /// Create a keystore from an existing seed.
    pub fn from_seed(seed: MasterSeed) -> AnimaResult<Self> {
        let ed25519_key = seed.derive_ed25519_key();
        let ed25519 = Ed25519Identity::from_key_bytes(&ed25519_key)?;

        let p256_key = seed.derive_p256_key();
        let p256 = EcdsaP256Identity::from_key_bytes(&p256_key)?;

        let secp256k1_bytes = seed.derive_secp256k1_key();
        let secp256k1_signing = Secp256k1SigningKey::from_bytes(secp256k1_bytes.as_ref().into())
            .map_err(|e| AnimaError::Crypto(format!("secp256k1 key derivation: {e}")))?;

        let address = derive_address(&secp256k1_signing);
        let wallet_address = WalletAddress {
            address,
            chain: ChainId::base(),
        };

        Ok(Self {
            seed,
            p256,
            ed25519,
            secp256k1_key: Zeroizing::new(secp256k1_bytes.to_vec()),
            wallet_address,
        })
    }

    /// Decrypt and load a keystore from an encrypted seed.
    pub fn from_encrypted(
        encrypted: &EncryptedSeed,
        encryption_key: &[u8; 32],
    ) -> AnimaResult<Self> {
        let seed = MasterSeed::decrypt(encrypted, encryption_key)?;
        Self::from_seed(seed)
    }

    /// Encrypt the master seed for persistent storage.
    pub fn encrypt_seed(&self, encryption_key: &[u8; 32]) -> AnimaResult<EncryptedSeed> {
        self.seed.encrypt(encryption_key)
    }

    /// Access the legacy Ed25519 identity. Kept for backwards-compat tests +
    /// for verifying historical events signed before the Spec D L4-D6
    /// cutover. NEW sign paths should use [`Self::p256`].
    pub fn ed25519(&self) -> &Ed25519Identity {
        &self.ed25519
    }

    /// Access the current P-256 (ES256) auth identity (Spec D L4-D6).
    pub fn p256(&self) -> &EcdsaP256Identity {
        &self.p256
    }

    /// Access the wallet address (for Haima integration).
    pub fn wallet_address(&self) -> &WalletAddress {
        &self.wallet_address
    }

    /// Get the secp256k1 private key bytes (for creating a Haima LocalSigner).
    pub fn secp256k1_key_bytes(&self) -> &Zeroizing<Vec<u8>> {
        &self.secp256k1_key
    }

    /// Build the complete `AgentIdentity` record.
    ///
    /// Post-Spec-D the `auth_public_key` and `did` fields use the P-256
    /// identity. The legacy Ed25519 pubkey is kept on the struct but isn't
    /// populated here — historical-event verifiers carry their own pubkey
    /// via the rotation chain.
    pub fn build_identity(
        &self,
        agent_id: impl Into<String>,
        host_id: impl Into<String>,
    ) -> AgentIdentity {
        AgentIdentity {
            agent_id: agent_id.into(),
            host_id: host_id.into(),
            auth_public_key: self.p256.public_key_bytes().to_vec(),
            wallet_address: self.wallet_address.clone(),
            did: Some(self.p256.did_key()),
            lifecycle: LifecycleState::Active,
            created_at: Utc::now(),
            expires_at: None,
            seed_blob_ref: None,
        }
    }

    /// Sign an Agent Auth Protocol JWT using the current (P-256) auth key.
    pub fn sign_agent_jwt(
        &self,
        agent_id: &str,
        audience: &str,
        ttl_secs: i64,
    ) -> AnimaResult<String> {
        self.p256.sign_agent_jwt(agent_id, audience, ttl_secs)
    }

    /// Sign an Agent Auth Protocol JWT using the *legacy* Ed25519 key.
    /// Used only by the migration / historical-replay paths and tests
    /// covering the Ed25519 verifier path.
    pub fn sign_agent_jwt_ed25519_legacy(
        &self,
        agent_id: &str,
        audience: &str,
        ttl_secs: i64,
    ) -> AnimaResult<String> {
        self.ed25519.sign_agent_jwt(agent_id, audience, ttl_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_keystore() {
        let ks = AnimaKeystore::generate().unwrap();

        // P-256 compressed public key is 33 bytes
        assert_eq!(ks.p256().public_key_bytes().len(), 33);

        // Wallet address starts with 0x
        assert!(ks.wallet_address().address.starts_with("0x"));

        // DID is generated and uses the P-256 multicodec (zDn… prefix).
        let identity = ks.build_identity("agt_001", "host_arcan");
        assert!(identity.did.is_some());
        let did = identity.did.as_ref().unwrap();
        assert!(did.starts_with("did:key:zDn"));

        // Legacy Ed25519 path still works for historical-event verification.
        assert_eq!(ks.ed25519().public_key_bytes().len(), 32);
    }

    #[test]
    fn deterministic_from_seed() {
        let bytes = [42u8; 32];
        let ks1 = AnimaKeystore::from_seed(MasterSeed::from_bytes(bytes)).unwrap();
        let ks2 = AnimaKeystore::from_seed(MasterSeed::from_bytes(bytes)).unwrap();

        assert_eq!(ks1.p256().public_key_bytes(), ks2.p256().public_key_bytes());
        assert_eq!(
            ks1.ed25519().public_key_bytes(),
            ks2.ed25519().public_key_bytes()
        );
        assert_eq!(ks1.wallet_address().address, ks2.wallet_address().address);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let ks = AnimaKeystore::generate().unwrap();
        let original_p256 = ks.p256().public_key_bytes();
        let original_ed25519 = ks.ed25519().public_key_bytes();
        let original_wallet = ks.wallet_address().address.clone();

        let encryption_key = [77u8; 32];
        let encrypted = ks.encrypt_seed(&encryption_key).unwrap();

        let recovered = AnimaKeystore::from_encrypted(&encrypted, &encryption_key).unwrap();
        assert_eq!(recovered.p256().public_key_bytes(), original_p256);
        assert_eq!(recovered.ed25519().public_key_bytes(), original_ed25519);
        assert_eq!(recovered.wallet_address().address, original_wallet);
    }

    #[test]
    fn build_identity_fields_use_p256() {
        let ks = AnimaKeystore::generate().unwrap();
        let id = ks.build_identity("agt_test", "host_test");

        assert_eq!(id.agent_id, "agt_test");
        assert_eq!(id.host_id, "host_test");
        assert_eq!(id.lifecycle, LifecycleState::Active);
        // auth_public_key now carries the 33-byte compressed P-256 pubkey
        assert_eq!(id.auth_public_key.len(), 33);
        assert_eq!(id.auth_public_key, ks.p256().public_key_bytes().to_vec());
    }

    #[test]
    fn sign_jwt_uses_es256() {
        let ks = AnimaKeystore::generate().unwrap();
        let jwt = ks
            .sign_agent_jwt("agt_001", "https://broomva.tech", 60)
            .unwrap();

        assert!(!jwt.is_empty());
        assert_eq!(jwt.split('.').count(), 3);

        // Decode header to assert ES256
        use base64::Engine;
        let parts: Vec<&str> = jwt.split('.').collect();
        let header_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[0])
            .unwrap();
        let header: serde_json::Value = serde_json::from_slice(&header_bytes).unwrap();
        assert_eq!(header["alg"], "ES256");
    }

    #[test]
    fn legacy_ed25519_jwt_path_still_works() {
        let ks = AnimaKeystore::generate().unwrap();
        let jwt = ks
            .sign_agent_jwt_ed25519_legacy("agt_001", "https://broomva.tech", 60)
            .unwrap();
        assert_eq!(jwt.split('.').count(), 3);

        use base64::Engine;
        let parts: Vec<&str> = jwt.split('.').collect();
        let header_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[0])
            .unwrap();
        let header: serde_json::Value = serde_json::from_slice(&header_bytes).unwrap();
        assert_eq!(header["alg"], "EdDSA");
    }
}
