//! `SomaCustody` — soma admin custody-oracle backend (Spec D D-Sub-E).
//!
//! Calls into soma's admin UDS via tonic's
//! `life.admin.kernel.v1.CustodyOracle` service for every signing
//! operation. The auth + wallet pubkeys are fetched once at
//! construction (via `kernel.GetAuthPubkey` + `kernel.GetWalletPubkey`)
//! and cached on the handle so trait accessors (`user_did`,
//! `auth_pubkey`, `wallet_address`) are referentially transparent
//! and never block on I/O.
//!
//! ## SPEC-D-DEVIATION
//!
//! - **Network-call cost per signature**. Every `sign_jws` /
//!   `sign_digest` / `sign_evm_tx` / `sign_eip712` call is a UDS RPC
//!   to soma. The current implementation uses a single shared
//!   `Arc<Mutex<Channel>>` and serialises requests through it. For
//!   high-throughput deployments callers SHOULD wrap this backend in
//!   their own connection-pooling layer — `life-runtime-pool::Pool`
//!   is the canonical primitive (see lifed sub-phase E for the
//!   pattern). A connection pool inside SomaCustody itself is
//!   deliberately deferred so we don't bake one-pool-per-handle into
//!   the trait surface.
//!
//! - **Fallback semantics on UDS unavailability**. When the soma
//!   admin UDS is unreachable (daemon down, socket missing, group
//!   membership rejected), every method returns
//!   `AnimaError::Crypto("soma rpc: ...")`. We do NOT silently
//!   fall back to `InProcessAnima`. Callers that want degraded-mode
//!   behaviour MUST construct an `InProcessAnima` themselves and
//!   handle the failover at the application layer. This matches
//!   `VaultTransitAnima` discipline — the trust boundary is opt-in.
//!
//! - **`kernel.SignWallet` requires soma to hold the wallet key**.
//!   Operators decide where wallet keys actually live: soma's
//!   `InProcessCustodyKeys` (zeroized in process memory), TPM via
//!   PKCS#11, a custom HSM sidecar. The trait surface of soma's
//!   `CustodyOracleService` is intentionally agnostic. Backends that
//!   physically can't sign secp256k1 (most TPMs) MUST return
//!   `Status::Unimplemented("kms.wallet_unsupported")`; SomaCustody
//!   surfaces this as `AnimaError::Crypto`.
//!
//! - **`rotate()` is NOT implemented at the soma RPC layer in
//!   D-Sub-E**. The spec's rotation surface is journal-driven
//!   (`anima.identity_rotated` events); soma's job is signing, not
//!   rotation. Calling `SomaCustody::rotate` returns
//!   `AnimaError::Crypto("soma: rotation must go through anima-lago
//!   write_rotation_event helper, not the custody trait")`. This
//!   matches `VaultTransitAnima`'s pattern but is louder — Vault has
//!   a `transit/keys/<key>/rotate` endpoint; soma intentionally does
//!   not.
//!
//! - **`sign_eip712` only supports EIP-3009 `TransferWithAuthorization`**
//!   in D-Sub-E (mirrors D-Sub-A / D-Sub-B). Generic EIP-712 encoding
//!   stays a follow-up. The error message points at the same
//!   limitation noted in `InProcessAnima` and `VaultTransitAnima` so
//!   callers see uniform behaviour across backends.

use std::sync::{Arc, OnceLock};

use anima_core::error::{AnimaError, AnimaResult};
use anima_core::identity_document::{
    AgentIdentityDocument, AgentType, IdentityDocumentBuilder, VerificationMethod,
};
use haima_core::wallet::{ChainId, WalletAddress};
use serde_json::Value;
use tokio::sync::Mutex as TokioMutex;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::custody::{
    AnimaCustody, BackendKind, DidRotationEvent, Eip712Domain, EvmSignature, TxRequest,
};
use crate::rlp;

use life_kernel_proto::custody as oracle_pb;

