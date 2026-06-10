//! M9-E PR-1 (BRO-1215): canonical secp256k1 deterministic-ECDSA
//! test-vector verifier.
//!
//! This is the **agent-runnable proof** behind the softhsm pre-flight
//! `deploy/lifegw/vault-sidecar/test-vectors/secp256k1-sign.sh`. The
//! pre-flight script generates a signature with softhsm via PKCS#11
//! against the *same* private key + message used here, then compares
//! its output to the expected hex emitted by [`emit_canonical_vector`].
//!
//! ## Why this is structured the way it is
//!
//! RFC 6979 mandates deterministic ECDSA (k = HMAC-DRBG(seed)), so any
//! correct implementation that signs a fixed (private key, message)
//! pair must emit byte-identical (r, s). The constants below pin the
//! private key + message; the **expected (r, s)** is derived by
//! `k256` and asserted byte-equal against an embedded copy. If a
//! workspace dep upgrade ever silently changed `k256`'s RFC 6979 path,
//! this test goes red immediately — that is a security-grade event for
//! every wallet signature in the stack.
//!
//! ## Cross-tool reproduction (operator workflow)
//!
//! The operator boots softhsm + initialises a secp256k1 keypair seeded
//! with the same private key (see
//! `deploy/lifegw/vault-sidecar/init-softhsm.sh`), then runs
//! `pkcs11-tool --sign --mechanism ECDSA --input-file ./sample-digest.bin`
//! and compares the result to the hex below. Byte-for-byte match =
//! signing path sound.
//!
//! ## Test vector
//!
//! - **Curve**: secp256k1
//! - **Hash**: SHA-256
//! - **Private key**: `C9AFA9D845BA75166B5C215767B1D6934E50C3DB36E89B127B8A622B120F6721`
//! - **Message**: ASCII `"sample"` (matches RFC 6979 §A.2.5 message)
//!
//! The private-key value matches RFC 6979 §A.2.5's `x` (the RFC's
//! `secp256k1` cousin). The expected (r, s) below is derived by k256
//! inline; the embedded constants pin the bytes the softhsm script
//! must produce.

use k256::ecdsa::{Signature, SigningKey, VerifyingKey, signature::Signer, signature::Verifier};
use k256::elliptic_curve::sec1::FromEncodedPoint;
use k256::{EncodedPoint, PublicKey};
use sha2::{Digest, Sha256};

/// Canonical (private key, message) pair feeding both this test and
/// the operator's softhsm pre-flight script.
mod vector {
    /// Private key `x` (32 bytes, big-endian hex). RFC 6979 §A.2.5
    /// `x` value — chosen because it's published in a frozen standards
    /// document, reproducible across any RFC 6979-conformant signer.
    pub const PRIVATE_KEY_HEX: &str =
        "C9AFA9D845BA75166B5C215767B1D6934E50C3DB36E89B127B8A622B120F6721";

    /// Message bytes — literal ASCII `"sample"`.
    pub const MESSAGE: &[u8] = b"sample";

    /// SHA-256 of `"sample"`. The pre-flight script signs THIS digest
    /// directly (PKCS#11's `CKM_ECDSA` mechanism signs a pre-computed
    /// digest, not the raw message — pre-hashing is the operator's
    /// job).
    pub const SHA256_OF_MESSAGE_HEX: &str =
        "AF2BDBE1AA9B6EC1E2ADE1D694F41FC71A831D0268E9891562113D8A62ADD1BF";

    /// Expected `r || s` in the deterministic ECDSA signature `k256`
    /// produces. Emitted by [`super::emit_canonical_vector`] — embedded
    /// here so the bytes are pinned at review time. If this constant
    /// ever needs updating, the change is observable in the diff (the
    /// only legitimate reason is upgrading k256 to a version that
    /// fixes an RFC 6979 bug — which would be a coordinated cross-repo
    /// event, not silent).
    pub const EXPECTED_RS_HEX: &str =
        // 64 bytes = 128 hex chars. Derived by k256 v0.13 against
        // (PRIVATE_KEY_HEX, MESSAGE). Any RFC 6979-conformant signer
        // (softhsm v2 + pkcs11-tool, libsecp256k1, BouncyCastle, …)
        // produces the same bytes.
        "432310E32CB80EB6503A26CE83CC165C783B870845FB8AAD6D970889FCD7A6C8530128B6B81C548874A6305D93ED071CA6E05074D85863D4056CE89B02BFAB69";
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
        .collect()
}

fn bytes_to_upper_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02X}", b));
    }
    s
}

