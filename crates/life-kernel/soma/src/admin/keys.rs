//! In-process custody-oracle key store (Spec D D-Sub-E).
//!
//! soma's CustodyOracle service holds the (P-256 auth + secp256k1
//! wallet) key material per user. This module is the simplest viable
//! implementation: keys live in process memory, wrapped in `Zeroizing`
//! so they wipe on drop.
//!
//! ## Operator choice (out of scope for this trait)
//!
//! Production deploys MAY swap this implementation for:
//!
//! - **TPM via PKCS#11** — preferred for single-user mission-control
//!   boxes. Pulls in `cryptoki` 0.x as a dep; drop-in replacement that
//!   delegates `sign_auth` / `sign_wallet` to the TPM rather than
//!   holding raw scalars.
//! - **Custom HSM sidecar** — for deploys where soma sits inside a
//!   privileged µVM and the HSM lives on the host. The sidecar speaks
//!   a private socket protocol; the soma side of the trait stays
//!   identical.
//! - **In-process Zeroizing<Vec<u8>>** — what this module ships. Useful
//!   for dev and freelance-tenant deploys without dedicated key
//!   hardware.
//!
//! The trait shape doesn't constrain which choice operators make, and
//! SomaCustody (anima-side) doesn't observe the difference — the wire
//! contract is just "a 32-byte digest goes in, a signature comes out".
//!
//! See `crates/anima/anima-identity/src/soma.rs` SPEC-D-DEVIATION block
//! for the caller-side framing.

use std::collections::HashMap;
use std::sync::Arc;

use anima_core::error::AnimaError;
use parking_lot::RwLock;
use sha3::Digest as Sha3Digest;
use sha3::Keccak256;
use zeroize::Zeroizing;

/// Result type used by the custody key store. We intentionally reuse
/// `AnimaError` so the soma admin handler can map errors back to
/// tonic statuses with a single conversion step (avoids leaking a
/// soma-internal error type to the wire).
pub type CustodyResult<T> = Result<T, AnimaError>;

/// In-process key entry — one per user.
///
/// Both halves are zeroed on drop. The auth key is held as a 32-byte
/// scalar so the `p256::ecdsa::SigningKey` can be reconstructed
/// per-call without holding a mutex on the signing key itself; the
/// wallet half is the same shape on secp256k1.
#[derive(Debug)]
struct UserKeys {
    auth_scalar: Zeroizing<[u8; 32]>,
    wallet_scalar: Zeroizing<[u8; 32]>,
}

/// In-process custody-oracle key store. Thread-safe — every method is
/// `&self`.
///
/// ## Bootstrap
///
/// Operators provision keys at daemon start via [`Self::insert_user`].
/// The store does NOT auto-create keys on first SignAuth/SignWallet —
/// missing-user is a fail-closed error (`AnimaError::Crypto("user not
/// provisioned")`). This matches the VaultTransitAnima discipline:
/// tenant boundary owns key lifecycle.
#[derive(Debug, Default, Clone)]
pub struct InProcessCustodyKeys {
    inner: Arc<RwLock<HashMap<String, UserKeys>>>,
}

impl InProcessCustodyKeys {
    /// Construct an empty key store.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Provision a user. Replaces any existing entry under `user_id`.
    /// Operator-facing — typically driven by the soma config or a
    /// management RPC (deferred to a future sub-phase).
    pub fn insert_user(
        &self,
        user_id: impl Into<String>,
        auth_scalar: [u8; 32],
        wallet_scalar: [u8; 32],
    ) {
        let entry = UserKeys {
            auth_scalar: Zeroizing::new(auth_scalar),
            wallet_scalar: Zeroizing::new(wallet_scalar),
        };
        self.inner.write().insert(user_id.into(), entry);
    }

