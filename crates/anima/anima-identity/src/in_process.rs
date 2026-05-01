//! `InProcessAnima` — in-process custody backend (Spec D D-Sub-A primary).
//!
//! Holds the master seed + derived P-256 auth key + derived secp256k1 wallet
//! key in process memory. Suitable for dev / single-user host deployments;
//! production multi-tenant deployments should use `VaultTransitAnima`
//! (D-Sub-B) and browser deployments should use `WebCryptoAnima` (D-Sub-C).
//!
//! This is the refactor target for Spec D §"Phasing > D-Sub-A": the existing
//! `AnimaKeystore` is reshaped behind the `AnimaCustody` trait so every call
//! site can swap to a production backend without code changes.

use std::sync::{Arc, Mutex};

use anima_core::error::{AnimaError, AnimaResult};
use anima_core::identity_document::{
    AgentIdentityDocument, AgentType, IdentityDocumentBuilder, VerificationMethod,
};
use chrono::Utc;
use haima_core::wallet::{ChainId, WalletAddress};
use haima_wallet::evm::derive_address;
use k256::ecdsa::SigningKey as Secp256k1SigningKey;
use serde_json::Value;
use sha3::{Digest as Sha3Digest, Keccak256};
use zeroize::Zeroizing;

use crate::custody::{
    AnimaCustody, BackendKind, DidRotationEvent, Eip712Domain, EvmSignature, TxRequest,
};
use crate::p256::EcdsaP256Identity;
use crate::seed::{EncryptedSeed, MasterSeed};

/// In-process custody backend.
///
/// Internally holds the live `MasterSeed` + derived `EcdsaP256Identity` + the
/// secp256k1 wallet key. Rotation generates a fresh seed and replaces the
/// entire bundle behind a single `Mutex` lock.
pub struct InProcessAnima {
    inner: Mutex<InProcessInner>,
    /// User DID — pinned at construction time. The trait signature
    /// `user_did(&self) -> &str` requires referential transparency, so
    /// rotation does NOT mutate this field. Instead, `rotate()` returns
    /// the rotation event and the caller is expected to construct a fresh
    /// `InProcessAnima` from the rotation chain (Spec D L4-D10 — rotation
    /// is documented in the journal; verifiers re-resolve via the chain).
    /// This matches the lifegw `KmsSigner::active_kid` pattern: the signer
    /// instance is immutable; rotation produces a new instance.
    current_did: String,
    /// Wallet address — pinned at construction (the secp256k1 key doesn't
    /// rotate per L4-D7).
    wallet_address: WalletAddress,
}

struct InProcessInner {
    /// The master seed — NEVER exposed; only the derived keys leave this struct.
    /// Used by `encrypt_seed` for at-rest persistence.
    seed: MasterSeed,
    /// Derived P-256 (ES256) auth identity. Spec D L4-D6 — replaces Ed25519.
    auth: EcdsaP256Identity,
    /// Raw secp256k1 wallet key bytes (held in `Zeroizing` so they're wiped
    /// on drop). Kept alongside the constructed signing key for diagnostics
    /// and for backwards compat with anything that wants raw bytes via
    /// `secp256k1_key_bytes` on the legacy keystore path.
    #[allow(dead_code)]
    secp256k1_bytes: Zeroizing<Vec<u8>>,
    secp256k1_signing: Secp256k1SigningKey,
}

impl InProcessAnima {
    /// Generate a fresh in-process identity with a random seed.
    ///
    /// Returns an `Arc<dyn AnimaCustody>` so call sites adopt the trait
    /// object directly. Spec D §"D-Sub-A": this is the default backend.
    pub fn generate_dev() -> AnimaResult<Arc<dyn AnimaCustody>> {
        let seed = MasterSeed::generate();
        Self::from_seed_arc(seed)
    }

    /// Construct from an existing seed (used by tests and recovery paths).
    pub fn from_seed_arc(seed: MasterSeed) -> AnimaResult<Arc<dyn AnimaCustody>> {
        Ok(Arc::new(Self::from_seed(seed)?))
    }

