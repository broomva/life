//! Integration tests for `HardwareWalletAnima` (Spec D D-Sub-F).
//!
//! These tests use a `MockHidTransport` that mimics a Ledger Ethereum
//! app over HID. Real Ledger interaction is gated behind the `#[ignore]`
//! attribute on `live_ledger_*` tests; operators run those manually
//! after plugging in a Ledger Nano X / S+ via:
//!
//! ```bash
//! cargo test -p anima-identity --features hw-wallet -- --ignored
//! ```
//!
//! The mock's job is to verify the shape of the APDU traffic + the
//! happy-path semantics:
//! - `GET PUBLIC KEY` (INS = 0x02) → returns a canned 65-byte
//!   uncompressed pubkey.
//! - `SIGN TRANSACTION` (INS = 0x04) → signs the (recomputed) Keccak
//!   digest with a known secp256k1 key + returns `(v, r, s)`.
//! - `SIGN EIP712` (INS = 0x0C) → same flow, signing the
//!   `keccak256(0x1901 || domain || message)` digest.

#![cfg(feature = "hw-wallet")]

use std::sync::{Arc, Mutex};

use anima_identity::custody::{AnimaCustody, BackendKind, TxRequest};
use anima_identity::hardware_wallet::{HardwareWalletAnima, HidTransport, ledger};
use anima_identity::in_process::InProcessAnima;
use anima_identity::seed::MasterSeed;
use k256::SecretKey;
use k256::ecdsa::{SigningKey as K256SigningKey, VerifyingKey as K256VerifyingKey};
use k256::elliptic_curve::sec1::ToEncodedPoint;

/// Mock HID transport that drives a fake Ledger Ethereum app from a
/// known secp256k1 secret key. Tests that exercise the wallet half
/// instantiate this with a known seed and assert that the resulting
/// signature ecrecovers to the expected wallet address.
struct MockLedger {
    /// The "wallet" secret key the fake Ledger pretends to hold.
    /// `wallet_pubkey_uncompressed` is derived from this once at
    /// construction; mocked SIGN_TRANSACTION + SIGN_EIP712 use this
    /// key to produce real-looking signatures.
    signing_key: K256SigningKey,
    /// Cached uncompressed pubkey (`0x04 || x || y`) — what the mock
    /// returns from `GET PUBLIC KEY`.
    pubkey_uncompressed: [u8; 65],
    /// Last APDU the host sent. Tests assert against this to verify
    /// the wire-format shape.
    last_apdu_seen: Mutex<Vec<u8>>,
    /// Total APDU calls seen — a few tests assert call count.
    apdu_count: Mutex<usize>,
}

impl MockLedger {
    fn new(secret_bytes: [u8; 32]) -> Self {
        let secret = SecretKey::from_bytes(&secret_bytes.into()).unwrap();
        let signing_key: K256SigningKey = secret.clone().into();
        let pk = secret.public_key();
        let pt = pk.to_encoded_point(false);
        let mut pubkey_uncompressed = [0u8; 65];
        pubkey_uncompressed.copy_from_slice(pt.as_bytes());
        Self {
            signing_key,
            pubkey_uncompressed,
            last_apdu_seen: Mutex::new(Vec::new()),
            apdu_count: Mutex::new(0),
        }
    }

    fn pubkey_uncompressed(&self) -> [u8; 65] {
        self.pubkey_uncompressed
    }

    /// Build the canonical `GET PUBLIC KEY` response payload (without
    /// status word — the transport contract is to strip the status
    /// word before returning).
    ///
    /// Layout: `[pubkey_len u8] [pubkey 65 bytes] [address_len u8] [address ASCII...]`.
    fn get_pubkey_response(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(self.pubkey_uncompressed.len() as u8);
        out.extend_from_slice(&self.pubkey_uncompressed);
        // The address ASCII is decorative — `HardwareWalletAnima` re-
        // derives the address from the pubkey itself. We provide a
        // valid-looking 40-char hex so the parser doesn't trip.
        let addr_ascii = "abcdef0123456789abcdef0123456789abcdef01"; // 40 chars
        out.push(addr_ascii.len() as u8);
        out.extend_from_slice(addr_ascii.as_bytes());
        out
    }
}

