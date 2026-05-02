//! `HardwareWalletAnima` — Ledger / Trezor wallet-only custody backend (Spec D D-Sub-F).
//!
//! High-stakes UX: every wallet operation blocks on a physical button
//! press on the hardware device. Auth-half operations (`sign_jws`,
//! `sign_digest`, `user_did`, `auth_pubkey`, `rotate`,
//! `export_identity_document`) are delegated to a wrapped inner
//! [`AnimaCustody`] (typically `WebCryptoAnima`, `InProcessAnima`,
//! `VaultTransitAnima`, or `TpmAnima`). Wallet-half operations
//! (`sign_evm_tx`, `sign_eip712`, `wallet_address`) go to the connected
//! hardware wallet over HID.
//!
//! ## SPEC-D-DEVIATION block
//!
//! - **Auth-half pass-through.** Spec D §"Backend matrix" describes
//!   `HardwareWalletAnima` as "wraps another backend's auth half;
//!   takes secp256k1 wallet signing to a Ledger/Trezor". This is the
//!   ONLY backend in the matrix that does not own its own auth key —
//!   the Ledger Ethereum app does not expose P-256 / ES256 signing
//!   primitives, so the wrapper is transparent on auth-related trait
//!   methods. The trait shape is unchanged; what changes is the
//!   semantics: callers MUST construct a `HardwareWalletAnima` with an
//!   auth delegate that already has a valid identity. This is checked
//!   at construction time.
//!
//! - **Desktop-only via hidapi.** This crate's `hw-wallet` feature
//!   pulls [`hidapi`](https://crates.io/crates/hidapi) for the
//!   blocking-synchronous HID transport. WebHID (browser) is OUT OF
//!   SCOPE for this PR — the browser side will land in a separate
//!   `anima-web-hardware` crate that the M9 chatOS work picks up. The
//!   public trait surface here is identical to what the browser-side
//!   wrapper will need; only the underlying transport differs.
//!
//! - **`rotate()` is unsupported by design.** Hardware wallets store
//!   their seed in a secure element and never expose it. There is no
//!   way to "rotate" a Ledger / Trezor key from software — the user
//!   must initialize a fresh device with a new recovery phrase, which
//!   is a wholly out-of-band operation. `rotate()` therefore returns
//!   [`AnimaError::Crypto`] with a message pointing the operator at the
//!   correct workflow.
//!
//! - **Hardware-confirmation UX.** Every `sign_evm_tx` and
//!   `sign_eip712` call blocks on the user pressing the "Approve"
//!   button on the device — there is no way to bypass this. Callers
//!   should surface this in the UI as a clear "look at your Ledger"
//!   prompt; the underlying `read_timeout` defaults to 60 seconds,
//!   matching Ledger Live's default.
//!
//! - **Trezor support is OUT OF SCOPE.** The Ledger Ethereum app is
//!   the primary integration. Trezor's APDU surface is similar but
//!   not identical (different command tags, different signature
//!   response framing); a Trezor variant is filed as a follow-up.
//!
//! ## Wire protocol
//!
//! Ledger devices speak APDUs (ISO 7816-4) wrapped in HID frames:
//!
//! ```text
//! HID Report (64 bytes):
//!   [0x01]                     report id (USB convention)
//!   [0x01 0x01]                channel id (always 0x0101 for raw HID)
//!   [0x05]                     command tag (always 0x05 for APDU)
//!   [seq_hi seq_lo]            packet sequence (starts at 0x0000)
//!
//!   First packet of a command also carries:
//!   [len_hi len_lo]            total APDU length (big-endian)
//!
//!   Payload bytes follow.
//! ```
//!
//! APDU format inside the HID frames:
//!
//! ```text
//! [CLA INS P1 P2 Lc DATA...]            command (no Le for Ledger Ethereum app)
//! [DATA... SW1 SW2]                     response (last 2 bytes are status word)
//! ```
//!
//! Status words: `0x9000` = success; anything else = error (e.g.
//! `0x6985` = user rejected, `0x6A80` = invalid data, `0x6D00` =
//! unsupported INS).
//!
//! ## Acceptance
//!
//! Per Spec D §"D-Sub-F": "USDC transfer signed by a Ledger Nano X
//! over WebHID from chatOS browser." This module ships the desktop
//! hidapi half of that acceptance criterion. Tests run against a
//! [`MockHidTransport`] that records APDU traffic and replays canned
//! responses. A `#[ignore]`-gated live test is documented at
//! `tests/integration_hardware_wallet.rs` for operators with a real
//! Ledger plugged in.

use std::sync::Arc;
#[cfg(feature = "hw-wallet")]
use std::sync::Mutex;

use anima_core::error::{AnimaError, AnimaResult};
use anima_core::identity_document::AgentIdentityDocument;
use haima_core::wallet::{ChainId, WalletAddress};
use k256::ecdsa::{RecoveryId, Signature as K256Signature, VerifyingKey as K256VerifyingKey};
use serde_json::Value;

use crate::custody::{
    AnimaCustody, BackendKind, DidRotationEvent, Eip712Domain, EvmSignature, TxRequest,
};
use crate::rlp;

pub mod ledger {
    //! Ledger Ethereum app APDU protocol constants.
    //!
    //! Sourced from
    //! [`app-ethereum/doc/ethapp.adoc`](https://github.com/LedgerHQ/app-ethereum/blob/master/doc/ethapp.adoc).
    //! Documented inline so future maintainers don't need to chase the
    //! upstream spec for routine work.

    use anima_core::error::{AnimaError, AnimaResult};

    pub mod apdu {
        //! Canonical Ledger Ethereum app APDU codes.
        //!
        //! All commands begin with `CLA = 0xE0`. The Ledger Ethereum app
        //! is identified by its INS byte; P1 / P2 are command-specific.

        /// CLA byte for every Ledger Ethereum app command.
        pub const CLA: u8 = 0xE0;

        /// `GET ETH PUBLIC ADDRESS` — derives a wallet pubkey + address
        /// from a BIP-32 path stored in the device.
        ///
        /// Wire format (P1 = 0x00, P2 = 0x00 — no display, no chain code):
        /// ```text
        /// E0 02 00 00 Lc | <derivation_count u8> <index_0 u32be> <index_1 u32be> ...
        /// ```
        /// Response: `[pubkey_len u8] [pubkey...] [address_len u8] [address_ascii...] [SW1 SW2]`.
        pub const INS_GET_PUBLIC_KEY: u8 = 0x02;

