//! P-256 (ES256) identity operations — Spec D L4-D6 cutover.
//!
//! Mirrors the API surface of [`crate::ed25519::Ed25519Identity`] so the swap
//! from EdDSA to ES256 is mechanical at every call site. The wire-format
//! invariants enforced here:
//!
//! - JWS `alg = ES256` (P-256 + SHA-256)
//! - Public key serialised as SEC1 compressed (33 bytes)
//! - DID uses multicodec `0x1200` (P-256), produces `did:key:zDn…`
//! - `jwk_thumbprint` follows RFC 7638 over the canonical EC JWK
//!
//! Per Spec D L4-D6, this replaces `Ed25519Identity` as the production auth
//! identity. The Ed25519 path is kept around (in `crate::ed25519`) only for
//! verifying historical events signed before D-Sub-A — see
//! `lago_auth::jwt::detect_alg`.

use anima_core::error::{AnimaError, AnimaResult};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use p256::ecdsa::{Signature as P256Signature, SigningKey, VerifyingKey, signature::Signer};
#[allow(unused_imports)]
use p256::elliptic_curve::sec1::ToEncodedPoint;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Maximum JWT lifetime in seconds (Agent Auth Protocol spec — unchanged from
/// the Ed25519 era).
pub const MAX_JWT_TTL_SECS: i64 = 60;

/// A P-256 (secp256r1 / ES256) keypair for agent authentication.
///
/// The private scalar is held in a `SigningKey` whose backing storage is
/// zeroized on drop by the `p256` crate.
pub struct EcdsaP256Identity {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
}

impl EcdsaP256Identity {
    /// Create from derived 32-byte scalar bytes (from `MasterSeed`).
    ///
    /// The bytes are treated as a big-endian P-256 private scalar in the
    /// canonical range `[1, n-1]`. Out-of-range scalars are rejected by
    /// `p256::ecdsa::SigningKey::from_bytes`.
    pub fn from_key_bytes(key_bytes: &Zeroizing<[u8; 32]>) -> AnimaResult<Self> {
        let signing_key = SigningKey::from_bytes(key_bytes.as_ref().into())
            .map_err(|e| AnimaError::Crypto(format!("p256 from_bytes: {e}")))?;
        let verifying_key = VerifyingKey::from(&signing_key);
        Ok(Self {
            signing_key,
            verifying_key,
        })
    }

    /// Get the SEC1-compressed public key (33 bytes — `0x02|0x03` || x-coord).
    ///
    /// This is the on-wire format used in `did:key:zDn…` and in the
    /// `auth_pubkey()` trait method on `AnimaCustody`.
    pub fn public_key_bytes(&self) -> [u8; 33] {
        let point = self.verifying_key.to_encoded_point(true);
        let bytes = point.as_bytes();
        debug_assert_eq!(bytes.len(), 33, "compressed P-256 point must be 33 bytes");
        let mut out = [0u8; 33];
        out.copy_from_slice(bytes);
        out
    }

    /// Get the SEC1-uncompressed public key (65 bytes — `0x04` || x || y).
    /// Used internally for the JWK thumbprint and the `did:key` doc's
    /// `publicKeyMultibase` field on backends that prefer uncompressed.
    pub fn public_key_uncompressed(&self) -> [u8; 65] {
        let point = self.verifying_key.to_encoded_point(false);
        let bytes = point.as_bytes();
        debug_assert_eq!(bytes.len(), 65, "uncompressed P-256 point must be 65 bytes");
        let mut out = [0u8; 65];
        out.copy_from_slice(bytes);
        out
    }

    /// Hex encoding of the compressed public key (66 chars).
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.public_key_bytes())
    }

    /// Borrow the verifying key — used by tests + the lago-auth verifier.
    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.verifying_key
    }

    /// Compute the JWK thumbprint (RFC 7638) for use as JWT `kid` / `iss`.
    ///
    /// Canonical JWK members for EC P-256 (RFC 7638 §3.2):
    /// `{"crv":"P-256","kty":"EC","x":"<base64url>","y":"<base64url>"}`.
    pub fn jwk_thumbprint(&self) -> String {
        let uncompressed = self.public_key_uncompressed();
        // `0x04 || X (32) || Y (32)` — strip the leading 0x04 byte.
        let x = URL_SAFE_NO_PAD.encode(&uncompressed[1..33]);
        let y = URL_SAFE_NO_PAD.encode(&uncompressed[33..65]);
        let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);

        let hash = Sha256::digest(canonical.as_bytes());
        URL_SAFE_NO_PAD.encode(hash)
    }

    /// Generate a `did:key` identifier from the P-256 public key.
    ///
    /// Format: `did:key:zDn<base58btc-encoded-multicodec-key>`.
    /// The multicodec prefix for P-256 public key is the unsigned varint
    /// `0x80 0x24` (= 0x1200), followed by the 33-byte SEC1-compressed point.
    pub fn did_key(&self) -> String {
        crate::did::generate_did_key_p256(&self.public_key_bytes())
    }

    /// Sign an arbitrary message with the auth (P-256) key.
    /// Returns the IEEE-P1363 64-byte form (`r || s`, no recovery byte).
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        let signature: P256Signature = self.signing_key.sign(message);
        let bytes = signature.to_bytes();
        let mut out = [0u8; 64];
        out.copy_from_slice(bytes.as_slice());
        out
    }

    /// Sign a 32-byte digest directly. Used for `AnimaCustody::sign_digest`.
    pub fn sign_digest(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        // `Signer::sign` runs SHA-256 internally. For a pre-computed digest
        // we use `sign_prehash` to skip re-hashing.
        use p256::ecdsa::signature::hazmat::PrehashSigner;
        let signature: P256Signature = self
            .signing_key
            .sign_prehash(digest)
            .map_err(|e| AnimaError::Crypto(format!("p256 sign_prehash: {e}")))?;
        let bytes = signature.to_bytes();
        let mut out = [0u8; 64];
        out.copy_from_slice(bytes.as_slice());
        Ok(out)
    }

    /// Sign an Agent Auth Protocol JWT with ES256.
    ///
    /// JWT shape (Spec D L4-D6 swap):
    /// - `typ`: "agent+jwt"
    /// - `alg`: "ES256" (was "EdDSA")
    /// - `kid`: DID of the signer (`did:key:zDn…`)
    /// - `iss`: JWK thumbprint
    /// - `sub`: agent_id
    /// - `aud`: server URL
    /// - `jti`: UUID v4
    /// - `iat` / `exp`: 60-second TTL cap
    pub fn sign_agent_jwt(
        &self,
        agent_id: &str,
        audience: &str,
        ttl_secs: i64,
    ) -> AnimaResult<String> {
        let ttl = ttl_secs.min(MAX_JWT_TTL_SECS);
        let now = Utc::now().timestamp();

        let header = AgentJwtHeader {
            typ: "agent+jwt".into(),
            alg: "ES256".into(),
            kid: self.did_key(),
        };
        let claims = AgentJwtClaims {
            iss: self.jwk_thumbprint(),
            sub: agent_id.to_string(),
            aud: audience.to_string(),
            jti: Uuid::new_v4().to_string(),
            iat: now,
            exp: now + ttl,
        };

        let header_json = serde_json::to_vec(&header)
            .map_err(|e| AnimaError::Jwt(format!("header serialization: {e}")))?;
        let claims_json = serde_json::to_vec(&claims)
            .map_err(|e| AnimaError::Jwt(format!("claims serialization: {e}")))?;

        let header_b64 = URL_SAFE_NO_PAD.encode(&header_json);
        let claims_b64 = URL_SAFE_NO_PAD.encode(&claims_json);
        let signing_input = format!("{header_b64}.{claims_b64}");

        let signature = self.sign(signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(signature);

        Ok(format!("{signing_input}.{sig_b64}"))
    }

    /// Sign an arbitrary JWS body with ES256, building the header
    /// (alg=ES256, kid=DID). Used by `AnimaCustody::sign_jws`.
    pub fn sign_jws(&self, claims: &serde_json::Value) -> AnimaResult<String> {
        let header = serde_json::json!({
            "alg": "ES256",
            "typ": "JWT",
            "kid": self.did_key(),
        });
        let header_json = serde_json::to_vec(&header)
            .map_err(|e| AnimaError::Jwt(format!("header serialization: {e}")))?;
        let claims_json = serde_json::to_vec(claims)
            .map_err(|e| AnimaError::Jwt(format!("claims serialization: {e}")))?;

        let header_b64 = URL_SAFE_NO_PAD.encode(&header_json);
        let claims_b64 = URL_SAFE_NO_PAD.encode(&claims_json);
        let signing_input = format!("{header_b64}.{claims_b64}");
        let signature = self.sign(signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(signature);

        Ok(format!("{signing_input}.{sig_b64}"))
    }
}

/// Verify a JWS produced by `EcdsaP256Identity::sign_jws` against a SEC1
/// compressed public key. Returns the decoded claims body on success.
///
/// Used by:
/// - `InProcessAnima::rotate` regression test (verifies the rotation_proof_jws)
/// - lago-auth's full verifier path (resolves the DID, looks up the pubkey
///   from JWKS / journal, calls this to validate the signature)
/// - broomva.tech AAP verifier (when D-Sub-A's coordination TODO lands)
pub fn verify_jws_with_pubkey(
    jws: &str,
    pubkey_sec1_compressed: &[u8; 33],
) -> AnimaResult<serde_json::Value> {
    use p256::PublicKey;
    use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};

    let mut parts = jws.split('.');
    let header_b64 = parts
        .next()
        .ok_or_else(|| AnimaError::Jwt("jws missing header".into()))?;
    let body_b64 = parts
        .next()
        .ok_or_else(|| AnimaError::Jwt("jws missing body".into()))?;
    let sig_b64 = parts
        .next()
        .ok_or_else(|| AnimaError::Jwt("jws missing signature".into()))?;
    if parts.next().is_some() {
        return Err(AnimaError::Jwt("jws has extra dot-separated parts".into()));
    }

    // Decode signature (raw 64-byte r||s).
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|e| AnimaError::Jwt(format!("jws signature base64: {e}")))?;
    if sig_bytes.len() != 64 {
        return Err(AnimaError::Jwt(format!(
            "jws signature wrong length (expected 64, got {})",
            sig_bytes.len()
        )));
    }
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| AnimaError::Jwt(format!("jws signature parse: {e}")))?;

    // Rebuild verifying key from SEC1 compressed bytes.
    let public_key = PublicKey::from_sec1_bytes(pubkey_sec1_compressed)
        .map_err(|e| AnimaError::Crypto(format!("pubkey from_sec1_bytes: {e}")))?;
    let verifying_key = VerifyingKey::from(&public_key);

    // Verify signature over the signing input.
    let signing_input = format!("{header_b64}.{body_b64}");
    verifying_key
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|e| AnimaError::Jwt(format!("jws signature verify failed: {e}")))?;

    // Decode and return body claims.
    let body_json = URL_SAFE_NO_PAD
        .decode(body_b64)
        .map_err(|e| AnimaError::Jwt(format!("jws body base64: {e}")))?;
    serde_json::from_slice(&body_json).map_err(|e| AnimaError::Jwt(format!("jws body json: {e}")))
}