/// `MockHidTransport` — wraps a `MockLedger` and implements
/// `HidTransport` directly without going through HID frames.
///
/// `HardwareWalletAnima::new` calls `transport.exchange(apdu)` where
/// the APDU is a full ISO 7816-4 command (`CLA INS P1 P2 Lc DATA`).
/// The mock parses INS to decide what canned response to return.
struct MockHidTransport {
    ledger: Arc<MockLedger>,
}

impl MockHidTransport {
    fn new(ledger: Arc<MockLedger>) -> Self {
        Self { ledger }
    }
}

impl HidTransport for MockHidTransport {
    fn exchange(&self, apdu: &[u8]) -> anima_core::error::AnimaResult<Vec<u8>> {
        // Record the APDU + bump count.
        {
            let mut last = self.ledger.last_apdu_seen.lock().unwrap();
            *last = apdu.to_vec();
            let mut count = self.ledger.apdu_count.lock().unwrap();
            *count += 1;
        }

        // Parse the APDU header.
        if apdu.len() < 5 {
            return Err(anima_core::error::AnimaError::Crypto(
                "mock: APDU too short for header".into(),
            ));
        }
        let cla = apdu[0];
        let ins = apdu[1];
        let _p1 = apdu[2];
        let _p2 = apdu[3];
        let lc = apdu[4] as usize;
        if cla != ledger::apdu::CLA {
            return Err(anima_core::error::AnimaError::Crypto(format!(
                "mock: unexpected CLA {cla:#04x}"
            )));
        }
        let data = &apdu[5..5 + lc];

        match ins {
            ledger::apdu::INS_GET_PUBLIC_KEY => Ok(self.ledger.get_pubkey_response()),
            ledger::apdu::INS_SIGN_TRANSACTION => {
                // Layout: [count][indices...][rlp...]
                if data.is_empty() {
                    return Err(anima_core::error::AnimaError::Crypto(
                        "mock: empty SIGN_TRANSACTION payload".into(),
                    ));
                }
                let path_count = data[0] as usize;
                let path_bytes = 1 + path_count * 4;
                if data.len() < path_bytes {
                    return Err(anima_core::error::AnimaError::Crypto(
                        "mock: SIGN_TRANSACTION payload too short for path".into(),
                    ));
                }
                let rlp_envelope = &data[path_bytes..];

                // Recompute keccak256 over the full envelope and sign with
                // the mock secret key.
                use sha3::{Digest, Keccak256};
                let digest_bytes = Keccak256::digest(rlp_envelope);
                let mut digest = [0u8; 32];
                digest.copy_from_slice(&digest_bytes);
                let (sig, recid) = self
                    .ledger
                    .signing_key
                    .sign_prehash_recoverable(&digest)
                    .map_err(|e| {
                        anima_core::error::AnimaError::Crypto(format!("mock sign: {e}"))
                    })?;
                // Ledger's response: [v u8][r 32][s 32]. We use the
                // 0/1 y-parity convention since that's what the mock
                // matches against (the `HardwareWalletAnima` ecrecover
                // loop tries both candidates regardless).
                let r_s = sig.to_bytes();
                let mut out = Vec::with_capacity(65);
                out.push(recid.to_byte());
                out.extend_from_slice(&r_s);
                Ok(out)
            }
            ledger::apdu::INS_SIGN_EIP712 => {
                // Layout: [count][indices...][domain_hash 32][message_hash 32]
                if data.is_empty() {
                    return Err(anima_core::error::AnimaError::Crypto(
                        "mock: empty SIGN_EIP712 payload".into(),
                    ));
                }
                let path_count = data[0] as usize;
                let path_bytes_len = 1 + path_count * 4;
                if data.len() < path_bytes_len + 64 {
                    return Err(anima_core::error::AnimaError::Crypto(
                        "mock: SIGN_EIP712 payload too short for hashes".into(),
                    ));
                }
                let mut domain_hash = [0u8; 32];
                domain_hash.copy_from_slice(&data[path_bytes_len..path_bytes_len + 32]);
                let mut message_hash = [0u8; 32];
                message_hash.copy_from_slice(&data[path_bytes_len + 32..path_bytes_len + 64]);

                // Recombine into the canonical EIP-712 digest:
                // keccak256(0x1901 || domain || message).
                use sha3::{Digest, Keccak256};
                let mut buf = Vec::with_capacity(2 + 64);
                buf.extend_from_slice(&[0x19, 0x01]);
                buf.extend_from_slice(&domain_hash);
                buf.extend_from_slice(&message_hash);
                let digest_bytes = Keccak256::digest(&buf);
                let mut digest = [0u8; 32];
                digest.copy_from_slice(&digest_bytes);
                let (sig, recid) = self
                    .ledger
                    .signing_key
                    .sign_prehash_recoverable(&digest)
                    .map_err(|e| {
                        anima_core::error::AnimaError::Crypto(format!("mock sign-712: {e}"))
                    })?;
                let r_s = sig.to_bytes();
                let mut out = Vec::with_capacity(65);
                out.push(recid.to_byte());
                out.extend_from_slice(&r_s);
                Ok(out)
            }
            ledger::apdu::INS_GET_APP_VERSION => Ok(vec![0x01, 0x01, 0x0A, 0x00]), // flags + 1.10.0
            other => Err(anima_core::error::AnimaError::Crypto(format!(
                "mock: unsupported INS {other:#04x}"
            ))),
        }
    }
}

