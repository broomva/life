//! KMS provider abstraction for Tier-2 capability-token signing.
//!
//! Per Spec C₃ §5.4 LOCKED L4-D2: Tier-2 tokens are ES256 over P-256.
//! The signing key MUST live behind a KMS so the gateway never holds
//! private key material in process memory beyond what's needed to
//! complete an in-flight signature operation.
//!
//! Sub-phase B introduces the [`KmsSigner`] trait — the seam between
//! the in-process dev keystore (used in CI / unit tests) and the
//! production providers. The auth `Layer` and `Tier2Minter` consume
//! a `dyn KmsSigner` so swapping providers requires no call-site changes.
//!
//! ## Providers
//!
//! | Provider | Feature gate | Status (Sub-phase B) |
//! |---|---|---|
//! | [`StaticKeystore`] | none (always available; gated to dev / tests) | wired |
//! | [`VaultTransit`] | `kms-vault` (default for production) | skeleton — connects, signs over HTTP, JWKS publish |
//! | `AwsKms` | `kms-aws` | skeleton (Sub-phase E completes) |
//! | `GcpKms` | `kms-gcp` | skeleton (Sub-phase E completes) |
//!
//! Production daemons enable `kms-vault` (or another KMS feature) and
//! the auth `Layer` resolves the trait object from configuration.

use std::sync::Arc;

use jsonwebtoken::{Algorithm, Header, encode};

use crate::auth::keystore::{Jwks, JwksKey, Keystore};
use crate::error::{LifegwError, LifegwResult};

/// Trait implemented by every Tier-2 capability-token signer.
///
/// The trait is deliberately narrow: a signer produces a JWS-encoded
/// token over the supplied claims and exposes the public key material
/// the gateway publishes via JWKS so downstream verifiers (lifed) can
/// validate. Key rotation is a higher-level concern — Sub-phase B
/// publishes both the current and the previously-current key to the
/// JWKS file, keeping retired keys for the duration of the rotation
/// grace.
///
/// Implementations MUST:
/// - sign over ES256 (P-256). Other algorithms are out of scope.
/// - return a kid that uniquely identifies the active key.
/// - be `Send + Sync + 'static` so the auth Layer can hold it via
///   `Arc<dyn KmsSigner>`.
pub trait KmsSigner: Send + Sync + 'static {
    /// Stable identifier of the currently-active signing key.
    fn active_kid(&self) -> &str;

    /// Sign a JWS body using the active key. Implementations build the
    /// JWS header (alg=ES256, kid=active_kid) and return the compact
    /// JWS string.
    fn sign_jws(&self, claims_json: &serde_json::Value) -> LifegwResult<String>;

    /// JWKS document containing all keys (current + retired-in-grace)
    /// that downstream verifiers should trust. The current key MUST be
    /// listed first.
    fn publish_jwks(&self) -> Jwks;
}

/// In-process P-256 keystore signer. Not for production — gated to
/// dev / CI builds. Production daemons set the [`KmsProvider`] config
/// field to `Vault` (or another KMS) and skip this entirely.
///
/// Wraps the existing [`Keystore`] for backwards compatibility with
/// Sub-phase A tests. Sub-phase B's dev path constructs one via
/// [`StaticKeystore::generate_dev`] and the conformance test rig uses it.
pub struct StaticKeystore {
    keystore: Keystore,
}

impl StaticKeystore {
    /// Generate a fresh dev keypair. NOT for production.
    pub fn generate_dev() -> LifegwResult<Self> {
        Ok(Self {
            keystore: Keystore::generate_dev()?,
        })
    }

    /// Construct from a pre-generated [`Keystore`]. Used by integration
    /// tests that need the verifier to hold the same keystore as the
    /// signer.
    pub fn from_keystore(keystore: Keystore) -> Self {
        Self { keystore }
    }

    /// Borrow the underlying keystore — used by tests + the conformance
    /// rig.
    pub fn inner(&self) -> &Keystore {
        &self.keystore
    }
}

impl KmsSigner for StaticKeystore {
    fn active_kid(&self) -> &str {
        &self.keystore.kid
    }

    fn sign_jws(&self, claims_json: &serde_json::Value) -> LifegwResult<String> {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.keystore.kid.clone());
        encode(&header, claims_json, &self.keystore.encoding)
            .map_err(|e| LifegwError::Auth(format!("encode tier-2: {e}")))
    }

    fn publish_jwks(&self) -> Jwks {
        self.keystore.publish_jwks()
    }
}