/// `SomaCustody` — soma admin custody-oracle backend.
///
/// Holds:
/// - the path to soma's admin UDS,
/// - the user_id passed on every RPC,
/// - cached pubkeys + DID + wallet address resolved at construction,
/// - a shared tonic `Channel` (cheap to clone — the Mutex serialises
///   exclusive access for the duration of each RPC, but the Channel
///   itself is multiplexable; future sub-phases can swap the Mutex
///   for a connection pool).
pub struct SomaCustody {
    uds_path: String,
    user_id: String,
    /// JWS `kid` in headers. By convention `did:key:zDn…` (the user
    /// DID); we cache it so JWS minting doesn't allocate on the hot
    /// path.
    kid: String,
    user_did: String,
    auth_pubkey: [u8; 33],
    /// Cached secp256k1 uncompressed pubkey. Held alongside the
    /// wallet address so callers (and a future cross-backend digest
    /// equivalence test) can introspect the underlying key without
    /// reaching back into soma.
    #[allow(dead_code)]
    wallet_pubkey_uncompressed: [u8; 65],
    wallet_address: WalletAddress,
    auth_public_pem_cache: OnceLock<String>,
    /// Async client, behind a Mutex so concurrent callers serialise
    /// per-handle. soma's UDS multiplexes streams under tonic; the
    /// Mutex is here only because tonic clients are `&mut self` —
    /// future sub-phases can swap this for a `life-runtime-pool` Pool.
    client: Arc<TokioMutex<oracle_pb::custody_oracle_client::CustodyOracleClient<Channel>>>,
}

impl SomaCustody {
    /// Connect to soma's admin UDS at `uds_path` and resolve the
    /// pubkeys for `user_id` + `kid` via `GetAuthPubkey` +
    /// `GetWalletPubkey`. Returns a fully populated handle.
    ///
    /// Failures bubble up as `AnimaError::Crypto`. Operators
    /// provisioning soma must ensure the user exists in soma's key
    /// store before calling this — otherwise both bootstrap calls
    /// return `Status::NotFound` which surfaces as a startup error.
    pub async fn new(
        uds_path: impl Into<String>,
        user_id: impl Into<String>,
        kid: impl Into<String>,
    ) -> AnimaResult<Self> {
        let uds_path: String = uds_path.into();
        let user_id: String = user_id.into();
        let kid: String = kid.into();

        // Build a tonic Channel that tunnels over a Unix domain socket.
        // The URI is a placeholder — tonic only uses the authority for
        // host resolution; we hand it the explicit connector via
        // `connect_with_connector`.
        let connect_path = uds_path.clone();
        let endpoint = Endpoint::try_from("http://[::]:0")
            .map_err(|e| AnimaError::Crypto(format!("soma endpoint: {e}")))?;
        let channel = endpoint
            .connect_with_connector(service_fn(move |_: Uri| {
                let path = connect_path.clone();
                async move {
                    let stream = tokio::net::UnixStream::connect(path).await?;
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
                }
            }))
            .await
            .map_err(|e| AnimaError::Crypto(format!("soma uds connect ({uds_path}): {e}")))?;

        let mut client = oracle_pb::custody_oracle_client::CustodyOracleClient::new(channel);

        // Fetch auth pubkey.
        let auth_resp = client
            .get_auth_pubkey(oracle_pb::GetAuthPubkeyRequest {
                user_id: user_id.clone(),
            })
            .await
            .map_err(|e| AnimaError::Crypto(format!("soma get_auth_pubkey: {e}")))?
            .into_inner();
        let auth_pubkey = parse_compressed_33(&auth_resp.pubkey_sec1_compressed)
            .map_err(|e| AnimaError::Crypto(format!("soma auth pubkey: {e}")))?;

        // Fetch wallet pubkey.
        let wallet_resp = client
            .get_wallet_pubkey(oracle_pb::GetWalletPubkeyRequest {
                user_id: user_id.clone(),
            })
            .await
            .map_err(|e| AnimaError::Crypto(format!("soma get_wallet_pubkey: {e}")))?
            .into_inner();
        let wallet_pubkey_uncompressed =
            parse_uncompressed_65(&wallet_resp.pubkey_sec1_uncompressed)
                .map_err(|e| AnimaError::Crypto(format!("soma wallet pubkey: {e}")))?;

        let wallet_addr_hex = derive_wallet_address(&wallet_pubkey_uncompressed);
        let user_did = crate::did::generate_did_key_p256(&auth_pubkey);

        Ok(Self {
            uds_path,
            user_id,
            kid,
            user_did,
            auth_pubkey,
            wallet_pubkey_uncompressed,
            wallet_address: WalletAddress {
                address: wallet_addr_hex,
                chain: ChainId::base(),
            },
            auth_public_pem_cache: OnceLock::new(),
            client: Arc::new(TokioMutex::new(client)),
        })
    }

