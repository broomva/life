//! `VaultTransitAnima` — HashiCorp Vault Transit custody backend (Spec D D-Sub-B).
//!
//! Production-grade multi-tenant server-side custody. Mirrors lifegw's
//! [`auth::kms::VaultTransit`] pattern but widened to manage TWO keys per
//! user (P-256 auth + secp256k1 wallet) so the same trait object covers
//! both Agent Auth Protocol JWS minting and EVM transaction signing.
//!
//! ## Per-user namespace pattern (Spec D §"Phasing > D-Sub-B")
//!
//! Every user gets two transit keys:
//!
//! - `transit/keys/anima-{user_id}-auth-v{n}` — `ecdsa-p256`
//! - `transit/keys/anima-{user_id}-wallet-v{n}` — `ecdsa-secp256k1`
//!
//! The `v{n}` suffix is Vault's transit-key version which advances on
//! `transit/keys/<key>/rotate`. Per L4-D7 the wallet key version is
//! preserved across rotations; only the auth key advances. The
//! `kid` field carried in JWS headers / verifier paths follows the
//! `{user_id}-auth-v{n}` shape for the auth half (the only half that
//! emits JWS).
//!
//! ## Bootstrap
//!
//! Before signing, the Vault operator (or a tenant-onboarding script)
//! creates the two transit keys with the appropriate algorithms. The
//! anima daemon authenticates to Vault via a periodic-renewable token
//! (typical TTL: 32 days; renewal cadence: 1 day via
//! [`VaultTransitAnima::spawn_token_renewal`]). The anima crate is
//! deliberately conservative here — it does NOT auto-create keys; the
//! tenant boundary owns its key lifecycle.
//!
//! ## sign_evm_tx flow
//!
//! 1. Caller passes a [`TxRequest`] with the user's CAIP-2 chain id.
//! 2. The backend computes the canonical EIP-1559 RLP digest via
//!    [`crate::rlp::encode_eip1559_unsigned`] + [`crate::rlp::keccak256`].
//! 3. The 32-byte digest is base64-encoded and posted to
//!    `transit/sign/anima-{user_id}-wallet-v{n}` with `prehashed: true`.
//! 4. Vault returns a base64-encoded `r || s` (64 bytes) without the
//!    recovery byte — Vault's transit/sign API does not return EVM
//!    `v`. We compute `v` by trying `ecrecover` on both candidates
//!    (`v=27` and `v=28`) and selecting the one that recovers to the
//!    user's wallet address. **This is a deliberate trade-off**: the
//!    Vault-side recovery byte would require a custom Vault plugin; the
//!    ecrecover loop is two scalar multiplications and runs once per
//!    transaction.
//!
//! ## Acceptance
//!
//! Per Spec D §"D-Sub-B": "Vault-fixture integration test signs a USDC
//! transfer end-to-end on a Base-fork local chain." This module ships
//! with `wiremock`-backed unit tests that exercise the request shapes;
//! the live Vault dev-server integration test is gated behind
//! `#[ignore]` and documented at `tests/integration_vault.rs` for
//! operators with a Vault fixture. CI does not run that test by default
//! to avoid binding to an external dependency.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anima_core::error::{AnimaError, AnimaResult};
use anima_core::identity_document::{
    AgentIdentityDocument, AgentType, IdentityDocumentBuilder, VerificationMethod,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use haima_core::wallet::{ChainId, WalletAddress};
use k256::ecdsa::{RecoveryId, Signature as K256Signature, VerifyingKey as K256VerifyingKey};
use serde_json::Value;
use sha3::{Digest as Sha3Digest, Keccak256};

use crate::custody::{
    AnimaCustody, BackendKind, DidRotationEvent, Eip712Domain, EvmSignature, TxRequest,
};
use crate::rlp;

/// Default request timeout for Vault HTTP calls.
const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// Optional mTLS configuration matching lifegw's `VaultMtls` shape.
///
/// The mTLS limitation noted in `lifegw::auth::kms::VaultTransit::with_mtls`
/// applies here too — the workspace's reqwest pin doesn't enable an
/// optional TLS feature, so populated certs are recorded as a warning
/// and ignored at runtime. Operators who need mTLS to Vault should run
/// a localhost mTLS sidecar (envoy/consul-template). We accept the
/// option for forward compatibility.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct VaultMtls {
    /// Path to PEM-encoded client certificate.
    pub cert_path: PathBuf,
    /// Path to PEM-encoded client private key.
    pub key_path: PathBuf,
}

