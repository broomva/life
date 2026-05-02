//! # RemoteAnima — Browser/Remote Custody Bridge (Spec D D-Sub-C)
//!
//! `RemoteAnima` is the Rust-side `AnimaCustody` impl that talks to a
//! lifegw `/anima/custody/*` HTTP/JSON proxy. It exists so Rust callers
//! (CLIs, agents-as-services, native apps) can use the same wallet path
//! the browser uses via `WebCryptoAnima` — wallet ops are forwarded to
//! a server-side anima daemon (typically `VaultTransitAnima`) over the
//! same lifegw edge gateway used by lifed.
//!
//! ## Wire surface (Stream R-2 ships routes)
//!
//! 6 HTTP/JSON routes on lifegw:
//!
//! - `POST /anima/custody/sign_auth   { user_id, digest_b64 }` →
//!   `{ signature_b64 }` — 64-byte raw `r||s` from soma / Vault auth half.
//! - `POST /anima/custody/sign_wallet { user_id, digest_b64 }` →
//!   `{ signature_b64 }` — 64-byte raw `r||s` from secp256k1 signer.
//! - `GET  /anima/custody/get_auth_pubkey/{user_id}`   → `{ pubkey_b64 }`
//!   — 33-byte SEC1 compressed P-256.
//! - `GET  /anima/custody/get_wallet_pubkey/{user_id}` → `{ pubkey_b64 }`
//!   — 65-byte SEC1 uncompressed secp256k1 (`0x04 || x || y`).
//! - `POST /anima/custody/mint_session_cap` — refresh tier-user cap
//!   (Stream R-2 — RemoteAnima holds the cap and is responsible for
//!   refreshing before its `expires_at`).
//! - `POST /anima/custody/enroll_passkey` — first-time passkey
//!   enrolment (browser-only; Rust callers don't hit this).
//!
//! All routes require `Authorization: Bearer <tier-user-or-tier-2-jwt>`.
//!
//! ## Why HTTP/JSON not gRPC
//!
//! Sidesteps M8.1 (Connect-vs-grpc-web mismatch in life-sdk-ts). The
//! consumer surface is browser-shaped (small, stable, easy to call via
//! `fetch()` from chatOS); using HTTP/JSON keeps the Rust path
//! symmetric with the browser path so the same lifegw routes serve
//! both.
//!
//! ## Pubkey caching
//!
//! Auth + wallet pubkeys are fetched once at `RemoteAnima::new` and
//! cached. Trait accessors (`user_did`, `auth_pubkey`,
//! `wallet_address`) are referentially transparent and never block on
//! I/O. Pubkey rotation invalidates this cache — callers must
//! reconstruct a fresh `RemoteAnima` after observing a rotation event.
//!
//! ## SPEC-D-DEVIATION
//!
//! - **`rotate()` returns an error.** Mirrors `SomaCustody::rotate`:
//!   rotation flow is journal-driven (`anima.identity_rotated` events
//!   via `anima-lago::write_rotation_event`), not RPC-driven. Even
//!   though lifegw COULD expose a `POST /anima/custody/rotate` route,
//!   wiring it would create two divergent rotation surfaces (RPC vs
//!   journal) and break the Spec D L4-D10 invariant that rotations are
//!   documented in the journal. The error message points operators at
//!   the journal helper.
//!
//! - **`sign_eip712` constraint matches the family** — only EIP-3009
//!   `TransferWithAuthorization` is supported, same as
//!   `InProcessAnima` / `VaultTransitAnima` / `SomaCustody`. Generic
//!   EIP-712 encoder is a deferred follow-up.
//!
//! - **`block_on` runtime caveat** — `RemoteAnima` exposes `sign_*`
//!   trait methods synchronously per the trait shape. The HTTP calls
//!   are async; we drive them via `Handle::current().block_on` when a
//!   tokio runtime is available, falling back to a single-shot
//!   current-thread runtime otherwise. Callers in tokio contexts MUST
//!   use a multi-thread runtime — `block_in_place` panics on
//!   `current_thread`.