/// Helper — build a `HardwareWalletAnima` with a fresh
/// `InProcessAnima` auth delegate and a mock ledger. Returns the
/// custody handle plus a back-channel `Arc<MockLedger>` so tests can
/// inspect APDU traffic.
fn build_test_custody(secret_bytes: [u8; 32]) -> (HardwareWalletAnima, Arc<MockLedger>) {
    let auth_delegate: Arc<dyn AnimaCustody> =
        InProcessAnima::from_seed_arc(MasterSeed::from_bytes([0xAB; 32])).unwrap();
    let ledger = Arc::new(MockLedger::new(secret_bytes));
    let transport = Box::new(MockHidTransport::new(Arc::clone(&ledger)));
    let custody = HardwareWalletAnima::new(
        auth_delegate,
        transport,
        Some(ledger::DEFAULT_DERIVATION_PATH.to_vec()),
    )
    .expect("construct HardwareWalletAnima with mock ledger");
    (custody, ledger)
}

/// Construction round-trip — `new()` issues a `GET PUBLIC KEY` APDU,
/// caches the resulting wallet address, and surfaces it via
/// `wallet_address()`.
#[test]
fn new_resolves_wallet_address_via_get_pubkey_apdu() {
    let (custody, ledger) = build_test_custody([0x11; 32]);
    let addr = custody.wallet_address().expect("wallet half present");
    assert!(addr.address.starts_with("0x"));
    assert_eq!(addr.address.len(), 42);

    // Verify the APDU we sent had the right shape:
    //   CLA = 0xE0, INS = 0x02 (GET_PUBLIC_KEY), P1 = 0x00, P2 = 0x00
    let last = ledger.last_apdu_seen.lock().unwrap();
    assert_eq!(last[0], ledger::apdu::CLA);
    assert_eq!(last[1], ledger::apdu::INS_GET_PUBLIC_KEY);
    assert_eq!(last[2], 0x00);
    assert_eq!(last[3], 0x00);

    // The wallet pubkey we cached must match the mock's pubkey.
    let cached = custody.wallet_pubkey_uncompressed();
    assert_eq!(cached, &ledger.pubkey_uncompressed());

    // The backend kind is HardwareWallet.
    assert_eq!(custody.backend_kind(), BackendKind::HardwareWallet);
}