/// Build the signing key + verifying key + canonical signature in one
/// place. Tests below all use the result of this helper to keep the
/// derivation chain visible (no hidden setup).
fn canonical_keypair_and_signature() -> (SigningKey, VerifyingKey, Signature) {
    let priv_bytes = hex_to_bytes(vector::PRIVATE_KEY_HEX);
    let signing_key =
        SigningKey::from_slice(&priv_bytes).expect("RFC 6979 §A.2.5 private key parses");
    let verifying_key = *signing_key.verifying_key();
    // `k256::ecdsa::SigningKey` signs via RFC 6979 deterministic-k
    // unconditionally — same code path softhsm + any RFC 6979-conformant
    // PKCS#11 module will exercise.
    let signature: Signature = signing_key.sign(vector::MESSAGE);
    (signing_key, verifying_key, signature)
}

/// SHA-256 of `"sample"` matches the digest the operator's softhsm
/// pre-flight feeds to `pkcs11-tool --sign`. This pins the hashing dep
/// — if a workspace bump silently changed SHA-256 output (impossible
/// in practice, but cheap to verify), every EIP-712 / Tier-2 signature
/// in the stack breaks.
#[test]
fn sha256_of_sample_matches_published_digest() {
    let expected = hex_to_bytes(vector::SHA256_OF_MESSAGE_HEX);
    let mut hasher = Sha256::new();
    hasher.update(vector::MESSAGE);
    let actual = hasher.finalize();
    assert_eq!(
        actual.as_slice(),
        expected.as_slice(),
        "SHA-256(\"sample\") must match the canonical digest"
    );
}

/// The k256 deterministic signature must round-trip against its own
/// verifying key. This is the *floor* — if this fails, k256 itself is
/// broken and nothing else in the stack can be trusted.
#[test]
fn k256_deterministic_signature_round_trips() {
    let (_signing_key, verifying_key, signature) = canonical_keypair_and_signature();
    verifying_key
        .verify(vector::MESSAGE, &signature)
        .expect("k256 deterministic signature must verify against its own key");
}

/// The private-key→pubkey derivation pins the secp256k1 scalar
/// multiplication. Independent of the signature path itself.
#[test]
fn private_key_derives_consistent_pubkey() {
    let (_signing_key, verifying_key, _signature) = canonical_keypair_and_signature();
    let point = verifying_key.to_encoded_point(false);
    let x = point.x().expect("uncompressed pubkey has X");
    let y = point.y().expect("uncompressed pubkey has Y");

    // X and Y are 32 bytes each (secp256k1 coordinate width). Pin the
    // sizes — a future curve-impl change that returned a different
    // length would be a security-grade event.
    assert_eq!(x.len(), 32, "secp256k1 X coordinate is 32 bytes");
    assert_eq!(y.len(), 32, "secp256k1 Y coordinate is 32 bytes");

    // Round-trip the SEC1 uncompressed encoding through PublicKey to
    // confirm the point is on-curve (k256 rejects off-curve points).
    let mut sec1 = Vec::with_capacity(65);
    sec1.push(0x04);
    sec1.extend_from_slice(x.as_slice());
    sec1.extend_from_slice(y.as_slice());
    let encoded = EncodedPoint::from_bytes(&sec1).expect("SEC1 round-trip");
    let _pubkey =
        PublicKey::from_encoded_point(&encoded).expect("derived point lives on secp256k1");
}

/// Emit the canonical signature bytes the operator's softhsm script
/// must reproduce. This test is **always on**: it asserts that the
/// signature `k256` emits for the canonical (private key, message)
/// pair matches [`vector::EXPECTED_RS_HEX`] byte-for-byte.
///
/// On first commit of this file, `EXPECTED_RS_HEX` is a placeholder
/// (all zeros) — running this test prints the actual hex into the
/// failure message so the engineer can copy it into the constant.
/// After that, the constant is pinned and the test guards against
/// any future dep change that would silently alter the signature.
#[test]
fn signature_matches_pinned_canonical_bytes() {
    let (_signing_key, _verifying_key, signature) = canonical_keypair_and_signature();
    let actual_hex = bytes_to_upper_hex(signature.to_bytes().as_slice());

    if vector::EXPECTED_RS_HEX.chars().all(|c| c == '0') {
        // First-commit bootstrap: the placeholder is still in place.
        // Print the canonical bytes so they can be pinned. Fail the
        // test so a subsequent commit pins the value before this PR
        // merges.
        panic!(
            "EXPECTED_RS_HEX is the placeholder. \
             Pin it to this value (uppercase hex, 128 chars):\n\n\
             pub const EXPECTED_RS_HEX: &str =\n    \"{}\";\n",
            actual_hex
        );
    }

    assert_eq!(
        actual_hex,
        vector::EXPECTED_RS_HEX,
        "k256 deterministic signature must match the pinned canonical bytes"
    );
}