use std::sync::Arc;

use anima_core::error::{AnimaError, AnimaResult};
use anima_core::identity_document::{
    AgentIdentityDocument, AgentType, IdentityDocumentBuilder, VerificationMethod,
};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD as B64_STANDARD, URL_SAFE_NO_PAD};
use haima_core::wallet::{ChainId, WalletAddress};
use k256::ecdsa::{RecoveryId, Signature as K256Signature, VerifyingKey as K256VerifyingKey};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha3::{Digest as Sha3Digest, Keccak256};

use crate::custody::{
    AnimaCustody, BackendKind, DidRotationEvent, Eip712Domain, EvmSignature, TxRequest,
};
use crate::rlp;

/// Default request timeout for lifegw HTTP calls.
const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// Tier-User capability — browser-shaped JWT. Stored separately because
/// the token's lifetime is bounded by the cap's `exp` claim. The caller
/// is responsible for refreshing via `mint_session_cap` (Stream R-2).
#[derive(Debug, Clone)]
pub struct TierUserCap {
    /// Compact JWT in `<header>.<body>.<signature>` form.
    pub token: String,
    /// Unix timestamp of expiry. Refresh well before this elapses.
    pub expires_at_unix: i64,
}

/// Request body for `POST /anima/custody/sign_{auth,wallet}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignBody {
    user_id: String,
    /// Standard base64 (with padding) — matches lifegw's body convention.
    digest_b64: String,
}

/// Response body for `POST /anima/custody/sign_{auth,wallet}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignResp {
    signature_b64: String,
}

/// Response body for `GET /anima/custody/get_{auth,wallet}_pubkey/{user_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PubkeyResp {
    pubkey_b64: String,
}

/// `RemoteAnima` — HTTP/JSON bridge to lifegw `/anima/custody/*`.
///
/// Holds:
/// - the lifegw base URL,
/// - the user_id passed on every RPC,
/// - a Tier-User capability behind a Mutex (refreshable),
/// - cached auth + wallet pubkeys + DID + wallet address resolved at
///   construction so trait accessors never block on I/O,
/// - a shared `reqwest::Client` for keep-alive / connection pooling.
pub struct RemoteAnima {
    base_url: String,
    user_id: String,
    /// Cached SEC1-compressed P-256 auth pubkey (33 bytes).
    auth_pubkey: [u8; 33],
    /// Cached SEC1-uncompressed secp256k1 wallet pubkey (65 bytes).
    /// Used by `sign_evm_tx` for the ecrecover-based v-byte recovery
    /// loop. Same shape as `VaultTransitAnima::wallet_pubkey_uncompressed`.
    wallet_pubkey_uncompressed: [u8; 65],
    /// Cached EVM wallet address derived from the wallet pubkey.
    wallet_address: WalletAddress,
    /// Cached user DID (`did:key:zDn…` per Spec D L4-D6).
    user_did: String,
    /// Tier-User cap. Holding it under a Mutex so a future Stream R-2
    /// refresh task can swap it out without re-creating the handle.
    cap: Arc<Mutex<TierUserCap>>,
    /// Shared HTTP client.
    client: Arc<reqwest::Client>,
}

