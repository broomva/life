//! `AnimaCustody` — the production custody trait abstraction (Spec D §"Trait shape").
//!
//! SPEC-D-DEVIATION: Notes on where the implementation differs from the
//! Spec D §"Trait shape" pseudocode and why.
//!
//! - `sign_eip712` takes `domain: &Eip712Domain` (re-exported from
//!   `haima_wallet`). The spec listed `&Eip712Domain` without specifying the
//!   crate; we reuse the existing haima-wallet type instead of duplicating it
//!   in anima. This keeps the wallet-half encoding logic colocated with the
//!   ECDSA-secp256k1 signing primitive.
//!
//! - `sign_eip712`'s `types` and `message` parameters are typed as
//!   `serde_json::Value` per the spec, but in D-Sub-A's `InProcessAnima` body
//!   the Eip712 path goes through `haima_wallet::sign_transfer_authorization`
//!   directly because that is the only EIP-712 typed-data shape haima
//!   currently signs. A generic Eip712 encoder is deferred to a follow-up;
//!   non-EIP-3009 typed-data signing returns
//!   `AnimaError::Crypto("eip712: only EIP-3009 transferWithAuthorization is
//!   supported in D-Sub-A")`.
//!
//! - `rotate()` returns `(DidRotationEvent, Arc<dyn AnimaCustody>)` but does
//!   NOT itself write to the Lago journal — that is the caller's
//!   responsibility (the anima-lago bridge). This keeps the trait pure (no
//!   I/O) and matches the lifegw `KmsSigner::publish_jwks` shape (returns
//!   the data; the caller atomically swaps it). The Arc handle returned is
//!   the NEW post-rotation custody; the original handle remains a valid
//!   snapshot of pre-rotation state for verifying historical signatures.
//!
//! - **`sign_evm_tx` — D-Sub-B fixed (with v-byte caveat)**: D-Sub-A
//!   shipped a JSON-canonicalisation stub that did not produce
//!   transactions of any kind. D-Sub-B replaces the stub with proper
//!   EIP-1559 RLP encoding via `crate::rlp::encode_eip1559_unsigned` +
//!   `crate::rlp::keccak256`. Both `InProcessAnima` and
//!   `VaultTransitAnima` go through the same shared RLP path.
//!
//!   **v-byte semantics (I1 review note)**: the returned 65-byte
//!   `r||s||v` signature uses the **legacy `v ∈ {27, 28}` form** — the
//!   haima-wallet / EIP-3009 / Ethereum-`ecrecover` convention. This
//!   is the right shape for the production hot path (haima-x402 +
//!   EIP-3009 typed-data signing). For broadcasting a raw EIP-1559
//!   typed envelope on-chain (which expects `y_parity ∈ {0, 1}` in
//!   the signed payload, NOT 27/28), the caller MUST subtract 27 from
//!   the last byte before splicing into the envelope. We deliberately
//!   keep the 27/28 form here for symmetry with haima-wallet's
//!   `LocalSigner::sign_transfer_authorization`. Direct EIP-1559
//!   broadcasting from this trait surface is a follow-up — until then,
//!   raw broadcasting requires a caller-side `v -= 27` transform.
//!
//! - `export_identity_document()` returns the `AgentIdentityDocument` shape
//!   from `anima-core`. The `rotation_chain` field is added in this PR
//!   (`anima_core::identity_document::DidRotation`).
//!
//! Backend matrix (Spec D §"Backend matrix") — only `InProcessAnima` ships in
//! D-Sub-A. The other backends (`VaultTransitAnima`, `WebCryptoAnima`,
//! `TpmAnima`, `SomaCustody`, `HardwareWalletAnima`, `RemoteAnima`) land in
//! D-Sub-B…F. The trait shape is deliberately wide enough to accommodate all
//! of them — see [`BackendKind`].

use std::sync::Arc;