/// JWT header for Agent Auth Protocol — ES256 era.
#[derive(Debug, Serialize, Deserialize)]
struct AgentJwtHeader {
    typ: String,
    alg: String,
    kid: String,
}

/// JWT claims for Agent Auth Protocol — unchanged from EdDSA era.
#[derive(Debug, Serialize, Deserialize)]
struct AgentJwtClaims {
    iss: String,
    sub: String,
    aud: String,
    jti: String,
    iat: i64,
    exp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::MasterSeed;
    use p256::ecdsa::signature::Verifier;

    fn test_identity() -> EcdsaP256Identity {
        let seed = MasterSeed::from_bytes([42u8; 32]);
        let key_bytes = seed.derive_p256_key();
        EcdsaP256Identity::from_key_bytes(&key_bytes).unwrap()
    }

    #[test]
    fn public_key_is_33_bytes_compressed() {
        let id = test_identity();
        assert_eq!(id.public_key_bytes().len(), 33);
        // Compressed encoding starts with 0x02 or 0x03.
        let prefix = id.public_key_bytes()[0];
        assert!(prefix == 0x02 || prefix == 0x03);
    }

    #[test]
    fn public_key_uncompressed_is_65_bytes() {
        let id = test_identity();
        let p = id.public_key_uncompressed();
        assert_eq!(p.len(), 65);
        assert_eq!(p[0], 0x04);
    }

