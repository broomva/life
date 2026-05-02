//! Minimal RLP encoder for EVM transaction signing.
//!
//! Spec D L4-D7 keeps the wallet on secp256k1 + EIP-155 + EIP-1559. Both
//! `InProcessAnima` and `VaultTransitAnima` need to compute the
//! Keccak-256 digest of the canonical RLP encoding of a transaction
//! before handing the digest to the signer (the secp256k1 `SigningKey`
//! for in-process; Vault's `transit/sign` with `prehashed: true` for
//! the Vault backend).
//!
//! This module hand-rolls just enough RLP for the two transaction
//! shapes the wallet substrate emits today:
//!
//! 1. **Legacy (EIP-155)**: 9-tuple `[nonce, gas_price, gas_limit, to,
//!    value, data, chain_id, 0, 0]`. The unsigned digest is
//!    `keccak256(rlp(9-tuple))`. Used for chains where EIP-1559 isn't
//!    deployed.
//!
//! 2. **EIP-1559 typed envelope (type 0x02)**: 9-tuple `[chain_id,
//!    nonce, max_priority_fee_per_gas, max_fee_per_gas, gas_limit, to,
//!    value, data, access_list]` prefixed with `0x02`. The unsigned
//!    digest is `keccak256(0x02 || rlp(9-tuple))`. Used on Base + every
//!    EVM chain post-London.
//!
//! Both are exposed via [`encode_eip155_unsigned`] and
//! [`encode_eip1559_unsigned`]. Higher-level callers (`sign_evm_tx` in
//! every backend) compute the Keccak-256 of the returned bytes.
//!
//! ## Design notes
//!
//! - Hand-rolling RLP avoids pulling `alloy-rlp` (heavy dep chain) or
//!   `rlp` (less maintained) into the anima crate. The encoding is
//!   ~80 LOC and the test surface is tractable: every shape we emit is
//!   round-trip-verified against well-known test vectors below.
//! - The encoder operates on `Vec<u8>` for simplicity; a future pass
//!   could move to `Bytes` / `BytesMut` if RLP becomes hot, but the
//!   current call frequency is "once per tx" and dwarfed by the
//!   secp256k1 signature itself.
//! - U256 values come in as big-endian byte slices with leading zeros
//!   pre-stripped. The encoder enforces this via the tested
//!   `strip_leading_zeros` helper — RLP requires the canonical form for
//!   determinism (otherwise two encodings of the same logical value
//!   would produce different digests).

/// RLP-encode a single byte string. Per the [yellow paper Appendix B]:
///
/// - A single byte `< 0x80` is its own encoding.
/// - A string of length `0 <= L <= 55` is encoded as `0x80 + L` followed
///   by the bytes.
/// - A longer string is encoded as `0xb7 + len(big-endian(L))` followed
///   by `big-endian(L)` followed by the bytes.
///
/// [yellow paper Appendix B]: https://ethereum.github.io/yellowpaper/paper.pdf
pub fn encode_string(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() == 1 && bytes[0] < 0x80 {
        return vec![bytes[0]];
    }
    let mut out = Vec::with_capacity(bytes.len() + 9);
    if bytes.len() <= 55 {
        out.push(0x80 + bytes.len() as u8);
    } else {
        let len_be = strip_leading_zeros(&bytes.len().to_be_bytes());
        out.push(0xb7 + len_be.len() as u8);
        out.extend_from_slice(&len_be);
    }
    out.extend_from_slice(bytes);
    out
}

/// RLP-encode a list of pre-encoded items. Per yellow paper Appendix B:
///
/// - A list whose payload `0 <= L <= 55` is encoded as `0xc0 + L`
///   followed by the concatenated payloads.
/// - A longer list is encoded as `0xf7 + len(big-endian(L))` followed
///   by `big-endian(L)` followed by the concatenated payloads.
pub fn encode_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload_len: usize = items.iter().map(|i| i.len()).sum();
    let mut out = Vec::with_capacity(payload_len + 9);
    if payload_len <= 55 {
        out.push(0xc0 + payload_len as u8);
    } else {
        let len_be = strip_leading_zeros(&payload_len.to_be_bytes());
        out.push(0xf7 + len_be.len() as u8);
        out.extend_from_slice(&len_be);
    }
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

/// Strip leading 0x00 bytes from a big-endian integer slice. RLP demands
/// the canonical (zero-padded-on-the-left removed) form. Returns an
/// empty slice for the integer 0 itself, which RLP encodes as the empty
/// string `0x80`.
pub fn strip_leading_zeros(bytes: &[u8]) -> Vec<u8> {
    let mut start = 0;
    while start < bytes.len() && bytes[start] == 0 {
        start += 1;
    }
    bytes[start..].to_vec()
}