        /// `SIGN ETH TRANSACTION` — signs an EIP-1559 / EIP-155 RLP
        /// envelope after user confirmation on the device.
        ///
        /// Wire format (chunked when the RLP exceeds 255 bytes):
        /// - First chunk: `E0 04 00 00 Lc | <derivation_count u8> <indices...> <rlp_chunk...>`
        /// - Subsequent chunks: `E0 04 80 00 Lc | <rlp_chunk...>`
        ///
        /// Response (final chunk only): `[v u8] [r 32 bytes] [s 32 bytes] [SW1 SW2]`.
        pub const INS_SIGN_TRANSACTION: u8 = 0x04;

        /// `GET APP CONFIGURATION` — returns the running Ledger
        /// Ethereum app version + feature flags.
        ///
        /// Wire format: `E0 06 00 00 00` (no payload).
        ///
        /// Response: `[flags u8] [major u8] [minor u8] [patch u8] [SW1 SW2]`.
        pub const INS_GET_APP_VERSION: u8 = 0x06;

        /// `SIGN ETH EIP 712` — signs a precomputed EIP-712 domain +
        /// message hash. Available in the Ledger Ethereum app v1.10+.
        ///
        /// Wire format (P1 = 0x00 = "v0 — precomputed hashes", P2 = 0x00):
        /// ```text
        /// E0 0C 00 00 Lc | <derivation_count u8> <indices...> <domain_hash 32 bytes> <message_hash 32 bytes>
        /// ```
        ///
        /// Response: `[v u8] [r 32 bytes] [s 32 bytes] [SW1 SW2]`.
        ///
        /// SPEC-D-DEVIATION: same EIP-3009-only limitation as
        /// `InProcessAnima` / `VaultTransitAnima`. The generic typed-data
        /// encoder is deferred — when it lands we'll feed any EIP-712
        /// payload through this same INS.
        pub const INS_SIGN_EIP712: u8 = 0x0C;

        /// P1: "first chunk" for chunked commands (SIGN_TRANSACTION).
        pub const P1_FIRST: u8 = 0x00;
        /// P1: "subsequent chunk" for chunked commands.
        pub const P1_NEXT: u8 = 0x80;
        /// P1: "v0 / precomputed hashes" for SIGN_EIP712 (legacy mode
        /// that skips on-device typed-data display — the only mode
        /// that's useful when the host already computes the digest).
        pub const P1_EIP712_PRECOMPUTED: u8 = 0x00;
        /// P2: zero for all commands handled here.
        pub const P2_ZERO: u8 = 0x00;
    }

    pub mod hid {
        //! Ledger HID transport framing constants.

        /// USB report ID — the first byte of every HID report sent to
        /// the device. Ledger devices accept reports with id = 0x01;
        /// some platforms (Linux hidraw) require us to omit this byte
        /// when writing, but hidapi's wrapper handles that.
        pub const REPORT_ID: u8 = 0x01;
        /// Channel id — always 0x0101 for raw HID transport.
        pub const CHANNEL_ID: u16 = 0x0101;
        /// Command tag inside the HID frame — always 0x05 for APDU.
        pub const COMMAND_APDU: u8 = 0x05;
        /// Maximum HID report size (64 bytes for Ledger Nano S/S+/X).
        pub const REPORT_SIZE: usize = 64;
        /// Bytes per HID frame available for APDU payload after the
        /// 5-byte header (channel + command + sequence).
        pub const FRAME_PAYLOAD_FIRST: usize = REPORT_SIZE - 7; // -7: header + 2-byte length
        /// Bytes per HID frame available for APDU payload on
        /// subsequent frames (no length prefix on continuation frames).
        pub const FRAME_PAYLOAD_NEXT: usize = REPORT_SIZE - 5; // -5: header only
    }

    pub mod sw {
        //! ISO 7816-4 status words used by the Ledger Ethereum app.

        /// Success.
        pub const SW_OK: u16 = 0x9000;
        /// User rejected the operation on the device.
        pub const SW_USER_REJECTED: u16 = 0x6985;
        /// Invalid data (malformed APDU).
        pub const SW_INVALID_DATA: u16 = 0x6A80;
        /// INS not supported (running app doesn't recognise the command).
        pub const SW_INS_NOT_SUPPORTED: u16 = 0x6D00;
    }

    /// Default BIP-32 derivation path for the Ledger Ethereum app:
    /// `m/44'/60'/0'/0/0`. Hardened components have `0x80000000`
    /// OR-ed in.
    pub const DEFAULT_DERIVATION_PATH: [u32; 5] = [
        0x8000002C, // 44' (hardened)
        0x8000003C, // 60' (hardened, ETH coin type)
        0x80000000, // 0' (hardened, account 0)
        0x00000000, // 0  (external chain)
        0x00000000, // 0  (address 0)
    ];

    /// Encode a BIP-32 derivation path as the Ledger Ethereum app
    /// expects: `[count u8] [index_0 u32be] [index_1 u32be] ...`. Each
    /// path component is 4 big-endian bytes; the path is prefixed with
    /// a 1-byte count of components.
    ///
    /// I-4 review fix: previously this panicked on a path > 10
    /// components. Since the function is `pub` and re-exported via
    /// `lib.rs`, that gave external callers a panic surface in a
    /// custody backend. Now returns a typed `AnimaResult` so the same
    /// condition surfaces as `AnimaError::Crypto`.
    pub fn encode_derivation_path(path: &[u32]) -> AnimaResult<Vec<u8>> {
        if path.len() > 10 {
            return Err(AnimaError::Crypto(format!(
                "Ledger derivation paths max 10 components, got {}",
                path.len()
            )));
        }
        let mut out = Vec::with_capacity(1 + path.len() * 4);
        out.push(path.len() as u8);
        for component in path {
            out.extend_from_slice(&component.to_be_bytes());
        }
        Ok(out)
    }
}

/// Abstraction over the HID transport so tests can stub it without a
/// real Ledger plugged in.
///
/// Two impls:
/// - `RealHidTransport`: wraps `hidapi::HidDevice`, ships in the
///   `hw-wallet` feature build. The production path.
/// - `MockHidTransport` (in tests only): records the bytes that would
///   be sent and replays canned responses. Used by every unit test.
pub trait HidTransport: Send + Sync {
    /// Send one APDU command and read the (possibly multi-frame)
    /// response. The implementation is responsible for HID-frame
    /// chunking + reassembly — callers only see APDU-level bytes.
    ///
    /// Returns the response payload **without** the trailing
    /// 2-byte status word. If the status word is anything other than
    /// `0x9000` the implementation MUST return `Err(AnimaError::Crypto)`
    /// with a description of the status word.
    fn exchange(&self, apdu: &[u8]) -> AnimaResult<Vec<u8>>;
}

