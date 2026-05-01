//! DID (Decentralized Identifier) generation and resolution.
//!
//! Implements the `did:key` method for both Ed25519 and P-256 public keys,
//! following the W3C DID specification and the did:key method specification.
//!
//! Format: `did:key:z<base58btc-encoded-multicodec-key>`
//!
//! Multicodec prefixes (varint-encoded):
//! - `0xed 0x01` — Ed25519 public key (legacy; pre-Spec-D auth identity)
//! - `0x80 0x24` — P-256 public key (Spec D L4-D6 cutover; current auth identity)
//!
//! All DIDs minted post-D-Sub-A use P-256. Ed25519 resolution is preserved
//! ONLY for verifying historical events signed before the cutover.
//!
//! # References
//!
//! - W3C DID Core: <https://www.w3.org/TR/did-core/>
//! - did:key Method: <https://w3c-ccg.github.io/did-method-key/>
//! - Multicodec: <https://github.com/multiformats/multicodec>

use anima_core::error::{AnimaError, AnimaResult};
use serde::{Deserialize, Serialize};

/// Multicodec prefix for Ed25519 public key (legacy / historical events only).
///
/// Two-byte varint: 0xed 0x01.
const ED25519_MULTICODEC_PREFIX: [u8; 2] = [0xed, 0x01];

/// Multicodec prefix for P-256 public key (Spec D L4-D6 — current auth curve).
///
/// Two-byte unsigned varint encoding of 0x1200:
///   0x1200 binary: 1 0010 0000 0000
///   varint:        0x80 0x24
const P256_MULTICODEC_PREFIX: [u8; 2] = [0x80, 0x24];

/// The DID method prefix for key-based identifiers.
const DID_KEY_PREFIX: &str = "did:key:z";

/// Auth algorithm encoded in a `did:key` DID.
///
/// `#[non_exhaustive]` so future curves (e.g. P-384, secp256k1 auth) can be
/// added without breaking match exhaustiveness.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthAlg {
    /// Ed25519 — legacy. Resolves multicodec `0xed01`.
    Ed25519,
    /// ECDSA over P-256 — current (Spec D L4-D6). Resolves multicodec `0x1200`.
    P256,
}

/// Result of resolving a `did:key` DID.
#[derive(Debug, Clone)]
pub struct DidResolution {
    /// Which curve / algorithm the key uses.
    pub algorithm: AuthAlg,
    /// Raw public key bytes — 32 for Ed25519, 33 (SEC1 compressed) for P-256.
    pub public_key: Vec<u8>,
}

/// Generate a `did:key` DID from an Ed25519 public key.
///
/// Steps:
/// 1. Prepend the Ed25519 multicodec prefix (0xed01) to the 32-byte public key
/// 2. Encode the resulting 34 bytes as base58-btc
/// 3. Prepend the multibase 'z' prefix and the `did:key:` scheme
pub fn generate_did_key(public_key: &[u8; 32]) -> String {
    let mut bytes = Vec::with_capacity(34);
    bytes.extend_from_slice(&ED25519_MULTICODEC_PREFIX);
    bytes.extend_from_slice(public_key);

    let encoded = bs58::encode(&bytes).into_string();
    format!("{DID_KEY_PREFIX}{encoded}")
}

/// Generate a `did:key` DID from a P-256 SEC1-compressed public key (33 bytes).
///
/// Spec D L4-D6 — this is the production DID generator from D-Sub-A onward.
/// Steps:
/// 1. Prepend the P-256 multicodec prefix (varint-encoded `0x1200` =
///    `[0x80, 0x24]`) to the 33-byte compressed public key
/// 2. Encode the resulting 35 bytes as base58-btc
/// 3. Prepend the multibase 'z' prefix and the `did:key:` scheme
pub fn generate_did_key_p256(public_key: &[u8; 33]) -> String {
    let mut bytes = Vec::with_capacity(35);
    bytes.extend_from_slice(&P256_MULTICODEC_PREFIX);
    bytes.extend_from_slice(public_key);

    let encoded = bs58::encode(&bytes).into_string();
    format!("{DID_KEY_PREFIX}{encoded}")
}

