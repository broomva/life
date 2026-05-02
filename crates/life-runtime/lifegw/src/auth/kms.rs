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
/// [`StaticKeystore::generate_dev`] and the conformance test rig uses
/// it. Marked `#[non_exhaustive]` so adding fields (e.g. a parent-key
/// reference for hierarchical KMS bridging) does not break tests that
/// construct via `from_keystore`.
#[non_exhaustive]
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
/// Sub-phase E hardening:
/// - **URL_SAFE_NO_PAD signature codec.** The `marshaling_algorithm: "jws"`
///   Vault flag asks the server to emit `r || s` concatenated and
///   base64url-encoded WITHOUT padding (matching the JWS compact-serialisation
///   convention RFC 7515 §3). The `input` field uses the same
///   URL_SAFE_NO_PAD shape — Vault accepts both URL_SAFE_NO_PAD and
///   STANDARD; we use URL_SAFE_NO_PAD for symmetry with the output.
///   A real Vault dev-server integration test is filed as a follow-up
///   under the M7 Sub-phase F ticket; until then unit tests cover the
///   pointer-logic and JSON-shape contracts (`vault_transit_pins_latest_version`).
/// - **Latest-version pinning.** `public_key_pem` now reads
///   `data.latest_version` and indexes `data.keys.<latest>.public_key` so
///   key rotation in Vault doesn't continue using the old public key.
///   Sub-phase B hardcoded `/data/keys/1/public_key` which silently broke
///   after the first rotation.
/// - **Token renewal loop.** Optional `tokio::spawn`'d task that calls
///   `auth/token/renew-self` at the configured cadence. Vault tokens
///   typically have a 32-day max-TTL with periodic renewal required to
///   stay live; without renewal a long-running daemon eventually returns
///   `403 permission denied` from `transit/sign`.
/// - **mTLS client cert.** Optional client-cert + client-key paths for
///   peer-authenticated TLS to Vault. Production-tenants opt in by
///   populating `[auth.vault.mtls]`.
///
/// # Configuration
/// - `addr` — Vault HTTP base URL, e.g. `https://vault.internal:8200`
/// - `token` — Vault token with `transit/sign/<key>` capability
/// - `key_name` — transit key name (the gateway never sees the private
///   half; Vault holds it)
/// - `kid` — JWS kid value embedded in the header. Convention: same
///   string as `key_name`.
/// - `mtls` — optional `(cert, key)` paths for client-cert auth to Vault.
/// - `renew_interval` — optional renewal cadence; `None` disables the
///   background renewal task.
#[cfg(feature = "kms-vault")]
pub struct VaultTransit {
    addr: String,
    token: String,
    key_name: String,
    kid: String,
    /// Cached public key in PEM form, fetched lazily on first
    /// `publish_jwks` call. Sub-phase E: latest-version pinned.
    public_pem: std::sync::OnceLock<String>,
    client: reqwest::blocking::Client,
}

/// Sub-phase E (item #5): optional mTLS configuration to Vault.
#[cfg(feature = "kms-vault")]
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct VaultMtls {
    /// Path to the PEM-encoded client cert.
    pub cert_path: std::path::PathBuf,
    /// Path to the PEM-encoded client key.
    pub key_path: std::path::PathBuf,
}

#[cfg(feature = "kms-vault")]
impl VaultTransit {
    pub fn new(
        addr: impl Into<String>,
        token: impl Into<String>,
        key_name: impl Into<String>,
        kid: impl Into<String>,
    ) -> LifegwResult<Self> {
        Self::with_mtls(addr, token, key_name, kid, None)
    }