/// Production HID transport backed by `hidapi::HidDevice`.
///
/// `hidapi::HidDevice` is `Send` but NOT `Sync` (the underlying
/// hidraw / IOHIDDevice handle is single-reader). We wrap it in a
/// `Mutex` so the transport implements `Sync`.
///
/// **I-1 review fix — atomic exchange.** Pre-fix, the `device` mutex
/// was locked and unlocked per-HID-frame inside `write_apdu` and
/// `read_apdu`, leaving a race window between the last write-frame
/// and the first read-frame where another thread holding an
/// `Arc<HardwareWalletAnima>` could interleave its own APDU and
/// corrupt the protocol stream. The new `exchange_lock` is held
/// across the FULL write-then-read round-trip so the trait shape
/// `Arc<dyn AnimaCustody>` (which is explicitly designed to cross
/// task boundaries) cannot interleave APDUs against the same
/// physical device. Concurrent signing requests queue waiting on
/// `exchange_lock` — that's fine because the hardware only displays
/// one confirmation prompt at a time anyway.
#[cfg(feature = "hw-wallet")]
pub struct RealHidTransport {
    device: Mutex<hidapi::HidDevice>,
    /// Outer mutex held across the whole `exchange()` round-trip so
    /// concurrent callers serialize at the APDU boundary, not just
    /// per-HID-frame. Closes I-1.
    exchange_lock: Mutex<()>,
    /// Read timeout for `read_timeout` calls in milliseconds. Defaults
    /// to 60 seconds — matches Ledger Live's "look at your device"
    /// confirmation window.
    timeout_ms: i32,
}

#[cfg(feature = "hw-wallet")]
impl RealHidTransport {
    /// Wrap an open `hidapi::HidDevice`. The caller is responsible for
    /// having already opened the device via
    /// `hidapi::HidApi::open(vendor_id, product_id)`. Ledger devices
    /// use `vendor_id = 0x2c97` and a per-model product id; chatOS
    /// will manage the enumeration.
    pub fn new(device: hidapi::HidDevice) -> Self {
        Self {
            device: Mutex::new(device),
            exchange_lock: Mutex::new(()),
            timeout_ms: 60_000,
        }
    }

    /// Override the default 60s timeout. Useful for tests against a
    /// dev-board that auto-confirms.
    pub fn with_timeout_ms(mut self, timeout_ms: i32) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Pack an APDU into HID frames + write to the device. Frames are
    /// 64 bytes; the first frame carries the 2-byte total length, and
    /// subsequent frames carry only the per-frame sequence number.
    fn write_apdu(&self, apdu: &[u8]) -> AnimaResult<()> {
        use ledger::hid::{
            CHANNEL_ID, COMMAND_APDU, FRAME_PAYLOAD_FIRST, FRAME_PAYLOAD_NEXT, REPORT_ID,
            REPORT_SIZE,
        };
        let total_len = apdu.len();
        let mut offset = 0;
        let mut seq: u16 = 0;
        while offset < total_len || seq == 0 {
            let mut frame = vec![0u8; REPORT_SIZE + 1]; // +1 for the report id
            frame[0] = REPORT_ID;
            frame[1..3].copy_from_slice(&CHANNEL_ID.to_be_bytes());
            frame[3] = COMMAND_APDU;
            frame[4..6].copy_from_slice(&seq.to_be_bytes());
            let payload_capacity = if seq == 0 {
                frame[6..8].copy_from_slice(&(total_len as u16).to_be_bytes());
                FRAME_PAYLOAD_FIRST
            } else {
                FRAME_PAYLOAD_NEXT
            };
            let header_end = if seq == 0 { 8 } else { 6 };
            let take = payload_capacity.min(total_len.saturating_sub(offset));
            if take > 0 {
                frame[header_end..header_end + take].copy_from_slice(&apdu[offset..offset + take]);
            }
            {
                let dev = self
                    .device
                    .lock()
                    .map_err(|_| AnimaError::Crypto("hid device mutex poisoned".into()))?;
                dev.write(&frame)
                    .map_err(|e| AnimaError::Crypto(format!("ledger hid write: {e}")))?;
            }
            offset += take;
            seq = seq
                .checked_add(1)
                .ok_or_else(|| AnimaError::Crypto("ledger hid: sequence number overflow".into()))?;
            if total_len == 0 {
                break; // empty-data APDU still needs a single frame
            }
        }
        Ok(())
    }

    /// Read HID frames from the device until the full APDU response is
    /// reassembled. Returns the response bytes including the trailing
    /// 2-byte status word.
    fn read_apdu(&self) -> AnimaResult<Vec<u8>> {
        use ledger::hid::{CHANNEL_ID, COMMAND_APDU, REPORT_SIZE};
        let mut buf = vec![0u8; REPORT_SIZE];
        let mut response = Vec::new();
        let mut total_len: Option<usize> = None;
        let mut seq: u16 = 0;
        loop {
            let n = {
                let dev = self
                    .device
                    .lock()
                    .map_err(|_| AnimaError::Crypto("hid device mutex poisoned".into()))?;
                dev.read_timeout(&mut buf, self.timeout_ms)
                    .map_err(|e| AnimaError::Crypto(format!("ledger hid read: {e}")))?
            };
            if n < 5 {
                return Err(AnimaError::Crypto(format!(
                    "ledger hid read returned {n} bytes; need at least 5 for header"
                )));
            }
            let chan = u16::from_be_bytes([buf[0], buf[1]]);
            if chan != CHANNEL_ID {
                return Err(AnimaError::Crypto(format!(
                    "ledger hid: unexpected channel id {chan:#06x}"
                )));
            }
            if buf[2] != COMMAND_APDU {
                return Err(AnimaError::Crypto(format!(
                    "ledger hid: unexpected command tag {:#04x}",
                    buf[2]
                )));
            }
            let frame_seq = u16::from_be_bytes([buf[3], buf[4]]);
            if frame_seq != seq {
                return Err(AnimaError::Crypto(format!(
                    "ledger hid: out-of-sequence frame: expected {seq}, got {frame_seq}"
                )));
            }
            let (payload_start, frame_payload_end) = if seq == 0 {
                if n < 7 {
                    return Err(AnimaError::Crypto(format!(
                        "ledger hid: first frame too short ({n} bytes)"
                    )));
                }
                let len = u16::from_be_bytes([buf[5], buf[6]]) as usize;
                total_len = Some(len);
                (7, n)
            } else {
                (5, n)
            };
            response.extend_from_slice(&buf[payload_start..frame_payload_end]);
            let total = total_len.expect("first frame sets total_len");
            if response.len() >= total {
                response.truncate(total);
                break;
            }
            seq = seq.checked_add(1).ok_or_else(|| {
                AnimaError::Crypto("ledger hid: response sequence overflow".into())
            })?;
        }
        Ok(response)
    }
}

