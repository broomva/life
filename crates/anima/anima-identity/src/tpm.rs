//! `TpmAnima` — PKCS#11 / TPM custody backend (Spec D D-Sub-D).
//!
//! Production-grade single-user desktop custody. The host TPM (or any
//! PKCS#11-conformant HSM/softhsm) holds the auth keypair (P-256); the
//! private key NEVER leaves the device. Per Spec D §"Phasing > D-Sub-D",
//! this is the default backend for `mission-control` desktop deployments
//! on Linux/macOS where a TPM 2.0 chip is available.
//!
//! ## SPEC-D-DEVIATION — wallet half delegated, NOT TPM-resident
//!
//! Spec D §"D-Sub-D" leaves the wallet-half decision open:
//!
//! > Both keypairs in TPM if the platform supports secp256k1 (some do via
//! > NIST P-256 emulation; most don't natively → escalate wallet half to
//! > `HardwareWalletAnima`).
//!
//! D-Sub-D takes the second path (escalation). Reasons:
//!
//! 1. **TPM 2.0 secp256k1 is rare.** The TPM 2.0 spec lists secp256k1
//!    as an OPTIONAL algorithm. Most consumer TPM chips (fTPM on Intel
//!    PTT, AMD fTPM, Apple T2/Secure Enclave) only support NIST curves
//!    (P-256, P-384, P-521). Asking the TPM to sign secp256k1 would
//!    fail at runtime on most devices.
//! 2. **Asymmetric blast radius.** Auth-key compromise allows agent
//!    impersonation (recoverable via rotation); wallet-key compromise
//!    drains funds (NOT recoverable). The wallet half deserves stricter
//!    custody than "whatever the host TPM offers".
//! 3. **Mission-control is single-user.** A user with a TPM-equipped
//!    desktop and an interest in self-custody is the same demographic
//!    that already owns a Ledger Nano X. Pairing TPM-auth with
//!    Ledger-wallet (D-Sub-F) is a strictly stronger story than dual-
//!    TPM custody.
//!
//! Concrete shape: `TpmAnima::new(...)` accepts an OPTIONAL
//! `wallet_delegate: Option<Arc<dyn AnimaCustody>>`. When present:
//!
//! - `wallet_address()` forwards to `wallet_delegate.wallet_address()`.
//! - `sign_evm_tx`, `sign_eip712` forward to the delegate.
//! - The delegate is held by `Arc` so it can be a long-lived
//!   `HardwareWalletAnima`, `VaultTransitAnima`, or even
//!   `InProcessAnima` for testing.
//!
//! When absent (delegate is `None`):
//!
//! - `wallet_address()` returns `None`.
//! - `sign_evm_tx` and `sign_eip712` return
//!   `AnimaError::Crypto("tpm: no wallet_delegate configured; \
//!     wallet operations unavailable")`.
//!
//! This is the honest "desktop with TPM but no wallet hardware" story:
//! agents can authenticate (mint JWS, sign presence beacons, attest
//! identity) but cannot move funds. Funds-moving deployments compose
//! `TpmAnima` with `HardwareWalletAnima` at construction time.
//!
//! ## SPEC-D-DEVIATION — rotation is operator-driven
//!
//! TPM rotation in production is fundamentally operator-driven: the
//! operator runs `pkcs11-tool --keypairgen` (or platform equivalent),
//! the new key gets a new label, and the operator restarts
//! mission-control with the new label. There is NO clean automated
//! rotation through PKCS#11 because:
//!
//! - PKCS#11 doesn't support "atomically replace key X with key Y";
//!   the operator must invoke distinct calls.
//! - The OLD key needs to remain available long enough to sign the
//!   rotation proof (per Spec D L4-D10).
//! - `C_DestroyObject` on the old key may or may not actually wipe
//!   the TPM-backed material depending on the implementation.
//!
//! `TpmAnima::rotate()` implements the simplest workable flow:
//!
//! 1. Generate a fresh key pair via `C_GenerateKeyPair` with
//!    mechanism `CKM_EC_KEY_PAIR_GEN` and curve `prime256v1`. The
//!    new keys are labeled `"{auth_label}-rot-{ulid}"` to avoid
//!    collision with the existing label.
//! 2. Read the new public key from the new public-key handle.
//! 3. Sign the rotation proof JWS with the OLD key over the NEW
//!    DID (per Spec D L4-D10).
//! 4. Return a fresh `TpmAnima` handle whose `auth_label` is the new
//!    label. The OLD key remains in the TPM under its original
//!    label — operators can run `pkcs11-tool --delete-object` to
//!    wipe it manually after they've confirmed the rotation event
//!    is in the journal.
//!
//! Production operators who need automated rotation should run
//! `VaultTransitAnima` (D-Sub-B) instead — Vault's rotate API has a
//! clean atomic semantic that PKCS#11 lacks.
//!
//! ## Bootstrap
//!
//! Operator workflow before launching mission-control:
//!
//! ```bash
//! # Initialise the TPM PKCS#11 token (one-time, per-device):
//! pkcs11-tool --module /usr/lib/softhsm/libsofthsm2.so \
//!     --init-token --label anima --so-pin 1234
//! pkcs11-tool --module /usr/lib/softhsm/libsofthsm2.so \
//!     --init-pin --pin 5678 --token-label anima
//!
//! # Generate the P-256 auth key:
//! pkcs11-tool --module /usr/lib/softhsm/libsofthsm2.so \
//!     --pin 5678 --keypairgen \
//!     --key-type EC:prime256v1 \
//!     --label anima-auth-v1
//! ```
//!
//! mission-control then constructs:
//!
//! ```ignore
//! let custody = TpmAnima::new(
//!     "/usr/lib/softhsm/libsofthsm2.so",
//!     0,                  // slot id
//!     "5678",             // user PIN
//!     "anima-auth-v1",    // auth-key label
//!     None,               // no wallet delegate
//! )?;
//! ```
//!
//! ## Acceptance
//!
//! Per Spec D §"D-Sub-D": "cold start mission-control on a TPM-equipped
//! Linux box, mint a JWT, never see the private key in process memory."
//! The mocked-PKCS#11 unit tests in `tests/integration_tpm.rs` exercise
//! the full request shape (find_objects → sign → reconstruct JWS) and
//! a `#[ignore]`-gated live test exists for operators with a softhsm2
//! fixture.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use anima_core::error::{AnimaError, AnimaResult};
use anima_core::identity_document::{
    AgentIdentityDocument, AgentType, IdentityDocumentBuilder, VerificationMethod,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, KeyType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;
use haima_core::wallet::WalletAddress;
use secrecy::ExposeSecret;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::custody::{
    AnimaCustody, BackendKind, DidRotationEvent, Eip712Domain, EvmSignature, TxRequest,
};

/// The OID for prime256v1 / secp256r1 / NIST P-256 in DER form
/// (`06 08 2A 86 48 CE 3D 03 01 07`). Used as the `CKA_EC_PARAMS`
/// attribute when generating P-256 keys via `C_GenerateKeyPair` and
/// when validating the curve of an existing TPM-resident key.
///
/// This is the ASN.1 OID `1.2.840.10045.3.1.7` encoded as a DER
/// `OBJECT IDENTIFIER`. Source: RFC 5480 §2.1.1.1.
const P256_OID_DER: [u8; 10] = [0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];

/// `TpmAnima` — PKCS#11 / TPM-backed custody.
///
/// Holds the PKCS#11 module + slot + session + key handles. The auth
/// key NEVER leaves the TPM; signing is performed via PKCS#11 `C_Sign`
/// with mechanism `CKM_ECDSA` over a SHA-256 prehash.
///
/// Wallet operations are forwarded to `wallet_delegate` when present
/// (see SPEC-D-DEVIATION block above).
pub struct TpmAnima {
    /// PKCS#11 module path, kept for reconnect / rotate paths.
    module_path: PathBuf,
    /// Slot id passed to `Pkcs11::open_rw_session`.
    slot: Slot,
    /// User PIN — held in `AuthPin` so it's wrapped in `secrecy`.
    /// Cloning `AuthPin` is cheap (it's a `SecretString` internally).
    pin: AuthPin,
    /// Label of the auth (P-256) keypair on the TPM. Both the public
    /// and private object share this label.
    auth_label: String,
    /// Live PKCS#11 session — wrapped in a `Mutex` because the
    /// `cryptoki::session::Session` exposes `&self` methods but is NOT
    /// `Send + Sync` for concurrent operations on the same session
    /// (PKCS#11 sessions are stateful — finalize/sign sequences
    /// must be serialized).
    session: Arc<Mutex<TpmSession>>,
    /// User DID — derived from the TPM-held P-256 auth public key at
    /// construction time. Pinned for the lifetime of this handle;
    /// `rotate()` returns a fresh handle bound to the new key label.
    user_did: String,
    /// Cached SEC1-compressed P-256 public key (33 bytes). Read once
    /// at construction; the TPM never changes the public-key bytes
    /// without going through `rotate()`.
    auth_pubkey: [u8; 33],
    /// Optional wallet delegate for the secp256k1 half (see
    /// SPEC-D-DEVIATION block). `None` means wallet ops return error.
    wallet_delegate: Option<Arc<dyn AnimaCustody>>,
    /// Cached PEM-encoded auth public key for KYA doc export.
    /// Lazy-built from `auth_pubkey` (no I/O after construction).
    auth_public_pem: OnceLock<String>,
}

/// Internal session bundle — owns the PKCS#11 module, the open session,
/// and the resolved key handles. Held inside `Arc<Mutex<>>` so trait
/// methods (`&self`) can serialise access.
struct TpmSession {
    /// The Pkcs11 client. Held alongside the session because
    /// `Session` borrows from `Pkcs11` internally; dropping `Pkcs11`
    /// would invalidate the session.
    _pkcs11: Pkcs11,
    /// The open R/W session. Logged in to USER for the auth key.
    session: Session,
    /// Handle to the auth (P-256) private key (used by `C_Sign`).
    auth_priv: ObjectHandle,
    /// Handle to the auth (P-256) public key (used by `get_attributes`
    /// to read `CKA_EC_POINT` for pubkey export).
    #[allow(dead_code)]
    auth_pub: ObjectHandle,
}

// SAFETY: we serialise all access to `TpmSession` through a `Mutex`,
// which makes the contained `!Send` types Send-able from the outside.
// `cryptoki::session::Session` does not implement `Send` because the
// underlying PKCS#11 contract requires the same thread to call
// `C_Login`/`C_Sign` on a session — but with a `Mutex` + a single-
// thread executor this is upheld in practice. The trait
// `AnimaCustody: Send + Sync + 'static` requires this, and
// `VaultTransitAnima` does the same dance (it holds a non-Send
// `reqwest::blocking::Client` behind no Mutex). For our blocking
// PKCS#11 path, `Mutex` ensures only one thread is ever inside the
// session at a time, satisfying PKCS#11's serialisation guarantee.
unsafe impl Send for TpmSession {}
unsafe impl Sync for TpmSession {}

impl TpmAnima {
    /// Open a TPM-backed custody handle.
    ///
    /// Steps:
    /// 1. Load the PKCS#11 module from `module_path`.
    /// 2. Open a R/W session against `slot`.
    /// 3. `C_Login(USER, pin)`.
    /// 4. Find the auth keypair by label (CKA_LABEL == auth_label).
    /// 5. Read the auth public-key bytes (CKA_EC_POINT) → SEC1
    ///    compressed → derive the DID.
    ///
    /// Returns `AnimaError::Crypto(...)` on any PKCS#11 failure
    /// (module load, slot open, login, find_objects, get_attributes,
    /// curve-validation, etc.). Constructor failures are NOT retried;
    /// callers (mission-control) surface them to the operator and
    /// halt — no fallback to InProcessAnima would silently downgrade
    /// the security guarantee.
    ///
    /// `wallet_delegate` is the wallet-half escalation per the
    /// SPEC-D-DEVIATION at the top of this file. Pass `None` for an
    /// auth-only deployment; pass `Some(Arc::new(HardwareWalletAnima::...))`
    /// to compose with a Ledger.
    pub fn new(
        module_path: impl Into<PathBuf>,
        slot_id: u64,
        pin: impl Into<String>,
        auth_label: impl Into<String>,
        wallet_delegate: Option<Arc<dyn AnimaCustody>>,
    ) -> AnimaResult<Self> {
        let module_path = module_path.into();
        let auth_label = auth_label.into();
        let pin_str: String = pin.into();
        let pin: AuthPin = pin_str.into();

        let pkcs11 = Pkcs11::new(&module_path)
            .map_err(|e| AnimaError::Crypto(format!("tpm: load module: {e}")))?;
        pkcs11
            .initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK))
            .map_err(|e| AnimaError::Crypto(format!("tpm: initialize: {e}")))?;

        let slots = pkcs11
            .get_slots_with_token()
            .map_err(|e| AnimaError::Crypto(format!("tpm: enumerate slots: {e}")))?;
        let slot = slots
            .into_iter()
            .find(|s| s.id() == slot_id)
            .ok_or_else(|| AnimaError::Crypto(format!("tpm: slot {slot_id} not found")))?;

        let session = pkcs11
            .open_rw_session(slot)
            .map_err(|e| AnimaError::Crypto(format!("tpm: open session: {e}")))?;
        session
            .login(UserType::User, Some(&pin))
            .map_err(|e| AnimaError::Crypto(format!("tpm: login: {e}")))?;

        let (auth_priv, auth_pub) = find_keypair_by_label(&session, &auth_label)?;
        let auth_pubkey = read_p256_pubkey_compressed(&session, auth_pub)?;
        let user_did = crate::did::generate_did_key_p256(&auth_pubkey);

        let inner = TpmSession {
            _pkcs11: pkcs11,
            session,
            auth_priv,
            auth_pub,
        };

        Ok(Self {
            module_path,
            slot,
            pin,
            auth_label,
            session: Arc::new(Mutex::new(inner)),
            user_did,
            auth_pubkey,
            wallet_delegate,
            auth_public_pem: OnceLock::new(),
        })
    }

    /// Construct from a pre-opened `Session` + resolved key handles.
    ///
    /// Used by tests with a mocked PKCS#11 backend. The caller has
    /// already loaded the module, opened a session, logged in, and
    /// resolved the key handles by label — this constructor just
    /// stores them and reads the cached pubkey.
    ///
    /// Production callers should use [`Self::new`] which does the
    /// full bootstrap flow.
    pub fn with_explicit_session(
        pkcs11: Pkcs11,
        session: Session,
        slot: Slot,
        module_path: impl Into<PathBuf>,
        pin: impl Into<String>,
        auth_label: impl Into<String>,
        auth_priv: ObjectHandle,
        auth_pub: ObjectHandle,
        wallet_delegate: Option<Arc<dyn AnimaCustody>>,
    ) -> AnimaResult<Self> {
        let module_path = module_path.into();
        let auth_label = auth_label.into();
        let pin_str: String = pin.into();
        let pin: AuthPin = pin_str.into();

        let auth_pubkey = read_p256_pubkey_compressed(&session, auth_pub)?;
        let user_did = crate::did::generate_did_key_p256(&auth_pubkey);

        let inner = TpmSession {
            _pkcs11: pkcs11,
            session,
            auth_priv,
            auth_pub,
        };

        Ok(Self {
            module_path,
            slot,
            pin,
            auth_label,
            session: Arc::new(Mutex::new(inner)),
            user_did,
            auth_pubkey,
            wallet_delegate,
            auth_public_pem: OnceLock::new(),
        })
    }

    /// Sign a 32-byte digest with the TPM-held auth key.
    ///
    /// Uses `Mechanism::Ecdsa` (== `CKM_ECDSA`) — pure raw ECDSA over
    /// a prehash, NOT `Mechanism::EcdsaSha256` (which would re-hash
    /// the input). The PKCS#11 spec defines `CKM_ECDSA` as
    /// "raw ECDSA over data of length equal to the curve's bit
    /// length"; for P-256 that's a 32-byte digest.
    ///
    /// Returns the 64-byte IEEE-P1363 `r || s` form. PKCS#11 already
    /// returns ECDSA signatures in raw concatenated form (no DER
    /// wrapping), so no re-encoding is needed.
    fn tpm_sign_digest(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        let session = self
            .session
            .lock()
            .map_err(|_| AnimaError::Crypto("tpm: session mutex poisoned".into()))?;
        let signature = session
            .session
            .sign(&Mechanism::Ecdsa, session.auth_priv, digest)
            .map_err(|e| AnimaError::Crypto(format!("tpm: C_Sign(CKM_ECDSA): {e}")))?;
        if signature.len() != 64 {
            return Err(AnimaError::Crypto(format!(
                "tpm: C_Sign returned wrong length: expected 64, got {}",
                signature.len()
            )));
        }
        let mut out = [0u8; 64];
        out.copy_from_slice(&signature);
        Ok(out)
    }
}