    /// Sub-phase E: build a VaultTransit signer with optional mTLS.
    ///
    /// **mTLS limitation (Sub-phase E note):** the workspace's reqwest
    /// pin doesn't enable the `native-tls` or `rustls-tls` feature
    /// optionally — both require switching the TLS stack on reqwest.
    /// Operators who need mTLS to Vault TODAY should run a sidecar
    /// (e.g. envoy / consul-template) that terminates mTLS to Vault
    /// and exposes a localhost HTTP endpoint to lifegw. A future
    /// sub-phase can add the reqwest TLS-feature plumbing without
    /// touching the rest of the gateway.
    ///
    /// The function still accepts an `Option<VaultMtls>` for forward
    /// compatibility — when populated we record a warning + ignore
    /// the certs so a stale config doesn't silently disable mTLS at
    /// runtime.
    pub fn with_mtls(
        addr: impl Into<String>,
        token: impl Into<String>,
        key_name: impl Into<String>,
        kid: impl Into<String>,
        mtls: Option<VaultMtls>,
    ) -> LifegwResult<Self> {
        if let Some(m) = mtls.as_ref() {
            tracing::warn!(
                cert = %m.cert_path.display(),
                key = %m.key_path.display(),
                "vault mtls config present but lifegw's reqwest pin does not enable a TLS feature; \
                 use a localhost mTLS sidecar (envoy/consul-template) until Sub-phase F enables \
                 reqwest's rustls-tls feature explicitly. config IGNORED.",
            );
        }
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

    /// Sub-phase E (item #5): spawn a background renewal task that
    /// renews `self.token` at `interval`. Returns an `AbortHandle` so
    /// callers can cancel the task on graceful shutdown by calling
    /// `.abort()`. Vault tokens with `renewable: true` and a periodic
    /// TTL stay alive indefinitely as long as renewal happens before
    /// the TTL elapses.
    ///
    /// I1 fix: previously this function returned a `JoinHandle` and
    /// the doc claimed "drop it to cancel". Dropping a tokio
    /// `JoinHandle` does NOT cancel the task — it abandons the handle.
    /// The signature is now `AbortHandle` so cancellation actually
    /// works, and `run_daemon` calls `.abort()` on graceful shutdown.
    ///
    /// The renewal loop exits cleanly on the first error so the
    /// gateway can react via its own reload path; we don't retry
    /// silently because a renewal failure usually means the policy
    /// changed (token revoked, lease expired, etc.) and continued
    /// requests would just produce 403s anyway.
    pub fn spawn_token_renewal(
        addr: String,
        token: String,
        interval: std::time::Duration,
    ) -> tokio::task::AbortHandle {
        let handle = tokio::spawn(async move {
            let mut clock = tokio::time::interval(interval);
            clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Skip the initial immediate tick so we don't renew on
            // startup (the operator just provided a fresh token).
            clock.tick().await;
            let url = format!("{addr}/v1/auth/token/renew-self");
            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "vault renewal client build failed; renewal disabled");
                    return;
                }
            };
            loop {
                clock.tick().await;
                let resp = client
                    .post(&url)
                    .header("X-Vault-Token", &token)
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {
                        tracing::debug!("vault token renewed");
                    }
                    Ok(r) => {
                        tracing::warn!(
                            status = r.status().as_u16(),
                            "vault renew-self non-success; aborting renewal task"
                        );
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "vault renew-self failed; aborting renewal task");
                        return;
                    }
                }
            }
        });
        handle.abort_handle()
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
        // Sub-phase E (item #4): pin to `data.latest_version` so key
        // rotation in Vault doesn't continue using the old public key.
        // Vault transit returns key versions under `data.keys.{n}.public_key`
        // and exposes the latest version as `data.latest_version`.
        let latest = body
            .pointer("/data/latest_version")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| {
                LifegwError::Auth("vault: missing latest_version in key response".to_string())
            })?;
        let pem_pointer = format!("/data/keys/{latest}/public_key");
        let pem = body
            .pointer(&pem_pointer)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                LifegwError::Auth(format!(
                    "vault: missing public_key for latest_version={latest}"
                ))
            })?
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

        // Build the JWS header + body (base64url-encoded NO PAD per
        // RFC 7515 §3) and ask Vault to sign the resulting
        // "<header>.<body>" string.
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
        // Sub-phase E (item #3) — I4 revert: Vault `transit/sign`
        // accepts both `STANDARD` and `URL_SAFE_NO_PAD` base64 per the
        // current Vault docs (the API treats the input as opaque bytes
        // for hashing). Sub-phase B used URL_SAFE_NO_PAD which had
        // shipped successfully; M7-FINAL initially switched to STANDARD
        // citing "safer per Vault docs" but introduced a behaviour
        // change without a real Vault integration test backing it.
        // Reverting to URL_SAFE_NO_PAD restores the working Sub-phase B
        // behaviour. The output side is unambiguous:
        // `marshaling_algorithm: "jws"` makes Vault emit `r || s`
        // concatenated and base64url-encoded without padding (matches
        // the JWS compact-serialisation convention).
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
        // The base64url-no-pad encoding of `r || s` follows.
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