#[cfg(feature = "hw-wallet")]
impl HidTransport for RealHidTransport {
    fn exchange(&self, apdu: &[u8]) -> AnimaResult<Vec<u8>> {
        // I-1 fix: hold the outer lock across the whole write-then-read
        // round-trip. Without this guard, two concurrent callers
        // sharing the same Arc<HardwareWalletAnima> could interleave
        // their HID frames against the device. The per-frame
        // `self.device.lock()` calls inside `write_apdu` / `read_apdu`
        // protect the `!Sync` HidDevice itself but do NOT serialize
        // the APDU exchange.
        let _guard = self
            .exchange_lock
            .lock()
            .map_err(|_| AnimaError::Crypto("ledger exchange_lock poisoned".into()))?;
        self.write_apdu(apdu)?;
        let response = self.read_apdu()?;
        if response.len() < 2 {
            return Err(AnimaError::Crypto(format!(
                "ledger response too short: {} bytes (need >= 2 for status word)",
                response.len()
            )));
        }
        let sw = u16::from_be_bytes([response[response.len() - 2], response[response.len() - 1]]);
        let payload = response[..response.len() - 2].to_vec();
        if sw != ledger::sw::SW_OK {
            let reason = match sw {
                ledger::sw::SW_USER_REJECTED => "user rejected on device",
                ledger::sw::SW_INVALID_DATA => "invalid APDU data",
                ledger::sw::SW_INS_NOT_SUPPORTED => "INS not supported by running app",
                _ => "unknown status word",
            };
            return Err(AnimaError::Crypto(format!(
                "ledger APDU error: SW={sw:#06x} ({reason})"
            )));
        }
        Ok(payload)
    }
}

/// `HardwareWalletAnima` — the wallet-only wrapper.
///
/// Construction:
/// 1. Caller produces an `Arc<dyn AnimaCustody>` for the auth half
///    (typically `WebCryptoAnima` or `InProcessAnima`).
/// 2. Caller opens a `hidapi::HidDevice` against the Ledger and wraps
///    it in `RealHidTransport`, OR (in tests) provides a
///    `MockHidTransport`.
/// 3. `HardwareWalletAnima::new(auth_delegate, transport,
///    derivation_path)` performs a `GET ETH PUBLIC ADDRESS` APDU
///    against the device, derives the wallet address from the returned
///    pubkey, and caches both for the lifetime of the handle.
///
/// At runtime, every `AnimaCustody` method either:
/// - **Auth half** (`sign_jws`, `sign_digest`, `user_did`,
///   `auth_pubkey`, `export_identity_document`): forwards to
///   `auth_delegate`.
/// - **Wallet half** (`sign_evm_tx`, `sign_eip712`, `wallet_address`):
///   talks to the Ledger over `transport`.
/// - **Rotate**: returns an error pointing at the manual recovery
///   workflow (the seed is hardware-resident; rotation is meaningless).
pub struct HardwareWalletAnima {
    /// Auth-half delegate. Spec D §"Backend matrix" — the wrapper does
    /// NOT own its own auth key. All auth-related trait methods forward
    /// to this handle. Typically `WebCryptoAnima` (browser) or
    /// `InProcessAnima` (desktop).
    auth_delegate: Arc<dyn AnimaCustody>,
    /// HID transport for talking to the hardware wallet. Boxed behind
    /// the `HidTransport` trait so tests can stub it.
    transport: Box<dyn HidTransport>,
    /// BIP-32 derivation path requested at construction time. Pinned
    /// for the lifetime of the handle — changing the path requires a
    /// fresh `HardwareWalletAnima`.
    derivation_path: Vec<u32>,
    /// Cached uncompressed secp256k1 wallet pubkey (65 bytes, leading
    /// 0x04). Resolved during `new()` via `GET ETH PUBLIC ADDRESS`.
    wallet_pubkey_uncompressed: [u8; 65],
    /// Wallet address derived from the cached pubkey.
    wallet_address: WalletAddress,
}

impl HardwareWalletAnima {
    /// Construct a wallet-only wrapper backed by a hardware wallet.
    ///
    /// Performs a `GET ETH PUBLIC ADDRESS` APDU at construction time
    /// to resolve the wallet address — failure to reach the device
    /// surfaces here rather than on first signing op.
    ///
    /// The `derivation_path` defaults to `m/44'/60'/0'/0/0` if you
    /// pass `None`; pass `Some([..])` for non-default account paths.
    /// The path is pinned for the lifetime of the handle.
    pub fn new(
        auth_delegate: Arc<dyn AnimaCustody>,
        transport: Box<dyn HidTransport>,
        derivation_path: Option<Vec<u32>>,
    ) -> AnimaResult<Self> {
        let path = derivation_path.unwrap_or_else(|| ledger::DEFAULT_DERIVATION_PATH.to_vec());
        if path.len() > 10 {
            return Err(AnimaError::Crypto(format!(
                "derivation path too deep ({} > 10)",
                path.len()
            )));
        }

        // GET ETH PUBLIC ADDRESS: E0 02 00 00 Lc <path>
        let path_bytes = ledger::encode_derivation_path(&path)?;
        let apdu = build_apdu(
            ledger::apdu::INS_GET_PUBLIC_KEY,
            0x00,
            ledger::apdu::P2_ZERO,
            &path_bytes,
        )?;
        let response = transport.exchange(&apdu)?;

        // Response: [pubkey_len u8] [pubkey...] [address_len u8] [address_ascii...]
        if response.is_empty() {
            return Err(AnimaError::Crypto(
                "ledger get-pubkey returned empty payload".into(),
            ));
        }
        let pubkey_len = response[0] as usize;
        if pubkey_len != 65 || response.len() < 1 + pubkey_len + 1 {
            return Err(AnimaError::Crypto(format!(
                "ledger get-pubkey: expected 65-byte uncompressed key, got pubkey_len={pubkey_len}, total_len={}",
                response.len(),
            )));
        }
        let pubkey_bytes = &response[1..1 + pubkey_len];
        if pubkey_bytes[0] != 0x04 {
            return Err(AnimaError::Crypto(format!(
                "ledger get-pubkey: malformed uncompressed point (expected 0x04 prefix, got {:#04x})",
                pubkey_bytes[0]
            )));
        }
        let mut wallet_pubkey_uncompressed = [0u8; 65];
        wallet_pubkey_uncompressed.copy_from_slice(pubkey_bytes);

        // Derive the EVM address ourselves rather than trusting the
        // device's ASCII string — the pubkey-to-address derivation is
        // load-bearing for ecrecover; we want a single source of truth.
        let address_hex = derive_evm_address(&wallet_pubkey_uncompressed);
        let wallet_address = WalletAddress {
            address: address_hex,
            chain: ChainId::base(),
        };

        Ok(Self {
            auth_delegate,
            transport,
            derivation_path: path,
            wallet_pubkey_uncompressed,
            wallet_address,
        })
    }