impl RemoteAnima {
    /// Construct a remote-anima handle for `user_id` against the given
    /// lifegw base URL. Performs two `GET /anima/custody/get_*_pubkey`
    /// requests + derives DID + wallet address. Failures bubble as
    /// `AnimaError::Crypto`.
    ///
    /// `cap` is the caller's Tier-User JWT — typically minted via
    /// lifegw's tier-user minter (Stream R-2). The handle stores it
    /// behind a Mutex so a refresh task can rotate it without
    /// re-creating the handle.
    pub async fn new(
        base_url: impl Into<String>,
        user_id: impl Into<String>,
        cap: TierUserCap,
    ) -> AnimaResult<Self> {
        let base_url = base_url.into();
        let user_id = user_id.into();
        let client = Arc::new(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                .build()
                .map_err(|e| AnimaError::Crypto(format!("RemoteAnima reqwest build: {e}")))?,
        );
        let cap_arc = Arc::new(Mutex::new(cap));

        let auth_pubkey = Self::fetch_auth_pubkey(&client, &base_url, &user_id, &cap_arc).await?;
        let wallet_pubkey_uncompressed =
            Self::fetch_wallet_pubkey(&client, &base_url, &user_id, &cap_arc).await?;
        let wallet_addr_hex = derive_wallet_address(&wallet_pubkey_uncompressed)?;
        let user_did = crate::did::generate_did_key_p256(&auth_pubkey);

        Ok(Self {
            base_url,
            user_id,
            auth_pubkey,
            wallet_pubkey_uncompressed,
            wallet_address: WalletAddress {
                address: wallet_addr_hex,
                chain: ChainId::base(),
            },
            user_did,
            cap: cap_arc,
            client,
        })
    }

    /// Borrow the configured base URL — used by tests + diagnostics.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Borrow the user_id — used by tests + diagnostics.
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// Snapshot the current Tier-User cap (clone). Callers should NOT
    /// pass this around long-term — it's a snapshot of mutable state.
    pub fn current_cap(&self) -> TierUserCap {
        self.cap.lock().clone()
    }

    /// Replace the stored Tier-User cap. Used by a Stream R-2 refresh
    /// task to rotate the bearer token without re-creating the handle.
    pub fn set_cap(&self, new_cap: TierUserCap) {
        *self.cap.lock() = new_cap;
    }

    fn cap_token(&self) -> String {
        self.cap.lock().token.clone()
    }

    /// Fetch the SEC1-compressed P-256 auth pubkey for `user_id` from
    /// `GET /anima/custody/get_auth_pubkey/{user_id}`.
    async fn fetch_auth_pubkey(
        client: &reqwest::Client,
        base_url: &str,
        user_id: &str,
        cap: &Arc<Mutex<TierUserCap>>,
    ) -> AnimaResult<[u8; 33]> {
        let url = format!("{base_url}/anima/custody/get_auth_pubkey/{user_id}");
        let token = cap.lock().token.clone();
        let resp = client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima get_auth_pubkey: {e}")))?;
        if !resp.status().is_success() {
            return Err(AnimaError::Crypto(format!(
                "RemoteAnima get_auth_pubkey non-success: {}",
                resp.status()
            )));
        }
        let body: PubkeyResp = resp
            .json()
            .await
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima get_auth_pubkey parse: {e}")))?;
        let bytes = decode_pubkey_b64(&body.pubkey_b64)
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima auth pubkey b64: {e}")))?;
        if bytes.len() != 33 {
            return Err(AnimaError::Crypto(format!(
                "RemoteAnima auth pubkey: expected 33 bytes (SEC1 compressed), got {}",
                bytes.len()
            )));
        }
        if bytes[0] != 0x02 && bytes[0] != 0x03 {
            return Err(AnimaError::Crypto(format!(
                "RemoteAnima auth pubkey: SEC1 compressed prefix must be 0x02 or 0x03, got 0x{:02x}",
                bytes[0]
            )));
        }
        let mut out = [0u8; 33];
        out.copy_from_slice(&bytes);
        Ok(out)
    }