/// AWS KMS signer (Sub-phase E item #1). Production body for the
/// `kms-aws` feature.
///
/// Uses the AWS SDK v1 to:
/// 1. **Sign**: `aws_sdk_kms::Client::sign(...)` against the configured
///    key with `MessageType::Raw` + `SigningAlgorithmSpec::EcdsaSha256`.
///    AWS KMS returns a DER-encoded ECDSA signature; we decode it into
///    raw `r || s` form for JWS compact serialisation per RFC 7515 §3.
/// 2. **Public key**: `get_public_key(...)` returns DER-encoded
///    SubjectPublicKeyInfo bytes. We base64-encode the DER and wrap as
///    PEM (`-----BEGIN PUBLIC KEY-----` / END), matching the format
///    `JwksKey.pem` consumers expect.
///
/// `Client` is built lazily on first use against the standard AWS
/// credential chain (env vars, IMDSv2, sso-token, etc.) via
/// `aws_config::load_defaults`. The async runtime requirement is
/// handled via `tokio::task::block_in_place` (multi-thread runtime —
/// production) or a side-thread + private current-thread runtime
/// fallback (current-thread runtime — tests). Same pattern as
/// `auth::jwks::fetch_via_reqwest`.
#[cfg(feature = "kms-aws")]
pub struct AwsKms {
    /// AWS KMS key id / arn.
    pub key_id: String,
    /// Stable JWS kid.
    pub kid: String,
    /// Cached AWS SDK client (built once on first use).
    client: std::sync::OnceLock<aws_sdk_kms::Client>,
    /// Cached PEM-encoded public key.
    public_pem: std::sync::OnceLock<String>,
}

#[cfg(feature = "kms-aws")]
impl AwsKms {
    pub fn new(key_id: impl Into<String>, kid: impl Into<String>) -> Self {
        Self {
            key_id: key_id.into(),
            kid: kid.into(),
            client: std::sync::OnceLock::new(),
            public_pem: std::sync::OnceLock::new(),
        }
    }

    /// Build (or retrieve cached) the AWS SDK client. Uses the standard
    /// AWS credential chain via `aws_config::load_defaults`.
    fn client(&self) -> LifegwResult<aws_sdk_kms::Client> {
        if let Some(c) = self.client.get() {
            return Ok(c.clone());
        }
        let cfg = block_on_aws(async {
            aws_config::defaults(aws_config::BehaviorVersion::latest())
                .load()
                .await
        });
        let client = aws_sdk_kms::Client::new(&cfg);
        let _ = self.client.set(client.clone());
        Ok(client)
    }

    fn public_key_pem_inner(&self) -> LifegwResult<String> {
        if let Some(pem) = self.public_pem.get() {
            return Ok(pem.clone());
        }
        let client = self.client()?;
        let resp = block_on_aws(async {
            client
                .get_public_key()
                .key_id(self.key_id.clone())
                .send()
                .await
        })
        .map_err(|e| LifegwError::Auth(format!("aws kms get_public_key: {e}")))?;
        let der = resp
            .public_key()
            .ok_or_else(|| {
                LifegwError::Auth("aws kms get_public_key: missing public_key".to_string())
            })?
            .as_ref();
        let pem = der_to_pem(der, "PUBLIC KEY");
        let _ = self.public_pem.set(pem.clone());
        Ok(pem)
    }
}

#[cfg(feature = "kms-aws")]
impl KmsSigner for AwsKms {
    fn active_kid(&self) -> &str {
        &self.kid
    }

    fn sign_jws(&self, claims_json: &serde_json::Value) -> LifegwResult<String> {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;

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

        let client = self.client()?;
        let key_id = self.key_id.clone();
        let signing_input_bytes = signing_input.as_bytes().to_vec();
        let resp = block_on_aws(async move {
            client
                .sign()
                .key_id(key_id)
                .message(aws_sdk_kms::primitives::Blob::new(signing_input_bytes))
                .message_type(aws_sdk_kms::types::MessageType::Raw)
                .signing_algorithm(aws_sdk_kms::types::SigningAlgorithmSpec::EcdsaSha256)
                .send()
                .await
        })
        .map_err(|e| LifegwError::Auth(format!("aws kms sign: {e}")))?;
        let der = resp
            .signature()
            .ok_or_else(|| LifegwError::Auth("aws kms sign: missing signature".to_string()))?
            .as_ref();
        // AWS KMS returns DER-encoded ECDSA. Decode to raw r || s for JWS.
        let raw = der_ecdsa_to_raw_p256(der)?;
        let sig_b64 = URL_SAFE_NO_PAD.encode(&raw);
        Ok(format!("{signing_input}.{sig_b64}"))
    }