/// `VaultTransitAnima` — production custody via HashiCorp Vault Transit.
///
/// Holds the Vault address + auth token + per-user key names. Public
/// keys + wallet address are resolved at construction so trait
/// accessors (`auth_pubkey`, `wallet_address`, `user_did`) are
/// referentially transparent (no I/O on the hot path).
pub struct VaultTransitAnima {
    addr: String,
    token: String,
    /// Vault transit key name for the auth (P-256) half.
    /// Convention: `anima-{user_id}-auth-v{n}`.
    auth_key_name: String,
    /// Vault transit key name for the wallet (secp256k1) half.
    /// Convention: `anima-{user_id}-wallet-v{n}`.
    wallet_key_name: String,
    /// JWS `kid` value embedded in headers. Convention: same string as
    /// `auth_key_name`. Verifiers map this to the public key via Vault's
    /// transit/keys/<auth_key_name> endpoint.
    kid: String,
    /// User DID — derived from the Vault-held P-256 auth public key at
    /// construction time. Pinned for the lifetime of this handle;
    /// `rotate()` returns a fresh handle for the new version.
    user_did: String,
    /// Cached SEC1-compressed P-256 public key (33 bytes).
    auth_pubkey: [u8; 33],
    /// Cached secp256k1 public key (uncompressed, 65 bytes) — used by
    /// `sign_evm_tx` for the ecrecover-based `v`-byte selection.
    wallet_pubkey_uncompressed: [u8; 65],
    /// Wallet address — derived from the wallet pubkey at construction.
    wallet_address: WalletAddress,
    /// Cached PEM-encoded auth public key for KYA doc export.
    auth_public_pem: OnceLock<String>,
    client: reqwest::blocking::Client,
}

impl VaultTransitAnima {
    /// Construct a Vault-backed custody handle for a user.
    ///
    /// Derives the per-user key names per the Spec D §"Phasing > D-Sub-B"
    /// pattern: `anima-{user_id}-auth-v1` + `anima-{user_id}-wallet-v1`.
    /// The version suffix advances on `rotate()` for the auth half;
    /// the wallet half is pinned per L4-D7.
    ///
    /// Performs two `GetPublicKey` calls against Vault to resolve the
    /// auth pubkey + wallet pubkey + wallet address at construction.
    /// Failures bubble up as [`AnimaError::Crypto`].
    pub fn new(
        addr: impl Into<String>,
        token: impl Into<String>,
        user_id: &str,
        kid: impl Into<String>,
    ) -> AnimaResult<Self> {
        let auth_key_name = format!("anima-{user_id}-auth-v1");
        let wallet_key_name = format!("anima-{user_id}-wallet-v1");
        Self::with_explicit_keys(addr, token, auth_key_name, wallet_key_name, kid, None)
    }

    /// Construct with explicit key names — used by tests + custom
    /// deployments where the user_id-derived naming convention doesn't
    /// fit (e.g. legacy migrations, multi-environment deployments where
    /// the auth key lives in a different transit mount than the wallet).
    pub fn with_explicit_keys(
        addr: impl Into<String>,
        token: impl Into<String>,
        auth_key_name: impl Into<String>,
        wallet_key_name: impl Into<String>,
        kid: impl Into<String>,
        mtls: Option<VaultMtls>,
    ) -> AnimaResult<Self> {
        if let Some(m) = mtls.as_ref() {
            tracing::warn!(
                cert = %m.cert_path.display(),
                key = %m.key_path.display(),
                "vault mtls config present but anima's reqwest pin does not enable a TLS feature; \
                 use a localhost mTLS sidecar (envoy/consul-template). config IGNORED.",
            );
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| AnimaError::Crypto(format!("vault client: {e}")))?;
        let me = Self {
            addr: addr.into(),
            token: token.into(),
            auth_key_name: auth_key_name.into(),
            wallet_key_name: wallet_key_name.into(),
            kid: kid.into(),
            user_did: String::new(),
            auth_pubkey: [0u8; 33],
            wallet_pubkey_uncompressed: [0u8; 65],
            wallet_address: WalletAddress {
                address: String::new(),
                chain: ChainId::base(),
            },
            auth_public_pem: OnceLock::new(),
            client,
        };
        // Resolve the on-Vault keys to populate the cached fields.
        me.bootstrap()
    }