    /// Sign a 32-byte digest with the user's auth (P-256) key.
    ///
    /// Returns the IEEE-P1363 raw `r||s` (64 bytes). Mirrors
    /// `EcdsaP256Identity::sign_digest` — the hot path inside the soma
    /// admin handler.
    pub fn sign_auth_digest(&self, user_id: &str, digest: &[u8; 32]) -> CustodyResult<[u8; 64]> {
        use p256::ecdsa::Signature;
        use p256::ecdsa::SigningKey;
        use p256::ecdsa::signature::hazmat::PrehashSigner;
        let inner = self.inner.read();
        let user = inner
            .get(user_id)
            .ok_or_else(|| AnimaError::Crypto(format!("soma: user {user_id} not provisioned")))?;
        let signing_key = SigningKey::from_bytes(user.auth_scalar.as_ref().into())
            .map_err(|e| AnimaError::Crypto(format!("soma p256 from_bytes: {e}")))?;
        let signature: Signature = signing_key
            .sign_prehash(digest)
            .map_err(|e| AnimaError::Crypto(format!("soma p256 sign_prehash: {e}")))?;
        let bytes = signature.to_bytes();
        let mut out = [0u8; 64];
        out.copy_from_slice(bytes.as_slice());
        Ok(out)
    }

    /// Sign a 32-byte digest with the user's wallet (secp256k1) key
    /// and return the EVM-flavoured 65-byte `r||s||v` signature.
    ///
    /// `v` is in legacy 27/28 form (haima-wallet convention).
    pub fn sign_wallet_digest(&self, user_id: &str, digest: &[u8; 32]) -> CustodyResult<[u8; 65]> {
        use k256::ecdsa::{RecoveryId, Signature, SigningKey};
        let inner = self.inner.read();
        let user = inner
            .get(user_id)
            .ok_or_else(|| AnimaError::Crypto(format!("soma: user {user_id} not provisioned")))?;
        let signing_key = SigningKey::from_bytes(user.wallet_scalar.as_ref().into())
            .map_err(|e| AnimaError::Crypto(format!("soma secp256k1 from_bytes: {e}")))?;
        let (sig, recid): (Signature, RecoveryId) = signing_key
            .sign_prehash_recoverable(digest)
            .map_err(|e| AnimaError::Crypto(format!("soma secp256k1 sign_prehash: {e}")))?;
        let mut out = [0u8; 65];
        out[..64].copy_from_slice(sig.to_bytes().as_slice());
        out[64] = recid.to_byte() + 27;
        Ok(out)
    }

