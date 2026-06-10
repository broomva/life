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
    /// Inline-embeds the canonical dev ES256 keypair (the same content
    /// originally generated via `openssl` and committed to
    /// `auth/dev_keys/lifed-dev-key.{pem,pub.pem}`). The keys are
    /// intentionally NOT secret — they exist only to make the dev / CI
    /// substrate-token round-trip reproducible.
    ///
    /// **BRO-1208**: inlined as string literals instead of `include_str!`
    /// to remove the file-system dependency at compile time. Railway's
    /// Docker build context was repeatedly omitting the `.pem` files
    /// from the build (4 consecutive FAILED builds at commit
    /// `d3569bbd`+ with `include_str!` error: "couldn't read … No such
    /// file or directory"). Earlier builds at commit `ad4dc85f` worked
    /// with the same files on disk — the omission is environment-
    /// sensitive (BuildKit caching, secret-shape filtering, or
    /// context-upload race; not pinned down). Inlining makes the binary
    /// self-contained and immune to whatever upload path Railway chooses.
    ///
    /// The on-disk `.pem` files are kept for parity / regeneration
    /// reference but are no longer consumed at compile time.
    pub fn generate_dev() -> Self {
        const DEV_PRIV_PEM: &str = "\
-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgR13Ec+bww984GGn/\n\
wqqi0BtBgAHMSyXgBzvl+ptrXHShRANCAAReifOTgJ8lUHdUfirjSAfFfZv3/tU8\n\
4KQl1BTsqIGAoLum1Bvs0GVeQvWGKUESa6rlY6pAax/zTQZfJKRe2of0\n\
-----END PRIVATE KEY-----\n";
        const DEV_PUB_PEM: &str = "\
-----BEGIN PUBLIC KEY-----\n\
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEXonzk4CfJVB3VH4q40gHxX2b9/7V\n\
POCkJdQU7KiBgKC7ptQb7NBlXkL1hilBEmuq5WOqQGsf800GXySkXtqH9A==\n\
-----END PUBLIC KEY-----\n";
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
    ///
    /// Sub-phase D includes the `pem` field so substrates can verify
    /// without sharing in-memory keystore state. The `pem` form is the
    /// most direct: substrates load the PEM-encoded SPKI public key and
    /// pass it to their JWS verifier. Sub-phase E wires the alternate
    /// `x` / `y` JWK component encoding alongside `pem` for clients
    /// that prefer the canonical RFC 7517 shape.
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
    /// Sub-phase D: the PEM-encoded SPKI public key. Substrates load
    /// this directly into their JWS verifier — no shared in-memory
    /// keystore state required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pem: Option<String>,
}