/// Auth-half pass-through invariant — `auth_pubkey()`, `user_did()`,
/// `sign_jws()`, `sign_digest()`, and `export_identity_document()` all
/// forward to the wrapped delegate. The wrapper does NOT own its own
/// auth key (Spec D §"Backend matrix").
#[test]
fn auth_half_passes_through_to_inner_delegate() {
    let auth_seed = MasterSeed::from_bytes([0xCD; 32]);
    let auth_delegate: Arc<dyn AnimaCustody> = InProcessAnima::from_seed_arc(auth_seed).unwrap();

    // Capture the delegate's identity BEFORE wrapping.
    let expected_did = auth_delegate.user_did().to_string();
    let expected_pubkey = auth_delegate.auth_pubkey();

    // Wrap.
    let ledger = Arc::new(MockLedger::new([0x22; 32]));
    let transport = Box::new(MockHidTransport::new(Arc::clone(&ledger)));
    let custody = HardwareWalletAnima::new(
        Arc::clone(&auth_delegate),
        transport,
        Some(ledger::DEFAULT_DERIVATION_PATH.to_vec()),
    )
    .unwrap();

    // Auth half must match the wrapped delegate exactly.
    assert_eq!(custody.user_did(), expected_did);
    assert_eq!(custody.auth_pubkey(), expected_pubkey);

    // sign_jws goes through the delegate (the underlying signer is
    // P-256, which the Ledger Ethereum app does not support).
    let claims = serde_json::json!({"sub": "test"});
    let jws_via_wrapper = custody.sign_jws(&claims).unwrap();
    assert_eq!(jws_via_wrapper.split('.').count(), 3);

    // The KYA document also comes from the delegate — auth-half-pass-
    // through means the wallet half does NOT show up in the doc's
    // verification methods (those are for the auth keypair).
    let doc = custody.export_identity_document().unwrap();
    assert!(doc.did.starts_with("did:key:zDn"));
    assert_eq!(doc.did, expected_did);
}

/// `rotate()` is unsupported by design — the seed is hardware-resident.
#[test]
fn rotate_returns_unsupported_error() {
    let (custody, _ledger) = build_test_custody([0x33; 32]);
    // Use match rather than `.expect_err()` because the success arm
    // wraps `Arc<dyn AnimaCustody>` which does not implement `Debug`.
    match custody.rotate() {
        Ok(_) => panic!("rotate must error on HardwareWalletAnima"),
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("HardwareWalletAnima does not support rotation"),
                "rotate error must explain why (got: {msg})"
            );
            assert!(
                msg.contains("hardware-resident"),
                "rotate error should explain the seed is hardware-resident (got: {msg})"
            );
        }
    }
}

