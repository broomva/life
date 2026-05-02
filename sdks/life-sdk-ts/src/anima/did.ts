/**
 * `did:key` DID generation + resolution for P-256 (Spec D L4-D6).
 *
 * Cross-language compatibility (REQUIRED): produces byte-identical
 * output to Rust's `crates/anima/anima-identity/src/did.rs` for the
 * same SEC1-compressed pubkey input. Tests pin this via fixture
 * vectors generated from the Rust side.
 *
 * Format: `did:key:zDn<base58btc-encoded-multicodec-key>`
 *
 * Multicodec prefix (varint-encoded):
 *   - `0x80 0x24` → P-256 public key (Spec D L4-D6, multicodec 0x1200)
 *   - `0xed 0x01` → Ed25519 (legacy, Pre-D-Sub-A — not exposed here;
 *                  browser custody only mints P-256)
 *
 * @see https://w3c-ccg.github.io/did-method-key/
 * @see https://github.com/multiformats/multicodec
 */

/**
 * Multicodec prefix for P-256 public key (Spec D L4-D6 — current auth curve).
 *
 * Two-byte unsigned varint encoding of 0x1200:
 *   0x1200 binary:  1 0010 0000 0000
 *   varint encoding (LSB-first 7-bit groups with continuation bit):
 *     low 7 bits  = 0000000  → with continuation = 0x80
 *     next 7 bits = 0100100  → terminator        = 0x24
 *   → bytes: 0x80, 0x24
 */
const P256_MULTICODEC_PREFIX = new Uint8Array([0x80, 0x24]);

/** Standard `did:key` scheme + multibase-z prefix. */
const DID_KEY_PREFIX = "did:key:z";

/**
 * Generate a `did:key` DID from a P-256 SEC1-compressed public key.
 *
 * Steps (mirroring Rust's `generate_did_key_p256`):
 *   1. Prepend the P-256 multicodec varint prefix `[0x80, 0x24]` to
 *      the 33-byte compressed public key.
 *   2. Encode the resulting 35 bytes as base58btc.
 *   3. Prepend the multibase 'z' prefix and the `did:key:` scheme.
 *
 * @param pubkey SEC1-compressed P-256 public key (33 bytes; first byte
 *               must be `0x02` or `0x03`).
 * @returns `did:key:zDn…` formatted DID.
 * @throws if the input is not 33 bytes or doesn't start with 0x02/0x03.
 */
export function generateDidKeyP256(pubkey: Uint8Array): string {
  if (pubkey.byteLength !== 33) {
    throw new Error(
      `P-256 SEC1-compressed pubkey must be 33 bytes, got ${pubkey.byteLength}`,
    );
  }
  const first = pubkey[0];
  if (first !== 0x02 && first !== 0x03) {
    throw new Error(
      `P-256 SEC1-compressed point must start with 0x02 or 0x03, got 0x${first?.toString(16).padStart(2, "0")}`,
    );
  }

  const buf = new Uint8Array(P256_MULTICODEC_PREFIX.byteLength + pubkey.byteLength);
  buf.set(P256_MULTICODEC_PREFIX, 0);
  buf.set(pubkey, P256_MULTICODEC_PREFIX.byteLength);

  return `${DID_KEY_PREFIX}${base58btcEncode(buf)}`;
}

/**
 * Resolve a P-256 `did:key` DID and return the SEC1-compressed pubkey.
 *
 * Strict — rejects any DID that's not P-256 (multicodec prefix
 * mismatch). Use {@link verifyDidKeyP256} for a non-throwing check.
 *
 * @throws if the DID format is invalid, base58 decoding fails, the
 *         multicodec prefix isn't P-256, or the inner key bytes aren't
 *         a valid SEC1-compressed point.
 */