    /// Fetch the SEC1-uncompressed secp256k1 wallet pubkey for
    /// `user_id` from `GET /anima/custody/get_wallet_pubkey/{user_id}`.
    async fn fetch_wallet_pubkey(
        client: &reqwest::Client,
        base_url: &str,
        user_id: &str,
        cap: &Arc<Mutex<TierUserCap>>,
    ) -> AnimaResult<[u8; 65]> {
        let url = format!("{base_url}/anima/custody/get_wallet_pubkey/{user_id}");
        let token = cap.lock().token.clone();
        let resp = client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima get_wallet_pubkey: {e}")))?;
        if !resp.status().is_success() {
            return Err(AnimaError::Crypto(format!(
                "RemoteAnima get_wallet_pubkey non-success: {}",
                resp.status()
            )));
        }
        let body: PubkeyResp = resp
            .json()
            .await
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima get_wallet_pubkey parse: {e}")))?;
        let bytes = decode_pubkey_b64(&body.pubkey_b64)
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima wallet pubkey b64: {e}")))?;
        if bytes.len() != 65 {
            return Err(AnimaError::Crypto(format!(
                "RemoteAnima wallet pubkey: expected 65 bytes (SEC1 uncompressed), got {}",
                bytes.len()
            )));
        }
        if bytes[0] != 0x04 {
            return Err(AnimaError::Crypto(format!(
                "RemoteAnima wallet pubkey: SEC1 uncompressed prefix must be 0x04, got 0x{:02x}",
                bytes[0]
            )));
        }
        let mut out = [0u8; 65];
        out.copy_from_slice(&bytes);
        Ok(out)
    }

    /// Async signing: POST a 32-byte digest to
    /// `/anima/custody/sign_auth` and return the 64-byte raw r||s.
    async fn sign_auth_async(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        let url = format!("{}/anima/custody/sign_auth", self.base_url);
        let body = SignBody {
            user_id: self.user_id.clone(),
            digest_b64: B64_STANDARD.encode(digest),
        };
        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.cap_token())
            .json(&body)
            .send()
            .await
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima sign_auth: {e}")))?;
        if !resp.status().is_success() {
            return Err(AnimaError::Crypto(format!(
                "RemoteAnima sign_auth non-success: {}",
                resp.status()
            )));
        }
        let parsed: SignResp = resp
            .json()
            .await
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima sign_auth parse: {e}")))?;
        let raw = decode_signature_b64(&parsed.signature_b64)
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima sign_auth b64: {e}")))?;
        if raw.len() != 64 {
            return Err(AnimaError::Crypto(format!(
                "RemoteAnima sign_auth: expected 64-byte raw r||s, got {}",
                raw.len()
            )));
        }
        let mut out = [0u8; 64];
        out.copy_from_slice(&raw);
        Ok(out)
    }

    /// Async signing: POST a 32-byte digest to
    /// `/anima/custody/sign_wallet` and return the 64-byte raw r||s.
    /// The recovery byte is computed by [`Self::compute_v_byte`].
    async fn sign_wallet_async(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        let url = format!("{}/anima/custody/sign_wallet", self.base_url);
        let body = SignBody {
            user_id: self.user_id.clone(),
            digest_b64: B64_STANDARD.encode(digest),
        };
        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.cap_token())
            .json(&body)
            .send()
            .await
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima sign_wallet: {e}")))?;
        if !resp.status().is_success() {
            return Err(AnimaError::Crypto(format!(
                "RemoteAnima sign_wallet non-success: {}",
                resp.status()
            )));
        }
        let parsed: SignResp = resp
            .json()
            .await
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima sign_wallet parse: {e}")))?;
        let raw = decode_signature_b64(&parsed.signature_b64)
            .map_err(|e| AnimaError::Crypto(format!("RemoteAnima sign_wallet b64: {e}")))?;
        if raw.len() != 64 {
            return Err(AnimaError::Crypto(format!(
                "RemoteAnima sign_wallet: expected 64-byte raw r||s, got {}",
                raw.len()
            )));
        }
        let mut out = [0u8; 64];
        out.copy_from_slice(&raw);
        Ok(out)
    }

    /// Compute the EVM `v` recovery byte for a secp256k1 signature.
    /// Tries both candidate recovery ids (0/1) and selects the one
    /// whose recovered pubkey matches the cached wallet pubkey.
    /// Returns the legacy `v ∈ {27, 28}` form per the
    /// `EvmSignature` convention (matches `VaultTransitAnima` and
    /// `haima-wallet::LocalSigner::sign_transfer_authorization`).
    fn compute_v_byte(&self, digest: &[u8; 32], r_s: &[u8; 64]) -> AnimaResult<u8> {
        let signature = K256Signature::from_slice(r_s)
            .map_err(|e| AnimaError::Crypto(format!("secp256k1 sig parse: {e}")))?;
        let expected_pubkey =
            K256VerifyingKey::from_sec1_bytes(&self.wallet_pubkey_uncompressed)
                .map_err(|e| AnimaError::Crypto(format!("expected pubkey parse: {e}")))?;
        for cand in 0u8..=1 {
            let recid = match RecoveryId::try_from(cand) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if let Ok(recovered) = K256VerifyingKey::recover_from_prehash(digest, &signature, recid)
                && recovered == expected_pubkey
            {
                return Ok(cand + 27);
            }
        }
        Err(AnimaError::Crypto(
            "RemoteAnima ecrecover: neither recovery id matched the wallet pubkey".into(),
        ))
    }

    /// Drive an async future from the sync trait method.
    ///
    /// **IMPORTANT — runtime-flavor caveat:** `block_in_place` panics
    /// if called from a `current_thread` tokio runtime. Callers MUST
    /// use a multi-thread tokio runtime — integration tests use
    /// `#[tokio::test(flavor = "multi_thread")]`. Calling
    /// `RemoteAnima::sign_*` from inside a Future on a
    /// `current_thread` runtime will panic. Same caveat as
    /// `SomaCustody::block_on`.
    fn block_on<F, T>(&self, fut: F) -> AnimaResult<T>
    where
        F: std::future::Future<Output = AnimaResult<T>>,
    {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
            Err(_) => {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| AnimaError::Crypto(format!("RemoteAnima rt build: {e}")))?;
                rt.block_on(fut)
            }
        }
    }
}