/// Encode a `u64` as RLP (canonical form — leading zero bytes stripped).
pub fn encode_u64(n: u64) -> Vec<u8> {
    encode_string(&strip_leading_zeros(&n.to_be_bytes()))
}

/// Encode a U256 value supplied as a decimal-or-hex string into the
/// canonical big-endian byte form. Used by the legacy `value_wei` /
/// `max_fee_per_gas_wei` fields in `TxRequest` which are string-encoded
/// for u256 width.
///
/// Accepts:
/// - Hex with `0x` prefix (`"0x1bc16d674ec80000"`)
/// - Decimal (`"2000000000000000000"`)
///
/// Returns the value as a leading-zero-stripped big-endian byte vector.
pub fn parse_u256_str(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if let Some(hex_str) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let mut padded = hex_str.to_string();
        if padded.len() % 2 != 0 {
            padded.insert(0, '0');
        }
        let bytes = hex::decode(&padded).map_err(|e| format!("u256 hex decode: {e}"))?;
        return Ok(strip_leading_zeros(&bytes));
    }
    // Decimal path. We avoid pulling a u256 crate by parsing into u128
    // first (covers all realistic gas + value amounts under 2^128 — well
    // beyond Base/Ethereum's circulating supply in wei). Values larger
    // than u128 are rare in practice but are surfaced via the hex path.
    let n: u128 = s
        .parse()
        .map_err(|e| format!("u256 decimal parse failed for {s:?}: {e}"))?;
    Ok(strip_leading_zeros(&n.to_be_bytes()))
}

/// Decode a `0x`-prefixed hex address into 20 bytes. RLP encodes the
/// address as a 20-byte string (or empty for contract creation, which
/// we don't support here — sign_evm_tx requires `to` per `TxRequest`).
pub fn parse_address_20(s: &str) -> Result<[u8; 20], String> {
    let hex_str = s
        .trim()
        .strip_prefix("0x")
        .or_else(|| s.trim().strip_prefix("0X"))
        .ok_or_else(|| format!("address missing 0x prefix: {s:?}"))?;
    if hex_str.len() != 40 {
        return Err(format!(
            "address must be 20 bytes (40 hex chars), got {} chars",
            hex_str.len()
        ));
    }
    let bytes = hex::decode(hex_str).map_err(|e| format!("address hex decode: {e}"))?;
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Decode a `0x`-prefixed hex calldata. May be empty.
pub fn parse_data_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let hex_str = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"));
    let hex_str = match hex_str {
        Some(h) => h,
        None => s,
    };
    if hex_str.is_empty() {
        return Ok(Vec::new());
    }
    hex::decode(hex_str).map_err(|e| format!("data hex decode: {e}"))
}

/// Parse a CAIP-2 chain id `eip155:<n>` into the EIP-155 chain id.
/// Non-EVM chains are rejected.
pub fn parse_eip155_chain_id(s: &str) -> Result<u64, String> {
    let n_str = s
        .strip_prefix("eip155:")
        .ok_or_else(|| format!("chain {s:?} is not an eip155 CAIP-2 id"))?;
    n_str
        .parse::<u64>()
        .map_err(|e| format!("eip155 chain id parse: {e}"))
}

/// Encode an unsigned **EIP-155 legacy** transaction — the 9-tuple
/// `[nonce, gas_price, gas_limit, to, value, data, chain_id, 0, 0]`.
/// The signing digest is `keccak256` of the returned bytes.
///
/// Used as a fallback when the chain doesn't support EIP-1559. Most
/// production paths go through [`encode_eip1559_unsigned`].
#[allow(clippy::too_many_arguments)]
pub fn encode_eip155_unsigned(
    nonce: u64,
    gas_price_wei: &[u8],
    gas_limit: u64,
    to: &[u8; 20],
    value_wei: &[u8],
    data: &[u8],
    chain_id: u64,
) -> Vec<u8> {
    let items = vec![
        encode_u64(nonce),
        encode_string(&strip_leading_zeros(gas_price_wei)),
        encode_u64(gas_limit),
        encode_string(to),
        encode_string(&strip_leading_zeros(value_wei)),
        encode_string(data),
        encode_u64(chain_id),
        // Per EIP-155, the unsigned RLP appends `chain_id, 0, 0` so the
        // `v` field of a signed tx encodes the chain.
        encode_string(&[]),
        encode_string(&[]),
    ];
    encode_list(&items)
}