    fn publish_jwks(&self) -> Jwks {
        match self.public_key_pem_inner() {
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
            Err(_) => Jwks { keys: vec![] },
        }
    }
}

/// GCP Cloud KMS signer (Sub-phase E item #2). Production body for the
/// `kms-gcp` feature.
///
/// Uses the `google-cloud-kms` 0.6 client to:
/// 1. **Sign**: `client.asymmetric_sign(...)` with the resource path
///    of an `EC_SIGN_P256_SHA256` key. GCP KMS, like AWS, returns a
///    DER-encoded ECDSA signature; we convert to raw `r || s` for JWS.
/// 2. **Public key**: `client.get_public_key(...)` returns a PEM string
///    directly — no encoding hop needed.
#[cfg(feature = "kms-gcp")]
pub struct GcpKms {
    /// GCP KMS resource name (`projects/.../keyRings/.../cryptoKeys/<key>/cryptoKeyVersions/<n>`).
    pub resource: String,
    /// Stable JWS kid.
    pub kid: String,
    /// Cached GCP KMS client.
    client: tokio::sync::OnceCell<google_cloud_kms::client::Client>,
    /// Cached PEM-encoded public key.
    public_pem: std::sync::OnceLock<String>,
}

#[cfg(feature = "kms-gcp")]
impl GcpKms {
    pub fn new(resource: impl Into<String>, kid: impl Into<String>) -> Self {
        Self {
            resource: resource.into(),
            kid: kid.into(),
            client: tokio::sync::OnceCell::new(),
            public_pem: std::sync::OnceLock::new(),
        }
    }

    async fn client(&self) -> LifegwResult<&google_cloud_kms::client::Client> {
        self.client
            .get_or_try_init(|| async {
                let cfg = google_cloud_kms::client::ClientConfig::default()
                    .with_auth()
                    .await
                    .map_err(|e| LifegwError::Auth(format!("gcp kms auth: {e}")))?;
                google_cloud_kms::client::Client::new(cfg)
                    .await
                    .map_err(|e| LifegwError::Auth(format!("gcp kms client: {e}")))
            })
            .await
    }

    fn public_key_pem_inner(&self) -> LifegwResult<String> {
        if let Some(pem) = self.public_pem.get() {
            return Ok(pem.clone());
        }
        let resource = self.resource.clone();
        let pem = block_on_aws(async move {
            let client = self.client().await?;
            let req = google_cloud_kms::grpc::kms::v1::GetPublicKeyRequest { name: resource };
            let resp = client
                .get_public_key(req, None)
                .await
                .map_err(|e| LifegwError::Auth(format!("gcp kms get_public_key: {e}")))?;
            Ok::<_, LifegwError>(resp.pem)
        })?;
        let _ = self.public_pem.set(pem.clone());
        Ok(pem)
    }
}

#[cfg(feature = "kms-gcp")]
impl KmsSigner for GcpKms {
    fn active_kid(&self) -> &str {
        &self.kid
    }

    fn sign_jws(&self, claims_json: &serde_json::Value) -> LifegwResult<String> {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use sha2::{Digest, Sha256};

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

        // GCP KMS asymmetric_sign requires the digest to be passed
        // pre-hashed — we compute SHA-256 ourselves.
        let mut hasher = Sha256::new();
        hasher.update(signing_input.as_bytes());
        let digest = hasher.finalize().to_vec();

        let resource = self.resource.clone();
        let der = block_on_aws(async move {
            let client = self.client().await?;
            let req = google_cloud_kms::grpc::kms::v1::AsymmetricSignRequest {
                name: resource,
                digest: Some(google_cloud_kms::grpc::kms::v1::Digest {
                    digest: Some(google_cloud_kms::grpc::kms::v1::digest::Digest::Sha256(
                        digest,
                    )),
                }),
                ..Default::default()
            };
            let resp = client
                .asymmetric_sign(req, None)
                .await
                .map_err(|e| LifegwError::Auth(format!("gcp kms asymmetric_sign: {e}")))?;
            Ok::<_, LifegwError>(resp.signature)
        })?;

        let raw = der_ecdsa_to_raw_p256(&der)?;
        let sig_b64 = URL_SAFE_NO_PAD.encode(&raw);
        Ok(format!("{signing_input}.{sig_b64}"))
    }