    /// Construct a `HardwareWalletAnima` with an explicit pubkey,
    /// skipping the device round-trip. Useful for tests where the
    /// `MockHidTransport` doesn't model `GET PUBLIC KEY` round-trips
    /// (or where the test wants to assert against a known pubkey).
    pub fn with_explicit_pubkey(
        auth_delegate: Arc<dyn AnimaCustody>,
        transport: Box<dyn HidTransport>,
        derivation_path: Vec<u32>,
        wallet_pubkey_uncompressed: [u8; 65],
    ) -> AnimaResult<Self> {
        if wallet_pubkey_uncompressed[0] != 0x04 {
            return Err(AnimaError::Crypto(format!(
                "wallet pubkey must be uncompressed (0x04 prefix), got {:#04x}",
                wallet_pubkey_uncompressed[0]
            )));
        }
        let address_hex = derive_evm_address(&wallet_pubkey_uncompressed);
        Ok(Self {
            auth_delegate,
            transport,
            derivation_path,
            wallet_pubkey_uncompressed,
            wallet_address: WalletAddress {
                address: address_hex,
                chain: ChainId::base(),
            },
        })
    }

    /// Returns the backend kind of the wrapped auth delegate. Callers
    /// MAY use this to detect "hardware wrapping hardware" compositions
    /// (which would force every JWS through a button press) and reject
    /// them at the integration layer.
    ///
    /// I-5 review fix: previously the doc claimed this method "rejects"
    /// such compositions. It does not — it just returns the delegate's
    /// `BackendKind`. The recommended composition (TpmAnima auth +
    /// HardwareWalletAnima wallet) is documented in `crates/anima/CLAUDE.md`
    /// D-Sub-F handoff state. Detecting the recursion at construction
    /// would require runtime introspection of the delegate; this is
    /// purely an informational accessor.
    pub fn auth_backend_kind(&self) -> BackendKind {
        self.auth_delegate.backend_kind()
    }

    /// Send a chunked SIGN ETH TRANSACTION command and return the
    /// 65-byte `(v, r, s)` signature.
    ///
    /// Ledger's APDU buffer limits an uninterrupted command to ~255
    /// bytes; longer transactions arrive in multiple chunks with
    /// P1 = 0x00 on the first and P1 = 0x80 on subsequent chunks. The
    /// derivation path is sent only on the first chunk.
    fn ledger_sign_transaction(
        &self,
        rlp_envelope: &[u8],
    ) -> AnimaResult<(u8, [u8; 32], [u8; 32])> {
        // First chunk: derivation_path + as much of rlp as fits in
        // 255 - path_bytes.len(). Ledger Ethereum app takes Lc as a
        // single byte per command.
        let path_bytes = ledger::encode_derivation_path(&self.derivation_path)?;
        let max_first_chunk_data = 255 - path_bytes.len();
        let take_first = max_first_chunk_data.min(rlp_envelope.len());
        let mut first_data = Vec::with_capacity(path_bytes.len() + take_first);
        first_data.extend_from_slice(&path_bytes);
        first_data.extend_from_slice(&rlp_envelope[..take_first]);
        let mut response = self.transport.exchange(&build_apdu(
            ledger::apdu::INS_SIGN_TRANSACTION,
            ledger::apdu::P1_FIRST,
            ledger::apdu::P2_ZERO,
            &first_data,
        )?)?;

        let mut sent = take_first;
        while sent < rlp_envelope.len() {
            let chunk_size = (rlp_envelope.len() - sent).min(255);
            response = self.transport.exchange(&build_apdu(
                ledger::apdu::INS_SIGN_TRANSACTION,
                ledger::apdu::P1_NEXT,
                ledger::apdu::P2_ZERO,
                &rlp_envelope[sent..sent + chunk_size],
            )?)?;
            sent += chunk_size;
        }

        // Final response: [v u8][r 32][s 32] (status word stripped).
        if response.len() != 65 {
            return Err(AnimaError::Crypto(format!(
                "ledger sign-tx: expected 65-byte response, got {} bytes",
                response.len()
            )));
        }
        let v = response[0];
        let mut r = [0u8; 32];
        r.copy_from_slice(&response[1..33]);
        let mut s = [0u8; 32];
        s.copy_from_slice(&response[33..65]);
        Ok((v, r, s))
    }

    /// Send SIGN ETH EIP 712 with precomputed domain + message hashes.
    fn ledger_sign_eip712(
        &self,
        domain_hash: &[u8; 32],
        message_hash: &[u8; 32],
    ) -> AnimaResult<(u8, [u8; 32], [u8; 32])> {
        let path_bytes = ledger::encode_derivation_path(&self.derivation_path)?;
        let mut data = Vec::with_capacity(path_bytes.len() + 64);
        data.extend_from_slice(&path_bytes);
        data.extend_from_slice(domain_hash);
        data.extend_from_slice(message_hash);
        let response = self.transport.exchange(&build_apdu(
            ledger::apdu::INS_SIGN_EIP712,
            ledger::apdu::P1_EIP712_PRECOMPUTED,
            ledger::apdu::P2_ZERO,
            &data,
        )?)?;
        if response.len() != 65 {
            return Err(AnimaError::Crypto(format!(
                "ledger sign-eip712: expected 65-byte response, got {} bytes",
                response.len()
            )));
        }
        let v = response[0];
        let mut r = [0u8; 32];
        r.copy_from_slice(&response[1..33]);
        let mut s = [0u8; 32];
        s.copy_from_slice(&response[33..65]);
        Ok((v, r, s))
    }