use anima_core::error::AnimaResult;
use anima_core::identity_document::{AgentIdentityDocument, DidRotation};
use chrono::{DateTime, Utc};
use haima_core::wallet::WalletAddress;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// EIP-712 typed-data domain — re-exported from haima-wallet.
///
/// Spec D L4-D7 keeps the wallet keypair on secp256k1 + EIP-712 + EIP-155;
/// the existing haima-wallet type is the canonical shape.
pub use haima_wallet::Eip712Domain;

/// Custody backend identifier (Spec D §"Event additions").
///
/// `#[non_exhaustive]` so future backends (W3C-DID-resolver, custom TEEs,
/// passkey-attested HSMs, …) can be added without breaking match
/// exhaustiveness in downstream verifiers.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// `InProcessAnima` — keys live in process memory (dev / single-user host).
    /// Spec D D-Sub-A primary backend.
    InProcess,
    /// `VaultTransitAnima` — HashiCorp Vault Transit per-user keys.
    /// Spec D D-Sub-B; multi-tenant production server-side.
    Vault,
    /// `WebCryptoAnima` — passkey-managed non-extractable browser `CryptoKey`.
    /// Spec D D-Sub-C; auth half only (wallet half is delegated).
    WebCrypto,
    /// `TpmAnima` — TPM-resident keys via PKCS#11.
    /// Spec D D-Sub-D; desktop single-user host.
    Tpm,
    /// `SomaCustody` — soma admin RPC oracle (UDS + SO_PEERCRED).
    /// Spec D D-Sub-E; user-scope analogue of Tier-2 KMS unification.
    Soma,
    /// `HardwareWalletAnima` — Ledger/Trezor secp256k1 wallet half.
    /// Spec D D-Sub-F; high-stakes wallet operations.
    HardwareWallet,
    /// `RemoteAnima` — proxy to a server-side anima daemon (browser pairing).
    /// Spec D D-Sub-C, paired with `WebCryptoAnima`.
    Remote,
}

/// EVM transaction request — narrow shape used by `AnimaCustody::sign_evm_tx`.
///
/// Spec D L4-D7 keeps the wallet on secp256k1 + EIP-155, so this matches the
/// EVM EOA model. Solana / non-EVM transaction shapes will get their own
/// trait method when the wallet substrate adds those chains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxRequest {
    /// `from` address (must equal `wallet_address().address`).
    pub from: String,
    /// `to` address (recipient).
    pub to: String,
    /// Value in wei (string-encoded for u256 width — chains may exceed u64).
    pub value_wei: String,
    /// Transaction `data` (calldata; hex-encoded, may be empty for plain transfers).
    pub data_hex: String,
    /// Nonce (user's wallet account nonce).
    pub nonce: u64,
    /// Gas limit.
    pub gas_limit: u64,
    /// Maximum fee per gas (EIP-1559) in wei (string-encoded).
    pub max_fee_per_gas_wei: String,
    /// Maximum priority fee per gas (EIP-1559) in wei (string-encoded).
    pub max_priority_fee_per_gas_wei: String,
    /// CAIP-2 chain id (e.g. `eip155:8453`). Used to pick the EIP-155 chain id.
    pub chain: String,
}

/// EVM signature output — `(r, s, v)` in 65-byte recoverable form.
///
/// Same shape as `haima_wallet::LocalSigner::sign_transfer_authorization`
/// returns. Production verifiers ecrecover from this and compare the
/// recovered address against `WalletAddress::address`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvmSignature {
    /// Raw 65-byte recoverable signature (r || s || v).
    pub bytes: Vec<u8>,
}

impl EvmSignature {
    /// Construct from a raw byte vector — must be 65 bytes (r||s||v).
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// Borrow as a slice.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

/// Output of `AnimaCustody::rotate`.
///
/// Documents a DID rotation: the old DID, the new DID, and a JWS signed by
/// the *old* key over the *new* DID — the rotation proof. Verifiers seeing
/// the old DID resolve this event from the journal to learn the new DID.
///
/// Spec D L4-D10 — Rotation is documented in the journal, not implicit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DidRotationEvent {
    /// The DID that was rotated away from. Remains valid for verifying
    /// historical events but cannot mint new signatures.
    pub old_did: String,
    /// The DID that signing now flows through. All new signatures use this.
    pub new_did: String,
    /// Detached JWS by the *old* key over the *new* DID, proving the rotation
    /// was authorised by the holder of the old key. Compact form
    /// (`<header>.<body>.<signature>`).
    pub rotation_proof_jws: String,
    /// Wall-clock timestamp of the rotation.
    pub rotated_at: DateTime<Utc>,
}