    #[test]
    fn deterministic_key_derivation() {
        let id1 = test_identity();
        let id2 = test_identity();
        assert_eq!(id1.public_key_bytes(), id2.public_key_bytes());
    }

    #[test]
    fn jwk_thumbprint_is_stable() {
        let id1 = test_identity();
        let id2 = test_identity();
        assert_eq!(id1.jwk_thumbprint(), id2.jwk_thumbprint());
        assert!(!id1.jwk_thumbprint().is_empty());
    }

    #[test]
    fn did_key_format() {
        let id = test_identity();
        let did = id.did_key();
        assert!(did.starts_with("did:key:z"));
        // P-256 multicodec produces `zDn…` prefix.
        assert!(
            did.starts_with("did:key:zDn"),
            "P-256 did:key should start with zDn, got: {did}"
        );
    }

    #[test]
    fn sign_and_verify_message() {
        let id = test_identity();
        let message = b"hello, anima -- p256 era";
        let sig_bytes = id.sign(message);
        let signature = P256Signature::from_slice(&sig_bytes).unwrap();
        id.verifying_key.verify(message, &signature).unwrap();
    }

    #[test]
    fn sign_digest_round_trip() {
        let id = test_identity();
        let digest = sha2::Sha256::digest(b"some payload");
        let mut digest_arr = [0u8; 32];
        digest_arr.copy_from_slice(&digest);
        let sig_bytes = id.sign_digest(&digest_arr).unwrap();

        // Verify with prehash (because we signed the prehash).
        use p256::ecdsa::signature::hazmat::PrehashVerifier;
        let signature = P256Signature::from_slice(&sig_bytes).unwrap();
        id.verifying_key
            .verify_prehash(&digest_arr, &signature)
            .unwrap();
    }