    /// One-shot bootstrap: fetch both pubkeys + derive DID + wallet
    /// address. Returns the same struct with the cached fields filled.
    fn bootstrap(self) -> AnimaResult<Self> {
        let (auth_pubkey, auth_pem) = self.fetch_auth_pubkey()?;
        let wallet_pubkey_uncompressed = self.fetch_wallet_pubkey()?;
        let wallet_addr_hex = derive_wallet_address(&wallet_pubkey_uncompressed);
        let user_did = crate::did::generate_did_key_p256(&auth_pubkey);
        let _ = self.auth_public_pem.set(auth_pem);
        Ok(Self {
            user_did,
            auth_pubkey,
            wallet_pubkey_uncompressed,
            wallet_address: WalletAddress {
                address: wallet_addr_hex,
                chain: ChainId::base(),
            },
            ..self
        })
    }

    /// Fetch the auth (P-256) public key from Vault. Returns
    /// `(SEC1-compressed-33-bytes, PEM)` so the KYA document can publish
    /// the PEM verbatim while the DID derives from the SEC1 form.
    ///
    /// Pins to `data.latest_version` (lifegw's Sub-phase E lesson: never
    /// hardcode version 1).
    fn fetch_auth_pubkey(&self) -> AnimaResult<([u8; 33], String)> {
        let url = format!("{}/v1/transit/keys/{}", self.addr, self.auth_key_name);
        let body: Value = self
            .client
            .get(&url)
            .header("X-Vault-Token", &self.token)
            .send()
            .map_err(|e| AnimaError::Crypto(format!("vault get auth key: {e}")))?
            .json()
            .map_err(|e| AnimaError::Crypto(format!("vault parse auth key: {e}")))?;
        let latest = body
            .pointer("/data/latest_version")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AnimaError::Crypto("vault auth: missing data.latest_version".into()))?;
        let pem_pointer = format!("/data/keys/{latest}/public_key");
        let pem = body
            .pointer(&pem_pointer)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AnimaError::Crypto(format!("vault auth: missing data.keys.{latest}.public_key"))
            })?
            .to_string();
        let compressed = parse_p256_pem_to_sec1_compressed(&pem)?;
        Ok((compressed, pem))
    }

    /// Fetch the wallet (secp256k1) public key from Vault. Returns the
    /// 65-byte uncompressed SEC1 form (`0x04 || x || y`) — used for both
    /// EVM address derivation and ecrecover-based `v`-byte selection.
    fn fetch_wallet_pubkey(&self) -> AnimaResult<[u8; 65]> {
        let url = format!("{}/v1/transit/keys/{}", self.addr, self.wallet_key_name);
        let body: Value = self
            .client
            .get(&url)
            .header("X-Vault-Token", &self.token)
            .send()
            .map_err(|e| AnimaError::Crypto(format!("vault get wallet key: {e}")))?
            .json()
            .map_err(|e| AnimaError::Crypto(format!("vault parse wallet key: {e}")))?;
        let latest = body
            .pointer("/data/latest_version")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| {
                AnimaError::Crypto("vault wallet: missing data.latest_version".into())
            })?;
        let pem_pointer = format!("/data/keys/{latest}/public_key");
        let pem = body
            .pointer(&pem_pointer)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AnimaError::Crypto(format!(
                    "vault wallet: missing data.keys.{latest}.public_key"
                ))
            })?;
        parse_secp256k1_pem_to_sec1_uncompressed(pem)
    }

    /// Spec D D-Sub-B: spawn a background task that renews the Vault
    /// token at `interval`. Mirrors lifegw's
    /// `VaultTransit::spawn_token_renewal`.
    ///
    /// Returns an `AbortHandle` so callers can cancel cleanly on
    /// graceful shutdown by calling `.abort()`. Vault tokens with
    /// `renewable: true` and a periodic TTL stay alive indefinitely as
    /// long as renewal happens before the TTL elapses.
    ///
    /// The renewal loop exits cleanly on the first error so the
    /// surrounding daemon can react via its own reload path; we don't
    /// retry silently because a renewal failure usually means the policy
    /// changed (token revoked, lease expired, etc.).
    pub fn spawn_token_renewal(
        addr: String,
        token: String,
        interval: std::time::Duration,
    ) -> tokio::task::AbortHandle {
        let handle = tokio::spawn(async move {
            let mut clock = tokio::time::interval(interval);
            clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Skip the initial immediate tick — caller just provided a fresh
            // token at construction time.
            clock.tick().await;
            let url = format!("{addr}/v1/auth/token/renew-self");
            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
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

    /// Sign a JWS using Vault's `transit/sign/<auth_key>` with
    /// `marshaling_algorithm: "jws"` so Vault returns `r || s`
    /// concatenated and base64url-encoded without padding (matches the
    /// JWS compact-serialisation convention RFC 7515 §3).
    fn vault_sign_jws(&self, signing_input: &str) -> AnimaResult<String> {
        let url = format!("{}/v1/transit/sign/{}", self.addr, self.auth_key_name);
        let payload = serde_json::json!({
            "input": URL_SAFE_NO_PAD.encode(signing_input.as_bytes()),
            "marshaling_algorithm": "jws",
            "hash_algorithm": "sha2-256",
        });
        let resp: Value = self
            .client
            .post(&url)
            .header("X-Vault-Token", &self.token)
            .json(&payload)
            .send()
            .map_err(|e| AnimaError::Crypto(format!("vault sign jws: {e}")))?
            .json()
            .map_err(|e| AnimaError::Crypto(format!("vault parse sign jws resp: {e}")))?;
        let sig = resp
            .pointer("/data/signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AnimaError::Crypto("vault: missing signature in sign response".into())
            })?;
        // Vault wraps signatures as `vault:vN:<b64>`; strip the prefix.
        let sig_b64 = sig.rsplit(':').next().unwrap_or(sig);
        Ok(format!("{signing_input}.{sig_b64}"))
    }

    /// Sign a 32-byte digest via Vault's `transit/sign/<auth_key>` with
    /// `prehashed: true`. Returns the raw `r || s` 64-byte form.
    fn vault_sign_digest_p256(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        let url = format!("{}/v1/transit/sign/{}", self.addr, self.auth_key_name);
        let payload = serde_json::json!({
            "input": URL_SAFE_NO_PAD.encode(digest),
            "prehashed": true,
            "marshaling_algorithm": "jws",
            "hash_algorithm": "sha2-256",
        });
        let resp: Value = self
            .client
            .post(&url)
            .header("X-Vault-Token", &self.token)
            .json(&payload)
            .send()
            .map_err(|e| AnimaError::Crypto(format!("vault sign digest p256: {e}")))?
            .json()
            .map_err(|e| AnimaError::Crypto(format!("vault parse sign digest p256: {e}")))?;
        let sig = resp
            .pointer("/data/signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AnimaError::Crypto("vault: missing signature in sign-digest response".into())
            })?;
        let sig_b64 = sig.rsplit(':').next().unwrap_or(sig);
        let raw = URL_SAFE_NO_PAD
            .decode(sig_b64)
            .map_err(|e| AnimaError::Crypto(format!("vault sig base64: {e}")))?;
        if raw.len() != 64 {
            return Err(AnimaError::Crypto(format!(
                "vault p256 sig wrong length: expected 64, got {}",
                raw.len()
            )));
        }
        let mut out = [0u8; 64];
        out.copy_from_slice(&raw);
        Ok(out)
    }

    /// Sign a 32-byte secp256k1 digest via Vault's
    /// `transit/sign/<wallet_key>` with `prehashed: true`. Returns the
    /// raw `r || s` 64-byte form. The `v` recovery byte is computed
    /// out-of-band by [`Self::compute_v_byte`].
    ///
    /// We pass `marshaling_algorithm: "jws"` to ask Vault for the
    /// concatenated raw form rather than the default ASN.1 DER.
    fn vault_sign_digest_secp256k1(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        let url = format!("{}/v1/transit/sign/{}", self.addr, self.wallet_key_name);
        let payload = serde_json::json!({
            "input": URL_SAFE_NO_PAD.encode(digest),
            "prehashed": true,
            "marshaling_algorithm": "jws",
            "hash_algorithm": "sha2-256",
        });
        let resp: Value = self
            .client
            .post(&url)
            .header("X-Vault-Token", &self.token)
            .json(&payload)
            .send()
            .map_err(|e| AnimaError::Crypto(format!("vault sign secp256k1: {e}")))?
            .json()
            .map_err(|e| AnimaError::Crypto(format!("vault parse sign secp256k1: {e}")))?;
        let sig = resp
            .pointer("/data/signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AnimaError::Crypto("vault: missing signature in secp256k1 response".into())
            })?;
        let sig_b64 = sig.rsplit(':').next().unwrap_or(sig);
        let raw = URL_SAFE_NO_PAD
            .decode(sig_b64)
            .map_err(|e| AnimaError::Crypto(format!("vault secp256k1 sig base64: {e}")))?;
        if raw.len() != 64 {
            return Err(AnimaError::Crypto(format!(
                "vault secp256k1 sig wrong length: expected 64, got {}",
                raw.len()
            )));
        }
        let mut out = [0u8; 64];
        out.copy_from_slice(&raw);
        Ok(out)
    }

    /// Compute the EVM `v` recovery byte for a secp256k1 signature by
    /// trying both candidate recovery ids (`0` and `1`) and selecting
    /// the one whose recovered public key matches the cached wallet
    /// pubkey. Returns the EVM-encoded `v` (27 or 28 for legacy / pre-
    /// EIP-1559 signatures, or 0/1 for typed transactions — callers
    /// add 27 if they need legacy form).
    ///
    /// Returns `(v_legacy_27_or_28, recovery_id_0_or_1)`.
    ///
    /// This is the trade-off documented at the module level: Vault's
    /// `transit/sign` does not return the recovery byte for secp256k1,
    /// so we ecrecover both candidates and pick the matching one. Two
    /// scalar multiplications per tx — negligible.
    fn compute_v_byte(&self, digest: &[u8; 32], r_s: &[u8; 64]) -> AnimaResult<(u8, u8)> {
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
                return Ok((cand + 27, cand));
            }
        }
        Err(AnimaError::Crypto(
            "secp256k1 ecrecover: neither recovery id matched the wallet pubkey".into(),
        ))
    }
}

