//! Ed25519 identity operations — Agent Auth Protocol compatible.
//!
//! This module provides Ed25519 keypair management and JWT signing
//! compatible with the Agent Auth Protocol specification:
//!
//! - `typ: "agent+jwt"` header
//! - 60-second maximum TTL
//! - JWK thumbprint for `iss` claim (RFC 7638)
//! - Unique `jti` for replay protection

use anima_core::error::{AnimaError, AnimaResult};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Maximum JWT lifetime in seconds (Agent Auth Protocol spec).
pub const MAX_JWT_TTL_SECS: i64 = 60;

/// An Ed25519 keypair for agent authentication.
///
/// The private key is zeroized on drop.
pub struct Ed25519Identity {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
}

impl Ed25519Identity {
    /// Create from derived key bytes (from MasterSeed).
    pub fn from_key_bytes(key_bytes: &Zeroizing<[u8; 32]>) -> AnimaResult<Self> {
        let signing_key = SigningKey::from_bytes(key_bytes);
        let verifying_key = signing_key.verifying_key();

        Ok(Self {
            signing_key,
            verifying_key,
        })
    }

    /// Get the public key bytes (32 bytes).
    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.verifying_key.to_bytes().to_vec()
    }

    /// Get the public key as hex string.
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.verifying_key.to_bytes())
    }

    /// Compute the JWK thumbprint (RFC 7638) for use as JWT `iss` claim.
    ///
    /// The thumbprint is computed over the canonical JWK representation
    /// of the Ed25519 public key.
    pub fn jwk_thumbprint(&self) -> String {
        let x = URL_SAFE_NO_PAD.encode(self.verifying_key.to_bytes());

        // Canonical JWK members for OKP (RFC 7638 §3.2):
        // {"crv":"Ed25519","kty":"OKP","x":"<base64url>"}
        let canonical = format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{x}"}}"#);

        let hash = blake3::hash(canonical.as_bytes());
        URL_SAFE_NO_PAD.encode(&hash.as_bytes()[..32])
    }

    /// Generate a `did:key` identifier from the Ed25519 public key.
    ///
    /// Format: `did:key:z6Mk<base58btc-encoded-multicodec-key>`
    ///
    /// The multicodec prefix for Ed25519 public key is 0xed01.
    pub fn did_key(&self) -> String {
        let mut bytes = vec![0xed, 0x01]; // Ed25519 multicodec prefix
        bytes.extend_from_slice(&self.verifying_key.to_bytes());

        // Base58btc encoding with 'z' prefix
        let encoded = bs58::encode(&bytes).into_string();
        format!("did:key:z{encoded}")
    }

    /// Sign an arbitrary message.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.signing_key.sign(message).to_bytes().to_vec()
    }

    /// Sign an Agent Auth Protocol JWT.
    ///
    /// Creates a JWT with:
    /// - `typ`: "agent+jwt"
    /// - `alg`: "EdDSA"
    /// - `iss`: JWK thumbprint of the host's key
    /// - `sub`: agent_id
    /// - `aud`: server URL
    /// - `jti`: unique identifier (UUID v4)
    /// - `iat`: current timestamp
    /// - `exp`: iat + ttl_secs (max 60)
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
            alg: "EdDSA".into(),
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
        let sig_b64 = URL_SAFE_NO_PAD.encode(&signature);

        Ok(format!("{signing_input}.{sig_b64}"))
    }
}

/// JWT header for Agent Auth Protocol.
#[derive(Debug, Serialize, Deserialize)]
struct AgentJwtHeader {
    typ: String,
    alg: String,
}

/// JWT claims for Agent Auth Protocol.
#[derive(Debug, Serialize, Deserialize)]
struct AgentJwtClaims {
    /// Issuer — JWK thumbprint of the host's Ed25519 key.
    iss: String,
    /// Subject — the agent's unique identifier.
    sub: String,
    /// Audience — the server URL being authenticated to.
    aud: String,
    /// JWT ID — unique per request (replay protection).
    jti: String,
    /// Issued at — Unix timestamp.
    iat: i64,
    /// Expires — Unix timestamp (max iat + 60).
    exp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::MasterSeed;

    fn test_identity() -> Ed25519Identity {
        let seed = MasterSeed::from_bytes([42u8; 32]);
        let key_bytes = seed.derive_ed25519_key();
        Ed25519Identity::from_key_bytes(&key_bytes).unwrap()
    }

    #[test]
    fn public_key_is_32_bytes() {
        let id = test_identity();
        assert_eq!(id.public_key_bytes().len(), 32);
    }

    #[test]
    fn public_key_hex_is_64_chars() {
        let id = test_identity();
        assert_eq!(id.public_key_hex().len(), 64);
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
    }

    #[test]
    fn sign_and_verify_message() {
        let id = test_identity();
        let message = b"hello, anima";
        let signature = id.sign(message);

        // Verify with the public key
        let sig = ed25519_dalek::Signature::from_bytes(signature.as_slice().try_into().unwrap());
        assert!(id.verifying_key.verify_strict(message, &sig).is_ok());
    }

    #[test]
    fn agent_jwt_structure() {
        let id = test_identity();
        let jwt = id
            .sign_agent_jwt("agt_001", "https://broomva.tech", 60)
            .unwrap();

        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT must have 3 parts");

        // Decode and verify header
        let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).unwrap();
        let header: AgentJwtHeader = serde_json::from_slice(&header_bytes).unwrap();
        assert_eq!(header.typ, "agent+jwt");
        assert_eq!(header.alg, "EdDSA");

        // Decode and verify claims
        let claims_bytes = URL_SAFE_NO_PAD.decode(parts[1]).unwrap();
        let claims: AgentJwtClaims = serde_json::from_slice(&claims_bytes).unwrap();
        assert_eq!(claims.sub, "agt_001");
        assert_eq!(claims.aud, "https://broomva.tech");
        assert!(claims.exp - claims.iat <= 60);
        assert!(!claims.jti.is_empty());
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

        // Even though we requested 3600s, it should be capped at 60
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
        let sig = ed25519_dalek::Signature::from_bytes(sig_bytes.as_slice().try_into().unwrap());

        assert!(
            id.verifying_key
                .verify_strict(signing_input.as_bytes(), &sig)
                .is_ok()
        );
    }
}