impl AnimaCustody for RemoteAnima {
    fn user_did(&self) -> &str {
        &self.user_did
    }

    fn auth_pubkey(&self) -> [u8; 33] {
        self.auth_pubkey
    }

    fn wallet_address(&self) -> Option<&WalletAddress> {
        Some(&self.wallet_address)
    }

    fn sign_jws(&self, claims: &Value) -> AnimaResult<String> {
        // Build the JWS header + body locally (URL_SAFE_NO_PAD per
        // RFC 7515 §3) then ask lifegw to sign the SHA-256 of the
        // signing input.
        use sha2::{Digest, Sha256};

        let header = serde_json::json!({
            "alg": "ES256",
            "typ": "JWT",
            "kid": self.user_did,
        });
        let header_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&header)
                .map_err(|e| AnimaError::Crypto(format!("RemoteAnima encode header: {e}")))?,
        );
        let body_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(claims)
                .map_err(|e| AnimaError::Crypto(format!("RemoteAnima encode body: {e}")))?,
        );
        let signing_input = format!("{header_b64}.{body_b64}");
        let digest_array = {
            let hash = Sha256::digest(signing_input.as_bytes());
            let mut out = [0u8; 32];
            out.copy_from_slice(&hash);
            out
        };
        let sig = self.block_on(self.sign_auth_async(&digest_array))?;
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
        Ok(format!("{signing_input}.{sig_b64}"))
    }

    fn sign_digest(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        self.block_on(self.sign_auth_async(digest))
    }

    fn sign_evm_tx(&self, tx: &TxRequest) -> AnimaResult<EvmSignature> {
        // Mirror the VaultTransitAnima / SomaCustody path: parse
        // request, RLP-encode EIP-1559 envelope, Keccak-256, ask
        // remote to sign the prehash, ecrecover v.
        let chain_id = rlp::parse_eip155_chain_id(&tx.chain)
            .map_err(|e| AnimaError::Crypto(format!("evm tx: {e}")))?;
        let to = rlp::parse_address_20(&tx.to)
            .map_err(|e| AnimaError::Crypto(format!("evm tx to: {e}")))?;
        let value = rlp::parse_u256_str(&tx.value_wei)
            .map_err(|e| AnimaError::Crypto(format!("evm tx value: {e}")))?;
        let max_fee = rlp::parse_u256_str(&tx.max_fee_per_gas_wei)
            .map_err(|e| AnimaError::Crypto(format!("evm tx max_fee: {e}")))?;
        let max_priority = rlp::parse_u256_str(&tx.max_priority_fee_per_gas_wei)
            .map_err(|e| AnimaError::Crypto(format!("evm tx max_priority: {e}")))?;
        let data = rlp::parse_data_hex(&tx.data_hex)
            .map_err(|e| AnimaError::Crypto(format!("evm tx data: {e}")))?;
        let envelope = rlp::encode_eip1559_unsigned(
            chain_id,
            tx.nonce,
            &max_priority,
            &max_fee,
            tx.gas_limit,
            &to,
            &value,
            &data,
        );
        let digest = rlp::keccak256(&envelope);
        let r_s = self.block_on(self.sign_wallet_async(&digest))?;
        let v = self.compute_v_byte(&digest, &r_s)?;
        let mut out = Vec::with_capacity(65);
        out.extend_from_slice(&r_s);
        out.push(v);
        Ok(EvmSignature::from_bytes(out))
    }

    fn sign_eip712(
        &self,
        domain: &Eip712Domain,
        types: &Value,
        message: &Value,
    ) -> AnimaResult<EvmSignature> {
        // SPEC-D-DEVIATION (remote): same EIP-3009 only constraint as
        // the rest of the family. Generic encoder deferred.
        let primary = types
            .get("primaryType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if primary != "TransferWithAuthorization"
            && !(message.get("from").is_some() && message.get("validAfter").is_some())
        {
            return Err(AnimaError::Crypto(
                "eip712: only EIP-3009 TransferWithAuthorization is supported in D-Sub-C \
                 (matches D-Sub-A/B/E limitation; generic encoder deferred)"
                    .to_string(),
            ));
        }

        use haima_wallet::eip712::{hash_transfer_authorization, parse_eth_address};

        let from = message
            .get("from")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AnimaError::Crypto("eip712: missing 'from'".into()))?;
        let to = message
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AnimaError::Crypto("eip712: missing 'to'".into()))?;
        let value: u64 = message
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AnimaError::Crypto("eip712: missing 'value' (string)".into()))?
            .parse()
            .map_err(|e| AnimaError::Crypto(format!("eip712 value: {e}")))?;
        let valid_after: u64 = message
            .get("validAfter")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AnimaError::Crypto("eip712: missing 'validAfter'".into()))?
            .parse()
            .map_err(|e| AnimaError::Crypto(format!("eip712 validAfter: {e}")))?;
        let valid_before: u64 = message
            .get("validBefore")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AnimaError::Crypto("eip712: missing 'validBefore'".into()))?
            .parse()
            .map_err(|e| AnimaError::Crypto(format!("eip712 validBefore: {e}")))?;
        let nonce_hex = message
            .get("nonce")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AnimaError::Crypto("eip712: missing 'nonce'".into()))?;
        let nonce_bytes = hex::decode(nonce_hex.trim_start_matches("0x"))
            .map_err(|e| AnimaError::Crypto(format!("eip712 nonce hex: {e}")))?;
        if nonce_bytes.len() != 32 {
            return Err(AnimaError::Crypto(format!(
                "eip712 nonce must be 32 bytes, got {}",
                nonce_bytes.len()
            )));
        }
        let mut nonce = [0u8; 32];
        nonce.copy_from_slice(&nonce_bytes);

        let from_b =
            parse_eth_address(from).map_err(|e| AnimaError::Crypto(format!("eip712 from: {e}")))?;
        let to_b =
            parse_eth_address(to).map_err(|e| AnimaError::Crypto(format!("eip712 to: {e}")))?;

        let digest = hash_transfer_authorization(
            domain,
            &from_b,
            &to_b,
            value,
            valid_after,
            valid_before,
            &nonce,
        );
        let r_s = self.block_on(self.sign_wallet_async(&digest))?;
        let v = self.compute_v_byte(&digest, &r_s)?;
        let mut out = Vec::with_capacity(65);
        out.extend_from_slice(&r_s);
        out.push(v);
        Ok(EvmSignature::from_bytes(out))
    }

    fn rotate(&self) -> AnimaResult<(DidRotationEvent, Arc<dyn AnimaCustody>)> {
        // SPEC-D-DEVIATION (remote): rotation is journal-driven, not
        // RPC-driven. Even though lifegw COULD expose a `rotate` route,
        // doing so would create two divergent rotation surfaces and
        // break the Spec D L4-D10 invariant that rotations are
        // documented in the journal. Mirrors `SomaCustody::rotate`.
        Err(AnimaError::Crypto(
            "RemoteAnima::rotate is journal-driven; \
             use anima-lago::write_rotation_event from the server-side anima daemon \
             (typically VaultTransitAnima) and reconstruct a fresh RemoteAnima \
             after observing the anima.identity_rotated event"
                .to_string(),
        ))
    }

    fn backend_kind(&self) -> BackendKind {
        BackendKind::Remote
    }

    fn export_identity_document(&self) -> AnimaResult<AgentIdentityDocument> {
        let public_key_multibase = format!("z{}", bs58::encode(self.auth_pubkey).into_string());
        let did = self.user_did.clone();
        let vm = VerificationMethod {
            id: format!("{did}#key-1"),
            method_type: "JsonWebKey2020".to_string(),
            controller: did.clone(),
            public_key_multibase,
        };
        // D-Sub-C: rotation_chain stays empty here. The chain is
        // populated by `crate::rotation::walk_rotation_chain` via the
        // anima-lago bridge — same pattern as SomaCustody.
        let doc = IdentityDocumentBuilder::new(
            did,
            "anima-self".to_string(),
            format!("remote custody (lifegw {})", self.base_url),
            String::new(),
        )
        .agent_type(AgentType::Hosted)
        .verification_method(vm)
        .build();
        Ok(doc)
    }
}