    /// Borrow the configured UDS path — used by tests and diagnostic
    /// logs.
    pub fn uds_path(&self) -> &str {
        &self.uds_path
    }

    /// Borrow the user_id — used by tests and diagnostic logs.
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// Sync wrapper: drive the async signing call from the trait's sync
    /// context. We use a tokio handle because the trait method
    /// signatures are sync (matching VaultTransitAnima's pattern with
    /// reqwest::blocking).
    fn block_on<F, T>(&self, fut: F) -> AnimaResult<T>
    where
        F: std::future::Future<Output = AnimaResult<T>>,
    {
        // If the caller is on a tokio runtime, use `block_in_place` +
        // `Handle::current().block_on`; otherwise build a single-shot
        // current-thread runtime. Mirrors the pattern lifegw uses for
        // its sync admin handlers.
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
            Err(_) => {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| AnimaError::Crypto(format!("soma rt build: {e}")))?;
                rt.block_on(fut)
            }
        }
    }

    /// Async signing: call soma's `kernel.SignAuth` for a 32-byte digest.
    async fn sign_auth_digest_async(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        let mut client = self.client.lock().await;
        let resp = client
            .sign_auth(oracle_pb::SignAuthRequest {
                user_id: self.user_id.clone(),
                digest: digest.to_vec(),
            })
            .await
            .map_err(|e| AnimaError::Crypto(format!("soma sign_auth: {e}")))?
            .into_inner();
        if resp.signature_raw.len() != 64 {
            return Err(AnimaError::Crypto(format!(
                "soma sign_auth: expected 64-byte sig, got {}",
                resp.signature_raw.len()
            )));
        }
        let mut out = [0u8; 64];
        out.copy_from_slice(&resp.signature_raw);
        Ok(out)
    }

    /// Async signing: call soma's `kernel.SignWallet` for a 32-byte digest.
    async fn sign_wallet_digest_async(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 65]> {
        let mut client = self.client.lock().await;
        let resp = client
            .sign_wallet(oracle_pb::SignWalletRequest {
                user_id: self.user_id.clone(),
                digest: digest.to_vec(),
            })
            .await
            .map_err(|e| AnimaError::Crypto(format!("soma sign_wallet: {e}")))?
            .into_inner();
        if resp.signature_rsv.len() != 65 {
            return Err(AnimaError::Crypto(format!(
                "soma sign_wallet: expected 65-byte sig, got {}",
                resp.signature_rsv.len()
            )));
        }
        let mut out = [0u8; 65];
        out.copy_from_slice(&resp.signature_rsv);
        Ok(out)
    }
}

