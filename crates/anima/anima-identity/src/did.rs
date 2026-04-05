//! DID (Decentralized Identifier) generation and resolution.
//!
//! Implements the `did:key` method for Ed25519 public keys, following the
//! W3C DID specification and the did:key method specification.
//!
//! Format: `did:key:z6Mk<base58btc-encoded-multicodec-key>`
//!
//! The multicodec prefix for Ed25519 public key is `0xed01` (two bytes).
//! The public key bytes are prepended with this prefix, then encoded as
//! base58-btc with a `z` prefix (per the Multibase specification).
//!
//! # References
//!
//! - W3C DID Core: <https://www.w3.org/TR/did-core/>
//! - did:key Method: <https://w3c-ccg.github.io/did-method-key/>
//! - Multicodec: <https://github.com/multiformats/multicodec>

use anima_core::error::{AnimaError, AnimaResult};

/// Multicodec prefix for Ed25519 public key.
///
/// This is a two-byte varint: 0xed 0x01.
const ED25519_MULTICODEC_PREFIX: [u8; 2] = [0xed, 0x01];

/// The DID method prefix for key-based identifiers.
const DID_KEY_PREFIX: &str = "did:key:z";

/// Generate a `did:key` DID from an Ed25519 public key.
///
/// Steps:
/// 1. Prepend the Ed25519 multicodec prefix (0xed01) to the 32-byte public key
/// 2. Encode the resulting 34 bytes as base58-btc
/// 3. Prepend the multibase 'z' prefix and the `did:key:` scheme
///
/// # Arguments
///
/// * `public_key` - 32-byte Ed25519 public key
///
/// # Returns
///
/// A string in the format `did:key:z6Mk...`
pub fn generate_did_key(public_key: &[u8; 32]) -> String {
    let mut bytes = Vec::with_capacity(34);
    bytes.extend_from_slice(&ED25519_MULTICODEC_PREFIX);
    bytes.extend_from_slice(public_key);

    let encoded = bs58::encode(&bytes).into_string();
    format!("{DID_KEY_PREFIX}{encoded}")
}

/// Resolve a `did:key` DID and extract the Ed25519 public key.
///
/// Validates the DID format, decodes the base58-btc payload, verifies
/// the multicodec prefix matches Ed25519, and extracts the 32-byte key.
///
/// # Arguments
///
/// * `did` - A DID string in the format `did:key:z6Mk...`
///
/// # Errors
///
/// Returns `AnimaError::Identity` if:
/// - The DID does not start with `did:key:z`
/// - The base58-btc decoding fails
/// - The multicodec prefix does not match Ed25519
/// - The decoded key is not 32 bytes
pub fn resolve_did_key(did: &str) -> AnimaResult<[u8; 32]> {
    // Validate prefix
    let encoded = did
        .strip_prefix(DID_KEY_PREFIX)
        .ok_or_else(|| AnimaError::Identity(format!("invalid did:key format: {did}")))?;

    // Decode base58-btc
    let bytes = bs58::decode(encoded)
        .into_vec()
        .map_err(|e| AnimaError::Identity(format!("base58 decode failed: {e}")))?;

    // Verify multicodec prefix (2 bytes for Ed25519)
    if bytes.len() < 2 {
        return Err(AnimaError::Identity(
            "decoded DID too short for multicodec prefix".into(),
        ));
    }

    if bytes[0] != ED25519_MULTICODEC_PREFIX[0] || bytes[1] != ED25519_MULTICODEC_PREFIX[1] {
        return Err(AnimaError::Identity(format!(
            "unexpected multicodec prefix: [{:#04x}, {:#04x}] (expected Ed25519: [0xed, 0x01])",
            bytes[0], bytes[1]
        )));
    }

    // Extract 32-byte public key
    let key_bytes = &bytes[2..];
    if key_bytes.len() != 32 {
        return Err(AnimaError::Identity(format!(
            "Ed25519 public key must be 32 bytes, got {}",
            key_bytes.len()
        )));
    }

    let mut key = [0u8; 32];
    key.copy_from_slice(key_bytes);
    Ok(key)
}

/// Verify that a DID was derived from the given Ed25519 public key.
///
/// Regenerates the `did:key` from the public key and compares with the given DID.
pub fn verify_did_key(did: &str, public_key: &[u8; 32]) -> bool {
    generate_did_key(public_key) == did
}