/// `sign_evm_tx` produces a 65-byte `r||s||v` signature whose
/// recovered address matches the cached wallet pubkey. End-to-end
/// round-trip of the wallet half through the mock.
#[test]
fn sign_evm_tx_produces_recoverable_signature() {
    let (custody, _ledger) = build_test_custody([0x44; 32]);
    let from_addr = custody.wallet_address().unwrap().address.clone();

    let tx = TxRequest {
        from: from_addr.clone(),
        to: "0x036CbD53842c5426634e7929541eC2318f3dCF7e".into(),
        value_wei: "1000000000000000".into(), // 0.001 ETH
        data_hex: String::new(),
        nonce: 7,
        gas_limit: 21_000,
        max_fee_per_gas_wei: "30000000000".into(), // 30 gwei
        max_priority_fee_per_gas_wei: "1000000000".into(), // 1 gwei
        chain: "eip155:8453".into(),
    };

    let sig = custody.sign_evm_tx(&tx).unwrap();
    assert_eq!(sig.bytes.len(), 65);
    let v = sig.bytes[64];
    assert!(v == 27 || v == 28, "v must be in 27/28 form, got {v}");

    // ecrecover the signature back to the wallet pubkey to confirm
    // round-trip integrity.
    use anima_identity::rlp;
    let chain_id = 8453;
    let to_bytes = rlp::parse_address_20(&tx.to).unwrap();
    let value_bytes = rlp::parse_u256_str(&tx.value_wei).unwrap();
    let max_fee_bytes = rlp::parse_u256_str(&tx.max_fee_per_gas_wei).unwrap();
    let max_prio_bytes = rlp::parse_u256_str(&tx.max_priority_fee_per_gas_wei).unwrap();
    let envelope = rlp::encode_eip1559_unsigned(
        chain_id,
        tx.nonce,
        &max_prio_bytes,
        &max_fee_bytes,
        tx.gas_limit,
        &to_bytes,
        &value_bytes,
        &[],
    );
    let digest = rlp::keccak256(&envelope);

    let mut r_s = [0u8; 64];
    r_s.copy_from_slice(&sig.bytes[..64]);
    let signature = k256::ecdsa::Signature::from_slice(&r_s).unwrap();
    let cand = v - 27;
    let recid = k256::ecdsa::RecoveryId::try_from(cand).unwrap();
    let recovered = K256VerifyingKey::recover_from_prehash(&digest, &signature, recid).unwrap();
    let expected = K256VerifyingKey::from_sec1_bytes(custody.wallet_pubkey_uncompressed()).unwrap();
    assert_eq!(recovered, expected);
}

/// `sign_eip712` for an EIP-3009 `TransferWithAuthorization` produces
/// a 65-byte signature in the haima 27/28 form, ecrecoverable to the
/// wallet pubkey via the canonical EIP-712 digest.
#[test]
fn sign_eip712_eip3009_round_trip() {
    let (custody, _ledger) = build_test_custody([0x55; 32]);

    let domain = haima_wallet::USDC_BASE_MAINNET;
    let from_addr = custody.wallet_address().unwrap().address.clone();
    let nonce_hex = format!("0x{}", hex::encode([0x99u8; 32]));

    let message = serde_json::json!({
        "from": from_addr,
        "to": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
        "value": "1000000",         // 1 USDC (6 decimals)
        "validAfter": "1700000000",
        "validBefore": "1700000600",
        "nonce": nonce_hex,
    });
    let types = serde_json::json!({"primaryType": "TransferWithAuthorization"});

    let sig = custody.sign_eip712(&domain, &types, &message).unwrap();
    assert_eq!(sig.bytes.len(), 65);
    let v = sig.bytes[64];
    assert!(v == 27 || v == 28);

    // Verify the signature recovers correctly.
    use haima_wallet::eip712::{hash_transfer_authorization, parse_eth_address};
    let from_b = parse_eth_address(&from_addr).unwrap();
    let to_b = parse_eth_address("0x036CbD53842c5426634e7929541eC2318f3dCF7e").unwrap();
    let mut nonce = [0u8; 32];
    nonce.copy_from_slice(&hex::decode(nonce_hex.trim_start_matches("0x")).unwrap());
    let digest = hash_transfer_authorization(
        &domain,
        &from_b,
        &to_b,
        1_000_000,
        1_700_000_000,
        1_700_000_600,
        &nonce,
    );

    let mut r_s = [0u8; 64];
    r_s.copy_from_slice(&sig.bytes[..64]);
    let signature = k256::ecdsa::Signature::from_slice(&r_s).unwrap();
    let cand = v - 27;
    let recid = k256::ecdsa::RecoveryId::try_from(cand).unwrap();
    let recovered = K256VerifyingKey::recover_from_prehash(&digest, &signature, recid).unwrap();
    let expected = K256VerifyingKey::from_sec1_bytes(custody.wallet_pubkey_uncompressed()).unwrap();
    assert_eq!(recovered, expected);
}

