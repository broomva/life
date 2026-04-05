//! Keystore — unified identity creation from a single seed.
//!
//! The `AnimaKeystore` is the primary interface for creating and
//! managing agent identity. It holds the master seed and provides
//! access to both the Ed25519 auth identity and the secp256k1
//! wallet identity.

use anima_core::error::{AnimaError, AnimaResult};
use anima_core::identity::{AgentIdentity, LifecycleState};
use chrono::Utc;
use haima_core::wallet::{ChainId, WalletAddress};
use haima_wallet::evm::derive_address;
use k256::ecdsa::SigningKey as Secp256k1SigningKey;
use zeroize::Zeroizing;

use crate::ed25519::Ed25519Identity;
use crate::seed::{EncryptedSeed, MasterSeed};

/// Unified identity keystore that manages both authentication
/// and economic keypairs from a single master seed.
pub struct AnimaKeystore {
    seed: MasterSeed,
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

    /// Access the Ed25519 identity (for auth, JWT signing).
    pub fn ed25519(&self) -> &Ed25519Identity {
        &self.ed25519
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
    pub fn build_identity(
        &self,
        agent_id: impl Into<String>,
        host_id: impl Into<String>,
    ) -> AgentIdentity {
        AgentIdentity {
            agent_id: agent_id.into(),
            host_id: host_id.into(),
            auth_public_key: self.ed25519.public_key_bytes(),
            wallet_address: self.wallet_address.clone(),
            did: Some(self.ed25519.did_key()),
            lifecycle: LifecycleState::Active,
            created_at: Utc::now(),
            expires_at: None,
            seed_blob_ref: None,
        }
    }

    /// Sign an Agent Auth Protocol JWT.
    pub fn sign_agent_jwt(
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

        // Ed25519 public key is 32 bytes
        assert_eq!(ks.ed25519().public_key_bytes().len(), 32);

        // Wallet address starts with 0x
        assert!(ks.wallet_address().address.starts_with("0x"));

        // DID is generated
        let identity = ks.build_identity("agt_001", "host_arcan");
        assert!(identity.did.is_some());
        assert!(identity.did.as_ref().unwrap().starts_with("did:key:z"));
    }

    #[test]
    fn deterministic_from_seed() {
        let bytes = [42u8; 32];
        let ks1 = AnimaKeystore::from_seed(MasterSeed::from_bytes(bytes)).unwrap();
        let ks2 = AnimaKeystore::from_seed(MasterSeed::from_bytes(bytes)).unwrap();

        assert_eq!(
            ks1.ed25519().public_key_bytes(),
            ks2.ed25519().public_key_bytes()
        );
        assert_eq!(ks1.wallet_address().address, ks2.wallet_address().address);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let ks = AnimaKeystore::generate().unwrap();
        let original_pubkey = ks.ed25519().public_key_bytes();
        let original_wallet = ks.wallet_address().address.clone();

        let encryption_key = [77u8; 32];
        let encrypted = ks.encrypt_seed(&encryption_key).unwrap();

        let recovered = AnimaKeystore::from_encrypted(&encrypted, &encryption_key).unwrap();
        assert_eq!(recovered.ed25519().public_key_bytes(), original_pubkey);
        assert_eq!(recovered.wallet_address().address, original_wallet);
    }

    #[test]
    fn build_identity_fields() {
        let ks = AnimaKeystore::generate().unwrap();
        let id = ks.build_identity("agt_test", "host_test");

        assert_eq!(id.agent_id, "agt_test");
        assert_eq!(id.host_id, "host_test");
        assert_eq!(id.lifecycle, LifecycleState::Active);
        assert_eq!(id.auth_public_key, ks.ed25519().public_key_bytes());
    }

    #[test]
    fn sign_jwt() {
        let ks = AnimaKeystore::generate().unwrap();
        let jwt = ks
            .sign_agent_jwt("agt_001", "https://broomva.tech", 60)
            .unwrap();

        assert!(!jwt.is_empty());
        assert_eq!(jwt.split('.').count(), 3);
    }
}