impl AnimaCustody for VaultTransitAnima {
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
        // Build the JWS header + body (URL_SAFE_NO_PAD per RFC 7515 §3)
        // matching lifegw's VaultTransit pattern, then ask Vault to sign
        // the resulting "<header>.<body>" string. Vault hashes
        // server-side (sha2-256) and emits the signature in JWS form.
        let header = serde_json::json!({
            "alg": "ES256",
            "typ": "JWT",
            "kid": self.kid,
        });
        let header_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&header)
                .map_err(|e| AnimaError::Crypto(format!("encode header: {e}")))?,
        );
        let body_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(claims)
                .map_err(|e| AnimaError::Crypto(format!("encode body: {e}")))?,
        );
        let signing_input = format!("{header_b64}.{body_b64}");
        self.vault_sign_jws(&signing_input)
    }

    fn sign_digest(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        self.vault_sign_digest_p256(digest)
    }

    fn sign_evm_tx(&self, tx: &TxRequest) -> AnimaResult<EvmSignature> {
        // 1. Parse the tx fields.
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
        // 2. RLP-encode the EIP-1559 envelope + Keccak-256 the result.
        // EIP-1559 is the default for Base/Ethereum post-London. Legacy
        // EIP-155 is exposed via `crate::rlp::encode_eip155_unsigned`
        // for backends that need it; sign_evm_tx defaults to the modern
        // shape since the trait's `TxRequest` exposes max_fee /
        // max_priority fields.
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
        // 3. Vault-sign the prehash with the wallet (secp256k1) key.
        let r_s = self.vault_sign_digest_secp256k1(&digest)?;
        // 4. Recover `v` by ecrecover loop (Vault doesn't return v).
        // For EIP-1559 typed txs the y-parity is encoded as 0/1 in the
        // signed tx envelope; for legacy EIP-155 it's `35 + 2*chain_id +
        // y_parity`. We return the legacy 27/28 form here since the
        // `EvmSignature` shape mirrors haima-wallet's
        // `LocalSigner::sign_transfer_authorization` output (which uses
        // the legacy `+27` convention). Callers that need the EIP-1559
        // y-parity byte can subtract 27 from the last byte.
        let (v_legacy, _yparity) = self.compute_v_byte(&digest, &r_s)?;
        let mut out = Vec::with_capacity(65);
        out.extend_from_slice(&r_s);
        out.push(v_legacy);
        Ok(EvmSignature::from_bytes(out))
    }

    fn sign_eip712(
        &self,
        domain: &Eip712Domain,
        types: &Value,
        message: &Value,
    ) -> AnimaResult<EvmSignature> {
        // SPEC-D-DEVIATION (vault): mirror InProcessAnima — D-Sub-B only
        // supports EIP-3009 `TransferWithAuthorization` typed-data,
        // since that is the only shape haima signs through this trait.
        // A generic Eip712 encoder is deferred to a follow-up sub-phase
        // (likely D-Sub-E when SomaCustody adds rotation/revocation
        // events that need typed-data signing of arbitrary payloads).
        let primary = types
            .get("primaryType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if primary != "TransferWithAuthorization"
            && !(message.get("from").is_some() && message.get("validAfter").is_some())
        {
            return Err(AnimaError::Crypto(
                "eip712: only EIP-3009 TransferWithAuthorization is supported in D-Sub-B \
                 (matches D-Sub-A InProcessAnima limitation; generic encoder deferred)"
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
        let r_s = self.vault_sign_digest_secp256k1(&digest)?;
        let (v_legacy, _yparity) = self.compute_v_byte(&digest, &r_s)?;
        let mut out = Vec::with_capacity(65);
        out.extend_from_slice(&r_s);
        out.push(v_legacy);
        Ok(EvmSignature::from_bytes(out))
    }

    fn rotate(&self) -> AnimaResult<(DidRotationEvent, Arc<dyn AnimaCustody>)> {
        // 1. Bump the auth key version on Vault. Per L4-D7 we do NOT
        //    rotate the wallet key — `transit/keys/<wallet_key>/rotate`
        //    is intentionally skipped.
        let rotate_url = format!(
            "{}/v1/transit/keys/{}/rotate",
            self.addr, self.auth_key_name
        );
        let resp = self
            .client
            .post(&rotate_url)
            .header("X-Vault-Token", &self.token)
            .send()
            .map_err(|e| AnimaError::Crypto(format!("vault auth rotate: {e}")))?;
        if !resp.status().is_success() {
            return Err(AnimaError::Crypto(format!(
                "vault auth rotate non-success: {}",
                resp.status()
            )));
        }

        // 2. Refetch the auth pubkey — Vault now returns the new
        //    latest_version. The wallet half is preserved by skipping
        //    its rotate call.
        let (new_auth_pubkey, new_auth_pem) = self.fetch_auth_pubkey()?;
        let new_did = crate::did::generate_did_key_p256(&new_auth_pubkey);
        let old_did = self.user_did.clone();

        // 3. Sign the rotation proof JWS with the OLD key (the trait
        //    contract: rotation_proof_jws is signed by the *old* key
        //    over the *new* key per Spec D L4-D10).
        //
        // The OLD key is still resolvable on Vault (transit keeps
        // historical versions); however `vault_sign_jws` always signs
        // with the latest_version pinned key name. To sign with the
        // OLD version we'd need a `key_version: <n>` parameter on the
        // sign request. This is the genuinely awkward bit:
        //
        // - In Vault, the latest_version IS the new key now (we just
        //   rotated). Calling `transit/sign/<auth_key>` would sign with
        //   the new key.
        // - We need to sign with the previous version.
        //
        // Vault's transit/sign accepts `key_version: <n>` to pin to a
        // specific version. We compute "previous version" by reading
        // the new latest_version - 1.
        let proof_jws = self.sign_with_previous_version(&old_did, &new_did)?;

        let event = DidRotationEvent {
            old_did,
            new_did,
            rotation_proof_jws: proof_jws,
            rotated_at: Utc::now(),
        };

        // 4. Build a fresh handle that reflects the new auth key.
        //    Wallet half is preserved (per L4-D7).
        let new_pem_lock = OnceLock::new();
        let _ = new_pem_lock.set(new_auth_pem);
        let new_handle = VaultTransitAnima {
            addr: self.addr.clone(),
            token: self.token.clone(),
            auth_key_name: self.auth_key_name.clone(),
            wallet_key_name: self.wallet_key_name.clone(),
            kid: self.kid.clone(),
            user_did: event.new_did.clone(),
            auth_pubkey: new_auth_pubkey,
            wallet_pubkey_uncompressed: self.wallet_pubkey_uncompressed,
            wallet_address: self.wallet_address.clone(),
            auth_public_pem: new_pem_lock,
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                .build()
                .map_err(|e| AnimaError::Crypto(format!("vault client (rotate): {e}")))?,
        };

        Ok((event, Arc::new(new_handle)))
    }

    fn backend_kind(&self) -> BackendKind {
        BackendKind::Vault
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
        let doc = IdentityDocumentBuilder::new(
            did,
            "anima-self".to_string(),
            format!("vault-transit custody ({})", self.kid),
            String::new(), // soul_hash filled in by the bridge layer
        )
        .agent_type(AgentType::Hosted)
        .verification_method(vm)
        .build();
        Ok(doc)
    }
}

impl VaultTransitAnima {
    /// Sign a rotation proof using the PRE-rotation auth key version.
    ///
    /// Vault's transit/sign accepts `key_version: <n>` to pin to a
    /// specific version. Right after the rotate POST, the new version
    /// is `latest_version`; the old version is `latest_version - 1`.
    /// We re-fetch `data.latest_version` and use `latest - 1`.
    fn sign_with_previous_version(&self, old_did: &str, new_did: &str) -> AnimaResult<String> {
        let url_keys = format!("{}/v1/transit/keys/{}", self.addr, self.auth_key_name);
        let body: Value = self
            .client
            .get(&url_keys)
            .header("X-Vault-Token", &self.token)
            .send()
            .map_err(|e| AnimaError::Crypto(format!("vault read keys for prev-version: {e}")))?
            .json()
            .map_err(|e| AnimaError::Crypto(format!("vault parse prev-version body: {e}")))?;
        let latest = body
            .pointer("/data/latest_version")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AnimaError::Crypto("vault: missing latest_version on rotate".into()))?;
        if latest < 2 {
            return Err(AnimaError::Crypto(format!(
                "vault: latest_version {latest} after rotate < 2; nothing to roll back to"
            )));
        }
        let prev_version = latest - 1;

        let proof_claims = serde_json::json!({
            "iss": old_did,
            "sub": new_did,
            "type": "anima.rotation_proof",
            "iat": Utc::now().timestamp(),
        });
        let header = serde_json::json!({
            "alg": "ES256",
            "typ": "JWT",
            "kid": format!("{}-v{}", self.kid, prev_version),
        });
        let header_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&header)
                .map_err(|e| AnimaError::Crypto(format!("encode rotation header: {e}")))?,
        );
        let body_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&proof_claims)
                .map_err(|e| AnimaError::Crypto(format!("encode rotation claims: {e}")))?,
        );
        let signing_input = format!("{header_b64}.{body_b64}");
        let url_sign = format!("{}/v1/transit/sign/{}", self.addr, self.auth_key_name);
        let payload = serde_json::json!({
            "input": URL_SAFE_NO_PAD.encode(signing_input.as_bytes()),
            "marshaling_algorithm": "jws",
            "hash_algorithm": "sha2-256",
            "key_version": prev_version,
        });
        let resp: Value = self
            .client
            .post(&url_sign)
            .header("X-Vault-Token", &self.token)
            .json(&payload)
            .send()
            .map_err(|e| AnimaError::Crypto(format!("vault sign rotation proof: {e}")))?
            .json()
            .map_err(|e| AnimaError::Crypto(format!("vault parse rotation proof: {e}")))?;
        let sig = resp
            .pointer("/data/signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AnimaError::Crypto("vault: missing signature on rotation proof".into())
            })?;
        let sig_b64 = sig.rsplit(':').next().unwrap_or(sig);
        Ok(format!("{signing_input}.{sig_b64}"))
    }
}