/// `sign_eip712` rejects non-EIP-3009 typed-data payloads (matches the
/// D-Sub-A/B SPEC-D-DEVIATION limitation).
#[test]
fn sign_eip712_rejects_non_eip3009() {
    let (custody, _ledger) = build_test_custody([0x66; 32]);
    let domain = haima_wallet::USDC_BASE_MAINNET;
    let types = serde_json::json!({"primaryType": "Order"});
    let message = serde_json::json!({"foo": "bar"});
    let err = custody.sign_eip712(&domain, &types, &message).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("EIP-3009"),
        "rejection should mention EIP-3009 limitation (got: {msg})"
    );
}

/// `with_explicit_pubkey` skips the `GET PUBLIC KEY` round-trip — the
/// test asserts no APDU was exchanged when constructing.
#[test]
fn with_explicit_pubkey_skips_get_pubkey_round_trip() {
    let auth_delegate: Arc<dyn AnimaCustody> =
        InProcessAnima::from_seed_arc(MasterSeed::from_bytes([0xEF; 32])).unwrap();
    let ledger = Arc::new(MockLedger::new([0x77; 32]));
    let transport = Box::new(MockHidTransport::new(Arc::clone(&ledger)));

    let pubkey = ledger.pubkey_uncompressed();
    let custody = HardwareWalletAnima::with_explicit_pubkey(
        auth_delegate,
        transport,
        ledger::DEFAULT_DERIVATION_PATH.to_vec(),
        pubkey,
    )
    .unwrap();
    assert_eq!(custody.wallet_pubkey_uncompressed(), &pubkey);

    // No APDU was issued — the explicit constructor doesn't talk to
    // the device.
    let count = *ledger.apdu_count.lock().unwrap();
    assert_eq!(
        count, 0,
        "with_explicit_pubkey must not issue any APDUs (got {count})"
    );
}

/// Live-Ledger end-to-end test (requires a real Ledger plugged in,
/// running the Ethereum app, and willing to confirm).
///
/// **Operator setup**:
///
/// 1. Plug in a Ledger Nano X / S+ via USB.
/// 2. Unlock the device.
/// 3. Open the Ethereum app on the device.
/// 4. Run:
///    ```bash
///    cargo test -p anima-identity --features hw-wallet -- --ignored live_ledger
///    ```
/// 5. Confirm the prompt on the device when it appears.
///
/// The test does NOT run in CI — it requires manual interaction.
#[test]
#[ignore = "requires a real Ledger Nano X/S+ with the Ethereum app open"]
fn live_ledger_get_pubkey() {
    use hidapi::HidApi;
    let api = HidApi::new().expect("HID API init");
    // Ledger vendor id 0x2c97 — every Nano model uses this.
    const LEDGER_VENDOR_ID: u16 = 0x2c97;
    let device = api
        .device_list()
        .find(|d| d.vendor_id() == LEDGER_VENDOR_ID)
        .expect("no Ledger device found — plug in + unlock + open Eth app");
    let dev = api
        .open(device.vendor_id(), device.product_id())
        .expect("open Ledger device");
    let transport = Box::new(anima_identity::hardware_wallet::RealHidTransport::new(dev));
    let auth_delegate: Arc<dyn AnimaCustody> =
        InProcessAnima::from_seed_arc(MasterSeed::generate()).unwrap();
    let custody = HardwareWalletAnima::new(auth_delegate, transport, None)
        .expect("construct HardwareWalletAnima against live Ledger");
    let addr = custody.wallet_address().unwrap();
    println!("Live Ledger wallet address: {}", addr.address);
    assert!(addr.address.starts_with("0x"));
    assert_eq!(addr.address.len(), 42);
}