/// Resolve a `did:key` DID and extract the public key + algorithm.
///
/// Spec D L4-D6 — supports both Ed25519 (legacy, for verifying historical
/// events) and P-256 (current, post-D-Sub-A). Returns the algorithm so the
/// caller knows which verifier to use.
pub fn resolve_did_key(did: &str) -> AnimaResult<DidResolution> {
    // Validate prefix
    let encoded = did
        .strip_prefix(DID_KEY_PREFIX)
        .ok_or_else(|| AnimaError::Identity(format!("invalid did:key format: {did}")))?;

    // Decode base58-btc
    let bytes = bs58::decode(encoded)
        .into_vec()
        .map_err(|e| AnimaError::Identity(format!("base58 decode failed: {e}")))?;

    if bytes.len() < 2 {
        return Err(AnimaError::Identity(
            "decoded DID too short for multicodec prefix".into(),
        ));
    }

    // Detect algorithm from multicodec prefix.
    let prefix = [bytes[0], bytes[1]];
    if prefix == ED25519_MULTICODEC_PREFIX {
        let key_bytes = &bytes[2..];
        if key_bytes.len() != 32 {
            return Err(AnimaError::Identity(format!(
                "Ed25519 public key must be 32 bytes, got {}",
                key_bytes.len()
            )));
        }
        return Ok(DidResolution {
            algorithm: AuthAlg::Ed25519,
            public_key: key_bytes.to_vec(),
        });
    }
    if prefix == P256_MULTICODEC_PREFIX {
        let key_bytes = &bytes[2..];
        if key_bytes.len() != 33 {
            return Err(AnimaError::Identity(format!(
                "P-256 SEC1-compressed public key must be 33 bytes, got {}",
                key_bytes.len()
            )));
        }
        // SEC1 compressed point's first byte is 0x02 or 0x03.
        if key_bytes[0] != 0x02 && key_bytes[0] != 0x03 {
            return Err(AnimaError::Identity(format!(
                "P-256 SEC1-compressed point must start with 0x02 or 0x03, got 0x{:02x}",
                key_bytes[0]
            )));
        }
        return Ok(DidResolution {
            algorithm: AuthAlg::P256,
            public_key: key_bytes.to_vec(),
        });
    }

    Err(AnimaError::Identity(format!(
        "unknown multicodec prefix: [{:#04x}, {:#04x}] (expected Ed25519 [0xed, 0x01] or P-256 [0x80, 0x24])",
        bytes[0], bytes[1]
    )))
}

/// Resolve an Ed25519 `did:key` and return the raw 32-byte key.
///
/// Convenience wrapper for legacy callers. Errors if the DID is not Ed25519.
pub fn resolve_did_key_ed25519(did: &str) -> AnimaResult<[u8; 32]> {
    let resolution = resolve_did_key(did)?;
    if resolution.algorithm != AuthAlg::Ed25519 {
        return Err(AnimaError::Identity(format!(
            "expected Ed25519 DID, got {:?}: {did}",
            resolution.algorithm
        )));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&resolution.public_key);
    Ok(key)
}

/// Resolve a P-256 `did:key` and return the SEC1-compressed 33-byte key.
pub fn resolve_did_key_p256(did: &str) -> AnimaResult<[u8; 33]> {
    let resolution = resolve_did_key(did)?;
    if resolution.algorithm != AuthAlg::P256 {
        return Err(AnimaError::Identity(format!(
            "expected P-256 DID, got {:?}: {did}",
            resolution.algorithm
        )));
    }
    let mut key = [0u8; 33];
    key.copy_from_slice(&resolution.public_key);
    Ok(key)
}

/// Verify that a DID was derived from the given Ed25519 public key.
///
/// Regenerates the `did:key` from the public key and compares with the given DID.
pub fn verify_did_key(did: &str, public_key: &[u8; 32]) -> bool {
    generate_did_key(public_key) == did
}