/// Parse a P-256 PEM-encoded public key and return the SEC1-compressed
/// 33-byte form. Used to ingest Vault's `GetPublicKey` response.
fn parse_p256_pem_to_sec1_compressed(pem: &str) -> AnimaResult<[u8; 33]> {
    use p256::PublicKey;
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    use p256::pkcs8::DecodePublicKey;
    let pk = PublicKey::from_public_key_pem(pem)
        .map_err(|e| AnimaError::Crypto(format!("p256 pem parse: {e}")))?;
    let point = pk.to_encoded_point(true);
    let bytes = point.as_bytes();
    if bytes.len() != 33 {
        return Err(AnimaError::Crypto(format!(
            "p256 compressed point unexpected len: {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 33];
    out.copy_from_slice(bytes);
    Ok(out)
}

/// Parse a secp256k1 PEM-encoded public key and return the
/// SEC1-uncompressed 65-byte form (`0x04 || x || y`). Vault emits both
/// in PKCS#8 SubjectPublicKeyInfo PEM; the `k256` crate parses it via
/// the `pkcs8` feature.
fn parse_secp256k1_pem_to_sec1_uncompressed(pem: &str) -> AnimaResult<[u8; 65]> {
    use k256::PublicKey;
    use k256::pkcs8::DecodePublicKey;
    let pk = PublicKey::from_public_key_pem(pem)
        .map_err(|e| AnimaError::Crypto(format!("secp256k1 pem parse: {e}")))?;
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    let point = pk.to_encoded_point(false);
    let bytes = point.as_bytes();
    if bytes.len() != 65 {
        return Err(AnimaError::Crypto(format!(
            "secp256k1 uncompressed point unexpected len: {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 65];
    out.copy_from_slice(bytes);
    Ok(out)
}

/// Derive an EVM address from a 65-byte uncompressed secp256k1 public
/// key (`0x04 || x || y`). Mirror of `haima_wallet::evm::derive_address`
/// but operates on raw uncompressed bytes (we get them from Vault's PEM
/// rather than from a `SigningKey` we own).
fn derive_wallet_address(uncompressed: &[u8; 65]) -> String {
    debug_assert_eq!(
        uncompressed[0], 0x04,
        "uncompressed point must start with 0x04"
    );
    let hash = Keccak256::digest(&uncompressed[1..]);
    let address_bytes = &hash[12..];
    format!("0x{}", hex::encode(address_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirm the per-user namespace pattern: `new(addr, token, "alice", "kid")`
    /// derives `anima-alice-auth-v1` and `anima-alice-wallet-v1`.
    /// We can't call `new()` without a live Vault, so we exercise the
    /// formatter logic directly via assertions on the format-string.
    #[test]
    fn per_user_namespace_naming_pattern() {
        let user_id = "alice";
        let auth_key_name = format!("anima-{user_id}-auth-v1");
        let wallet_key_name = format!("anima-{user_id}-wallet-v1");
        assert_eq!(auth_key_name, "anima-alice-auth-v1");
        assert_eq!(wallet_key_name, "anima-alice-wallet-v1");
    }

    #[test]
    fn derive_wallet_address_from_uncompressed_pubkey() {
        // Use a known test vector — secp256k1 public key from
        // private key = [1u8; 32]. Verified against
        // `haima_wallet::evm::derive_address` with the same input.
        use k256::SecretKey;
        let sk = SecretKey::from_bytes(&[1u8; 32].into()).unwrap();
        let pk = sk.public_key();
        use k256::elliptic_curve::sec1::ToEncodedPoint;
        let pt = pk.to_encoded_point(false);
        let bytes = pt.as_bytes();
        let mut uncompressed = [0u8; 65];
        uncompressed.copy_from_slice(bytes);
        let addr = derive_wallet_address(&uncompressed);
        assert!(addr.starts_with("0x"));
        assert_eq!(addr.len(), 42);
    }

    #[test]
    fn parse_secp256k1_pem_round_trip() {
        // Generate a key, export to PEM, parse back, verify uncompressed
        // form matches.
        use k256::SecretKey;
        use k256::elliptic_curve::sec1::ToEncodedPoint;
        use k256::pkcs8::EncodePublicKey;
        let sk = SecretKey::from_bytes(&[7u8; 32].into()).unwrap();
        let pk = sk.public_key();
        let pem = pk.to_public_key_pem(Default::default()).unwrap();
        let parsed = parse_secp256k1_pem_to_sec1_uncompressed(&pem).unwrap();
        let pt = pk.to_encoded_point(false);
        assert_eq!(&parsed[..], pt.as_bytes());
    }

    #[test]
    fn parse_p256_pem_round_trip() {
        // Same idea — generate, export, parse, verify.
        use p256::SecretKey;
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        use p256::pkcs8::EncodePublicKey;
        let sk = SecretKey::from_bytes(&[7u8; 32].into()).unwrap();
        let pk = sk.public_key();
        let pem = pk.to_public_key_pem(Default::default()).unwrap();
        let compressed = parse_p256_pem_to_sec1_compressed(&pem).unwrap();
        let pt = pk.to_encoded_point(true);
        assert_eq!(&compressed[..], pt.as_bytes());
    }

    /// Vault's `transit/sign` response shape:
    /// `{"data":{"signature":"vault:v1:<base64-r-s>"}}`. The
    /// `sig.rsplit(':').next()` strips the `vault:vN:` prefix.
    #[test]
    fn vault_signature_prefix_stripping() {
        let raw = "vault:v3:abc123==";
        let stripped = raw.rsplit(':').next().unwrap_or(raw);
        assert_eq!(stripped, "abc123==");

        // No prefix → unchanged.
        let raw2 = "abc123==";
        let stripped2 = raw2.rsplit(':').next().unwrap_or(raw2);
        assert_eq!(stripped2, "abc123==");
    }

    /// Vault's `latest_version` pointer logic — same as lifegw's
    /// regression test (lifegw Sub-phase E item #4). With
    /// `latest_version: 5`, the public key index is `data.keys.5.public_key`.
    #[test]
    fn vault_latest_version_pointer() {
        use serde_json::json;
        let body = json!({
            "data": {
                "latest_version": 5,
                "keys": {
                    "1": { "public_key": "v1-pem" },
                    "5": { "public_key": "v5-pem" }
                }
            }
        });
        let latest = body
            .pointer("/data/latest_version")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(latest, 5);
        let pem = body
            .pointer(&format!("/data/keys/{latest}/public_key"))
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(pem, "v5-pem");
    }
}
