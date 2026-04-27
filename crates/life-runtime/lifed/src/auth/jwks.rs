//! Real ES256 + JWKS verifier for Tier-2 capability tokens.
//!
//! Per Spec C₂ §5.1, lifed verifies the JWS bearer token presented on every
//! public-plane request. Verification uses the published lifegw JWKS at
//! `cfg.auth.jwks_path`. Sub-phase A's dev verifier is preserved behind a
//! conditional path so existing integration tests that pass
//! `Bearer test-token-for-{user_id}` continue to work — the rest of the
//! daemon paths run real ES256.

use std::path::Path;
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aios_proto::aios::v1 as aios_v1;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};

use crate::auth::capability::{CapabilityClaims, Tier};
use crate::error::{LifedError, LifedResult};

#[derive(Serialize, Deserialize, Clone)]
struct JwksFile {
    keys: Vec<JwksKey>,
}

#[derive(Serialize, Deserialize, Clone)]
#[allow(dead_code)]
struct JwksKey {
    kid: String,
    kty: String,
    crv: String,
    alg: String,
    #[serde(default)]
    x: String,
    #[serde(default)]
    y: String,
    /// Optional convenience: when the JWKS file embeds a PEM-encoded public
    /// key directly (used by the dev path) we accept it instead of x/y.
    #[serde(default)]
    pem: Option<String>,
}

#[derive(Deserialize)]
struct Tier2Body {
    #[allow(dead_code)]
    iss: String,
    sub: String,
    #[allow(dead_code)]
    aud: String,
    sid: String,
    scopes: Vec<String>,
    tier: Option<String>,
    exp: u64,
    #[allow(dead_code)]
    nbf: Option<u64>,
}

/// JWKS cache used by the auth middleware for Tier-2 verification.
pub struct JwksCache {
    keys: RwLock<Vec<(String, DecodingKey)>>,
    /// Whether the dev `test-token-for-{user_id}` path is enabled. Set to
    /// `true` only when [`JwksCache::dev_only`] is used; production
    /// deployments load real JWKS via [`JwksCache::load_from_path`] and
    /// dev-token acceptance stays off.
    dev_signer_enabled: bool,
}

impl JwksCache {
    /// Load a JWKS from disk for production verification.
    pub fn load_from_path(path: &Path) -> LifedResult<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| LifedError::Auth(format!("read {}: {e}", path.display())))?;
        let file: JwksFile = serde_json::from_str(&text)
            .map_err(|e| LifedError::Auth(format!("parse jwks: {e}")))?;
        let mut keys = Vec::new();
        for k in file.keys {
            if k.kty != "EC" || k.crv != "P-256" || k.alg != "ES256" {
                continue;
            }
            let key = if let Some(pem) = k.pem.as_ref() {
                DecodingKey::from_ec_pem(pem.as_bytes())
                    .map_err(|e| LifedError::Auth(format!("decode pem {}: {e}", k.kid)))?
            } else {
                DecodingKey::from_ec_components(&k.x, &k.y)
                    .map_err(|e| LifedError::Auth(format!("decode key {}: {e}", k.kid)))?
            };
            keys.push((k.kid, key));
        }
        Ok(Self {
            keys: RwLock::new(keys),
            dev_signer_enabled: false,
        })
    }

    /// Dev convenience: build a cache containing the [`crate::auth::keystore::Keystore::generate_dev`]
    /// public key AND enable the `test-token-for-{user_id}` shortcut so existing
    /// integration tests keep passing.
    pub fn dev_only() -> Self {
        let ks = crate::auth::keystore::Keystore::generate_dev();
        let pubkey_pem = ks.public_key_pem();
        let key = DecodingKey::from_ec_pem(pubkey_pem.as_bytes()).expect("dev pem");
        Self {
            keys: RwLock::new(vec![(ks.kid, key)]),
            dev_signer_enabled: true,
        }
    }

    /// Validate a Tier-2 bearer token. Returns the parsed claims on success.
    ///
    /// When `dev_signer_enabled` is true, also accepts the
    /// `test-token-for-{user_id}` shortcut used by integration tests.
    pub fn validate(&self, bearer: &str) -> LifedResult<CapabilityClaims> {
        if self.dev_signer_enabled {
            if let Some(user_id) = bearer.strip_prefix("test-token-for-") {
                return Ok(CapabilityClaims {
                    user_id: user_id.to_string(),
                    project_id: "project-demo".to_string(),
                    sid: aios_v1::SessionId {
                        value: String::new(),
                    },
                    scopes: vec![
                        "agent:dispatch".to_string(),
                        "events:read".to_string(),
                        "wallet:debit".to_string(),
                        "identity:read".to_string(),
                    ],
                    tier: Tier::Free,
                    exp: Instant::now() + Duration::from_secs(900),
                });
            }
        }
        // Real ES256 path.
        let header = decode_header(bearer).map_err(|e| LifedError::Auth(format!("header: {e}")))?;
        let kid = header
            .kid
            .ok_or_else(|| LifedError::Auth("missing kid".to_string()))?;
        let key = {
            let guard = self
                .keys
                .read()
                .map_err(|_| LifedError::Auth("jwks lock".to_string()))?;
            guard
                .iter()
                .find(|(k, _)| k == &kid)
                .map(|(_, dk)| dk.clone())
        }
        .ok_or_else(|| LifedError::Auth(format!("unknown kid: {kid}")))?;

        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["lifed"]);
        v.set_issuer(&["lifegw"]);
        let token = decode::<Tier2Body>(bearer, &key, &v)
            .map_err(|e| LifedError::Auth(format!("verify: {e}")))?;
        let body = token.claims;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if body.exp <= now {
            return Err(LifedError::Auth("expired".to_string()));
        }
        let tier = match body.tier.as_deref() {
            Some("paid") => Tier::Paid,
            Some("enterprise") => Tier::Enterprise,
            _ => Tier::Free,
        };
        let exp_instant = Instant::now() + Duration::from_secs(body.exp - now);
        Ok(CapabilityClaims {
            user_id: body.sub,
            project_id: String::new(), // narrowed at handler boundary
            sid: aios_v1::SessionId { value: body.sid },
            scopes: body.scopes,
            tier,
            exp: exp_instant,
        })
    }
}