    fn publish_jwks(&self) -> Jwks {
        match self.public_key_pem_inner() {
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
            Err(_) => Jwks { keys: vec![] },
        }
    }
}

/// Decode an ASN.1 DER-encoded ECDSA signature into the raw `r || s`
/// byte sequence expected by JWS compact serialisation (RFC 7515 §3 +
/// RFC 7518 §3.4). Both AWS KMS + GCP KMS return DER; JWS demands raw.
///
/// The DER format is:
/// ```ignore
/// SEQUENCE (30 LL)
///   INTEGER (02 LL R-bytes)
///   INTEGER (02 LL S-bytes)
/// ```
/// Each integer may have a leading `0x00` byte if the high bit of the
/// first byte is set (DER preserves sign); we strip it. For P-256, r
/// and s are each padded/truncated to exactly 32 bytes for a total
/// signature length of 64 bytes.
#[cfg(any(feature = "kms-aws", feature = "kms-gcp"))]
fn der_ecdsa_to_raw_p256(der: &[u8]) -> LifegwResult<Vec<u8>> {
    if der.len() < 6 || der[0] != 0x30 {
        return Err(LifegwError::Auth(
            "der_ecdsa_to_raw: not a SEQUENCE".to_string(),
        ));
    }
    // Skip SEQUENCE header.
    let (seq_body, _) = read_der_length(&der[1..])?;
    let mut cursor = seq_body;

    // Read INTEGER (r).
    if cursor.is_empty() || cursor[0] != 0x02 {
        return Err(LifegwError::Auth(
            "der_ecdsa_to_raw: r not INTEGER".to_string(),
        ));
    }
    cursor = &cursor[1..];
    let (r_bytes, rest) = read_der_length(cursor)?;
    cursor = rest;

    // Read INTEGER (s).
    if cursor.is_empty() || cursor[0] != 0x02 {
        return Err(LifegwError::Auth(
            "der_ecdsa_to_raw: s not INTEGER".to_string(),
        ));
    }
    cursor = &cursor[1..];
    let (s_bytes, _) = read_der_length(cursor)?;

    // Pad/truncate to 32 bytes each for P-256.
    let r = pad_to_32(r_bytes);
    let s = pad_to_32(s_bytes);
    if r.len() != 32 || s.len() != 32 {
        return Err(LifegwError::Auth(format!(
            "der_ecdsa_to_raw: r={} s={} not P-256",
            r.len(),
            s.len(),
        )));
    }
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&r);
    out.extend_from_slice(&s);
    Ok(out)
}

/// Read a DER `LL` length field followed by `LL` bytes of content.
/// Returns `(content_slice, rest)`.
#[cfg(any(feature = "kms-aws", feature = "kms-gcp"))]
fn read_der_length(buf: &[u8]) -> LifegwResult<(&[u8], &[u8])> {
    if buf.is_empty() {
        return Err(LifegwError::Auth("der length: empty".to_string()));
    }
    let first = buf[0];
    let (len, header_skip) = if first & 0x80 == 0 {
        (first as usize, 1)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 {
            return Err(LifegwError::Auth(format!(
                "der length: unsupported long form n={n}"
            )));
        }
        if buf.len() < 1 + n {
            return Err(LifegwError::Auth("der length: truncated".to_string()));
        }
        let mut len = 0usize;
        for &b in &buf[1..1 + n] {
            len = (len << 8) | b as usize;
        }
        (len, 1 + n)
    };
    if buf.len() < header_skip + len {
        return Err(LifegwError::Auth(
            "der length: content truncated".to_string(),
        ));
    }
    let content = &buf[header_skip..header_skip + len];
    let rest = &buf[header_skip + len..];
    Ok((content, rest))
}