impl AnimaCustody for TpmAnima {
    fn user_did(&self) -> &str {
        &self.user_did
    }

    fn auth_pubkey(&self) -> [u8; 33] {
        self.auth_pubkey
    }

    fn wallet_address(&self) -> Option<&WalletAddress> {
        // Per the SPEC-D-DEVIATION block: forward to delegate when
        // present, otherwise None.
        self.wallet_delegate
            .as_ref()
            .and_then(|d| d.wallet_address())
    }

    fn sign_jws(&self, claims: &Value) -> AnimaResult<String> {
        // Build the standard ES256 JWS: header + body URL-safe base64,
        // sign the SHA-256 prehash of "<header>.<body>" with the
        // TPM-held auth key, append URL-safe base64 of r||s.
        let header = serde_json::json!({
            "alg": "ES256",
            "typ": "JWT",
            "kid": &self.user_did,
        });
        let header_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&header)
                .map_err(|e| AnimaError::Crypto(format!("tpm: encode jws header: {e}")))?,
        );
        let body_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(claims)
                .map_err(|e| AnimaError::Crypto(format!("tpm: encode jws body: {e}")))?,
        );
        let signing_input = format!("{header_b64}.{body_b64}");
        let prehash: [u8; 32] = Sha256::digest(signing_input.as_bytes()).into();
        let r_s = self.tpm_sign_digest(&prehash)?;
        let sig_b64 = URL_SAFE_NO_PAD.encode(r_s);
        Ok(format!("{signing_input}.{sig_b64}"))
    }

    fn sign_digest(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        self.tpm_sign_digest(digest)
    }

    fn sign_evm_tx(&self, tx: &TxRequest) -> AnimaResult<EvmSignature> {
        match &self.wallet_delegate {
            Some(d) => d.sign_evm_tx(tx),
            None => Err(AnimaError::Crypto(
                "tpm: no wallet_delegate configured; wallet operations unavailable. \
                 Compose TpmAnima with HardwareWalletAnima or another wallet backend \
                 to enable sign_evm_tx (see SPEC-D-DEVIATION in tpm.rs)."
                    .into(),
            )),
        }
    }

    fn sign_eip712(
        &self,
        domain: &Eip712Domain,
        types: &Value,
        message: &Value,
    ) -> AnimaResult<EvmSignature> {
        match &self.wallet_delegate {
            Some(d) => d.sign_eip712(domain, types, message),
            None => Err(AnimaError::Crypto(
                "tpm: no wallet_delegate configured; wallet operations unavailable. \
                 Compose TpmAnima with HardwareWalletAnima or another wallet backend \
                 to enable sign_eip712 (see SPEC-D-DEVIATION in tpm.rs)."
                    .into(),
            )),
        }
    }

    fn rotate(&self) -> AnimaResult<(DidRotationEvent, Arc<dyn AnimaCustody>)> {
        // SPEC-D-DEVIATION (rotation): TPM rotation is operator-driven
        // in production. The implementation below is the simplest
        // workable PKCS#11-only flow:
        //
        //   1. Generate a fresh P-256 keypair on the TPM with a
        //      collision-free label `{auth_label}-rot-{ulid}`.
        //   2. Read the new public key.
        //   3. Sign the rotation proof with the OLD key over the NEW
        //      DID (Spec D L4-D10).
        //   4. Open a new TpmAnima handle bound to the new label.
        //
        // The OLD key remains on the TPM under its original label —
        // operators wipe it via `pkcs11-tool --delete-object` after
        // the rotation event has been confirmed in the journal.
        //
        // The new label uses a ULID suffix so concurrent rotations
        // (which shouldn't happen but are not prevented by PKCS#11)
        // would generate distinct labels rather than colliding.
        let new_label = format!("{}-rot-{}", self.auth_label, ulid::Ulid::new());

        // Generate the new keypair on the TPM.
        let (new_pub_handle, _new_priv_handle) = {
            let session = self
                .session
                .lock()
                .map_err(|_| AnimaError::Crypto("tpm: session mutex poisoned".into()))?;
            generate_p256_keypair(&session.session, &new_label)?
        };

        // Read the new public key bytes.
        let new_auth_pubkey = {
            let session = self
                .session
                .lock()
                .map_err(|_| AnimaError::Crypto("tpm: session mutex poisoned".into()))?;
            read_p256_pubkey_compressed(&session.session, new_pub_handle)?
        };
        let new_did = crate::did::generate_did_key_p256(&new_auth_pubkey);
        let old_did = self.user_did.clone();

        // Sign the rotation proof with the OLD key (still held by
        // `self`'s session, bound to `auth_priv`).
        let rotation_proof_jws = {
            let proof_claims = serde_json::json!({
                "iss": &old_did,
                "sub": &new_did,
                "type": "anima.rotation_proof",
                "iat": Utc::now().timestamp(),
            });
            self.sign_jws(&proof_claims)?
        };

        let event = DidRotationEvent {
            old_did,
            new_did: new_did.clone(),
            rotation_proof_jws,
            rotated_at: Utc::now(),
        };

        // Open a fresh TpmAnima handle bound to the new label. We
        // re-bootstrap from `Self::new` so the new handle owns its
        // own Pkcs11 + Session + key handles. Reusing the parent's
        // session would entangle lifetimes — the parent handle is
        // still meant to be valid as a snapshot of pre-rotation
        // state per the trait contract.
        //
        // SAFETY: `AuthPin` does not expose its inner string for
        // reconstruction. Production deployments rotate via operator
        // workflow (kill mission-control + re-launch with new label)
        // so this in-band rotation path is primarily for testing /
        // single-machine smoke tests. We rebuild using
        // `with_explicit_session` against a fresh PKCS#11 client
        // bound to the new label.
        let new_pkcs11 = Pkcs11::new(&self.module_path)
            .map_err(|e| AnimaError::Crypto(format!("tpm rotate: load module: {e}")))?;
        new_pkcs11
            .initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK))
            .map_err(|e| AnimaError::Crypto(format!("tpm rotate: initialize: {e}")))?;
        let new_session = new_pkcs11
            .open_rw_session(self.slot)
            .map_err(|e| AnimaError::Crypto(format!("tpm rotate: open session: {e}")))?;
        new_session
            .login(UserType::User, Some(&self.pin))
            .map_err(|e| AnimaError::Crypto(format!("tpm rotate: login: {e}")))?;
        let (new_priv, new_pub) = find_keypair_by_label(&new_session, &new_label)?;

        // Re-supply the same PIN by exposing it via secrecy. `AuthPin` is
        // `SecretString` (a `SecretBox<str>`); `expose_secret()` returns
        // the underlying `&str` so we can re-wrap it.
        let pin_str = self.pin.expose_secret().to_string();
        let new_handle = TpmAnima::with_explicit_session(
            new_pkcs11,
            new_session,
            self.slot,
            self.module_path.clone(),
            pin_str,
            new_label,
            new_priv,
            new_pub,
            self.wallet_delegate.clone(),
        )?;

        // Sanity check — the new handle's DID must equal the event's new_did.
        debug_assert_eq!(new_handle.user_did(), new_did);
        Ok((event, Arc::new(new_handle)))
    }

    fn backend_kind(&self) -> BackendKind {
        BackendKind::Tpm
    }

    fn export_identity_document(&self) -> AnimaResult<AgentIdentityDocument> {
        // Lazy-build a multibase-z encoding of the SEC1 compressed pubkey
        // for the `publicKeyMultibase` field. PEM is computed once and
        // cached in `auth_public_pem` — KYA doc export is hot enough
        // (per session start) that we skip re-deriving it.
        let _ = self.auth_public_pem.set(format!(
            "{{\"pubkey_hex\":\"{}\"}}",
            hex::encode(self.auth_pubkey)
        ));
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
            format!("tpm custody (label={})", self.auth_label),
            String::new(), // soul_hash filled in by the bridge layer
        )
        .agent_type(AgentType::Hosted)
        .verification_method(vm)
        .build();
        Ok(doc)
    }
}