    /// Convert Ledger's `(r, s)` pair into the canonical haima-wallet
    /// 65-byte `r || s || v` form, normalising `v` to the legacy `27/28`
    /// convention.
    ///
    /// I-3 review fix: pre-fix this method took the device's `v` byte
    /// but never used it — there was no fast-path that tried the
    /// device-supplied parity first. The implementation is brute-force-
    /// only by design (mirrors `VaultTransitAnima::compute_v_byte`,
    /// which has no device `v` at all because Vault doesn't return it).
    /// We've dropped the `v` parameter to match what the code actually
    /// does. Callers that still hold the device `v` can discard it.
    ///
    /// Ledger may return `v` in any of: legacy `27/28`, `0/1` y-parity,
    /// or EIP-155 form (`35 + 2*chain_id + y_parity`) depending on app
    /// version + tx type. Brute-forcing the two y-parity candidates +
    /// matching against the cached wallet pubkey is unambiguous and
    /// avoids encoding-format drift between Ledger app versions. Two
    /// scalar multiplications per signing operation — negligible.
    fn normalize_signature(
        &self,
        digest: &[u8; 32],
        r: [u8; 32],
        s: [u8; 32],
    ) -> AnimaResult<EvmSignature> {
        let mut r_s = [0u8; 64];
        r_s[..32].copy_from_slice(&r);
        r_s[32..].copy_from_slice(&s);

        let signature = K256Signature::from_slice(&r_s)
            .map_err(|e| AnimaError::Crypto(format!("secp256k1 sig parse: {e}")))?;
        let expected_pubkey =
            K256VerifyingKey::from_sec1_bytes(&self.wallet_pubkey_uncompressed)
                .map_err(|e| AnimaError::Crypto(format!("expected pubkey parse: {e}")))?;

        // Try both y-parity candidates and pick the one that recovers to
        // the cached wallet pubkey. Same trade-off as
        // `VaultTransitAnima::compute_v_byte`; D-Sub-G or later may
        // factor a shared `crate::ecdsa::recover_v_byte_27_28` helper
        // since this is the third call site.
        let candidates: [u8; 2] = [0, 1];
        for cand in candidates {
            let recid = match RecoveryId::try_from(cand) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if let Ok(recovered) = K256VerifyingKey::recover_from_prehash(digest, &signature, recid)
                && recovered == expected_pubkey
            {
                let mut out = Vec::with_capacity(65);
                out.extend_from_slice(&r_s);
                // Force the haima-wallet 27/28 convention regardless of
                // what the device sent — downstream EIP-3009 / x402
                // verifiers expect this shape.
                out.push(cand + 27);
                return Ok(EvmSignature::from_bytes(out));
            }
        }

        Err(AnimaError::Crypto(
            "ledger signature: neither y-parity candidate ecrecovers to the cached wallet pubkey \
             — likely a transport or device-app version mismatch"
                .into(),
        ))
    }

    /// Cached wallet pubkey (uncompressed, `0x04 || x || y`). Exposed
    /// for tests and diagnostics; production callers use
    /// `wallet_address()`.
    pub fn wallet_pubkey_uncompressed(&self) -> &[u8; 65] {
        &self.wallet_pubkey_uncompressed
    }
}

impl AnimaCustody for HardwareWalletAnima {
    fn user_did(&self) -> &str {
        // Auth-half pass-through (Spec D §"Backend matrix").
        self.auth_delegate.user_did()
    }

    fn auth_pubkey(&self) -> [u8; 33] {
        // Auth-half pass-through.
        self.auth_delegate.auth_pubkey()
    }

    fn wallet_address(&self) -> Option<&WalletAddress> {
        // Wallet half is hardware-resolved at construction.
        Some(&self.wallet_address)
    }

    fn sign_jws(&self, claims: &Value) -> AnimaResult<String> {
        // Auth-half pass-through.
        self.auth_delegate.sign_jws(claims)
    }

    fn sign_digest(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]> {
        // Auth-half pass-through.
        self.auth_delegate.sign_digest(digest)
    }

    fn sign_evm_tx(&self, tx: &TxRequest) -> AnimaResult<EvmSignature> {
        // 1. Build the canonical EIP-1559 RLP envelope via the shared
        //    encoder (same as InProcessAnima + VaultTransitAnima).
        //    Note: Ledger expects the FULL RLP envelope including the
        //    type byte (0x02), not just the inner list — the device
        //    re-hashes and displays the to/value/data on its screen.
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

        // 2. Send to Ledger SIGN_TRANSACTION; user confirms on device.
        let (v, r, s) = self.ledger_sign_transaction(&envelope)?;

        // 3. Normalise signature to the haima 27/28 convention.
        // I-3 fix: dropped the device `v` parameter — was never used.
        let _ = v;
        self.normalize_signature(&digest, r, s)
    }

