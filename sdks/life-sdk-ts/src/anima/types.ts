/**
 * Anima types — Spec D D-Sub-C browser custody surface.
 *
 * These types mirror the Rust `AnimaCustody` trait shape from
 * `crates/anima/anima-identity/src/custody.rs`, translated to camelCase.
 *
 * Spec D L4-D5..D10 ground truth:
 *   - L4-D5: split custody (browser auth + delegated wallet)
 *   - L4-D6: P-256 (ES256) auth keypair
 *   - L4-D7: secp256k1 wallet keypair (delegated to remote)
 *   - L4-D10: rotation documented in journal, not implicit
 *
 * @see docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md
 */

/**
 * Custody backend identifier — Spec D §"Event additions".
 *
 * Mirrors Rust's `BackendKind` enum (snake_case serialization). Browser
 * custody surfaces only `WebCrypto` (auth half via passkey) + `Remote`
 * (wallet delegate). Other backends live server-side.
 */
export type BackendKind =
  | "in_process"
  | "vault"
  | "web_crypto"
  | "tpm"
  | "soma"
  | "hardware_wallet"
  | "remote";

/**
 * EVM transaction request — narrow shape used by `signEvmTx`.
 *
 * Mirrors `TxRequest` in Rust. Spec D L4-D7 keeps wallet on
 * secp256k1 + EIP-1559 + EIP-155, so this matches the EVM EOA model.
 */
export interface TxRequest {
  /** `from` address (must equal `walletAddress`). */
  from: string;
  /** `to` address (recipient). */
  to: string;
  /**
   * Value in wei (string-encoded for u256 width — chains may exceed
   * JS number precision).
   */
  valueWei: string;
  /** Calldata; hex-encoded with 0x prefix, may be empty for plain transfers. */
  dataHex: string;
  /** Nonce (user's wallet account nonce). */
  nonce: number;
  /** Gas limit. */
  gasLimit: number;
  /** Maximum fee per gas (EIP-1559) in wei (string-encoded). */
  maxFeePerGasWei: string;
  /** Maximum priority fee per gas (EIP-1559) in wei (string-encoded). */
  maxPriorityFeePerGasWei: string;
  /** CAIP-2 chain id (e.g. `eip155:8453`). */
  chain: string;
}

/**
 * EVM signature output — `(r, s, v)` in 65-byte recoverable form.
 *
 * Mirrors Rust's `EvmSignature`. Browser custody constructs this
 * by base64-decoding what the Rust wallet backend returned over the
 * `/anima/custody/sign_wallet` HTTP route.
 */
export interface EvmSignature {
  /** Raw 65-byte recoverable signature (r || s || v). */
  bytes: Uint8Array;
}

/**
 * EIP-712 typed-data domain — re-exported shape from haima-wallet.
 *
 * Spec D L4-D7 keeps wallet on secp256k1 + EIP-712 + EIP-155; this
 * struct is the canonical shape consumers pass to `signEip712`.
 */
export interface Eip712Domain {
  name: string;
  version: string;
  /** Chain id as decimal string (EIP-155). For Base mainnet: "8453". */
  chainId: string;
  /** Verifying contract address. */
  verifyingContract: string;
}

/**
 * Output of `WebCryptoAnima.rotate()`.
 *
 * Browser passkeys are seed-resident in the OS keychain — there is no
 * software-rotation primitive. This type is included for API symmetry
 * with the Rust trait shape; in practice `rotate()` rejects with a
 * `not_supported` error pointing the caller at the journal-driven flow.
 *
 * Mirrors Rust's `DidRotationEvent`.
 */
export interface DidRotationEvent {
  /** The DID that was rotated away from. */
  oldDid: string;
  /** The DID that signing now flows through. */
  newDid: string;
  /**
   * Detached JWS by the *old* key over the *new* DID. Compact form
   * `<header>.<body>.<signature>`.
   */
  rotationProofJws: string;
  /** Wall-clock ISO-8601 timestamp of the rotation. */
  rotatedAt: string;
}

/**
 * WebAuthn attestation object — output of `navigator.credentials.create`.
 *
 * The browser's PublicKeyCredential gives us:
 *   - `attestationObject`: CBOR-encoded `{fmt, attStmt, authData}`. Anima
 *     parses `authData` to extract the COSE_Key public key.
 *   - `clientDataJSON`: UTF-8 JSON `{type, challenge, origin, ...}`.
 */
export interface AttestationObject {
  /** CBOR-encoded attestation object bytes. */
  attestationObject: Uint8Array;
  /** UTF-8 JSON client-data bytes. */
  clientDataJson: Uint8Array;
  /** Credential ID — opaque handle the authenticator references later. */
  credentialId: ArrayBuffer;
  /** SEC1-compressed P-256 pubkey (33 bytes), parsed from COSE_Key. */
  pubkey: Uint8Array;
}

/**
 * WebAuthn assertion — output of `navigator.credentials.get`.
 *
 * Returned on every signing operation. The DER-encoded ECDSA signature
 * lives in `signature`; anima converts to JOSE compact form (r||s, 64
 * bytes) before returning to higher layers.
 */
export interface Assertion {
  /** Credential ID echoed back by the authenticator. */
  credentialId: ArrayBuffer;
  /** UTF-8 JSON client-data bytes (challenge + origin + type). */
  clientDataJson: Uint8Array;
  /** Authenticator data (signature_count, flags, RP ID hash). */
  authenticatorData: Uint8Array;
  /** DER-encoded ECDSA signature. */
  signature: Uint8Array;
}