export function resolveDidKeyP256(did: string): Uint8Array {
  if (!did.startsWith(DID_KEY_PREFIX)) {
    throw new Error(`invalid did:key format (missing prefix): ${did}`);
  }
  const encoded = did.slice(DID_KEY_PREFIX.length);
  const bytes = base58btcDecode(encoded);

  if (bytes.byteLength < 2) {
    throw new Error("decoded DID too short for multicodec prefix");
  }
  if (bytes[0] !== P256_MULTICODEC_PREFIX[0] || bytes[1] !== P256_MULTICODEC_PREFIX[1]) {
    throw new Error(
      `unknown multicodec prefix: [0x${bytes[0]?.toString(16).padStart(2, "0")}, 0x${bytes[1]?.toString(16).padStart(2, "0")}] (expected P-256 [0x80, 0x24])`,
    );
  }

  const keyBytes = bytes.slice(2);
  if (keyBytes.byteLength !== 33) {
    throw new Error(
      `P-256 SEC1-compressed public key must be 33 bytes, got ${keyBytes.byteLength}`,
    );
  }
  if (keyBytes[0] !== 0x02 && keyBytes[0] !== 0x03) {
    throw new Error(
      `P-256 SEC1-compressed point must start with 0x02 or 0x03, got 0x${keyBytes[0]?.toString(16).padStart(2, "0")}`,
    );
  }
  return keyBytes;
}

/**
 * Verify that a `did:key` DID was derived from the given P-256 pubkey.
 *
 * Non-throwing — returns `false` for any mismatch (wrong format,
 * wrong curve, wrong key bytes).
 */
export function verifyDidKeyP256(did: string, pubkey: Uint8Array): boolean {
  try {
    return generateDidKeyP256(pubkey) === did;
  } catch {
    return false;
  }
}

// ── base58btc encoding ────────────────────────────────────────────────
//
// Hand-rolled to avoid pulling in a dedicated dep and to guarantee
// byte-identical output to Rust's `bs58::encode`. Algorithm: standard
// big-int base-256 → base-58 conversion with leading-zero preservation.

const BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

const BASE58_INDEX: Record<string, number> = (() => {
  const out: Record<string, number> = {};
  for (let i = 0; i < BASE58_ALPHABET.length; i++) {
    const ch = BASE58_ALPHABET[i];
    if (ch !== undefined) out[ch] = i;
  }
  return out;
})();

/**
 * Encode bytes as base58btc — Bitcoin/Multibase variant.
 *
 * Equivalent to Rust's `bs58::encode(bytes).into_string()`. Pure JS
 * implementation so it works in browsers without subtle deps.
 */
function base58btcEncode(bytes: Uint8Array): string {
  if (bytes.byteLength === 0) return "";

  // Count leading zero bytes — they encode to leading '1's.
  let zeros = 0;
  while (zeros < bytes.byteLength && bytes[zeros] === 0) zeros++;

  // Convert to base-58 by repeated division.
  // We work with a copy of the input as a base-256 big-integer.
  const buf = Array.from(bytes);
  const out: number[] = [];
  let start = zeros;
  while (start < buf.length) {
    let remainder = 0;
    for (let i = start; i < buf.length; i++) {
      const value = (buf[i] ?? 0) + remainder * 256;
      buf[i] = Math.floor(value / 58);
      remainder = value % 58;
    }
    out.push(remainder);
    while (start < buf.length && buf[start] === 0) start++;
  }

  let str = "";
  // Leading zeros → leading '1's.
  for (let i = 0; i < zeros; i++) str += BASE58_ALPHABET[0];
  // Remaining digits in MSB-first order.
  for (let i = out.length - 1; i >= 0; i--) {
    const digit = out[i] ?? 0;
    str += BASE58_ALPHABET[digit];
  }
  return str;
}

/**
 * Decode base58btc-encoded text to bytes.
 *
 * @throws if any character is outside the base58 alphabet.
 */
function base58btcDecode(text: string): Uint8Array {
  if (text.length === 0) return new Uint8Array(0);

  // Count leading '1's — they decode to leading zero bytes.
  let zeros = 0;
  while (zeros < text.length && text[zeros] === BASE58_ALPHABET[0]) zeros++;

  // Convert from base-58 to base-256 (big-endian byte order in `out`).
  const out: number[] = [];
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (ch === undefined) continue;
    const value = BASE58_INDEX[ch];
    if (value === undefined) {
      throw new Error(`invalid base58 character: '${ch}'`);
    }
    let carry = value;
    for (let j = 0; j < out.length; j++) {
      const x = (out[j] ?? 0) * 58 + carry;
      out[j] = x & 0xff;
      carry = x >> 8;
    }
    while (carry > 0) {
      out.push(carry & 0xff);
      carry >>= 8;
    }
  }

  // out is little-endian; reverse to big-endian + add leading zeros.
  const bytes = new Uint8Array(zeros + out.length);
  for (let i = 0; i < out.length; i++) {
    bytes[zeros + i] = out[out.length - 1 - i] ?? 0;
  }
  return bytes;
}