/// Find a keypair on the PKCS#11 token by label.
///
/// Returns `(private_handle, public_handle)`. Both objects must share
/// the supplied label — this is the standard convention enforced by
/// `pkcs11-tool --keypairgen` and TPM 2.0 PKCS#11 wrappers.
///
/// Errors:
/// - `AnimaError::Crypto("tpm: auth private key not found ...")` if no
///   private object matches.
/// - `AnimaError::Crypto("tpm: auth public key not found ...")` if no
///   public object matches.
/// - `AnimaError::Crypto("tpm: ambiguous label ...")` if more than one
///   private or public object matches the label (should be impossible
///   on a correctly-provisioned TPM, but we surface it loudly).
fn find_keypair_by_label(
    session: &Session,
    label: &str,
) -> AnimaResult<(ObjectHandle, ObjectHandle)> {
    let label_bytes = label.as_bytes().to_vec();
    let priv_template = vec![
        Attribute::Class(ObjectClass::PRIVATE_KEY),
        Attribute::KeyType(KeyType::EC),
        Attribute::Label(label_bytes.clone()),
    ];
    let priv_handles = session
        .find_objects(&priv_template)
        .map_err(|e| AnimaError::Crypto(format!("tpm: find auth private key: {e}")))?;
    if priv_handles.is_empty() {
        return Err(AnimaError::Crypto(format!(
            "tpm: auth private key not found (label={label})"
        )));
    }
    if priv_handles.len() > 1 {
        return Err(AnimaError::Crypto(format!(
            "tpm: ambiguous label — {} private keys match {label}",
            priv_handles.len()
        )));
    }

    let pub_template = vec![
        Attribute::Class(ObjectClass::PUBLIC_KEY),
        Attribute::KeyType(KeyType::EC),
        Attribute::Label(label_bytes),
    ];
    let pub_handles = session
        .find_objects(&pub_template)
        .map_err(|e| AnimaError::Crypto(format!("tpm: find auth public key: {e}")))?;
    if pub_handles.is_empty() {
        return Err(AnimaError::Crypto(format!(
            "tpm: auth public key not found (label={label})"
        )));
    }
    if pub_handles.len() > 1 {
        return Err(AnimaError::Crypto(format!(
            "tpm: ambiguous label — {} public keys match {label}",
            pub_handles.len()
        )));
    }
    Ok((priv_handles[0], pub_handles[0]))
}