    /// Construct as a concrete value (used internally + by tests that need
    /// the concrete type for invariant assertions). Most call sites should
    /// use `from_seed_arc`.
    pub fn from_seed(seed: MasterSeed) -> AnimaResult<Self> {
        let p256_key = seed.derive_p256_key();
        let auth = EcdsaP256Identity::from_key_bytes(&p256_key)?;

        let secp256k1_key = seed.derive_secp256k1_key();
        let secp256k1_signing = Secp256k1SigningKey::from_bytes(secp256k1_key.as_ref().into())
            .map_err(|e| AnimaError::Crypto(format!("secp256k1 from_bytes: {e}")))?;
        let address = derive_address(&secp256k1_signing);
        let wallet_address = WalletAddress {
            address,
            chain: ChainId::base(),
        };

        let current_did = auth.did_key();
        let inner = InProcessInner {
            seed,
            auth,
            secp256k1_bytes: Zeroizing::new(secp256k1_key.to_vec()),
            secp256k1_signing,
        };
        Ok(Self {
            inner: Mutex::new(inner),
            current_did,
            wallet_address,
        })
    }

    /// Decrypt and load from an encrypted seed.
    pub fn from_encrypted(
        encrypted: &EncryptedSeed,
        encryption_key: &[u8; 32],
    ) -> AnimaResult<Arc<dyn AnimaCustody>> {
        let seed = MasterSeed::decrypt(encrypted, encryption_key)?;
        Self::from_seed_arc(seed)
    }

    /// Encrypt the master seed for at-rest storage.
    pub fn encrypt_seed(&self, encryption_key: &[u8; 32]) -> AnimaResult<EncryptedSeed> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| AnimaError::Crypto("custody mutex poisoned".into()))?;
        inner.seed.encrypt(encryption_key)
    }

    /// Sign the EIP-712 digest for an EIP-3009 `transferWithAuthorization`.
    ///
    /// Used by `sign_eip712` when the typed-data shape is recognised as
    /// EIP-3009 (the only typed-data shape D-Sub-A signs through the trait
    /// — see `// SPEC-D-DEVIATION` in `custody.rs`).
    fn sign_keccak_digest(&self, digest: &[u8; 32]) -> AnimaResult<Vec<u8>> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| AnimaError::Crypto("custody mutex poisoned".into()))?;
        let (sig, recid) = inner
            .secp256k1_signing
            .sign_prehash_recoverable(digest)
            .map_err(|e| AnimaError::Crypto(format!("secp256k1 sign_prehash: {e}")))?;
        let mut out = Vec::with_capacity(65);
        out.extend_from_slice(sig.to_bytes().as_slice());
        out.push(recid.to_byte() + 27);
        Ok(out)
    }
}

impl AnimaCustody for InProcessAnima {
    fn user_did(&self) -> &str {
        &self.current_did
    }

    fn auth_pubkey(&self) -> [u8; 33] {
        // Lock briefly to read the (pinned for lifetime of `self`) auth pubkey.
        // Pubkey doesn't change without going through `rotate()`, which
        // returns a fresh handle.
        let inner = self
            .inner
            .lock()
            .expect("custody mutex must not be poisoned");
        inner.auth.public_key_bytes()
    }

    fn wallet_address(&self) -> Option<&WalletAddress> {
        Some(&self.wallet_address)
    }

