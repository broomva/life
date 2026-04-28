//! Sub-phase B conformance test — Tier-2 token verification via the
//! published JWKS file (cross-daemon trust boundary closure).
//!
//! Per Spec C₃ §5: lifegw mints Tier-2 tokens using its KMS signer and
//! publishes the public JWKS to a well-known path. lifed (a separate
//! process in production) reads that JWKS file and uses the contents
//! to verify inbound Tier-2 bearer tokens. This test reproduces the
//! cross-daemon flow inside one process using only the file-system
//! handoff:
//!
//! 1. lifegw publishes its JWKS to a tempdir path.
//! 2. A separate "reader" — which mirrors lifed's
//!    `JwksCache::load_from_path` pattern — loads the file and builds
//!    a [`jsonwebtoken::DecodingKey`] from each entry.
//! 3. lifegw mints a Tier-2 token via the same `Tier2Minter` the
//!    production middleware uses.
//! 4. The reader verifies the token using ONLY the public material
//!    written to disk — no shared in-memory state with lifegw.
//!
//! Failure modes the test guards against:
//! - publish writes a subtly wrong shape (missing `pem`, wrong `alg`,
//!   wrong `kty`).
//! - mint uses a kid the reader can't find.
//! - Tier-2 claims drift (`aud`, `iss`, `exp`).

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;
use tempfile::TempDir;

use lifegw::auth::keystore::{Jwks, JwksKey, Keystore};
use lifegw::auth::kms::{KmsSigner, StaticKeystore};
use lifegw::auth::tier1::Tier1Claims;
use lifegw::auth::tier2::{Tier2Claims, Tier2Minter};
use lifegw::config::AuthConfig;

/// Mirror of lifed's `JwksCache::load_from_path` shape — kept here so
/// the conformance test exercises the *file format contract*, not
/// lifegw's in-process types. Any drift between what lifegw publishes
/// and what lifed expects to read fails this test.
struct LifedStyleJwksReader {
    keys: Vec<(String, DecodingKey)>,
}

impl LifedStyleJwksReader {
    fn load_from_path(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let parsed: PublishedJwksFile = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::other(format!("parse jwks: {e}")))?;
        let mut keys = Vec::new();
        for k in parsed.keys {
            // lifed's reader requires kty=EC, crv=P-256, alg=ES256.
            // Anything else is silently dropped (matches the current
            // lifed implementation in
            // crates/life-runtime/lifed/src/auth/jwks.rs).
            if k.kty != "EC" || k.crv != "P-256" || k.alg != "ES256" {
                continue;
            }
            let dk = if let Some(pem) = k.pem.as_ref() {
                DecodingKey::from_ec_pem(pem.as_bytes())
                    .map_err(|e| std::io::Error::other(format!("decode pem {}: {e}", k.kid)))?
            } else if !k.x.is_empty() && !k.y.is_empty() {
                DecodingKey::from_ec_components(&k.x, &k.y)
                    .map_err(|e| std::io::Error::other(format!("decode xy {}: {e}", k.kid)))?
            } else {
                continue;
            };
            keys.push((k.kid, dk));
        }
        Ok(Self { keys })
    }

    fn find(&self, kid: &str) -> Option<&DecodingKey> {
        self.keys.iter().find(|(k, _)| k == kid).map(|(_, dk)| dk)
    }
}

#[derive(Deserialize)]
struct PublishedJwksFile {
    keys: Vec<PublishedJwksEntry>,
}

#[derive(Deserialize)]
struct PublishedJwksEntry {
    kid: String,
    kty: String,
    #[serde(default)]
    crv: String,
    alg: String,
    #[serde(default, rename = "use")]
    _use_: String,
    #[serde(default)]
    x: String,
    #[serde(default)]
    y: String,
    #[serde(default)]
    pem: Option<String>,
}

/// Atomic-publish helper mirroring `bootstrap::publish_jwks_atomic` so
/// the conformance test doesn't need to bring up the full daemon.
fn write_jwks_atomic(path: &Path, jwks: &Jwks) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(jwks).map_err(std::io::Error::other)?;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let mut tmp = match parent {
        Some(p) => tempfile::NamedTempFile::new_in(p)?,
        None => tempfile::NamedTempFile::new()?,
    };
    use std::io::Write;
    tmp.write_all(&body)?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[test]