/// Verify that a DID was derived from the given P-256 (SEC1-compressed) public key.
pub fn verify_did_key_p256(did: &str, public_key: &[u8; 33]) -> bool {
    generate_did_key_p256(public_key) == did
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
    fn test_ed25519_public_key() -> [u8; 32] {
        [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
            0x1d, 0x1e, 0x1f, 0x20,
        ]
    }

    /// A valid 33-byte SEC1-compressed P-256 point. We construct a real one
    /// from a key derived from a deterministic seed so the test remains
    /// stable.
    fn test_p256_compressed_key() -> [u8; 33] {
        use crate::seed::MasterSeed;
        let seed = MasterSeed::from_bytes([7u8; 32]);
        let key = seed.derive_p256_key();
        let id = crate::p256::EcdsaP256Identity::from_key_bytes(&key).unwrap();
        id.public_key_bytes()
    }

    // ─── Ed25519 path (legacy) ──────────────────────────────────────

    #[test]
    fn generate_did_key_ed25519_format() {
        let did = generate_did_key(&test_ed25519_public_key());
        assert!(did.starts_with("did:key:z"));
        assert!(
            did.starts_with("did:key:z6Mk"),
            "Ed25519 did:key should start with z6Mk, got: {did}"
        );
    }

    #[test]
    fn generate_did_key_deterministic() {
        let key = test_ed25519_public_key();
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
    fn resolve_did_key_ed25519_legacy_still_resolves() {
        let original = test_ed25519_public_key();
        let did = generate_did_key(&original);
        let resolution = resolve_did_key(&did).unwrap();
        assert_eq!(resolution.algorithm, AuthAlg::Ed25519);
        assert_eq!(resolution.public_key, original.to_vec());

        // Convenience wrapper round-trip
        let key = resolve_did_key_ed25519(&did).unwrap();
        assert_eq!(key, original);
    }

    #[test]
    fn resolve_did_key_all_zeros() {
        let key = [0u8; 32];
        let did = generate_did_key(&key);
        let resolved = resolve_did_key_ed25519(&did).unwrap();
        assert_eq!(key, resolved);
    }

    #[test]
    fn resolve_did_key_all_ones() {
        let key = [0xff; 32];
        let did = generate_did_key(&key);
        let resolved = resolve_did_key_ed25519(&did).unwrap();
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
    fn resolve_unknown_multicodec_fails() {
        // Encode with a wrong prefix (secp256k1 = 0xe7 0x01)
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
        let key = test_ed25519_public_key();
        let did = generate_did_key(&key);
        assert!(verify_did_key(&did, &key));
    }

    #[test]
    fn verify_did_key_fails_for_wrong_key() {
        let key = test_ed25519_public_key();
        let did = generate_did_key(&key);
        let wrong_key = [99u8; 32];
        assert!(!verify_did_key(&did, &wrong_key));
    }

    #[test]
    fn verification_method_id_format() {
        let key = test_ed25519_public_key();
        let did = generate_did_key(&key);
        let vm_id = verification_method_id(&did);
        assert!(vm_id.starts_with("did:key:z"));
        assert!(vm_id.ends_with("#key-1"));
    }

    #[test]
    fn roundtrip_100_random_ed25519_keys() {
        for i in 0u8..100 {
            let mut key = [0u8; 32];
            key[0] = i;
            key[31] = 255 - i;
            let did = generate_did_key(&key);
            let resolved = resolve_did_key_ed25519(&did).unwrap();
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

        assert_eq!(did_from_module, did_from_identity);
    }

    // ─── P-256 path (Spec D L4-D6) ──────────────────────────────────

    #[test]
    fn generate_did_key_p256_format() {
        let key = test_p256_compressed_key();
        let did = generate_did_key_p256(&key);
        assert!(did.starts_with("did:key:z"));
        // P-256 multicodec produces `zDn…` prefix.
        assert!(
            did.starts_with("did:key:zDn"),
            "P-256 did:key should start with zDn, got: {did}"
        );
    }

    #[test]
    fn did_key_p256_round_trip() {
        let key = test_p256_compressed_key();
        let did = generate_did_key_p256(&key);
        let resolution = resolve_did_key(&did).unwrap();
        assert_eq!(resolution.algorithm, AuthAlg::P256);
        assert_eq!(resolution.public_key, key.to_vec());

        // Convenience wrapper
        let resolved = resolve_did_key_p256(&did).unwrap();
        assert_eq!(resolved, key);

        // verify_did_key_p256
        assert!(verify_did_key_p256(&did, &key));
    }

    #[test]
    fn p256_did_differs_per_key() {
        use crate::seed::MasterSeed;

        let key1 = MasterSeed::from_bytes([1u8; 32]).derive_p256_key();
        let key2 = MasterSeed::from_bytes([2u8; 32]).derive_p256_key();
        let id1 = crate::p256::EcdsaP256Identity::from_key_bytes(&key1).unwrap();
        let id2 = crate::p256::EcdsaP256Identity::from_key_bytes(&key2).unwrap();
        assert_ne!(id1.did_key(), id2.did_key());
    }

    #[test]
    fn resolve_p256_short_key_fails() {
        // Only 16 bytes after the multicodec prefix
        let mut bytes = vec![0x80, 0x24];
        bytes.extend_from_slice(&[0x02; 16]);
        let encoded = bs58::encode(&bytes).into_string();
        let did = format!("did:key:z{encoded}");
        let result = resolve_did_key(&did);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_p256_invalid_sec1_byte_fails() {
        // Prefix valid, length valid, but first byte is neither 0x02 nor 0x03.
        let mut bytes = vec![0x80, 0x24, 0x05];
        bytes.extend_from_slice(&[0u8; 32]);
        let encoded = bs58::encode(&bytes).into_string();
        let did = format!("did:key:z{encoded}");
        let result = resolve_did_key(&did);
        assert!(result.is_err());
    }

    #[test]
    fn p256_verify_did_key_fails_for_wrong_key() {
        let key = test_p256_compressed_key();
        let did = generate_did_key_p256(&key);
        let wrong_key = [0x02u8; 33];
        assert!(!verify_did_key_p256(&did, &wrong_key));
    }

    #[test]
    fn ed25519_and_p256_produce_distinct_dids() {
        // Even with the same byte prefix, the two curves should have
        // entirely different DID strings.
        let ed = generate_did_key(&[1u8; 32]);
        let mut p256_key = [0u8; 33];
        p256_key[0] = 0x02; // valid SEC1 byte
        p256_key[1..].copy_from_slice(&[1u8; 32]);
        let p = generate_did_key_p256(&p256_key);
        assert_ne!(ed, p);
    }
}