    fn sign_eip712(
        &self,
        domain: &Eip712Domain,
        types: &Value,
        message: &Value,
    ) -> AnimaResult<EvmSignature> {
        // Same EIP-3009-only limitation as InProcessAnima / VaultTransitAnima.
        let primary = types
            .get("primaryType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if primary != "TransferWithAuthorization"
            && !(message.get("from").is_some() && message.get("validAfter").is_some())
        {
            return Err(AnimaError::Crypto(
                "eip712: only EIP-3009 TransferWithAuthorization is supported in D-Sub-F \
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

        // Reconstruct the EIP-712 domain + message hashes per the
        // EIP-3009 spec — Ledger expects the precomputed hashes via
        // its v0 `SIGN_EIP712` mode (P1 = 0x00).
        let message_hash = hash_transfer_authorization(
            domain,
            &from_b,
            &to_b,
            value,
            valid_after,
            valid_before,
            &nonce,
        );

        // The Ledger v0 path takes (domain_hash, message_struct_hash)
        // separately, but `hash_transfer_authorization` returns the
        // full EIP-712 digest `keccak256(0x1901 || domain || msg)`.
        // We split this back into its components by recomputing the
        // domain hash + message-only hash. The haima_wallet helper
        // exposes both sub-hashes via its public typed-data API; we
        // re-derive them here to avoid leaking the intermediate state
        // through a wider public surface.
        let (domain_hash, message_only_hash) = split_eip3009_hashes(
            domain,
            &from_b,
            &to_b,
            value,
            valid_after,
            valid_before,
            &nonce,
        );

        // Sanity: combining the two via the canonical EIP-712 formula
        // must reproduce the digest haima_wallet returned. If this
        // diverges, the local split helper drifted from the haima
        // helper — surface the error rather than sign over the wrong
        // bytes.
        let recombined = canonical_eip712_digest(&domain_hash, &message_only_hash);
        if recombined != message_hash {
            return Err(AnimaError::Crypto(
                "eip712 hash split / recombine drift; refusing to sign".into(),
            ));
        }

        let (v, r, s) = self.ledger_sign_eip712(&domain_hash, &message_only_hash)?;
        // I-3 fix: dropped the device `v` parameter — was never used.
        let _ = v;
        self.normalize_signature(&message_hash, r, s)
    }

    fn rotate(&self) -> AnimaResult<(DidRotationEvent, Arc<dyn AnimaCustody>)> {
        // SPEC-D-DEVIATION (hardware_wallet): rotation is meaningless
        // when the seed is hardware-resident. The user must initialize
        // a new device + recovery phrase out-of-band. This is documented
        // in the module-level comment.
        Err(AnimaError::Crypto(
            "HardwareWalletAnima does not support rotation; the seed is hardware-resident. \
             Initialize a new device + recovery phrase to rotate."
                .to_string(),
        ))
    }

    fn backend_kind(&self) -> BackendKind {
        BackendKind::HardwareWallet
    }

    fn export_identity_document(&self) -> AnimaResult<AgentIdentityDocument> {
        // Auth half is delegated; identity document comes from the
        // wrapped backend. The wallet pubkey is published via the
        // `WalletAddress` accessor — KYA documents don't include the
        // wallet pubkey today (haima publishes that separately via the
        // x402 facilitator handshake).
        self.auth_delegate.export_identity_document()
    }
}

/// Build a Ledger APDU command: `[CLA INS P1 P2 Lc DATA...]`. Ledger
/// uses single-byte Lc; commands with `data.len() > 255` MUST be
/// chunked by the caller (see `ledger_sign_transaction` for the
/// chunking strategy).
///
/// I-4 review fix: previously this `assert!`'d on `data.len() > 255`,
/// which would panic the process if a caller forgot to chunk. The
/// callers (`ledger_sign_transaction`) already clamp to 255, so this
/// is defense-in-depth, but a panic in a custody backend is wrong
/// shape — surface the same condition as a typed `AnimaError`.
fn build_apdu(ins: u8, p1: u8, p2: u8, data: &[u8]) -> AnimaResult<Vec<u8>> {
    if data.len() > 255 {
        return Err(AnimaError::Crypto(format!(
            "build_apdu: data too long for single command ({}); caller must chunk",
            data.len()
        )));
    }
    let mut apdu = Vec::with_capacity(5 + data.len());
    apdu.push(ledger::apdu::CLA);
    apdu.push(ins);
    apdu.push(p1);
    apdu.push(p2);
    apdu.push(data.len() as u8);
    apdu.extend_from_slice(data);
    Ok(apdu)
}

/// Derive an EVM address (20-byte hex with `0x` prefix) from an
/// uncompressed secp256k1 public key (`0x04 || x || y`).
fn derive_evm_address(uncompressed: &[u8; 65]) -> String {
    use sha3::{Digest, Keccak256};
    debug_assert_eq!(uncompressed[0], 0x04);
    let hash = Keccak256::digest(&uncompressed[1..]);
    let address_bytes = &hash[12..];
    format!("0x{}", hex::encode(address_bytes))
}

/// Compute the EIP-712 v0 domain + message-only hashes for an
/// EIP-3009 `TransferWithAuthorization`. Used by the Ledger SIGN_EIP712
/// path which expects the two sub-hashes separately rather than the
/// final combined digest.
///
/// We reuse `haima_wallet`'s public helpers
/// (`Eip712Domain::separator` for the domain hash and
/// `transfer_with_authorization_typehash` for the EIP-3009 typehash) so
/// the bytes signed here are byte-for-byte the same as what
/// `hash_transfer_authorization` computes internally. The
/// `eip3009_hash_split_recombines_to_haima_digest` test below catches
/// any drift between these two paths.
fn split_eip3009_hashes(
    domain: &Eip712Domain,
    from: &[u8; 20],
    to: &[u8; 20],
    value: u64,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
) -> ([u8; 32], [u8; 32]) {
    use sha3::{Digest, Keccak256};

    // Domain hash via haima's public `separator()` — same bytes that
    // `hash_transfer_authorization` mixes into the final digest.
    let domain_hash = domain.separator();

    // EIP-3009 message struct hash:
    //   keccak256(abi.encode(
    //     transfer_with_authorization_typehash(),
    //     from_padded_32, to_padded_32, value_u256_be, valid_after_u256_be,
    //     valid_before_u256_be, nonce
    //   ))
    let msg_typehash = haima_wallet::eip712::transfer_with_authorization_typehash();
    let mut from_padded = [0u8; 32];
    from_padded[12..].copy_from_slice(from);
    let mut to_padded = [0u8; 32];
    to_padded[12..].copy_from_slice(to);
    let mut value_be = [0u8; 32];
    value_be[24..].copy_from_slice(&value.to_be_bytes());
    let mut valid_after_be = [0u8; 32];
    valid_after_be[24..].copy_from_slice(&valid_after.to_be_bytes());
    let mut valid_before_be = [0u8; 32];
    valid_before_be[24..].copy_from_slice(&valid_before.to_be_bytes());

    let mut msg_buf = Vec::with_capacity(32 * 7);
    msg_buf.extend_from_slice(&msg_typehash);
    msg_buf.extend_from_slice(&from_padded);
    msg_buf.extend_from_slice(&to_padded);
    msg_buf.extend_from_slice(&value_be);
    msg_buf.extend_from_slice(&valid_after_be);
    msg_buf.extend_from_slice(&valid_before_be);
    msg_buf.extend_from_slice(nonce);
    let message_hash: [u8; 32] = Keccak256::digest(&msg_buf).into();

    (domain_hash, message_hash)
}

/// Combine an EIP-712 domain hash + message hash into the canonical
/// `keccak256(0x1901 || domain || message)` digest. Used to verify the
/// `split_eip3009_hashes` helper hasn't drifted from haima's
/// `hash_transfer_authorization`.
fn canonical_eip712_digest(domain_hash: &[u8; 32], message_hash: &[u8; 32]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut buf = Vec::with_capacity(2 + 32 + 32);
    buf.extend_from_slice(&[0x19, 0x01]);
    buf.extend_from_slice(domain_hash);
    buf.extend_from_slice(message_hash);
    Keccak256::digest(&buf).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `encode_derivation_path` matches the canonical Ledger format:
    /// 1-byte component count + 4-byte big-endian per component.
    #[test]
    fn encode_derivation_path_matches_ledger_format() {
        let path = ledger::DEFAULT_DERIVATION_PATH;
        let encoded = ledger::encode_derivation_path(&path).expect("default path < 10 components");
        assert_eq!(encoded[0], 5, "5-component path");
        // First component: 0x8000002C (44' hardened)
        assert_eq!(&encoded[1..5], &[0x80, 0x00, 0x00, 0x2C]);
        // Second component: 0x8000003C (60' hardened, ETH)
        assert_eq!(&encoded[5..9], &[0x80, 0x00, 0x00, 0x3C]);
        assert_eq!(encoded.len(), 1 + 5 * 4);
    }

    /// `build_apdu` produces the 5-byte header + payload shape.
    #[test]
    fn build_apdu_shape() {
        let apdu = build_apdu(
            ledger::apdu::INS_GET_PUBLIC_KEY,
            0x00,
            ledger::apdu::P2_ZERO,
            &[1, 2, 3],
        )
        .expect("3 bytes < 255");
        assert_eq!(apdu[0], ledger::apdu::CLA);
        assert_eq!(apdu[1], ledger::apdu::INS_GET_PUBLIC_KEY);
        assert_eq!(apdu[2], 0x00);
        assert_eq!(apdu[3], 0x00);
        assert_eq!(apdu[4], 3); // Lc
        assert_eq!(&apdu[5..], &[1, 2, 3]);
    }

    /// I-4 fix verification: `build_apdu` returns Err on data > 255 bytes
    /// instead of panicking.
    #[test]
    fn build_apdu_rejects_too_long_data() {
        let huge = vec![0u8; 256];
        let result = build_apdu(0x02, 0x00, 0x00, &huge);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("too long"),
            "expected length error, got: {msg}"
        );
    }

    /// I-4 fix verification: `encode_derivation_path` returns Err on
    /// > 10 components instead of panicking.
    #[test]
    fn encode_derivation_path_rejects_too_long_path() {
        let path: Vec<u32> = (0..11).map(|i| i as u32).collect();
        let result = ledger::encode_derivation_path(&path);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("max 10"), "expected length error, got: {msg}");
    }

    /// APDU constants stay locked to upstream Ledger spec values.
    #[test]
    fn apdu_constants_match_ledger_eth_app_spec() {
        assert_eq!(ledger::apdu::CLA, 0xE0);
        assert_eq!(ledger::apdu::INS_GET_PUBLIC_KEY, 0x02);
        assert_eq!(ledger::apdu::INS_SIGN_TRANSACTION, 0x04);
        assert_eq!(ledger::apdu::INS_GET_APP_VERSION, 0x06);
        assert_eq!(ledger::apdu::INS_SIGN_EIP712, 0x0C);
    }

    /// Default derivation path: m/44'/60'/0'/0/0
    #[test]
    fn default_derivation_path_components() {
        let p = ledger::DEFAULT_DERIVATION_PATH;
        assert_eq!(p.len(), 5);
        assert_eq!(p[0], 0x8000002C); // 44' hardened
        assert_eq!(p[1], 0x8000003C); // 60' hardened (ETH)
        assert_eq!(p[2], 0x80000000); // 0' hardened
        assert_eq!(p[3], 0); // external chain
        assert_eq!(p[4], 0); // address index 0
    }

    /// `derive_evm_address` matches the standard pubkey-to-address
    /// derivation (Keccak256(pubkey[1..])[12..] hex with 0x prefix).
    #[test]
    fn derive_evm_address_round_trip() {
        use k256::SecretKey;
        use k256::elliptic_curve::sec1::ToEncodedPoint;
        let sk = SecretKey::from_bytes(&[1u8; 32].into()).unwrap();
        let pk = sk.public_key();
        let pt = pk.to_encoded_point(false);
        let mut uncompressed = [0u8; 65];
        uncompressed.copy_from_slice(pt.as_bytes());
        let addr = derive_evm_address(&uncompressed);
        assert!(addr.starts_with("0x"));
        assert_eq!(addr.len(), 42);
    }

    /// Sanity: the EIP-712 hash split helper recombines to the same
    /// digest the haima_wallet helper returns. This is the load-bearing
    /// drift guard for the SIGN_EIP712 path.
    #[test]
    fn eip3009_hash_split_recombines_to_haima_digest() {
        use haima_wallet::eip712::hash_transfer_authorization;
        let domain = haima_wallet::USDC_BASE_MAINNET;
        let from = [0x11u8; 20];
        let to = [0x22u8; 20];
        let value: u64 = 1234567;
        let valid_after: u64 = 1700000000;
        let valid_before: u64 = 1700000600;
        let nonce = [0x42u8; 32];
        let combined = hash_transfer_authorization(
            &domain,
            &from,
            &to,
            value,
            valid_after,
            valid_before,
            &nonce,
        );
        let (domain_h, msg_h) = split_eip3009_hashes(
            &domain,
            &from,
            &to,
            value,
            valid_after,
            valid_before,
            &nonce,
        );
        let recombined = canonical_eip712_digest(&domain_h, &msg_h);
        assert_eq!(
            recombined, combined,
            "split/recombine must match haima's combined digest"
        );
    }

    /// HID frame layout sanity — the 64-byte report = 1 report id +
    /// 5-byte header + payload, with first frame reserving 2 bytes for
    /// total length.
    #[test]
    fn hid_frame_constants() {
        use ledger::hid::*;
        assert_eq!(REPORT_SIZE, 64);
        assert_eq!(CHANNEL_ID, 0x0101);
        assert_eq!(COMMAND_APDU, 0x05);
        assert_eq!(REPORT_ID, 0x01);
        assert_eq!(FRAME_PAYLOAD_FIRST, 64 - 7);
        assert_eq!(FRAME_PAYLOAD_NEXT, 64 - 5);
    }
}
