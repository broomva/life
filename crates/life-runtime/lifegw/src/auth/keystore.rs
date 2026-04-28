//! Tier-2 capability-token signing keystore.
//!
//! Sub-phase A: an in-process P-256 (ES256) keypair generated at daemon
//! startup. The same `Keystore` is held by both the Tier-2 minter (signs
//! outbound tokens) and the test harness (verifies them). Production KMS
//! providers (AWS, GCP, Vault) replace this in Sub-phase E, gated behind
//! Cargo features.
//!
//! Per Spec C₃ §5.4 LOCKED L4-D2: Tier-2 tokens are ES256 over P-256. The
//! daemon publishes the public key as a JWKS at `/.well-known/jwks.json` so
//! lifed can verify (Sub-phase D wires the publish path; A keeps the
//! keystore in-memory only and exposes it to integration tests via the
//! crate-private API).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{DecodingKey, EncodingKey};
use serde::{Deserialize, Serialize};

use crate::error::{LifegwError, LifegwResult};

/// EC keypair (P-256) plus a key id used to sign Tier-2 capability
/// tokens.
///
/// Cloning is cheap (the underlying jsonwebtoken types are
/// `Arc`-backed). The struct is `#[non_exhaustive]` so future fields
/// (creation timestamp for rotation hints, parent-key ref for
/// hierarchical KMS) can be added without breaking downstream
/// constructors — production builds construct via
/// [`Keystore::generate_dev`] only.
#[derive(Clone)]
#[non_exhaustive]
pub struct Keystore {
    pub kid: String,
    pub encoding: EncodingKey,
    pub decoding: DecodingKey,
    pub public_pem: String,
}

impl Keystore {
    /// Generate a new dev-only P-256 keypair. NOT suitable for production —
    /// production deployments use a KMS-backed signer (Sub-phase E).
    ///
    /// Uses `rcgen` under the hood (already a dev-dependency for the TLS
    /// integration test). To keep the runtime crate free of `rcgen`, we
    /// embed an alternative pure-`ring` flow: generate a P-256 keypair and
    /// emit it as PKCS#8 PEM via `ring`'s built-in serialiser.
    pub fn generate_dev() -> LifegwResult<Self> {
        // ring's deterministic-RNG-free ECDSA P-256 keygen.
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &rng,
        )
        .map_err(|e| LifegwError::Auth(format!("generate ec keypair: {e}")))?;
        let pkcs8_bytes = pkcs8.as_ref();

        // Wrap the DER blob into a PEM string for jsonwebtoken's PEM loader.
        let priv_pem = der_to_pem("PRIVATE KEY", pkcs8_bytes);
        let encoding = EncodingKey::from_ec_pem(priv_pem.as_bytes())
            .map_err(|e| LifegwError::Auth(format!("encode priv pem: {e}")))?;

        // Extract the SubjectPublicKeyInfo from the PKCS#8 blob to give
        // verifiers a separate public-key handle.
        let kp = ring::signature::EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8_bytes,
            &rng,
        )
        .map_err(|e| LifegwError::Auth(format!("parse pkcs8: {e}")))?;
        // ring's `public_key()` returns the uncompressed SEC1 representation
        // (0x04 || X || Y, 65 bytes). Wrap it in a SubjectPublicKeyInfo so
        // jsonwebtoken accepts it as a P-256 PEM.
        let raw_pub = ring::signature::KeyPair::public_key(&kp).as_ref().to_vec();
        let spki_der = sec1_uncompressed_to_spki(&raw_pub);
        let pub_pem = der_to_pem("PUBLIC KEY", &spki_der);
        let decoding = DecodingKey::from_ec_pem(pub_pem.as_bytes())
            .map_err(|e| LifegwError::Auth(format!("decode pub pem: {e}")))?;

        // 64-bit random kid so dev rotations show up distinct in logs.
        let mut kid_bytes = [0u8; 8];
        ring::rand::SecureRandom::fill(&rng, &mut kid_bytes)
            .map_err(|e| LifegwError::Auth(format!("rand kid: {e}")))?;
        let kid = format!("dev-{}", URL_SAFE_NO_PAD.encode(kid_bytes));

        Ok(Self {
            kid,
            encoding,
            decoding,
            public_pem: pub_pem,
        })
    }

    /// Public key in PEM form. Sub-phase D writes this to disk so lifed (the
    /// downstream verifier) can read it from a known path.
    pub fn public_key_pem(&self) -> String {
        self.public_pem.clone()
    }

    /// JWKS metadata published at `/.well-known/jwks.json` (Sub-phase D).
    pub fn publish_jwks(&self) -> Jwks {
        Jwks {
            keys: vec![JwksKey {
                kid: self.kid.clone(),
                kty: "EC".to_string(),
                crv: "P-256".to_string(),
                alg: "ES256".to_string(),
                use_: "sig".to_string(),
                pem: Some(self.public_pem.clone()),
            }],
        }
    }
}