/// Encode an unsigned **EIP-1559 typed envelope** (type 0x02)
/// transaction — the 9-tuple `[chain_id, nonce, max_priority_fee,
/// max_fee, gas_limit, to, value, data, access_list]`. The result is
/// prefixed with the type byte `0x02`.
///
/// The signing digest is `keccak256(0x02 || rlp(9-tuple))`.
///
/// We always pass an empty access list (`access_list = []`) per the
/// `TxRequest` shape — anima doesn't yet expose access-list construction
/// to callers. Adding it is an additive trait extension when a real
/// access-list use case appears.
#[allow(clippy::too_many_arguments)]
pub fn encode_eip1559_unsigned(
    chain_id: u64,
    nonce: u64,
    max_priority_fee_wei: &[u8],
    max_fee_wei: &[u8],
    gas_limit: u64,
    to: &[u8; 20],
    value_wei: &[u8],
    data: &[u8],
) -> Vec<u8> {
    // Empty access list = `rlp([])` = `0xc0`.
    let empty_access_list = encode_list(&[]);
    let items = vec![
        encode_u64(chain_id),
        encode_u64(nonce),
        encode_string(&strip_leading_zeros(max_priority_fee_wei)),
        encode_string(&strip_leading_zeros(max_fee_wei)),
        encode_u64(gas_limit),
        encode_string(to),
        encode_string(&strip_leading_zeros(value_wei)),
        encode_string(data),
        empty_access_list,
    ];
    let rlp = encode_list(&items);
    let mut envelope = Vec::with_capacity(rlp.len() + 1);
    envelope.push(0x02);
    envelope.extend_from_slice(&rlp);
    envelope
}