    #[test]
    fn agent_jwt_structure_is_es256() {
        let id = test_identity();
        let jwt = id
            .sign_agent_jwt("agt_001", "https://broomva.tech", 60)
            .unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT must have 3 parts");

        let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).unwrap();
        let header: AgentJwtHeader = serde_json::from_slice(&header_bytes).unwrap();
        assert_eq!(header.typ, "agent+jwt");
        assert_eq!(header.alg, "ES256");
        assert!(header.kid.starts_with("did:key:zDn"));

        let claims_bytes = URL_SAFE_NO_PAD.decode(parts[1]).unwrap();
        let claims: AgentJwtClaims = serde_json::from_slice(&claims_bytes).unwrap();
        assert_eq!(claims.sub, "agt_001");
        assert!(claims.exp - claims.iat <= 60);
    }

    #[test]
    fn jwt_ttl_capped_at_60_seconds() {
        let id = test_identity();
        let jwt = id
            .sign_agent_jwt("agt_001", "https://example.com", 3600)
            .unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        let claims_bytes = URL_SAFE_NO_PAD.decode(parts[1]).unwrap();
        let claims: AgentJwtClaims = serde_json::from_slice(&claims_bytes).unwrap();
        assert!(claims.exp - claims.iat <= 60);
    }

    #[test]
    fn jwt_signature_verifies() {
        let id = test_identity();
        let jwt = id
            .sign_agent_jwt("agt_001", "https://example.com", 60)
            .unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let sig_bytes = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        let signature = P256Signature::from_slice(&sig_bytes).unwrap();
        id.verifying_key
            .verify(signing_input.as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn sign_jws_round_trip() {
        let id = test_identity();
        let claims = serde_json::json!({"sub": "user1", "iss": "lifegw", "exp": 9999999999u64});
        let jws = id.sign_jws(&claims).unwrap();
        let parts: Vec<&str> = jws.split('.').collect();
        assert_eq!(parts.len(), 3);

        // Header should declare ES256
        let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).unwrap();
        let header: serde_json::Value = serde_json::from_slice(&header_bytes).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert!(header["kid"].as_str().unwrap().starts_with("did:key:zDn"));

        // Verify the signature
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let sig_bytes = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        let signature = P256Signature::from_slice(&sig_bytes).unwrap();
        id.verifying_key
            .verify(signing_input.as_bytes(), &signature)
            .unwrap();
    }
}