/// Decode a pubkey base64 string. Tries standard b64 first, then
/// URL-safe (some JS callers default to urlsafe).
fn decode_pubkey_b64(s: &str) -> Result<Vec<u8>, String> {
    if let Ok(b) = B64_STANDARD.decode(s) {
        return Ok(b);
    }
    URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|e| format!("base64 decode: {e}"))
}

/// Decode a signature base64 string. Same dual-strategy as pubkeys.
fn decode_signature_b64(s: &str) -> Result<Vec<u8>, String> {
    if let Ok(b) = B64_STANDARD.decode(s) {
        return Ok(b);
    }
    URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|e| format!("base64 decode: {e}"))
}

/// Derive an EVM address from a 65-byte uncompressed secp256k1
/// pubkey. Mirror of `crate::vault::derive_wallet_address` and
/// `haima_wallet::evm::derive_address`. Returns the lowercase hex
/// `0x…` string (40 chars after the prefix).
fn derive_wallet_address(uncompressed: &[u8; 65]) -> AnimaResult<String> {
    if uncompressed[0] != 0x04 {
        return Err(AnimaError::Crypto(format!(
            "derive_wallet_address: uncompressed point must start with 0x04, got 0x{:02x}",
            uncompressed[0]
        )));
    }
    let hash = Keccak256::digest(&uncompressed[1..]);
    let address_bytes = &hash[12..];
    Ok(format!("0x{}", hex::encode(address_bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_kind_is_remote() {
        // Construct a RemoteAnima by hand (skipping the network call)
        // to assert the trait method returns Remote. We can do this
        // because the struct fields are crate-private and we're in
        // the same crate.
        let auth_pubkey = [0x02u8; 33];
        let mut wallet_uncompressed = [0u8; 65];
        wallet_uncompressed[0] = 0x04;
        let wallet_address = WalletAddress {
            address: "0x0000000000000000000000000000000000000000".into(),
            chain: ChainId::base(),
        };
        let cap = TierUserCap {
            token: "test".into(),
            expires_at_unix: i64::MAX,
        };
        let client = Arc::new(reqwest::Client::new());
        let anima = RemoteAnima {
            base_url: "http://localhost".into(),
            user_id: "test-user".into(),
            auth_pubkey,
            wallet_pubkey_uncompressed: wallet_uncompressed,
            wallet_address,
            user_did: "did:key:zDnTest".into(),
            cap: Arc::new(Mutex::new(cap)),
            client,
        };
        assert_eq!(anima.backend_kind(), BackendKind::Remote);
        assert!(anima.wallet_address().is_some());
        assert_eq!(anima.user_did(), "did:key:zDnTest");
    }

    #[test]
    fn rotate_returns_journal_directive() {
        let auth_pubkey = [0x02u8; 33];
        let mut wallet_uncompressed = [0u8; 65];
        wallet_uncompressed[0] = 0x04;
        let cap = TierUserCap {
            token: "test".into(),
            expires_at_unix: i64::MAX,
        };
        let client = Arc::new(reqwest::Client::new());
        let anima = RemoteAnima {
            base_url: "http://localhost".into(),
            user_id: "test-user".into(),
            auth_pubkey,
            wallet_pubkey_uncompressed: wallet_uncompressed,
            wallet_address: WalletAddress {
                address: "0x0".into(),
                chain: ChainId::base(),
            },
            user_did: "did:key:zDnTest".into(),
            cap: Arc::new(Mutex::new(cap)),
            client,
        };
        let err = match anima.rotate() {
            Ok(_) => panic!("rotate must error"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("journal-driven"),
            "error must point at journal flow (got: {msg})"
        );
        assert!(
            msg.contains("write_rotation_event"),
            "error must reference helper (got: {msg})"
        );
    }

    #[test]
    fn derive_wallet_address_format() {
        // Use a known test vector — secp256k1 public key from
        // privkey [1u8; 32]. Same pattern as
        // crate::vault::derive_wallet_address tests.
        use k256::SecretKey;
        use k256::elliptic_curve::sec1::ToEncodedPoint;
        let sk = SecretKey::from_bytes(&[1u8; 32].into()).unwrap();
        let pk = sk.public_key();
        let pt = pk.to_encoded_point(false);
        let bytes = pt.as_bytes();
        let mut uncompressed = [0u8; 65];
        uncompressed.copy_from_slice(bytes);
        let addr = derive_wallet_address(&uncompressed).unwrap();
        assert!(addr.starts_with("0x"));
        assert_eq!(addr.len(), 42);
    }

    #[test]
    fn derive_wallet_address_rejects_bad_prefix() {
        let mut bad = [0u8; 65];
        bad[0] = 0x02; // wrong prefix
        let err = derive_wallet_address(&bad).expect_err("must reject non-0x04 prefix");
        let msg = format!("{err}");
        assert!(
            msg.contains("0x04"),
            "error must mention required prefix (got: {msg})"
        );
    }

    #[test]
    fn cap_can_be_replaced() {
        let auth_pubkey = [0x02u8; 33];
        let mut wallet_uncompressed = [0u8; 65];
        wallet_uncompressed[0] = 0x04;
        let cap = TierUserCap {
            token: "old".into(),
            expires_at_unix: 100,
        };
        let client = Arc::new(reqwest::Client::new());
        let anima = RemoteAnima {
            base_url: "http://localhost".into(),
            user_id: "test-user".into(),
            auth_pubkey,
            wallet_pubkey_uncompressed: wallet_uncompressed,
            wallet_address: WalletAddress {
                address: "0x0".into(),
                chain: ChainId::base(),
            },
            user_did: "did:key:zDnTest".into(),
            cap: Arc::new(Mutex::new(cap)),
            client,
        };
        assert_eq!(anima.current_cap().token, "old");
        anima.set_cap(TierUserCap {
            token: "new".into(),
            expires_at_unix: 200,
        });
        assert_eq!(anima.current_cap().token, "new");
        assert_eq!(anima.current_cap().expires_at_unix, 200);
    }
}