impl AnimaCustody for SomaCustody {
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
        // RFC 7515 §3) then ask soma to sign the SHA-256 of the
        // signing input. soma signs the prehash; we compute it here.
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
                .map_err(|e| AnimaError::Crypto(format!("soma encode header: {e}")))?,
        );
        let body_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(claims)
                .map_err(|e| AnimaError::Crypto(format!("soma encode body: {e}")))?,
        );
        let signing_input = format!("{header_b64}.{body_b64}");
        let digest_array = {
            let hash = Sha256::digest(signing_input.as_bytes());
            let mut out = [0u8; 32];
            out.copy_from_slice(&hash);
            out
        };
        let sig = self.block_on(self.sign_auth_digest_async(&digest_array))?;
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
        Ok(format!("{signing_input}.{sig_b64}"))
    }

    fn sign_digest(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        self.block_on(self.sign_auth_digest_async(digest))
    }

    fn sign_evm_tx(&self, tx: &TxRequest) -> AnimaResult<EvmSignature> {
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
        let bytes = self.block_on(self.sign_wallet_digest_async(&digest))?;
        Ok(EvmSignature::from_bytes(bytes.to_vec()))
    }

    fn sign_eip712(
        &self,
        domain: &Eip712Domain,
        types: &Value,
        message: &Value,
    ) -> AnimaResult<EvmSignature> {
        // Same EIP-3009-only constraint as InProcessAnima /
        // VaultTransitAnima. Generic encoder is a follow-up.
        let primary = types
            .get("primaryType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if primary != "TransferWithAuthorization"
            && !(message.get("from").is_some() && message.get("validAfter").is_some())
        {
            return Err(AnimaError::Crypto(
                "eip712: only EIP-3009 TransferWithAuthorization is supported in D-Sub-E \
                 (matches D-Sub-A/B limitation; generic encoder deferred)"
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
        let bytes = self.block_on(self.sign_wallet_digest_async(&digest))?;
        Ok(EvmSignature::from_bytes(bytes.to_vec()))
    }

    fn rotate(&self) -> AnimaResult<(DidRotationEvent, Arc<dyn AnimaCustody>)> {
        // SPEC-D-DEVIATION: rotation does NOT go through soma's RPC
        // surface. The journal is the source of truth for rotation
        // events; callers route through `anima-lago::write_rotation_event`
        // and re-construct a SomaCustody bound to the new key version.
        Err(AnimaError::Crypto(
            "soma: rotation must go through anima-lago write_rotation_event helper, \
             not the custody trait. Provision the new key in soma's CustodyOracle, \
             write the anima.identity_rotated event to the journal, then construct \
             a fresh SomaCustody bound to the new key/kid."
                .to_string(),
        ))
    }

    fn backend_kind(&self) -> BackendKind {
        BackendKind::Soma
    }

    fn export_identity_document(&self) -> AnimaResult<AgentIdentityDocument> {
        let public_key_multibase = format!("z{}", bs58::encode(self.auth_pubkey).into_string());
        let did = self.user_did.clone();
        let _ = self.auth_public_pem_cache.set(
            "-----BEGIN PUBLIC KEY-----\n[soma-managed; pubkey via did:key]\n-----END PUBLIC KEY-----\n"
                .to_string(),
        );
        let vm = VerificationMethod {
            id: format!("{did}#key-1"),
            method_type: "JsonWebKey2020".to_string(),
            controller: did.clone(),
            public_key_multibase,
        };
        // D-Sub-E: rotation_chain stays empty here. The chain is
        // populated by `crate::rotation::walk_rotation_chain` via the
        // anima-lago bridge — the trait method preserves the
        // backend-local fields and a downstream replayer (e.g.
        // anima-lago::projection::reconstruct_identity_document) is
        // expected to merge in the rotation chain.
        let doc = IdentityDocumentBuilder::new(
            did,
            "anima-self".to_string(),
            format!("soma custody ({})", self.kid),
            String::new(),
        )
        .agent_type(AgentType::Hosted)
        .verification_method(vm)
        .build();
        Ok(doc)
    }
}

fn parse_compressed_33(bytes: &[u8]) -> Result<[u8; 33], String> {
    if bytes.len() != 33 {
        return Err(format!(
            "expected 33-byte SEC1 compressed P-256, got {}",
            bytes.len()
        ));
    }
    let mut out = [0u8; 33];
    out.copy_from_slice(bytes);
    Ok(out)
}

fn parse_uncompressed_65(bytes: &[u8]) -> Result<[u8; 65], String> {
    if bytes.len() != 65 {
        return Err(format!(
            "expected 65-byte SEC1 uncompressed secp256k1, got {}",
            bytes.len()
        ));
    }
    if bytes[0] != 0x04 {
        return Err(format!(
            "expected uncompressed point prefix 0x04, got 0x{:02x}",
            bytes[0]
        ));
    }
    let mut out = [0u8; 65];
    out.copy_from_slice(bytes);
    Ok(out)
}

/// Derive an EVM address from a 65-byte uncompressed secp256k1 public
/// key. Mirror of `haima_wallet::evm::derive_address` and soma's
/// `admin::keys::derive_wallet_address`.
fn derive_wallet_address(uncompressed: &[u8; 65]) -> String {
    use sha3::{Digest as Sha3Digest, Keccak256};
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

    #[test]
    fn parse_compressed_33_validates_len() {
        assert!(parse_compressed_33(&[0u8; 33]).is_ok());
        assert!(parse_compressed_33(&[0u8; 32]).is_err());
        assert!(parse_compressed_33(&[0u8; 34]).is_err());
    }

    #[test]
    fn parse_uncompressed_65_validates_len_and_prefix() {
        let mut good = [0u8; 65];
        good[0] = 0x04;
        assert!(parse_uncompressed_65(&good).is_ok());

        let mut bad_prefix = [0u8; 65];
        bad_prefix[0] = 0x02;
        assert!(parse_uncompressed_65(&bad_prefix).is_err());

        assert!(parse_uncompressed_65(&[0u8; 64]).is_err());
        assert!(parse_uncompressed_65(&[0u8; 66]).is_err());
    }

    #[test]
    fn derive_wallet_address_format() {
        // Use a known test vector — secp256k1 public key from
        // privkey [1u8; 32].
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
}