/// HashiCorp Vault Transit signer (primary production provider).
/// Reaches Vault over HTTP, requests a sign operation against a named
/// transit key, and serialises the result as a JWS.
///
/// Sub-phase B ships a working but minimal implementation: it connects
/// to Vault, signs, and publishes the public key as JWKS. Production
/// hardening (token renewal, mTLS to Vault, key-version pinning) lands
/// in Sub-phase E.
///
/// # Configuration
/// - `addr` — Vault HTTP base URL, e.g. `https://vault.internal:8200`
/// - `token` — Vault token with `transit/sign/<key>` capability
/// - `key_name` — transit key name (the gateway never sees the private
///   half; Vault holds it)
/// - `kid` — JWS kid value embedded in the header. Convention: same
///   string as `key_name`.
#[cfg(feature = "kms-vault")]
pub struct VaultTransit {
    addr: String,
    token: String,
    key_name: String,
    kid: String,
    /// Cached public key in PEM form, fetched lazily on first
    /// `publish_jwks` call.
    public_pem: std::sync::OnceLock<String>,
    client: reqwest::blocking::Client,
}

#[cfg(feature = "kms-vault")]
impl VaultTransit {
    pub fn new(
        addr: impl Into<String>,
        token: impl Into<String>,
        key_name: impl Into<String>,
        kid: impl Into<String>,
    ) -> LifegwResult<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| LifegwError::Auth(format!("vault client: {e}")))?;
        Ok(Self {
            addr: addr.into(),
            token: token.into(),
            key_name: key_name.into(),
            kid: kid.into(),
            public_pem: std::sync::OnceLock::new(),
            client,
        })
    }

    fn public_key_pem(&self) -> LifegwResult<String> {
        if let Some(pem) = self.public_pem.get() {
            return Ok(pem.clone());
        }
        let url = format!("{}/v1/transit/keys/{}", self.addr, self.key_name);
        let body: serde_json::Value = self
            .client
            .get(&url)
            .header("X-Vault-Token", &self.token)
            .send()
            .map_err(|e| LifegwError::Auth(format!("vault get key: {e}")))?
            .json()
            .map_err(|e| LifegwError::Auth(format!("vault parse key: {e}")))?;
        // Vault transit returns key versions under `data.keys.{n}.public_key`.
        let pem = body
            .pointer("/data/keys/1/public_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| LifegwError::Auth("vault: missing public_key in response".to_string()))?
            .to_string();
        let _ = self.public_pem.set(pem.clone());
        Ok(pem)
    }
}

#[cfg(feature = "kms-vault")]
impl KmsSigner for VaultTransit {
    fn active_kid(&self) -> &str {
        &self.kid
    }

    fn sign_jws(&self, claims_json: &serde_json::Value) -> LifegwResult<String> {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;

        // Build the JWS header + body (base64url-encoded, joined by ".")
        // and ask Vault to sign the resulting "<header>.<body>" string.
        let header = serde_json::json!({
            "alg": "ES256",
            "typ": "JWT",
            "kid": self.kid,
        });
        let header_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&header)
                .map_err(|e| LifegwError::Auth(format!("encode header: {e}")))?,
        );
        let body_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(claims_json)
                .map_err(|e| LifegwError::Auth(format!("encode body: {e}")))?,
        );
        let signing_input = format!("{header_b64}.{body_b64}");

        let url = format!("{}/v1/transit/sign/{}", self.addr, self.key_name);
        let payload = serde_json::json!({
            "input": URL_SAFE_NO_PAD.encode(signing_input.as_bytes()),
            "marshaling_algorithm": "jws",
            "hash_algorithm": "sha2-256",
        });
        let resp: serde_json::Value = self
            .client
            .post(&url)
            .header("X-Vault-Token", &self.token)
            .json(&payload)
            .send()
            .map_err(|e| LifegwError::Auth(format!("vault sign: {e}")))?
            .json()
            .map_err(|e| LifegwError::Auth(format!("vault parse sign resp: {e}")))?;
        let sig = resp
            .pointer("/data/signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| LifegwError::Auth("vault: missing signature".to_string()))?;
        // Vault wraps signatures as `vault:vN:<b64>`; strip the prefix.
        let sig_b64 = sig.rsplit(':').next().unwrap_or(sig);
        Ok(format!("{signing_input}.{sig_b64}"))
    }

    fn publish_jwks(&self) -> Jwks {
        match self.public_key_pem() {
            Ok(pem) => Jwks {
                keys: vec![JwksKey {
                    kid: self.kid.clone(),
                    kty: "EC".to_string(),
                    crv: "P-256".to_string(),
                    alg: "ES256".to_string(),
                    use_: "sig".to_string(),
                    pem: Some(pem),
                }],
            },
            // We can't fail this trait method; surface an empty JWKS so
            // verifiers reject (better than panicking).
            Err(_) => Jwks { keys: vec![] },
        }
    }
}

