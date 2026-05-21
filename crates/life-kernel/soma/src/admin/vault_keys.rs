//! `VaultCustodyKeys` — HashiCorp Vault Transit `CustodyKeyStore`
//! adapter (BRO-1215, Spec D D-Sub-E M9-E).
//!
//! Bridges `anima_identity::VaultTransitAnima` (per-user backend) to the
//! `CustodyKeyStore` trait (multi-user store) consumed by
//! `CustodyOracleService`. The adapter is the missing wiring that lets
//! production multi-tenant soma deploys delegate `sign_auth_digest` /
//! `sign_wallet_digest` / pubkey-fetch RPCs to Vault rather than holding
//! raw scalars in process memory.
//!
//! ## Per-user lazy bootstrap
//!
//! `VaultTransitAnima::new(addr, token, user_id, kid)` performs two
//! `GET /v1/transit/keys/...` calls at construction to resolve the auth
//! + wallet pubkeys. We cache the resulting handles per `user_id` in an
//!   `RwLock<HashMap>` — first call to any RPC for a user triggers the
//!   bootstrap; subsequent calls hit the cache.
//!
//! This matches the discipline in
//! `crates/anima/anima-identity/src/vault.rs`: tenants own their key
//! lifecycle; soma does NOT auto-provision keys. If a user's transit
//! keys are missing from Vault, the bootstrap returns
//! `AnimaError::Crypto(...)` which maps to `tonic::Status::not_found`
//! via the same path the `InProcessCustodyKeys::sign_*` "user not
//! provisioned" errors take (see `admin/service.rs::anima_err_to_status`).
//!
//! ## Why a cache and not a per-call bootstrap?
//!
//! Each `VaultTransitAnima::new` issues two HTTP calls to Vault. Without
//! a cache, every `sign_auth_digest` would add ~5-10ms of round-trip
//! latency on top of the sign call itself. The cache makes the hot path
//! a single Vault HTTP call (the actual `transit/sign`) — same
//! per-request cost as `InProcessCustodyKeys` plus the Vault network
//! hop.
//!
//! The cache is unbounded — multi-tenant soma deploys host O(thousands)
//! of users, well within the memory budget of a `HashMap<String,
//! Arc<VaultTransitAnima>>`. Each handle holds ~200 bytes (two cached
//! pubkeys + the cached PEM + a few `String`s). Production tenant
//! ceilings well above this would need a bounded LRU; deferred until
//! we see actual pressure.
//!
//! ## Fail-closed semantics
//!
//! The adapter does NOT mask Vault errors. A 5xx from Vault propagates
//! to the soma admin handler which maps it to `tonic::Status::internal`
//! — the lifegw-side route then surfaces this as `502 Bad Gateway` per
//! the `sanitize_upstream` pipeline in
//! `crates/life-runtime/lifegw/src/services/anima_custody.rs`.
//!
//! ## Threading model
//!
//! `VaultTransitAnima` uses `reqwest::blocking::Client` internally — the
//! `CustodyKeyStore` trait is sync (each method is `&self`), so soma's
//! tonic handlers `block_in_place` around the Vault call inside the
//! tokio runtime. This matches the InProcess path which also signs
//! synchronously inside the handler. The blocking client is a
//! deliberate trade-off documented in `anima-identity::vault`'s
//! module-level comment — Vault's transit/sign latency is dominated by
//! the network, not CPU, so the thread cost is minor.
//!
//! Note: this file is gated by `#[cfg(feature = "kms-vault")]` at the
//! `admin.rs` declaration site (`pub mod vault_keys;`), so no inner
//! `#![cfg(...)]` is needed here. See `crates/life-kernel/soma/src/admin.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use anima_core::error::AnimaError;
use anima_identity::vault::VaultTransitAnima;
use parking_lot::RwLock;
use tonic::Status;

use crate::admin::service::CustodyKeyStore;

/// Multi-user adapter from `VaultTransitAnima` to `CustodyKeyStore`.
///
/// Holds a lazy cache of per-user `VaultTransitAnima` handles. First
/// access for a user_id triggers the two-RPC bootstrap; subsequent
/// accesses hit the cache.
pub struct VaultCustodyKeys {
    /// Vault HTTP base URL (e.g. `https://vault.internal:8200`).
    addr: String,
    /// Vault token with `transit/sign/<kid_prefix>-*` capability.
    /// Held in-memory only — sourced from an env var at bootstrap.
    token: String,
    /// Kid prefix used when deriving per-user key names. Matches the
    /// convention in `VaultTransitAnima::new`:
    /// `{kid_prefix}-{user_id}-auth-v1` / `{kid_prefix}-{user_id}-wallet-v1`.
    /// The corresponding JWS kid is `{kid_prefix}-{user_id}-auth-v1`.
    kid_prefix: String,
    /// Per-user handle cache. `RwLock` because the bootstrap is rare
    /// (per-user, once) while the read path is hot.
    cache: RwLock<HashMap<String, Arc<VaultTransitAnima>>>,
}

