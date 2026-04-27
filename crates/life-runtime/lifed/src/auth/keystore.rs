//! Substrate-token signing keystore.
//!
//! Sub-phase B ships a deterministic dev keypair embedded in the binary;
//! production deployments load a PEM-encoded EC key from
//! `cfg.auth.substrate_signing_key_path` via [`Keystore::load_from_files`].
//!
//! Per Spec C₂ §5.2, lifed mints Tier-3 substrate-derived tokens (ES256)
//! with audience-scoped per-substrate claims. Substrates verify using the
//! published JWKS (see [`Keystore::publish_jwks`]).

use jsonwebtoken::{DecodingKey, EncodingKey};
use serde::{Deserialize, Serialize};

use crate::error::{LifedError, LifedResult};

/// EC keypair plus a key id used to sign Tier-3 substrate-tokens.
///
/// Cloning is cheap (the underlying jsonwebtoken types are `Arc`-backed).
#[derive(Clone)]
pub struct Keystore {
    pub kid: String,
    pub encoding: EncodingKey,
    pub decoding: DecodingKey,
    pub public_pem: String,
}

impl Keystore {
    /// Generate a deterministic dev keypair. NOT for production.
    ///
    /// Embeds the committed `auth/dev_keys/lifed-dev-key.{pem,pub.pem}`
    /// generated via openssl. The keys are intentionally not secret —
    /// they exist only to make the dev / CI substrate-token round-trip
    /// reproducible.
    pub fn generate_dev() -> Self {
        const DEV_PRIV_PEM: &str = include_str!("dev_keys/lifed-dev-key.pem");
        const DEV_PUB_PEM: &str = include_str!("dev_keys/lifed-dev-key.pub.pem");
        let encoding = EncodingKey::from_ec_pem(DEV_PRIV_PEM.as_bytes()).expect("dev key");
        let decoding = DecodingKey::from_ec_pem(DEV_PUB_PEM.as_bytes()).expect("dev pub");
        Self {
            kid: "dev-1".to_string(),
            encoding,
            decoding,
            public_pem: DEV_PUB_PEM.to_string(),
        }
    }

    /// Load a PEM-encoded EC keypair from disk for production use.
    pub fn load_from_files(
        priv_path: &std::path::Path,
        pub_path: &std::path::Path,
    ) -> LifedResult<Self> {
        let priv_pem = std::fs::read_to_string(priv_path)
            .map_err(|e| LifedError::Auth(format!("read {}: {e}", priv_path.display())))?;
        let pub_pem = std::fs::read_to_string(pub_path)
            .map_err(|e| LifedError::Auth(format!("read {}: {e}", pub_path.display())))?;
        let encoding = EncodingKey::from_ec_pem(priv_pem.as_bytes())
            .map_err(|e| LifedError::Auth(format!("parse priv: {e}")))?;
        let decoding = DecodingKey::from_ec_pem(pub_pem.as_bytes())
            .map_err(|e| LifedError::Auth(format!("parse pub: {e}")))?;
        Ok(Self {
            kid: "lifed-1".to_string(),
            encoding,
            decoding,
            public_pem: pub_pem,
        })
    }

    /// Public key in PEM form — substrates verify against this directly
    /// during conformance testing (see `lifed-conformance`).
    pub fn public_key_pem(&self) -> String {
        self.public_pem.clone()
    }

    /// JWKS metadata published at `cfg.auth.substrate_jwks_publish_path`.
    /// Substrates poll the file (Spec C₂ §5.2) for verification keys.
    pub fn publish_jwks(&self) -> Jwks {
        Jwks {
            keys: vec![JwksKey {
                kid: self.kid.clone(),
                kty: "EC".to_string(),
                crv: "P-256".to_string(),
                alg: "ES256".to_string(),
                use_: "sig".to_string(),
            }],
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Jwks {
    pub keys: Vec<JwksKey>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct JwksKey {
    pub kid: String,
    pub kty: String,
    pub crv: String,
    pub alg: String,
    #[serde(rename = "use")]
    pub use_: String,
}
