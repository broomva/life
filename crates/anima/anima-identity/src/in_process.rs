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
    /// True if this handle was constructed by `rotate()`. After rotation the
    /// handle's `seed` is the new auth-derivable seed, but `secp256k1_bytes`
    /// is preserved from the original (per L4-D7 — wallet curve doesn't
    /// rotate). Calling `encrypt_seed()` on such a handle would silently
    /// corrupt the wallet on reload because `from_encrypted` re-derives BOTH
    /// halves from the seed. To avoid that footgun, `encrypt_seed` returns
    /// an error when this is true. Dual-seed encryption that captures both
    /// the auth seed AND the wallet bytes is deferred to a follow-up
    /// sub-phase (see I5 in the D-Sub-A code-quality review).
    is_post_rotation: bool,
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
            is_post_rotation: false,
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

    /// Concrete-type rotation helper. Same semantics as the trait
    /// `rotate()` but returns `(DidRotationEvent, InProcessAnima)` so
    /// callers that need access to concrete-only methods (e.g.
    /// `encrypt_seed`) on the post-rotation handle can use them.
    ///
    /// The trait `rotate()` wraps this in `Arc::new(...)` and erases to
    /// `Arc<dyn AnimaCustody>`.
    pub fn rotate_concrete(&self) -> AnimaResult<(DidRotationEvent, InProcessAnima)> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| AnimaError::Crypto("custody mutex poisoned".into()))?;

        let old_did = inner.auth.did_key();
        let new_seed = MasterSeed::generate();
        let new_p256_key = new_seed.derive_p256_key();
        let new_auth = EcdsaP256Identity::from_key_bytes(&new_p256_key)?;
        let new_did = new_auth.did_key();

        let proof_claims = serde_json::json!({
            "iss": old_did,
            "sub": &new_did,
            "type": "anima.rotation_proof",
            "iat": Utc::now().timestamp(),
        });
        let rotation_proof_jws = inner.auth.sign_jws(&proof_claims)?;

        let new_inner = InProcessInner {
            seed: new_seed,
            auth: new_auth,
            secp256k1_bytes: inner.secp256k1_bytes.clone(),
            secp256k1_signing: inner.secp256k1_signing.clone(),
            is_post_rotation: true,
        };

        let new_concrete = InProcessAnima {
            inner: Mutex::new(new_inner),
            current_did: new_did.clone(),
            wallet_address: self.wallet_address.clone(),
        };

        let event = DidRotationEvent {
            old_did,
            new_did,
            rotation_proof_jws,
            rotated_at: Utc::now(),
        };

        Ok((event, new_concrete))
    }

    /// Encrypt the master seed for at-rest storage.
    ///
    /// Returns `Err(AnimaError::Crypto(...))` if this handle was constructed
    /// by `rotate()`, because round-tripping such a handle through
    /// `encrypt_seed` → `from_encrypted` would silently re-derive the wallet
    /// half from the new auth seed instead of preserving the original
    /// wallet bytes (per L4-D7 the wallet curve doesn't rotate).
    /// Dual-seed encryption that captures both the auth seed and the wallet
    /// bytes is deferred to a follow-up sub-phase.
    pub fn encrypt_seed(&self, encryption_key: &[u8; 32]) -> AnimaResult<EncryptedSeed> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| AnimaError::Crypto("custody mutex poisoned".into()))?;
        if inner.is_post_rotation {
            return Err(AnimaError::Crypto(
                "encrypt_seed: cannot persist a post-rotation InProcessAnima handle in D-Sub-A; \
                 a future sub-phase will introduce dual-seed encryption that captures both the \
                 new auth seed and the inherited wallet bytes."
                    .into(),
            ));
        }
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
        // D-Sub-B: closes the D-Sub-A SPEC-D-DEVIATION on `sign_evm_tx`.
        // The shared RLP encoder in `crate::rlp` produces the canonical
        // EIP-1559 typed-envelope digest. `InProcessAnima` and
        // `VaultTransitAnima` both go through this path so signatures
        // produced here are broadcast-ready Ethereum/Base transactions.
        //
        // Per the trait `TxRequest` shape (max_fee + max_priority_fee
        // fields), we always emit the EIP-1559 envelope. Legacy
        // EIP-155 callers can use `crate::rlp::encode_eip155_unsigned`
        // directly + `sign_keccak_digest` if they need the older shape.
        use crate::rlp;

        let chain_id = rlp::parse_eip155_chain_id(&tx.chain)
            .map_err(|e| AnimaError::Crypto(format!("evm tx: {e}")))?;
        let to = rlp::parse_address_20(&tx.to)
            .map_err(|e| AnimaError::Crypto(format!("evm tx to: {e}")))?;
        let value = rlp::parse_u256_str(&tx.value_wei)
            .map_err(|e| AnimaError::Crypto(format!("evm tx value: {e}")))?;
        let max_fee = rlp::parse_u256_str(&tx.max_fee_per_gas_wei)
            .map_err(|e| AnimaError::Crypto(format!("evm tx max_fee: {e}")))?;
        let max_priority = rlp::parse_u256_str(&tx.max_priority_fee_per_gas_wei)
            .map_err(|e| AnimaError::Crypto(format!("evm tx max_priority: {e}")))?;
        let data = rlp::parse_data_hex(&tx.data_hex)
            .map_err(|e| AnimaError::Crypto(format!("evm tx data: {e}")))?;
        let envelope = rlp::encode_eip1559_unsigned(
            chain_id,
            tx.nonce,
            &max_priority,
            &max_fee,
            tx.gas_limit,
            &to,
            &value,
            &data,
        );
        let digest = rlp::keccak256(&envelope);
        let bytes = self.sign_keccak_digest(&digest)?;
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

    fn rotate(&self) -> AnimaResult<(DidRotationEvent, Arc<dyn AnimaCustody>)> {
        let (event, new_concrete) = self.rotate_concrete()?;
        Ok((event, Arc::new(new_concrete)))
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
        let old_pubkey = custody.auth_pubkey();
        let (evt, new_handle) = custody.rotate().unwrap();
        assert_eq!(evt.old_did, old_did);
        assert_ne!(evt.new_did, old_did);
        // Both DIDs should be P-256 (zDn… prefix)
        assert!(evt.old_did.starts_with("did:key:zDn"));
        assert!(evt.new_did.starts_with("did:key:zDn"));
        // Proof JWS has 3 parts
        assert_eq!(evt.rotation_proof_jws.split('.').count(), 3);

        // B1 fix verification: the NEW handle reflects the new key.
        // user_did() and auth_pubkey() must be internally consistent on
        // the new handle (this is what the trait contract requires).
        assert_eq!(new_handle.user_did(), evt.new_did);
        assert_ne!(new_handle.auth_pubkey(), old_pubkey);

        // The original handle remains a snapshot of pre-rotation state
        // (NOT mutated). Verifiers walking historical signatures still
        // resolve via the old DID.
        assert_eq!(custody.user_did(), old_did);
        assert_eq!(custody.auth_pubkey(), old_pubkey);

        // Wallet half is preserved across rotation per L4-D7.
        assert_eq!(
            new_handle.wallet_address().map(|w| w.address.clone()),
            custody.wallet_address().map(|w| w.address.clone()),
        );
    }

    #[test]
    fn encrypt_seed_rejects_post_rotation_handle() {
        // I5 fix verification — calling encrypt_seed on a handle produced
        // by rotation returns an error rather than silently corrupting the
        // wallet half on reload.
        let original = InProcessAnima::from_seed(MasterSeed::generate()).unwrap();
        let encryption_key = [42u8; 32];

        // Pre-rotation: encrypt_seed succeeds. Original holds a single seed
        // that deterministically derives both auth and wallet, so the
        // round-trip through encrypt_seed → from_encrypted is correct.
        assert!(original.encrypt_seed(&encryption_key).is_ok());

        // Rotate via the concrete helper to access encrypt_seed on the
        // returned post-rotation handle.
        let (_evt, post_rotation) = original.rotate_concrete().unwrap();

        // Post-rotation: encrypt_seed must error. The post-rotation handle's
        // seed slot is the new auth seed; round-tripping through
        // encrypt_seed → from_encrypted would re-derive the wallet from
        // this seed, producing a DIFFERENT wallet address (per L4-D7 the
        // wallet curve doesn't rotate, so the original wallet bytes were
        // inherited and aren't recoverable from the new seed alone).
        let err = post_rotation.encrypt_seed(&encryption_key).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("post-rotation"),
            "encrypt_seed should reject post-rotation handle (got: {msg})"
        );
    }

    #[test]
    fn rotation_proof_jws_cryptographically_verifies() {
        // I4 follow-up — verify the rotation proof JWS actually validates
        // against the OLD DID's public key, with the NEW DID embedded in
        // the body claims. Without this, a malformed rotation proof could
        // not be distinguished from a valid one.
        use crate::did::{AuthAlg, DidResolution, resolve_did_key};
        use crate::p256::verify_jws_with_pubkey;

        let custody = InProcessAnima::generate_dev().unwrap();
        let old_did = custody.user_did().to_string();
        let (evt, _new_handle) = custody.rotate().unwrap();

        // Resolve the old DID to get the OLD public key.
        let DidResolution {
            algorithm,
            public_key,
        } = resolve_did_key(&old_did).unwrap();
        assert_eq!(algorithm, AuthAlg::P256);
        let old_pub: [u8; 33] = public_key.try_into().expect("P-256 SEC1 compressed");

        // Verify the JWS using the old pubkey.
        let claims: serde_json::Value = verify_jws_with_pubkey(&evt.rotation_proof_jws, &old_pub)
            .expect("rotation proof JWS verifies against old key");

        // Body must claim the new DID as `sub` and the old DID as `iss`.
        assert_eq!(
            claims["iss"],
            serde_json::Value::String(evt.old_did.clone())
        );
        assert_eq!(
            claims["sub"],
            serde_json::Value::String(evt.new_did.clone())
        );
        assert_eq!(
            claims["type"],
            serde_json::Value::String("anima.rotation_proof".into())
        );
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