impl VaultCustodyKeys {
    /// Build the adapter from raw credentials. The token is read from an
    /// env var by the caller (typically `soma/bootstrap.rs`) so the
    /// raw token never lands in disk-stored config.
    pub fn new(
        addr: impl Into<String>,
        token: impl Into<String>,
        kid_prefix: impl Into<String>,
    ) -> Self {
        Self {
            addr: addr.into(),
            token: token.into(),
            kid_prefix: kid_prefix.into(),
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Resolve a per-user handle from the cache, bootstrapping on the
    /// first call.
    ///
    /// Fail-closed: if Vault rejects the bootstrap (missing keys, bad
    /// token, network error), the error propagates out as
    /// `AnimaError::Crypto`. The caller's `anima_err_to_status` mapping
    /// turns "not provisioned" into `Code::NotFound` and everything
    /// else into `Code::Internal`.
    fn handle(&self, user_id: &str) -> Result<Arc<VaultTransitAnima>, AnimaError> {
        // Hot path — cache hit under read lock.
        if let Some(existing) = self.cache.read().get(user_id).cloned() {
            return Ok(existing);
        }
        // Cold path — drop the read lock before doing I/O, then take a
        // write lock to insert. Double-check after acquiring the write
        // lock to handle the race where two callers bootstrap the same
        // user concurrently.
        let kid = format!("{}-{}-auth-v1", self.kid_prefix, user_id);
        let handle = VaultTransitAnima::new(&self.addr, &self.token, user_id, kid)?;
        let handle = Arc::new(handle);
        let mut guard = self.cache.write();
        // Race-resolution: if another caller raced ahead, prefer the
        // already-inserted handle to keep `Arc` identity stable for any
        // downstream comparisons.
        let final_handle = guard
            .entry(user_id.to_string())
            .or_insert_with(|| handle.clone())
            .clone();
        Ok(final_handle)
    }

    /// Test-only: pre-populate the cache with a synthetic handle.
    /// Disabled in production builds; the production path always goes
    /// through `handle()` + Vault.
    #[cfg(test)]
    pub(crate) fn insert_handle_for_test(&self, user_id: &str, handle: Arc<VaultTransitAnima>) {
        self.cache.write().insert(user_id.to_string(), handle);
    }
}

/// Map an [`AnimaError`] to a [`tonic::Status`] using the same shape
/// the `admin/service.rs::anima_err_to_status` helper uses for
/// `InProcessCustodyKeys`. We deliberately match the helper's pattern
/// so both stores produce the same wire-level error vocabulary:
/// "user not provisioned" → `Code::NotFound`, everything else →
/// `Code::Internal`.
fn anima_err_to_status(err: AnimaError) -> Status {
    match err {
        AnimaError::Crypto(msg) if msg.contains("not provisioned") => Status::not_found(msg),
        // Vault HTTP errors surface as `vault get auth key: ...` /
        // `vault sign secp256k1: ...` etc. — propagate as Internal so
        // the lifegw `sanitize_upstream` layer surfaces 502, not 500.
        other => Status::internal(other.to_string()),
    }
}

impl CustodyKeyStore for VaultCustodyKeys {
    fn sign_auth_digest(&self, user_id: &str, digest: &[u8; 32]) -> Result<[u8; 64], Status> {
        let h = self.handle(user_id).map_err(anima_err_to_status)?;
        h.sign_auth_digest_raw(digest).map_err(anima_err_to_status)
    }

    fn sign_wallet_digest(&self, user_id: &str, digest: &[u8; 32]) -> Result<[u8; 65], Status> {
        let h = self.handle(user_id).map_err(anima_err_to_status)?;
        h.sign_wallet_digest_evm(digest)
            .map_err(anima_err_to_status)
    }

    fn auth_pubkey_sec1(&self, user_id: &str) -> Result<[u8; 33], Status> {
        let h = self.handle(user_id).map_err(anima_err_to_status)?;
        // VaultTransitAnima caches the auth pubkey at bootstrap; the
        // trait method is `&self -> [u8; 33]` and never fails post-bootstrap.
        use anima_identity::custody::AnimaCustody;
        Ok(h.auth_pubkey())
    }

    fn wallet_pubkey_sec1_uncompressed(&self, user_id: &str) -> Result<[u8; 65], Status> {
        let h = self.handle(user_id).map_err(anima_err_to_status)?;
        // The public accessor on VaultTransitAnima returns the cached
        // 65-byte uncompressed wallet pubkey.
        Ok(h.wallet_pubkey_uncompressed_sec1())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::service::CustodyKeyStore as _;

    /// Constructor smoke test — the adapter builds without I/O. Cache
    /// is empty initially; bootstrap is lazy.
    #[test]
    fn constructor_does_not_call_vault() {
        let store = VaultCustodyKeys::new("https://vault.example:8200", "fake-token", "anima");
        assert!(
            store.cache.read().is_empty(),
            "cache must be empty at construction"
        );
        // Pull out the config knobs to verify they round-tripped.
        assert_eq!(store.addr, "https://vault.example:8200");
        assert_eq!(store.token, "fake-token");
        assert_eq!(store.kid_prefix, "anima");
    }

    /// Bootstrap error path — when Vault is unreachable, the adapter
    /// surfaces an `internal` error rather than panicking. We can't
    /// stand up a wiremock server inside a sync test without dragging
    /// tokio into this unit test, so we just exercise the error path
    /// with an unroutable address. The bootstrap times out and we
    /// observe the error mapping.
    ///
    /// This is gated by `#[ignore]` because it relies on real DNS
    /// failure semantics + the 10s default timeout — running it in CI
    /// would inflate test time. The full wiremock-backed integration
    /// test lives in `tests/integration_vault_custody.rs`.
    #[test]
    #[ignore = "slow — exercises the 10s Vault HTTP timeout against an unroutable host"]
    fn unreachable_vault_surfaces_internal_error() {
        let store = VaultCustodyKeys::new(
            // RFC 5737 documentation address — guaranteed unroutable.
            "http://192.0.2.1:8200",
            "fake-token",
            "anima",
        );
        let err = store
            .sign_auth_digest("alice", &[0u8; 32])
            .expect_err("unroutable Vault must error");
        assert_eq!(err.code(), tonic::Code::Internal);
    }
}