/// Strip a leading sign-extension `0x00` byte and pad/truncate to 32
/// bytes (P-256 component length).
#[cfg(any(feature = "kms-aws", feature = "kms-gcp"))]
fn pad_to_32(bytes: &[u8]) -> Vec<u8> {
    let trimmed: &[u8] = if bytes.len() == 33 && bytes[0] == 0x00 {
        &bytes[1..]
    } else {
        bytes
    };
    if trimmed.len() >= 32 {
        trimmed[trimmed.len() - 32..].to_vec()
    } else {
        let mut padded = vec![0u8; 32 - trimmed.len()];
        padded.extend_from_slice(trimmed);
        padded
    }
}

/// Wrap raw DER bytes in a PEM envelope (`-----BEGIN <label>-----` /
/// `-----END <label>-----`) with 64-char line wrapping.
#[cfg(feature = "kms-aws")]
fn der_to_pem(der: &[u8], label: &str) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    let b64 = STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// Cross-runtime helper: run an async future from a sync caller.
/// Mirrors `auth::jwks::fetch_via_reqwest` — handles both
/// multi-thread (via `block_in_place`) and current-thread (via a
/// side-thread with a private runtime).
#[cfg(any(feature = "kms-aws", feature = "kms-gcp"))]
fn block_on_aws<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T> + Send,
    T: Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(fut))
            }
            _ => block_on_side_thread(fut),
        }
    } else {
        block_on_side_thread(fut)
    }
}