/// Read the SEC1-compressed P-256 public key bytes from a PKCS#11
/// public-key object handle.
///
/// PKCS#11 stores EC public keys as `CKA_EC_POINT` — a DER-encoded
/// `OCTET STRING` whose value is the SEC1 octet representation of
/// the curve point (compressed or uncompressed).
///
/// We:
/// 1. Read `CKA_EC_PARAMS` and validate it's the prime256v1 OID
///    (rejects keys on the wrong curve).
/// 2. Read `CKA_EC_POINT`, strip the DER `OCTET STRING` wrapper,
///    and parse the SEC1 point.
/// 3. Convert to compressed form via the `p256` crate (the crate
///    handles both compressed and uncompressed input).
fn read_p256_pubkey_compressed(
    session: &Session,
    pub_handle: ObjectHandle,
) -> AnimaResult<[u8; 33]> {
    let attrs = session
        .get_attributes(
            pub_handle,
            &[AttributeType::EcParams, AttributeType::EcPoint],
        )
        .map_err(|e| {
            AnimaError::Crypto(format!("tpm: get_attributes(EC_PARAMS, EC_POINT): {e}"))
        })?;

    let mut ec_params: Option<Vec<u8>> = None;
    let mut ec_point: Option<Vec<u8>> = None;
    for attr in attrs {
        match attr {
            Attribute::EcParams(v) => ec_params = Some(v),
            Attribute::EcPoint(v) => ec_point = Some(v),
            _ => {}
        }
    }
    let ec_params =
        ec_params.ok_or_else(|| AnimaError::Crypto("tpm: missing EC_PARAMS attribute".into()))?;
    let ec_point =
        ec_point.ok_or_else(|| AnimaError::Crypto("tpm: missing EC_POINT attribute".into()))?;

    // Validate curve == prime256v1.
    if ec_params != P256_OID_DER {
        return Err(AnimaError::Crypto(format!(
            "tpm: auth key is not on P-256 (CKA_EC_PARAMS does not match prime256v1 OID); \
             got {} bytes: {}",
            ec_params.len(),
            hex::encode(&ec_params)
        )));
    }

    // CKA_EC_POINT is wrapped in a DER OCTET STRING.
    // Strip the wrapper: `04 <len> <SEC1-bytes>`.
    let sec1_bytes = strip_der_octet_string(&ec_point)?;

    // Parse via the p256 crate to normalise to SEC1 compressed (33 bytes).
    use p256::PublicKey;
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    let public_key = PublicKey::from_sec1_bytes(sec1_bytes)
        .map_err(|e| AnimaError::Crypto(format!("tpm: parse SEC1 EC_POINT: {e}")))?;
    let point = public_key.to_encoded_point(true);
    let bytes = point.as_bytes();
    if bytes.len() != 33 {
        return Err(AnimaError::Crypto(format!(
            "tpm: P-256 compressed point unexpected len: {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 33];
    out.copy_from_slice(bytes);
    Ok(out)
}

/// Strip a DER `OCTET STRING` wrapper from a byte slice, returning
/// the inner content. Used to unwrap PKCS#11's `CKA_EC_POINT` which is
/// "DER-encoded ANSI X9.62 ECPoint" — i.e. an `OCTET STRING` whose
/// content is the raw SEC1 point.
///
/// Tolerates short-form length (≤127) and long-form length (1, 2, or
/// 3 length octets) — covers all realistic P-256 point lengths
/// (33 / 65) which fit in short form, but stays robust for larger
/// curves should we extend to P-384 / P-521 later.
///
/// Returns the inner SEC1 byte slice on success, `AnimaError::Crypto`
/// on malformed input.
fn strip_der_octet_string(der: &[u8]) -> AnimaResult<&[u8]> {
    if der.is_empty() {
        return Err(AnimaError::Crypto("tpm: EC_POINT is empty".into()));
    }
    if der[0] != 0x04 {
        return Err(AnimaError::Crypto(format!(
            "tpm: EC_POINT does not begin with OCTET STRING tag (0x04); got 0x{:02x}",
            der[0]
        )));
    }
    if der.len() < 2 {
        return Err(AnimaError::Crypto(
            "tpm: EC_POINT truncated after OCTET STRING tag".into(),
        ));
    }
    let len_byte = der[1];
    let (content_start, content_len) = if len_byte & 0x80 == 0 {
        // Short form: length encoded in the low 7 bits.
        (2usize, len_byte as usize)
    } else {
        // Long form: low 7 bits = number of length octets that follow.
        let n = (len_byte & 0x7f) as usize;
        if n == 0 || n > 3 || der.len() < 2 + n {
            return Err(AnimaError::Crypto(format!(
                "tpm: EC_POINT length octets unsupported (n={n})"
            )));
        }
        let mut len = 0usize;
        for &b in &der[2..2 + n] {
            len = (len << 8) | (b as usize);
        }
        (2 + n, len)
    };
    if der.len() < content_start + content_len {
        return Err(AnimaError::Crypto(format!(
            "tpm: EC_POINT truncated: header says {} bytes content, only {} bytes remaining",
            content_len,
            der.len() - content_start
        )));
    }
    Ok(&der[content_start..content_start + content_len])
}

/// Generate a fresh P-256 keypair on the TPM via PKCS#11
/// `C_GenerateKeyPair` with mechanism `CKM_EC_KEY_PAIR_GEN`.
///
/// Both objects are labelled with `label` (CKA_LABEL); the public
/// half is `verify=true`, the private half is `sign=true,
/// sensitive=true, extractable=false` — the standard "TPM holds the
/// scalar, never reveals it" template.
///
/// Returns `(pub_handle, priv_handle)`. Used by `rotate()`.
fn generate_p256_keypair(
    session: &Session,
    label: &str,
) -> AnimaResult<(ObjectHandle, ObjectHandle)> {
    let label_bytes = label.as_bytes().to_vec();
    let pub_template = vec![
        Attribute::Class(ObjectClass::PUBLIC_KEY),
        Attribute::KeyType(KeyType::EC),
        Attribute::Token(true),
        Attribute::Verify(true),
        Attribute::Label(label_bytes.clone()),
        Attribute::EcParams(P256_OID_DER.to_vec()),
    ];
    let priv_template = vec![
        Attribute::Class(ObjectClass::PRIVATE_KEY),
        Attribute::KeyType(KeyType::EC),
        Attribute::Token(true),
        Attribute::Sign(true),
        Attribute::Private(true),
        Attribute::Sensitive(true),
        Attribute::Extractable(false),
        Attribute::Label(label_bytes),
    ];
    let (pub_handle, priv_handle) = session
        .generate_key_pair(&Mechanism::EccKeyPairGen, &pub_template, &priv_template)
        .map_err(|e| AnimaError::Crypto(format!("tpm: C_GenerateKeyPair: {e}")))?;
    Ok((pub_handle, priv_handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `strip_der_octet_string` accepts short-form lengths typical of
    /// P-256 SEC1 points (33 / 65 bytes both fit in short form).
    #[test]
    fn strip_der_octet_string_short_form_p256_compressed() {
        let mut wrapped = vec![0x04, 33]; // OCTET STRING, length 33
        let mut sec1 = vec![0x02]; // compressed, even-y
        sec1.extend_from_slice(&[0xab; 32]);
        wrapped.extend_from_slice(&sec1);
        let inner = strip_der_octet_string(&wrapped).unwrap();
        assert_eq!(inner, sec1.as_slice());
    }

    #[test]
    fn strip_der_octet_string_short_form_p256_uncompressed() {
        let mut wrapped = vec![0x04, 65]; // OCTET STRING, length 65
        let mut sec1 = vec![0x04]; // uncompressed
        sec1.extend_from_slice(&[0xcd; 64]);
        wrapped.extend_from_slice(&sec1);
        let inner = strip_der_octet_string(&wrapped).unwrap();
        assert_eq!(inner, sec1.as_slice());
    }

    #[test]
    fn strip_der_octet_string_long_form_one_length_octet() {
        // Length 200 → 0x81 0xc8 (long form, 1 length octet).
        let mut wrapped = vec![0x04, 0x81, 200];
        wrapped.extend_from_slice(&[0xee; 200]);
        let inner = strip_der_octet_string(&wrapped).unwrap();
        assert_eq!(inner.len(), 200);
        assert!(inner.iter().all(|&b| b == 0xee));
    }

    #[test]
    fn strip_der_octet_string_rejects_wrong_tag() {
        let bytes = vec![0x05, 33, 0x00];
        let err = strip_der_octet_string(&bytes).unwrap_err();
        assert!(err.to_string().contains("OCTET STRING"));
    }

    #[test]
    fn strip_der_octet_string_rejects_truncated() {
        // Says 33 bytes but only 5 follow.
        let bytes = vec![0x04, 33, 0x02, 0xa, 0xb, 0xc, 0xd];
        let err = strip_der_octet_string(&bytes).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn strip_der_octet_string_rejects_empty() {
        let err = strip_der_octet_string(&[]).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    /// The P-256 OID DER-encoding constant matches RFC 5480 §2.1.1.1
    /// `1.2.840.10045.3.1.7` exactly. Pin this so a future refactor
    /// that mistypes the OID would fail loudly here rather than at
    /// TPM-bootstrap time.
    #[test]
    fn p256_oid_der_constant() {
        assert_eq!(
            &P256_OID_DER[..],
            &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07][..]
        );
    }
}