/// AWS KMS signer skeleton — feature-gated. Sub-phase E fills in the
/// real AWS SDK calls; Sub-phase B carries only the type so the trait
/// object resolution + config plumbing compiles.
#[cfg(feature = "kms-aws")]
pub struct AwsKms {
    /// AWS KMS key id / arn.
    pub key_id: String,
    /// Stable JWS kid.
    pub kid: String,
}

#[cfg(feature = "kms-aws")]
impl AwsKms {
    pub fn new(key_id: impl Into<String>, kid: impl Into<String>) -> Self {
        Self {
            key_id: key_id.into(),
            kid: kid.into(),
        }
    }
}

#[cfg(feature = "kms-aws")]
impl KmsSigner for AwsKms {
    fn active_kid(&self) -> &str {
        &self.kid
    }

    fn sign_jws(&self, _claims_json: &serde_json::Value) -> LifegwResult<String> {
        Err(LifegwError::Auth(
            "kms-aws provider not yet implemented (Sub-phase E)".to_string(),
        ))
    }

    fn publish_jwks(&self) -> Jwks {
        Jwks { keys: vec![] }
    }
}

/// GCP Cloud KMS signer skeleton — feature-gated. Sub-phase E fills in
/// the real GCP client; Sub-phase B carries only the type so the
/// configuration enum is exhaustive.
#[cfg(feature = "kms-gcp")]
pub struct GcpKms {
    /// GCP KMS resource name (`projects/.../keyRings/.../cryptoKeys/...`).
    pub resource: String,
    /// Stable JWS kid.
    pub kid: String,
}

#[cfg(feature = "kms-gcp")]
impl GcpKms {
    pub fn new(resource: impl Into<String>, kid: impl Into<String>) -> Self {
        Self {
            resource: resource.into(),
            kid: kid.into(),
        }
    }
}

#[cfg(feature = "kms-gcp")]
impl KmsSigner for GcpKms {
    fn active_kid(&self) -> &str {
        &self.kid
    }

    fn sign_jws(&self, _claims_json: &serde_json::Value) -> LifegwResult<String> {
        Err(LifegwError::Auth(
            "kms-gcp provider not yet implemented (Sub-phase E)".to_string(),
        ))
    }

    fn publish_jwks(&self) -> Jwks {
        Jwks { keys: vec![] }
    }
}

/// Convenience: the default dev signer used when no KMS provider is
/// configured. Wraps a freshly-generated [`StaticKeystore`].
pub fn default_dev_signer() -> LifegwResult<Arc<dyn KmsSigner>> {
    Ok(Arc::new(StaticKeystore::generate_dev()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Validation, decode, decode_header};
    use serde_json::json;

    #[test]
    fn static_keystore_signs_and_publishes_jwks() {
        let signer = StaticKeystore::generate_dev().expect("dev keystore");
        let kid = signer.active_kid().to_string();
        assert!(!kid.is_empty());

        let claims = json!({
            "sub": "user-1",
            "aud": "lifed",
            "iss": "lifegw",
            "exp": 9999999999u64,
        });
        let jws = signer.sign_jws(&claims).expect("sign");
        let header = decode_header(&jws).expect("decode header");
        assert_eq!(header.alg, Algorithm::ES256);
        assert_eq!(header.kid.as_deref(), Some(kid.as_str()));

        let jwks = signer.publish_jwks();
        assert_eq!(jwks.keys.len(), 1);
        assert_eq!(jwks.keys[0].kid, kid);
        assert_eq!(jwks.keys[0].alg, "ES256");

        // Verify round-trip — the published JWKS must verify the signed
        // token.
        let pem = jwks.keys[0]
            .pem
            .as_ref()
            .expect("dev publish includes pem")
            .as_bytes();
        let dk = jsonwebtoken::DecodingKey::from_ec_pem(pem).expect("decode pem");
        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["lifed"]);
        v.set_issuer(&["lifegw"]);
        let body: serde_json::Value = decode(&jws, &dk, &v).expect("verify").claims;
        assert_eq!(body["sub"], json!("user-1"));
    }

    #[test]
    fn default_dev_signer_round_trips() {
        let signer = default_dev_signer().expect("default dev signer");
        let claims = json!({
            "sub": "u",
            "aud": "lifed",
            "iss": "lifegw",
            "exp": 9999999999u64,
        });
        let jws = signer.sign_jws(&claims).expect("sign");
        let header = decode_header(&jws).expect("decode header");
        assert_eq!(header.alg, Algorithm::ES256);
        assert_eq!(header.kid.as_deref(), Some(signer.active_kid()));
    }
}