/// Top-level JWKS document the gateway publishes. Sub-phase B writes
/// a single key entry containing the active Tier-2 signing key;
/// Sub-phase D may add retired-in-grace keys for rotation overlap.
#[derive(Serialize, Deserialize, Clone)]
#[non_exhaustive]
pub struct Jwks {
    pub keys: Vec<JwksKey>,
}

/// A single JWKS key entry. RFC 7517 § 4 fields plus an optional `pem`
/// convenience field used by lifed's reader to skip the x/y → key
/// reconstruction.
#[derive(Serialize, Deserialize, Clone)]
#[non_exhaustive]
pub struct JwksKey {
    pub kid: String,
    pub kty: String,
    pub crv: String,
    pub alg: String,
    #[serde(rename = "use")]
    pub use_: String,
    /// Optional PEM-encoded public key. Sub-phase A writes this so
    /// consumers (lifed) can decode the key without parsing JWK x/y
    /// components. Sub-phase D will additionally publish x/y for
    /// browser-side verifiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pem: Option<String>,
}

fn der_to_pem(label: &str, der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut pem = format!("-----BEGIN {label}-----\n");
    // `b64` is pure ASCII (base64 standard alphabet), so 64-byte chunks of
    // the underlying `&str` are always valid UTF-8 sub-slices. `from_utf8`
    // returns `Ok` for every chunk; we still surface a fallback rather than
    // `.expect()` so the production path has no `panic!`-equivalent calls.
    for chunk in b64.as_bytes().chunks(64) {
        match std::str::from_utf8(chunk) {
            Ok(line) => {
                pem.push_str(line);
                pem.push('\n');
            }
            // Mathematically unreachable; included only to keep production
            // free of `expect`/`unwrap`-style panics.
            Err(_) => {
                let lossy = String::from_utf8_lossy(chunk);
                pem.push_str(&lossy);
                pem.push('\n');
            }
        }
    }
    pem.push_str(&format!("-----END {label}-----\n"));
    pem
}

/// Wrap a raw SEC1-uncompressed P-256 public key (65 bytes: 0x04 || X || Y)
/// in a SubjectPublicKeyInfo DER structure compatible with PEM
/// `-----BEGIN PUBLIC KEY-----` blocks. Hand-rolled because the workspace
/// dependency budget can't justify a full ASN.1 crate for this one shape.
fn sec1_uncompressed_to_spki(pubkey: &[u8]) -> Vec<u8> {
    // SubjectPublicKeyInfo for an EC P-256 key — the prefix is fixed and
    // exactly 26 bytes (handles AlgorithmIdentifier + BIT STRING wrapper).
    // The trailing 65 bytes are the SEC1 uncompressed point.
    debug_assert_eq!(pubkey.len(), 65, "sec1 uncompressed must be 65 bytes");
    const SPKI_PREFIX: &[u8] = &[
        0x30, 0x59, // SEQUENCE, length 89
        0x30, 0x13, // SEQUENCE, length 19 (AlgorithmIdentifier)
        0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02,
        0x01, // OID 1.2.840.10045.2.1 (id-ecPublicKey)
        0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01,
        0x07, // OID 1.2.840.10045.3.1.7 (P-256)
        0x03, 0x42, // BIT STRING, length 66
        0x00, // unused-bits prefix
    ];
    let mut spki = Vec::with_capacity(SPKI_PREFIX.len() + pubkey.len());
    spki.extend_from_slice(SPKI_PREFIX);
    spki.extend_from_slice(pubkey);
    spki
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Algorithm, Header, Validation, decode, decode_header, encode};

    #[test]
    fn keystore_generate_dev_round_trip() {
        let ks = Keystore::generate_dev().expect("generate dev keystore");
        assert!(!ks.kid.is_empty());
        assert!(ks.public_pem.contains("BEGIN PUBLIC KEY"));

        // Smoke test: sign a JWS, decode the header, verify with the same keystore.
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Probe {
            sub: String,
            aud: String,
            iss: String,
            exp: u64,
        }
        let claims = Probe {
            sub: "user-1".to_string(),
            aud: "lifed".to_string(),
            iss: "lifegw".to_string(),
            exp: (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs())
                + 60,
        };
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(ks.kid.clone());
        let token = encode(&header, &claims, &ks.encoding).expect("encode");

        let parsed_header = decode_header(&token).expect("decode_header");
        assert_eq!(parsed_header.alg, Algorithm::ES256);
        assert_eq!(parsed_header.kid.as_deref(), Some(ks.kid.as_str()));

        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["lifed"]);
        v.set_issuer(&["lifegw"]);
        let decoded = decode::<Probe>(&token, &ks.decoding, &v).expect("verify");
        assert_eq!(decoded.claims.sub, "user-1");
    }

    #[test]
    fn publish_jwks_includes_pem() {
        let ks = Keystore::generate_dev().expect("generate dev keystore");
        let jwks = ks.publish_jwks();
        assert_eq!(jwks.keys.len(), 1);
        assert_eq!(jwks.keys[0].alg, "ES256");
        assert_eq!(jwks.keys[0].crv, "P-256");
        assert!(jwks.keys[0].pem.is_some());
    }
}