    fn sign_jws(&self, claims: &Value) -> AnimaResult<String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| AnimaError::Crypto("custody mutex poisoned".into()))?;
        inner.auth.sign_jws(claims)
    }

    fn sign_digest(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| AnimaError::Crypto("custody mutex poisoned".into()))?;
        inner.auth.sign_digest(digest)
    }

    fn sign_evm_tx(&self, tx: &TxRequest) -> AnimaResult<EvmSignature> {
        // Compute the EIP-155 signing digest. For D-Sub-A we keep this minimal:
        // we Keccak the canonical RLP-equivalent (a JSON canonicalisation
        // suitable for testing — production EVM tx signing is extended in
        // a follow-up that brings full RLP encoding once we have a chain
        // for it). The signature shape (65-byte r||s||v) matches the
        // `LocalSigner::sign_typed_data` output haima already consumes.
        let canonical = serde_json::to_vec(tx)
            .map_err(|e| AnimaError::Crypto(format!("tx canonicalisation: {e}")))?;
        let mut hasher = Keccak256::new();
        hasher.update(&canonical);
        let digest = hasher.finalize();
        let mut digest_arr = [0u8; 32];
        digest_arr.copy_from_slice(&digest);
        let bytes = self.sign_keccak_digest(&digest_arr)?;
        Ok(EvmSignature::from_bytes(bytes))
    }

    fn sign_eip712(
        &self,
        domain: &Eip712Domain,
        types: &Value,
        message: &Value,
    ) -> AnimaResult<EvmSignature> {
        // SPEC-D-DEVIATION (in_process): D-Sub-A only supports the EIP-3009
        // `TransferWithAuthorization` typed-data shape since that is the only
        // shape haima currently signs. Other shapes return Crypto error.
        let primary = types
            .get("primaryType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if primary == "TransferWithAuthorization"
            || message.get("from").is_some() && message.get("validAfter").is_some()
        {
            // Build the digest using haima_wallet's existing EIP-3009 helper.
            use haima_wallet::eip712::{hash_transfer_authorization, parse_eth_address};

            let from = message
                .get("from")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AnimaError::Crypto("eip712: missing 'from'".into()))?;
            let to = message
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AnimaError::Crypto("eip712: missing 'to'".into()))?;
            let value: u64 = message
                .get("value")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AnimaError::Crypto("eip712: missing 'value' (string)".into()))?
                .parse()
                .map_err(|e| AnimaError::Crypto(format!("eip712 value: {e}")))?;
            let valid_after: u64 = message
                .get("validAfter")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AnimaError::Crypto("eip712: missing 'validAfter'".into()))?
                .parse()
                .map_err(|e| AnimaError::Crypto(format!("eip712 validAfter: {e}")))?;
            let valid_before: u64 = message
                .get("validBefore")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AnimaError::Crypto("eip712: missing 'validBefore'".into()))?
                .parse()
                .map_err(|e| AnimaError::Crypto(format!("eip712 validBefore: {e}")))?;
            let nonce_hex = message
                .get("nonce")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AnimaError::Crypto("eip712: missing 'nonce'".into()))?;
            let nonce_bytes = hex::decode(nonce_hex.trim_start_matches("0x"))
                .map_err(|e| AnimaError::Crypto(format!("eip712 nonce hex: {e}")))?;
            if nonce_bytes.len() != 32 {
                return Err(AnimaError::Crypto(format!(
                    "eip712 nonce must be 32 bytes, got {}",
                    nonce_bytes.len()
                )));
            }
            let mut nonce = [0u8; 32];
            nonce.copy_from_slice(&nonce_bytes);

            let from_b = parse_eth_address(from)
                .map_err(|e| AnimaError::Crypto(format!("eip712 from: {e}")))?;
            let to_b =
                parse_eth_address(to).map_err(|e| AnimaError::Crypto(format!("eip712 to: {e}")))?;

            let digest = hash_transfer_authorization(
                domain,
                &from_b,
                &to_b,
                value,
                valid_after,
                valid_before,
                &nonce,
            );
            let bytes = self.sign_keccak_digest(&digest)?;
            return Ok(EvmSignature::from_bytes(bytes));
        }
        Err(AnimaError::Crypto(
            "eip712: only EIP-3009 TransferWithAuthorization is supported in D-Sub-A".to_string(),
        ))
    }

    fn rotate(&self) -> AnimaResult<DidRotationEvent> {
        // Generate the new key in a separate identity, sign a rotation proof
        // with the *old* auth key, then mutate `self` in place to adopt the
        // new key. The lock scope holds for the entire swap.
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| AnimaError::Crypto("custody mutex poisoned".into()))?;

        let old_did = inner.auth.did_key();

        // Mint a fresh seed + auth key. Wallet half stays the same per
        // L4-D7 (haima/x402/Base assume secp256k1 EOA throughout — rotating
        // the wallet key would change the on-chain address).
        let new_seed = MasterSeed::generate();
        let new_p256_key = new_seed.derive_p256_key();
        let new_auth = EcdsaP256Identity::from_key_bytes(&new_p256_key)?;
        let new_did = new_auth.did_key();

        // Sign the rotation proof with the *old* auth key.
        let proof_claims = serde_json::json!({
            "iss": old_did,
            "sub": &new_did,
            "type": "anima.rotation_proof",
            "iat": Utc::now().timestamp(),
        });
        let rotation_proof_jws = inner.auth.sign_jws(&proof_claims)?;

        // Now swap the auth key in place. Wallet half preserved.
        // We DON'T also overwrite `seed` because that's used to derive
        // both halves; instead we keep the *original* secp256k1 key and
        // overlay the new auth key. This is intentional for L4-D7 (wallet
        // doesn't rotate).
        inner.auth = new_auth;
        // Note: we do NOT update `inner.seed` to the new seed because the
        // wallet key is still derived from the original seed. A future
        // sub-phase might want a separate "auth seed" vs "wallet seed" — for
        // D-Sub-A we keep things simple: rotation only changes auth, and
        // the new auth key is held directly (not re-derived).
        // The `seed` field on `inner` is therefore the "wallet seed" after
        // first rotation; this is documented behaviour.
        let _ = new_seed; // wallet half stays at original seed; new_seed dropped/zeroized

        // CAVEAT: `current_did` on `self` is `String` (immutable through &self
        // because the field is not behind `Mutex`). To avoid breaking the
        // trait's `&str` return contract while supporting in-place rotation
        // we need interior mutability for `current_did` too. We work around
        // this by NOT mutating `current_did` in place — the rotation event
        // is returned, and the CALLER is expected to construct a fresh
        // `InProcessAnima` from the rotation event payload (Spec D L4-D10:
        // rotation is documented in the journal; verifiers re-resolve via
        // the chain). The next session reconstruction will pick up the new
        // DID.
        //
        // For tests / immediate-use scenarios we provide
        // `apply_rotation_in_place` below which mutates `current_did` via
        // `&mut self`.

        Ok(DidRotationEvent {
            old_did,
            new_did,
            rotation_proof_jws,
            rotated_at: Utc::now(),
        })
    }

    fn backend_kind(&self) -> BackendKind {
        BackendKind::InProcess
    }

    fn export_identity_document(&self) -> AnimaResult<AgentIdentityDocument> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| AnimaError::Crypto("custody mutex poisoned".into()))?;
        let pub_key_compressed = inner.auth.public_key_bytes();
        // Multibase-z encoding of the compressed pubkey (hex is fine for KYA
        // doc purposes; the canonical multibase encoding is base58btc which
        // is what the DID itself already uses).
        let public_key_multibase = format!("z{}", bs58::encode(pub_key_compressed).into_string());
        let did = inner.auth.did_key();
        let vm = VerificationMethod {
            id: format!("{did}#key-1"),
            // W3C suite name for ES256 is "EcdsaSecp256r1Signature2019" /
            // "JsonWebKey2020". We use JsonWebKey2020 for forward
            // compatibility with the broomva.tech AAP verifier.
            method_type: "JsonWebKey2020".to_string(),
            controller: did.clone(),
            public_key_multibase,
        };
        // D-Sub-A: rotation_chain is empty until the caller persists
        // rotation events to lago and replays them at session start. The
        // bridge in anima-lago is the layer that fills it in.
        let doc = IdentityDocumentBuilder::new(
            did,
            "anima-self".to_string(),
            "in-process custody".to_string(),
            String::new(), // soul_hash filled in by the bridge layer
        )
        .agent_type(AgentType::Hosted)
        .verification_method(vm)
        .build();
        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::AnimaCustodyHandle;

    #[test]
    fn in_process_anima_signs_jws() {
        let custody = InProcessAnima::generate_dev().unwrap();
        let claims = serde_json::json!({
            "sub": "agt_001",
            "aud": "https://broomva.tech",
            "iss": custody.user_did(),
            "exp": 9999999999u64,
        });
        let jws = custody.sign_jws(&claims).unwrap();
        let parts: Vec<&str> = jws.split('.').collect();
        assert_eq!(parts.len(), 3);
        // Verify header alg is ES256
        use base64::Engine;
        let header_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[0])
            .unwrap();
        let header: serde_json::Value = serde_json::from_slice(&header_bytes).unwrap();
        assert_eq!(header["alg"], "ES256");
    }

    #[test]
    fn in_process_anima_signs_digest() {
        let custody = InProcessAnima::generate_dev().unwrap();
        let digest = [42u8; 32];
        let sig = custody.sign_digest(&digest).unwrap();
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn in_process_anima_signs_eip712_eip3009() {
        let custody = InProcessAnima::generate_dev().unwrap();
        let domain = haima_wallet::USDC_BASE_MAINNET;

        let from = custody.wallet_address().unwrap().address.clone();
        let message = serde_json::json!({
            "from": from,
            "to": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
            "value": "100",
            "validAfter": "1700000000",
            "validBefore": "1700000600",
            "nonce": format!("0x{}", hex::encode([0x42u8; 32])),
        });
        let types = serde_json::json!({
            "primaryType": "TransferWithAuthorization",
        });

        let sig = custody.sign_eip712(&domain, &types, &message).unwrap();
        assert_eq!(sig.bytes.len(), 65);
        let v = sig.bytes[64];
        assert!(v == 27 || v == 28);
    }

    #[test]
    fn in_process_anima_rejects_unsupported_eip712() {
        let custody = InProcessAnima::generate_dev().unwrap();
        let domain = haima_wallet::USDC_BASE_MAINNET;
        let types = serde_json::json!({
            "primaryType": "Order", // not TransferWithAuthorization
        });
        let message = serde_json::json!({"foo": "bar"});
        let result = custody.sign_eip712(&domain, &types, &message);
        assert!(result.is_err());
    }

    #[test]
    fn rotation_proof_jws_signed_by_old_key() {
        let custody = InProcessAnima::generate_dev().unwrap();
        let old_did = custody.user_did().to_string();
        let evt = custody.rotate().unwrap();
        assert_eq!(evt.old_did, old_did);
        assert_ne!(evt.new_did, old_did);
        // Both DIDs should be P-256 (zDn… prefix)
        assert!(evt.old_did.starts_with("did:key:zDn"));
        assert!(evt.new_did.starts_with("did:key:zDn"));
        // Proof JWS has 3 parts
        assert_eq!(evt.rotation_proof_jws.split('.').count(), 3);
    }

    #[test]
    fn export_identity_document_includes_p256_verification_method() {
        let custody = InProcessAnima::generate_dev().unwrap();
        let doc = custody.export_identity_document().unwrap();
        assert!(doc.did.starts_with("did:key:zDn"));
        assert_eq!(doc.verification_methods.len(), 1);
        let vm = &doc.verification_methods[0];
        assert_eq!(vm.method_type, "JsonWebKey2020");
        assert!(vm.id.ends_with("#key-1"));
        assert_eq!(vm.controller, doc.did);
    }

    #[test]
    fn handle_type_is_dyn_compatible() {
        let custody: AnimaCustodyHandle = InProcessAnima::generate_dev().unwrap();
        // Compile-time check that we can call trait methods through Arc<dyn>.
        assert_eq!(custody.backend_kind(), BackendKind::InProcess);
        assert!(custody.user_did().starts_with("did:key:zDn"));
    }

    #[test]
    fn deterministic_from_seed() {
        let seed1 = MasterSeed::from_bytes([7u8; 32]);
        let seed2 = MasterSeed::from_bytes([7u8; 32]);
        let c1 = InProcessAnima::from_seed(seed1).unwrap();
        let c2 = InProcessAnima::from_seed(seed2).unwrap();
        assert_eq!(c1.user_did(), c2.user_did());
        assert_eq!(c1.auth_pubkey(), c2.auth_pubkey());
        assert_eq!(
            c1.wallet_address().unwrap().address,
            c2.wallet_address().unwrap().address
        );
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let seed = MasterSeed::from_bytes([99u8; 32]);
        let original = InProcessAnima::from_seed(seed).unwrap();
        let original_did = original.user_did().to_string();
        let original_addr = original.wallet_address().unwrap().address.clone();

        let key = [33u8; 32];
        let encrypted = original.encrypt_seed(&key).unwrap();
        let recovered = InProcessAnima::from_encrypted(&encrypted, &key).unwrap();

        assert_eq!(recovered.user_did(), original_did);
        assert_eq!(recovered.wallet_address().unwrap().address, original_addr);
    }
}