/// Construct a verification method ID from a DID.
///
/// The verification method ID is the DID itself with a `#key-1` fragment.
/// This follows the did:key specification where the key is self-describing.
pub fn verification_method_id(did: &str) -> String {
    format!("{did}#key-1")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known test vector: a fixed 32-byte Ed25519 public key.
    fn test_public_key() -> [u8; 32] {
        [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
            0x1d, 0x1e, 0x1f, 0x20,
        ]
    }

    #[test]
    fn generate_did_key_format() {
        let did = generate_did_key(&test_public_key());
        assert!(did.starts_with("did:key:z"));
        // The 'z6Mk' prefix is characteristic of Ed25519 did:key DIDs
        assert!(
            did.starts_with("did:key:z6Mk"),
            "Ed25519 did:key should start with z6Mk, got: {did}"
        );
    }

    #[test]
    fn generate_did_key_deterministic() {
        let key = test_public_key();
        let did1 = generate_did_key(&key);
        let did2 = generate_did_key(&key);
        assert_eq!(did1, did2);
    }

    #[test]
    fn different_keys_produce_different_dids() {
        let key1 = [1u8; 32];
        let key2 = [2u8; 32];
        let did1 = generate_did_key(&key1);
        let did2 = generate_did_key(&key2);
        assert_ne!(did1, did2);
    }

    #[test]
    fn resolve_did_key_roundtrip() {
        let original = test_public_key();
        let did = generate_did_key(&original);
        let resolved = resolve_did_key(&did).unwrap();
        assert_eq!(original, resolved);
    }

    #[test]
    fn resolve_did_key_all_zeros() {
        let key = [0u8; 32];
        let did = generate_did_key(&key);
        let resolved = resolve_did_key(&did).unwrap();
        assert_eq!(key, resolved);
    }

    #[test]
    fn resolve_did_key_all_ones() {
        let key = [0xff; 32];
        let did = generate_did_key(&key);
        let resolved = resolve_did_key(&did).unwrap();
        assert_eq!(key, resolved);
    }

    #[test]
    fn resolve_invalid_prefix_fails() {
        let result = resolve_did_key("did:web:example.com");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_invalid_base58_fails() {
        let result = resolve_did_key("did:key:z0OOO");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_wrong_multicodec_fails() {
        // Encode with a wrong prefix (secp256k1 = 0xe7 0x01 instead of Ed25519)
        let mut bytes = vec![0xe7, 0x01];
        bytes.extend_from_slice(&[1u8; 32]);
        let encoded = bs58::encode(&bytes).into_string();
        let did = format!("did:key:z{encoded}");

        let result = resolve_did_key(&did);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_short_key_fails() {
        // Only 16 bytes instead of 32
        let mut bytes = vec![0xed, 0x01];
        bytes.extend_from_slice(&[1u8; 16]);
        let encoded = bs58::encode(&bytes).into_string();
        let did = format!("did:key:z{encoded}");

        let result = resolve_did_key(&did);
        assert!(result.is_err());
    }

    #[test]
    fn verify_did_key_succeeds() {
        let key = test_public_key();
        let did = generate_did_key(&key);
        assert!(verify_did_key(&did, &key));
    }

    #[test]
    fn verify_did_key_fails_for_wrong_key() {
        let key = test_public_key();
        let did = generate_did_key(&key);
        let wrong_key = [99u8; 32];
        assert!(!verify_did_key(&did, &wrong_key));
    }

    #[test]
    fn verification_method_id_format() {
        let key = test_public_key();
        let did = generate_did_key(&key);
        let vm_id = verification_method_id(&did);
        assert!(vm_id.starts_with("did:key:z"));
        assert!(vm_id.ends_with("#key-1"));
    }

    #[test]
    fn roundtrip_100_random_keys() {
        // Ensure roundtrip works for many different keys
        for i in 0u8..100 {
            let mut key = [0u8; 32];
            key[0] = i;
            key[31] = 255 - i;
            let did = generate_did_key(&key);
            let resolved = resolve_did_key(&did).unwrap();
            assert_eq!(key, resolved, "roundtrip failed for key variant {i}");
        }
    }

    #[test]
    fn known_ed25519_did_key_test_vector() {
        // Test with a known seed-derived key to pin the DID format
        use crate::seed::MasterSeed;

        let seed = MasterSeed::from_bytes([42u8; 32]);
        let ed25519_key = seed.derive_ed25519_key();
        let ed25519_id = crate::ed25519::Ed25519Identity::from_key_bytes(&ed25519_key).unwrap();

        let did_from_module =
            generate_did_key(ed25519_id.public_key_bytes().as_slice().try_into().unwrap());
        let did_from_identity = ed25519_id.did_key();

        // Both methods must produce the same DID
        assert_eq!(did_from_module, did_from_identity);
    }
}