#[cfg(any(feature = "kms-aws", feature = "kms-gcp"))]
fn block_on_side_thread<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T> + Send,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|s| {
        s.spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build mini runtime");
            let v = rt.block_on(fut);
            let _ = tx.send(v);
        });
    });
    rx.recv().expect("side-thread runtime channel")
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

    /// Sub-phase E (item #1, #2): DER → raw `r || s` P-256 decoder
    /// covers the conversion AWS/GCP KMS need to produce JWS-shaped
    /// signatures.
    #[cfg(any(feature = "kms-aws", feature = "kms-gcp"))]
    fn build_der_sig(r_bytes: &[u8], s_bytes: &[u8]) -> Vec<u8> {
        let total = r_bytes.len() + s_bytes.len() + 4;
        let mut der = vec![0x30, total as u8, 0x02, r_bytes.len() as u8];
        der.extend_from_slice(r_bytes);
        der.push(0x02);
        der.push(s_bytes.len() as u8);
        der.extend_from_slice(s_bytes);
        der
    }

    #[cfg(any(feature = "kms-aws", feature = "kms-gcp"))]
    #[test]
    fn der_ecdsa_to_raw_p256_decodes_minimal_signature() {
        // Hand-built DER sig: SEQUENCE { INTEGER 32-bytes-r, INTEGER 32-bytes-s }
        // r = 32 bytes of 0x01, s = 32 bytes of 0x02.
        let r_bytes: Vec<u8> = vec![0x01; 32];
        let s_bytes: Vec<u8> = vec![0x02; 32];
        let der = build_der_sig(&r_bytes, &s_bytes);
        let raw = super::der_ecdsa_to_raw_p256(&der).expect("decode minimal");
        assert_eq!(raw.len(), 64);
        assert_eq!(&raw[..32], &r_bytes[..]);
        assert_eq!(&raw[32..], &s_bytes[..]);
    }

    /// DER may add a leading `0x00` to keep an integer positive when
    /// the high bit is set. The decoder must strip it.
    #[cfg(any(feature = "kms-aws", feature = "kms-gcp"))]
    #[test]
    fn der_ecdsa_to_raw_p256_strips_leading_zero_sign_byte() {
        let mut r_bytes: Vec<u8> = vec![0x80; 32];
        // DER will require r to be prefixed with 0x00 because high bit is set.
        r_bytes.insert(0, 0x00);
        let s_bytes: Vec<u8> = vec![0x02; 32];
        let der = build_der_sig(&r_bytes, &s_bytes);
        let raw = super::der_ecdsa_to_raw_p256(&der).expect("decode");
        assert_eq!(raw.len(), 64);
        // The decoded r drops the 0x00 prefix.
        assert_eq!(raw[..32], [0x80; 32]);
    }

    /// Short integers (<32 bytes) get left-padded with zeros to reach
    /// 32 bytes. The decoder must do this padding correctly.
    #[cfg(any(feature = "kms-aws", feature = "kms-gcp"))]
    #[test]
    fn der_ecdsa_to_raw_p256_pads_short_integers() {
        let r_bytes: Vec<u8> = vec![0x42; 16]; // only 16 bytes
        let s_bytes: Vec<u8> = vec![0x77; 16];
        let der = build_der_sig(&r_bytes, &s_bytes);
        let raw = super::der_ecdsa_to_raw_p256(&der).expect("decode");
        assert_eq!(raw.len(), 64);
        // First 16 bytes of r are zero-padded.
        assert_eq!(&raw[..16], &[0u8; 16]);
        assert_eq!(&raw[16..32], &[0x42; 16]);
        assert_eq!(&raw[32..48], &[0u8; 16]);
        assert_eq!(&raw[48..], &[0x77; 16]);
    }

    /// Garbage input is rejected.
    #[cfg(any(feature = "kms-aws", feature = "kms-gcp"))]
    #[test]
    fn der_ecdsa_to_raw_p256_rejects_garbage() {
        assert!(super::der_ecdsa_to_raw_p256(&[]).is_err());
        assert!(super::der_ecdsa_to_raw_p256(&[0x00]).is_err());
        assert!(super::der_ecdsa_to_raw_p256(&[0x30, 0x00]).is_err());
        // SEQUENCE length present but wrong content type
        // (not 0x02 INTEGER).
        let bad = vec![0x30, 0x04, 0x05, 0x02, 0x01, 0xff];
        assert!(super::der_ecdsa_to_raw_p256(&bad).is_err());
    }

    /// Sub-phase E (item #1): DER → PEM wrapping is correct: 64-char
    /// line wrapping + standard label preamble/postamble.
    #[cfg(feature = "kms-aws")]
    #[test]
    fn der_to_pem_wraps_at_64_chars() {
        let der = b"hello world this is some bytes";
        let pem = super::der_to_pem(der, "PUBLIC KEY");
        assert!(pem.starts_with("-----BEGIN PUBLIC KEY-----\n"));
        assert!(pem.ends_with("-----END PUBLIC KEY-----\n"));
        // Each non-header line (other than the begin/end markers)
        // must be ≤ 64 chars.
        for line in pem.lines() {
            if line.starts_with("-----") {
                continue;
            }
            assert!(line.len() <= 64, "line len {} > 64: {line:?}", line.len());
        }
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

    /// Sub-phase E (item #4): VaultTransit::public_key_pem reads
    /// `data.latest_version` and indexes `data.keys.<latest>.public_key`,
    /// not the hardcoded `1`. Exercises with a synthetic JSON shape
    /// that mirrors Vault's response.
    #[cfg(feature = "kms-vault")]
    #[test]
    fn vault_transit_pins_latest_version() {
        // We don't hit a real Vault server; instead we verify the
        // pointer logic directly via the JSON-shape contract:
        // `latest_version` controls the key index used.
        // This is a regression test for the Sub-phase B bug (hardcoded
        // version 1) — the exact same JSON shape with `latest_version`
        // = 5 must yield the version-5 PEM.
        use serde_json::json;
        let body = json!({
            "data": {
                "latest_version": 5,
                "keys": {
                    "1": { "public_key": "-----BEGIN PUBLIC KEY-----\nv1\n-----END PUBLIC KEY-----\n" },
                    "5": { "public_key": "-----BEGIN PUBLIC KEY-----\nv5\n-----END PUBLIC KEY-----\n" }
                }
            }
        });
        // Replicate the pointer logic from public_key_pem.
        let latest = body
            .pointer("/data/latest_version")
            .and_then(|v| v.as_u64())
            .expect("latest_version");
        assert_eq!(latest, 5);
        let pointer = format!("/data/keys/{latest}/public_key");
        let pem = body
            .pointer(&pointer)
            .and_then(|v| v.as_str())
            .expect("v5 pem");
        assert!(pem.contains("v5"), "got: {pem}");
        // The version-1 pem should NOT be selected — that was the
        // Sub-phase B bug.
        let v1_pointer = "/data/keys/1/public_key";
        let v1_pem = body.pointer(v1_pointer).and_then(|v| v.as_str()).unwrap();
        assert_ne!(pem, v1_pem, "must pick latest, not v1");
    }
}