impl DidRotationEvent {
    /// Convert this event to the persistence shape used in
    /// `AgentIdentityDocument.rotation_chain` and the `anima.identity_rotated`
    /// event payload.
    pub fn as_did_rotation(&self, rotated_at_seq: u64) -> DidRotation {
        DidRotation {
            old_did: self.old_did.clone(),
            new_did: self.new_did.clone(),
            rotation_proof_jws: self.rotation_proof_jws.clone(),
            rotated_at_seq,
        }
    }
}

/// `AnimaCustody` — the production custody trait abstraction.
///
/// Mirrors `lifegw::auth::kms::KmsSigner` at the user-scope; widened to
/// support both auth (P-256 ECDSA) and wallet (secp256k1 ECDSA) signing
/// through the same trait object. Browser deployments hold only the auth
/// half locally and delegate wallet ops to a server-side backend
/// (Spec D L4-D5 split-custody).
///
/// Implementations MUST be `Send + Sync + 'static` so call sites can hold
/// `Arc<dyn AnimaCustody>` and pass it across task boundaries.
pub trait AnimaCustody: Send + Sync + 'static {
    /// User-scope DID. Format: `did:key:zDn…` (P-256 multicodec 0x1200) for
    /// any DID minted post-D-Sub-A. Old `did:key:z6Mk…` (Ed25519) DIDs
    /// remain resolvable for historical-event verification but are NEVER
    /// returned here — `rotate()` migrates them.
    fn user_did(&self) -> &str;

    /// SEC1-compressed P-256 public key (33 bytes). The corresponding DID is
    /// derived from this via `did::generate_did_key_p256`.
    fn auth_pubkey(&self) -> [u8; 33];

    /// Wallet half address, if a wallet has been resolved for this custody.
    /// `None` for browser passkey-only deployments where the wallet half is
    /// delegated to `RemoteAnima` / `HardwareWalletAnima` / `VaultTransitAnima`.
    fn wallet_address(&self) -> Option<&WalletAddress>;

    /// Sign a JWS over the supplied claims using the auth (P-256) key.
    /// Implementations build the header (alg=ES256, kid=DID) and return the
    /// compact JWS string `<header>.<body>.<signature>`.
    fn sign_jws(&self, claims: &Value) -> AnimaResult<String>;

    /// Sign a raw 32-byte digest with the auth key (used for non-JWT identity
    /// events such as Spaces presence beacons). Returns the 64-byte ECDSA
    /// `(r || s)` IEEE-P1363 form (no recovery byte — auth verifiers know
    /// the verifying key from the DID).
    fn sign_digest(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]>;

    /// Sign an EVM transaction with the wallet (secp256k1) key.
    /// Browser-side `WebCryptoAnima` delegates this to `RemoteAnima` /
    /// `VaultTransitAnima` / `HardwareWalletAnima`; in-process backends
    /// sign locally.
    fn sign_evm_tx(&self, tx: &TxRequest) -> AnimaResult<EvmSignature>;

    /// Sign an EIP-712 typed-data payload (used by haima for x402 + USDC
    /// `transferWithAuthorization` and any future EIP-712-shaped flows).
    ///
    /// In D-Sub-A, only the EIP-3009 `transferWithAuthorization` shape is
    /// supported — see `// SPEC-D-DEVIATION` block at the top of this file.
    fn sign_eip712(
        &self,
        domain: &Eip712Domain,
        types: &Value,
        message: &Value,
    ) -> AnimaResult<EvmSignature>;

    /// Mint a new auth keypair, sign a rotation proof with the *old* key,
    /// and return both the rotation event and a fresh custody handle that
    /// reflects the new key.
    ///
    /// Semantics:
    /// - The returned `DidRotationEvent` carries the rotation proof JWS
    ///   signed by the OLD key over the NEW key (Spec D L4-D10).
    /// - The returned `Arc<dyn AnimaCustody>` is a NEW handle whose
    ///   `user_did()` / `auth_pubkey()` reflect the NEW key.
    /// - The original handle is NOT mutated and remains valid as a
    ///   snapshot of pre-rotation state — useful for verifiers walking
    ///   historical signatures.
    /// - Wallet half (secp256k1) is preserved across rotation per L4-D7;
    ///   the new handle has the same `wallet_address()`.
    ///
    /// Each backend implements this differently — Vault rotates the
    /// underlying transit key version + returns a Vault-backed handle for
    /// the new version; in-process backends generate a fresh seed; soma
    /// forwards the call to its admin RPC and returns a soma-backed
    /// handle for the new key.
    ///
    /// Why a tuple return rather than `&mut self`-style in-place mutation:
    /// the trait method `user_did(&self) -> &str` requires referential
    /// transparency. To support both browser passkey backends (where the
    /// active credential is a non-extractable handle that can't be
    /// "swapped" in place) and Vault/HSM backends (where rotation produces
    /// a new key version), the cleanest contract is "return the new
    /// handle". Callers then `Arc::clone` the new handle into wherever
    /// they were holding the old one.
    fn rotate(&self) -> AnimaResult<(DidRotationEvent, Arc<dyn AnimaCustody>)>;

    /// Identify which backend produced this custody handle. Used for the
    /// `anima.custody_migrated` event and for diagnostic logging.
    fn backend_kind(&self) -> BackendKind;

    /// Export the agent's identity document (KYA shape) with the full
    /// rotation chain. The chain is replayed from the Lago journal at
    /// session start; D-Sub-A backends may return an empty chain when no
    /// rotations have occurred yet.
    fn export_identity_document(&self) -> AnimaResult<AgentIdentityDocument>;
}