fn lifed_style_reader_verifies_tier2_token_from_published_jwks() {
    // 1. Lifegw signer + Tier-2 minter.
    let signer: Arc<dyn KmsSigner> =
        Arc::new(StaticKeystore::generate_dev().expect("dev keystore"));
    let mut auth_cfg = AuthConfig::default();
    auth_cfg.tier2_audience = "lifed".to_string();
    auth_cfg.tier2_issuer = "lifegw".to_string();
    auth_cfg.tier2_ttl = Duration::from_secs(900);
    let minter = Tier2Minter::new(Arc::clone(&signer), &auth_cfg);

    // 2. Publish JWKS to disk atomically.
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("lifegw-jwks.json");
    let jwks = signer.publish_jwks();
    write_jwks_atomic(&path, &jwks).expect("publish");

    // 3. Lifed-style reader loads the file (no shared in-memory state
    //    with lifegw — only the file contents).
    let reader = LifedStyleJwksReader::load_from_path(&path).expect("load jwks");
    let dk = reader
        .find(signer.active_kid())
        .expect("reader finds kid published by lifegw");

    // 4. Mint a Tier-2 token via the production minter.
    let t1 = Tier1Claims::new(
        "user-conformance",
        "demo",
        vec!["agent:dispatch".to_string()],
    );
    let token = minter.mint(&t1).expect("mint tier-2");

    // 5. Verify the token using the disk-loaded key set.
    let mut v = Validation::new(Algorithm::ES256);
    v.set_audience(&["lifed"]);
    v.set_issuer(&["lifegw"]);
    v.validate_nbf = true;
    let claims = decode::<Tier2Claims>(&token, dk, &v)
        .expect("conformance: lifed-style reader verifies lifegw's tier-2 token")
        .claims;
    assert_eq!(claims.sub, "user-conformance");
    assert_eq!(claims.aud, "lifed");
    assert_eq!(claims.iss, "lifegw");
    assert!(claims.exp > claims.iat);
    assert!(claims.nbf <= claims.iat);
    assert!(!claims.jti.is_empty());
}

#[test]
fn published_jwks_excludes_non_es256_entries() {
    // Confirm that even if the published JWKS contained a non-ES256
    // entry (defensive future-proofing — Sub-phase B's KmsSigner only
    // emits ES256, but a future provider might include another), the
    // lifed-style reader silently skips it and the matching ES256 key
    // remains usable.
    let signer = StaticKeystore::generate_dev().expect("dev keystore");
    let mut jwks = signer.publish_jwks();
    jwks.keys
        .push(JwksKey::with_alg("nope", "RSA", "", "RS512"));

    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("jwks.json");
    write_jwks_atomic(&path, &jwks).expect("publish");

    let reader = LifedStyleJwksReader::load_from_path(&path).expect("load");
    assert!(reader.find(signer.active_kid()).is_some());
    assert!(reader.find("nope").is_none());
}

#[test]
fn published_jwks_contains_signer_kid_first() {
    // Spec C₃ §5: the signer's CURRENT key must be the first entry in
    // the published JWKS so consumers prioritise it during lookup.
    let signer = StaticKeystore::generate_dev().expect("dev keystore");
    let jwks = signer.publish_jwks();
    assert!(!jwks.keys.is_empty(), "publish_jwks emits at least one key");
    assert_eq!(jwks.keys[0].kid, signer.active_kid());
    assert_eq!(jwks.keys[0].alg, "ES256");
    assert_eq!(jwks.keys[0].kty, "EC");
    assert_eq!(jwks.keys[0].crv, "P-256");
}

#[test]
fn keystore_publish_jwks_round_trip_via_disk() {
    // Belt-and-suspenders: confirm that a Keystore-published JWKS
    // can also be loaded by the lifed-style reader. (Future sub-phases
    // may bypass StaticKeystore entirely; pinning this now catches
    // accidental shape drift.)
    let ks = Keystore::generate_dev().expect("ks");
    let jwks = ks.publish_jwks();
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("jwks.json");
    write_jwks_atomic(&path, &jwks).expect("publish");
    let reader = LifedStyleJwksReader::load_from_path(&path).expect("load");
    assert!(reader.find(&ks.kid).is_some());
}