    /// Fetch the user's auth (P-256) public key as SEC1 compressed (33 bytes).
    pub fn auth_pubkey_sec1(&self, user_id: &str) -> CustodyResult<[u8; 33]> {
        use p256::ecdsa::SigningKey;
        let inner = self.inner.read();
        let user = inner
            .get(user_id)
            .ok_or_else(|| AnimaError::Crypto(format!("soma: user {user_id} not provisioned")))?;
        let signing_key = SigningKey::from_bytes(user.auth_scalar.as_ref().into())
            .map_err(|e| AnimaError::Crypto(format!("soma p256 from_bytes: {e}")))?;
        let verifying = signing_key.verifying_key();
        let point = verifying.to_encoded_point(true);
        let bytes = point.as_bytes();
        if bytes.len() != 33 {
            return Err(AnimaError::Crypto(format!(
                "soma p256 compressed point len: {}",
                bytes.len()
            )));
        }
        let mut out = [0u8; 33];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    /// Fetch the user's wallet (secp256k1) public key as SEC1
    /// uncompressed (65 bytes — `0x04 || x || y`).
    pub fn wallet_pubkey_sec1_uncompressed(&self, user_id: &str) -> CustodyResult<[u8; 65]> {
        use k256::ecdsa::SigningKey;
        let inner = self.inner.read();
        let user = inner
            .get(user_id)
            .ok_or_else(|| AnimaError::Crypto(format!("soma: user {user_id} not provisioned")))?;
        let signing_key = SigningKey::from_bytes(user.wallet_scalar.as_ref().into())
            .map_err(|e| AnimaError::Crypto(format!("soma secp256k1 from_bytes: {e}")))?;
        let verifying = signing_key.verifying_key();
        let point = verifying.to_encoded_point(false);
        let bytes = point.as_bytes();
        if bytes.len() != 65 {
            return Err(AnimaError::Crypto(format!(
                "soma secp256k1 uncompressed point len: {}",
                bytes.len()
            )));
        }
        let mut out = [0u8; 65];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    /// True if `user_id` has both halves provisioned.
    pub fn has_user(&self, user_id: &str) -> bool {
        self.inner.read().contains_key(user_id)
    }
}

/// Helper: derive the EVM address from a 65-byte uncompressed
/// secp256k1 public key. Mirror of haima-wallet's `derive_address` so
/// admin handlers can return wallet addresses without reaching for the
/// haima-wallet dep tree.
pub fn derive_wallet_address(uncompressed: &[u8; 65]) -> String {
    debug_assert_eq!(
        uncompressed[0], 0x04,
        "uncompressed point must start with 0x04"
    );
    let hash = Keccak256::digest(&uncompressed[1..]);
    let address_bytes = &hash[12..];
    format!("0x{}", hex::encode(address_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `[a-zA-Z0-9_-]+` whitelist policy: tests use this fixed string.
    const ALICE: &str = "alice";

    fn fixture_keys() -> InProcessCustodyKeys {
        let store = InProcessCustodyKeys::new();
        store.insert_user(ALICE, [7u8; 32], [11u8; 32]);
        store
    }

    #[test]
    fn missing_user_fails() {
        let store = InProcessCustodyKeys::new();
        assert!(store.sign_auth_digest("nobody", &[0u8; 32]).is_err());
        assert!(store.sign_wallet_digest("nobody", &[0u8; 32]).is_err());
        assert!(store.auth_pubkey_sec1("nobody").is_err());
        assert!(store.wallet_pubkey_sec1_uncompressed("nobody").is_err());
    }

    #[test]
    fn sign_auth_returns_64_bytes() {
        let store = fixture_keys();
        let digest = [42u8; 32];
        let sig = store.sign_auth_digest(ALICE, &digest).unwrap();
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn sign_wallet_returns_65_bytes_with_v_byte() {
        let store = fixture_keys();
        let digest = [42u8; 32];
        let sig = store.sign_wallet_digest(ALICE, &digest).unwrap();
        assert_eq!(sig.len(), 65);
        let v = sig[64];
        assert!(v == 27 || v == 28);
    }

    #[test]
    fn auth_pubkey_is_compressed_33_bytes() {
        let store = fixture_keys();
        let pk = store.auth_pubkey_sec1(ALICE).unwrap();
        assert_eq!(pk.len(), 33);
        assert!(pk[0] == 0x02 || pk[0] == 0x03);
    }

    #[test]
    fn wallet_pubkey_is_uncompressed_65_bytes() {
        let store = fixture_keys();
        let pk = store.wallet_pubkey_sec1_uncompressed(ALICE).unwrap();
        assert_eq!(pk.len(), 65);
        assert_eq!(pk[0], 0x04);
    }

    #[test]
    fn derive_wallet_address_format() {
        let store = fixture_keys();
        let pk = store.wallet_pubkey_sec1_uncompressed(ALICE).unwrap();
        let addr = derive_wallet_address(&pk);
        assert!(addr.starts_with("0x"));
        assert_eq!(addr.len(), 42);
    }

    /// Verify the auth signature actually verifies — guards against
    /// curve-mismatch or byte-order regressions.
    #[test]
    fn auth_signature_verifies() {
        use p256::ecdsa::Signature;
        use p256::ecdsa::VerifyingKey;
        use p256::ecdsa::signature::hazmat::PrehashVerifier;

        let store = fixture_keys();
        let digest = [42u8; 32];
        let sig_bytes = store.sign_auth_digest(ALICE, &digest).unwrap();
        let pk_bytes = store.auth_pubkey_sec1(ALICE).unwrap();

        let signature = Signature::from_slice(&sig_bytes).unwrap();
        use p256::PublicKey;
        let pk = PublicKey::from_sec1_bytes(&pk_bytes).unwrap();
        let verifying = VerifyingKey::from(&pk);
        verifying.verify_prehash(&digest, &signature).unwrap();
    }
}