/// `Arc<dyn AnimaCustody>` — the canonical handle every call site holds.
pub type AnimaCustodyHandle = Arc<dyn AnimaCustody>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time test: trait is dyn-compatible.
    #[test]
    fn custody_trait_compiles() {
        fn assert_dyn_compatible<T: AnimaCustody>(_: &T) {}
        // The function itself is the test — if `AnimaCustody` is not
        // dyn-compatible the type alias above wouldn't compile.
        let _: Option<AnimaCustodyHandle> = None;
        // Sanity: the marker bounds compile.
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<DidRotationEvent>();
        assert_send_sync::<TxRequest>();
        assert_send_sync::<EvmSignature>();
        assert_send_sync::<BackendKind>();
        let _ = assert_dyn_compatible::<crate::in_process::InProcessAnima>;
    }

    #[test]
    fn backend_kind_serialises_snake_case() {
        let json = serde_json::to_string(&BackendKind::InProcess).unwrap();
        assert_eq!(json, "\"in_process\"");
        let json = serde_json::to_string(&BackendKind::HardwareWallet).unwrap();
        assert_eq!(json, "\"hardware_wallet\"");
    }

    #[test]
    fn did_rotation_event_to_did_rotation_round_trip() {
        let evt = DidRotationEvent {
            old_did: "did:key:z6MkOld".into(),
            new_did: "did:key:zDnNew".into(),
            rotation_proof_jws: "header.body.sig".into(),
            rotated_at: Utc::now(),
        };
        let stored = evt.as_did_rotation(42);
        assert_eq!(stored.old_did, evt.old_did);
        assert_eq!(stored.new_did, evt.new_did);
        assert_eq!(stored.rotation_proof_jws, evt.rotation_proof_jws);
        assert_eq!(stored.rotated_at_seq, 42);
    }
}