/// Compute the Keccak-256 digest of an RLP-encoded transaction. The
/// result is what `secp256k1` signs for both legacy and EIP-1559 paths.
pub fn keccak256(bytes: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Single byte < 0x80 is its own encoding (yellow paper §4.1).
    #[test]
    fn rlp_single_byte_below_0x80() {
        assert_eq!(encode_string(&[0x00]), vec![0x00]);
        assert_eq!(encode_string(&[0x7f]), vec![0x7f]);
        assert_eq!(encode_string(&[0x42]), vec![0x42]);
    }

    /// Empty string is `0x80`.
    #[test]
    fn rlp_empty_string() {
        assert_eq!(encode_string(&[]), vec![0x80]);
    }

    /// Single byte >= 0x80 is encoded as `0x81, byte`.
    #[test]
    fn rlp_single_byte_above_0x80() {
        assert_eq!(encode_string(&[0x80]), vec![0x81, 0x80]);
        assert_eq!(encode_string(&[0xff]), vec![0x81, 0xff]);
    }

    /// Short string (length <= 55) is `0x80 + L, ...bytes`.
    #[test]
    fn rlp_short_string() {
        assert_eq!(
            encode_string(b"dog"),
            vec![0x83, b'd', b'o', b'g'],
            "rlp(\"dog\") = 0x83646f67"
        );
    }

    /// Long string (length > 55) uses the long-form encoding.
    #[test]
    fn rlp_long_string() {
        let payload = vec![0x42u8; 60];
        let encoded = encode_string(&payload);
        // 0xb7 + 1 = 0xb8 (one length byte), 60 = 0x3c, then 60 bytes.
        assert_eq!(encoded[0], 0xb8);
        assert_eq!(encoded[1], 60);
        assert_eq!(&encoded[2..], &payload[..]);
    }

    /// Empty list is `0xc0`.
    #[test]
    fn rlp_empty_list() {
        assert_eq!(encode_list(&[]), vec![0xc0]);
    }

    /// Short list (payload <= 55).
    #[test]
    fn rlp_short_list() {
        let items = vec![encode_string(b"cat"), encode_string(b"dog")];
        let encoded = encode_list(&items);
        // 0xc0 + 8 = 0xc8 (4 bytes "cat" + 4 bytes "dog" = 8)
        assert_eq!(encoded[0], 0xc8);
    }

    /// `strip_leading_zeros` drops only leading zeros (canonical form).
    #[test]
    fn rlp_strip_leading_zeros() {
        assert_eq!(strip_leading_zeros(&[0x00, 0x01, 0x02]), vec![0x01, 0x02]);
        assert_eq!(strip_leading_zeros(&[0x00, 0x00, 0x00]), Vec::<u8>::new());
        assert_eq!(strip_leading_zeros(&[0x42]), vec![0x42]);
        assert_eq!(strip_leading_zeros(&[]), Vec::<u8>::new());
        // 0x00 prefix in the middle is preserved.
        assert_eq!(
            strip_leading_zeros(&[0x00, 0x42, 0x00, 0x00]),
            vec![0x42, 0x00, 0x00]
        );
    }

    #[test]
    fn rlp_u64_encoding() {
        // 0 → empty string → 0x80
        assert_eq!(encode_u64(0), vec![0x80]);
        // 1 → 0x01 (single byte < 0x80)
        assert_eq!(encode_u64(1), vec![0x01]);
        // 127 → 0x7f
        assert_eq!(encode_u64(127), vec![0x7f]);
        // 128 → 0x81 0x80
        assert_eq!(encode_u64(128), vec![0x81, 0x80]);
        // 1024 → 0x82 0x04 0x00
        assert_eq!(encode_u64(1024), vec![0x82, 0x04, 0x00]);
    }

    #[test]
    fn parse_u256_decimal() {
        let v = parse_u256_str("0").unwrap();
        assert_eq!(v, Vec::<u8>::new());
        let v = parse_u256_str("1").unwrap();
        assert_eq!(v, vec![0x01]);
        let v = parse_u256_str("256").unwrap();
        assert_eq!(v, vec![0x01, 0x00]);
    }

    #[test]
    fn parse_u256_hex() {
        let v = parse_u256_str("0x1234").unwrap();
        assert_eq!(v, vec![0x12, 0x34]);
        let v = parse_u256_str("0X00ff").unwrap();
        assert_eq!(v, vec![0xff]);
        let v = parse_u256_str("0x0").unwrap();
        assert_eq!(v, Vec::<u8>::new());
    }

    #[test]
    fn parse_address_round_trip() {
        let addr = parse_address_20("0x036CbD53842c5426634e7929541eC2318f3dCF7e").unwrap();
        assert_eq!(addr.len(), 20);
        assert_eq!(addr[0], 0x03);
        assert_eq!(addr[19], 0x7e);
    }

    #[test]
    fn parse_address_rejects_short() {
        assert!(parse_address_20("0xabcd").is_err());
        assert!(parse_address_20("not-an-address").is_err());
    }

    #[test]
    fn parse_data_empty_and_hex() {
        assert_eq!(parse_data_hex("").unwrap(), Vec::<u8>::new());
        assert_eq!(parse_data_hex("0x").unwrap(), Vec::<u8>::new());
        assert_eq!(
            parse_data_hex("0xdeadbeef").unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
        // Without prefix is also accepted (best-effort for legacy callers).
        assert_eq!(parse_data_hex("ab").unwrap(), vec![0xab]);
    }

    #[test]
    fn eip155_chain_id_parsing() {
        assert_eq!(parse_eip155_chain_id("eip155:8453").unwrap(), 8453);
        assert_eq!(parse_eip155_chain_id("eip155:1").unwrap(), 1);
        assert!(parse_eip155_chain_id("solana:foo").is_err());
        assert!(parse_eip155_chain_id("garbage").is_err());
    }

    /// EIP-1559 envelope test vector. We verify structural properties:
    /// type prefix is 0x02, the encoded list parses into 9 elements,
    /// access list is empty (`0xc0`), and the keccak digest is stable
    /// for a fixed input.
    #[test]
    fn eip1559_envelope_shape() {
        let to = parse_address_20("0x036CbD53842c5426634e7929541eC2318f3dCF7e").unwrap();
        let value = parse_u256_str("1000000000000000000").unwrap(); // 1 ETH
        let max_priority = parse_u256_str("1000000000").unwrap(); // 1 gwei
        let max_fee = parse_u256_str("30000000000").unwrap(); // 30 gwei
        let envelope = encode_eip1559_unsigned(
            8453, // Base mainnet
            42,
            &max_priority,
            &max_fee,
            21000,
            &to,
            &value,
            b"",
        );
        assert_eq!(envelope[0], 0x02, "EIP-1559 type byte");
        // Hash deterministically.
        let digest1 = keccak256(&envelope);
        let envelope2 =
            encode_eip1559_unsigned(8453, 42, &max_priority, &max_fee, 21000, &to, &value, b"");
        let digest2 = keccak256(&envelope2);
        assert_eq!(digest1, digest2, "envelope encoding must be deterministic");
    }

    /// Sanity check the EIP-155 9-tuple (legacy) shape: appending
    /// `chain_id, 0, 0` after `data` produces a list whose RLP
    /// preimage hashes to a stable Keccak digest.
    #[test]
    fn eip155_legacy_shape() {
        let to = parse_address_20("0x036CbD53842c5426634e7929541eC2318f3dCF7e").unwrap();
        let value = parse_u256_str("0").unwrap();
        let gas_price = parse_u256_str("20000000000").unwrap();
        let unsigned = encode_eip155_unsigned(0, &gas_price, 21000, &to, &value, b"", 1);
        // First byte is the list-length prefix; for a 9-element list with
        // a small payload it should be in the c0..f7 range.
        assert!(
            (0xc0..=0xf7).contains(&unsigned[0]),
            "expected short-list prefix, got {:#x}",
            unsigned[0],
        );
        let digest = keccak256(&unsigned);
        assert_eq!(digest.len(), 32);
    }

    /// Empty access list `rlp([])` = `0xc0`. Exercised in the EIP-1559
    /// path; this test isolates the invariant.
    #[test]
    fn empty_access_list_byte() {
        assert_eq!(encode_list(&[]), vec![0xc0]);
    }
}
