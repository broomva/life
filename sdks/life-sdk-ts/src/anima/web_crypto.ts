/**
 * `WebCryptoAnima` — composition wrapper for the browser custody path.
 *
 * Spec D L4-D5 (split custody) ground truth:
 *   - auth half  → {@link PasskeyOracle} (P-256, non-extractable)
 *   - wallet half → {@link RemoteAnimaClient} (delegated to server-side
 *                  anima daemon holding secp256k1 in Vault Transit)
 *
 * Mirrors Rust's `HardwareWalletAnima` shape (auth-only wrapping a
 * delegate, but here it's the wallet half delegated and the auth
 * half local). The class is composition-of-roles, not a single
 * keypair holder.
 *
 * Spec D §"Browser path (D-Sub-C in detail)" §1-3 implementation:
 *   - §1 auth keypair via passkey (`PasskeyOracle.enroll/load`)
 *   - §2 DID derivation (`generateDidKeyP256(authPubkey)`)
 *   - §3 signing flow (`signJws`/`signDigest` → passkey, `signEvmTx`/
 *        `signEip712` → remote)
 */

import { generateDidKeyP256 } from "./did.js";
import { AnimaError } from "./errors.js";
import type { PasskeyOracle } from "./passkey.js";
import type { RemoteAnimaClient } from "./remote.js";
import type { SessionCap } from "./session_cap.js";
import type {
  BackendKind,
  DidRotationEvent,
  Eip712Domain,
  EvmSignature,
  TxRequest,
} from "./types.js";

/**
 * Configuration for {@link WebCryptoAnima}.
 *
 * All four fields are mandatory — there are no degraded modes. The
 * `walletAddress` and `walletPubkey` are cached at enrollment so we
 * don't re-fetch on every `signEvmTx` / `signEip712`.
 */
export interface WebCryptoAnimaConfig {
  /** Auth-half passkey oracle (already enrolled or loaded). */
  auth: PasskeyOracle;
  /** Wallet-half delegate (HTTP client to lifegw `/anima/custody/*`). */
  wallet: RemoteAnimaClient;
  /** Tier-User cap JWT manager (passed to wallet for auto-refresh). */
  sessionCap: SessionCap;
  /** User id this custody handle is bound to. */
  userId: string;
  /** Cached wallet address (`0x…` hex) from enrollment. */
  walletAddress: string;
  /** Cached secp256k1 wallet pubkey (33 bytes SEC1-compressed). */
  walletPubkey: Uint8Array;
}

/**
 * `WebCryptoAnima` — the browser-side `AnimaCustody`-equivalent.
 *
 * Closely mirrors Rust's `AnimaCustody` trait method names (camelCase
 * here vs snake_case there). Public methods:
 *
 *   - `userDid()`        → `did:key:zDn…`
 *   - `authPubkey()`     → 33 bytes SEC1-compressed
 *   - `walletAddress()`  → `0x…` (always populated for this backend)
 *   - `backendKind()`    → `"web_crypto"`
 *   - `signJws(header, payload)` → compact JWS string
 *   - `signDigest(digest)`        → 64 bytes (r||s)
 *   - `signEvmTx(tx)`             → 65-byte EvmSignature (r||s||v)
 *   - `signEip712(domain, types, message)` → 65-byte EvmSignature
 *   - `rotate()`                  → rejects with `not_supported`
 */
export class WebCryptoAnima {
  private readonly cfg: WebCryptoAnimaConfig;

  constructor(cfg: WebCryptoAnimaConfig) {
    if (cfg.walletPubkey.byteLength !== 33) {
      throw AnimaError.state(
        `WebCryptoAnima: walletPubkey must be 33 bytes, got ${cfg.walletPubkey.byteLength}`,
      );
    }
    this.cfg = cfg;
  }

  /**
   * SEC1-compressed P-256 auth public key (33 bytes). Pulled from
   * the passkey oracle's in-memory cache (set at enroll/load time).
   */
  authPubkey(): Uint8Array {
    return this.cfg.auth.pubkey();
  }

  /**
   * User-scope DID — `did:key:zDn…` derived from the P-256 auth
   * pubkey. Cross-language compatible with Rust's
   * `EcdsaP256Identity::did_key`.
   */
  userDid(): string {
    return generateDidKeyP256(this.authPubkey());
  }

  /**
   * Wallet address (`0x…` hex). Always populated for `WebCryptoAnima`
   * — split custody's wallet half is mandatory.
   */
  walletAddress(): string {
    return this.cfg.walletAddress;
  }

  /** SEC1-compressed secp256k1 wallet pubkey (33 bytes). */
  walletPubkey(): Uint8Array {
    return this.cfg.walletPubkey;
  }

  /** User id this custody handle is bound to. */
  userId(): string {
    return this.cfg.userId;
  }

  /** Backend identifier (Spec D §"Event additions"). */
  backendKind(): BackendKind {
    return "web_crypto";
  }

  /**
   * Sign a JWS over the supplied header + claims using the auth key.
   *
   * Spec D §"Browser path" §3 — the OS prompts on every signing
   * operation unless a session-cap is active. Higher layers should
   * batch where possible.
   *
   * The resulting compact JWS shape is `<headerB64>.<bodyB64>.<sigB64>`
   * with `sigB64` being the JOSE-compact 64-byte raw signature
   * (mirrors Rust's `EcdsaP256Identity::sign_jws`).
   *
   * NOTE on WebAuthn semantics: the authenticator hashes
   * (clientDataJSON || authenticatorData) before signing, so the bytes
   * the authenticator actually signs are NOT the JWS signing-input.
   * For genuine JWS interop the caller MUST verify against the
   * `clientDataJSON.challenge` mechanism (use `signWithAssertion`
   * + the WebAuthn assertion verification path on the server). Use
   * this method only when your server-side verifier is WebAuthn-
   * aware, e.g. lago-auth's `verify_jwt` with the rotation_chain
   * + WebAuthn assertion validator extension. For purely-server-
   * resolved JWS (the lifegw cap mint flow) prefer the
   * `signWithAssertion` shape on the passkey directly.
   */
  async signJws(
    header: Record<string, unknown>,
    payload: Record<string, unknown>,
  ): Promise<string> {
    const finalHeader = {
      alg: "ES256",
      typ: "JWT",
      kid: this.userDid(),
      ...header,
    };
    const headerB64 = b64UrlEncode(new TextEncoder().encode(JSON.stringify(finalHeader)));
    const payloadB64 = b64UrlEncode(new TextEncoder().encode(JSON.stringify(payload)));
    const signingInput = `${headerB64}.${payloadB64}`;
    const digest = await sha256(new TextEncoder().encode(signingInput));
    const signature = await this.cfg.auth.sign(digest);
    return `${signingInput}.${b64UrlEncode(signature)}`;
  }

  /**
   * Sign a 32-byte digest with the auth (P-256) key. Returns the
   * 64-byte JOSE-compact form (`r||s`) — same shape as Rust's
   * `AnimaCustody::sign_digest`.
   */
  async signDigest(digest: Uint8Array): Promise<Uint8Array> {
    if (digest.byteLength !== 32) {
      throw AnimaError.state(`signDigest digest must be 32 bytes, got ${digest.byteLength}`);
    }
    return this.cfg.auth.sign(digest);
  }

  /**
   * Sign an EVM transaction with the wallet (secp256k1) key.
   *
   * Browser custody delegates this to the server-side anima daemon
   * (`RemoteAnimaClient.signEvmTx`) which holds the secp256k1 key in
   * `VaultTransitAnima`. The resulting `r||s||v` signature is
   * shaped identically to the Rust `EvmSignature.bytes`.
   *
   * `from` field MUST equal the cached wallet address — sanity-checked
   * here to catch caller bugs before the server rejects.
   */
  async signEvmTx(tx: TxRequest): Promise<EvmSignature> {
    if (tx.from.toLowerCase() !== this.cfg.walletAddress.toLowerCase()) {
      throw AnimaError.state(
        `signEvmTx: tx.from (${tx.from}) does not match wallet address (${this.cfg.walletAddress})`,
      );
    }
    return this.cfg.wallet.signEvmTx(this.cfg.userId, tx);
  }

  /**
   * Sign an EIP-712 typed-data payload (used by haima-x402 + USDC
   * `transferWithAuthorization` and any future EIP-712 flows).
   *
   * Same delegation pattern as `signEvmTx`.
   */
  async signEip712(
    domain: Eip712Domain,
    types: Record<string, unknown>,
    message: Record<string, unknown>,
  ): Promise<EvmSignature> {
    return this.cfg.wallet.signEip712(this.cfg.userId, domain, types, message);
  }

  /**
   * Rotation is journal-driven — `WebCryptoAnima.rotate` rejects.
   *
   * Spec D L4-D10: rotation is documented in the journal, not
   * implicit. To rotate the auth key, the user must:
   *   1. Enroll a fresh passkey on a still-trusted device.
   *   2. Have the server-side anima daemon emit
   *      `anima.identity_rotated` signed by the OLD key.
   *
   * Mirrors Rust's `RemoteAnima.rotate()` shape — rejects rather
   * than silently doing nothing.
   */
  async rotate(): Promise<DidRotationEvent> {
    throw AnimaError.notSupported(
      "WebCryptoAnima.rotate is journal-driven; use the soma admin RPC or anima-lago::write_rotation_event from the server-side anima daemon",
    );
  }
}

/**
 * High-level helper: complete the full enrollment flow and return a
 * ready-to-use `WebCryptoAnima` handle.
 *
 * Performs:
 *   1. `passkey.enroll(userId, displayName, challenge)` — OS UI fires
 *   2. `remote.enrollPasskey(userId, attestation)` — server provisions
 *      Vault Transit secp256k1 key + records the user's DID
 *   3. Construct `WebCryptoAnima` with the cached wallet address +
 *      pubkey from step 2.
 */
export async function enrollWebCryptoAnima(params: {
  passkey: PasskeyOracle;
  remote: RemoteAnimaClient;
  sessionCap: SessionCap;
  userId: string;
  displayName: string;
  challenge: Uint8Array;
}): Promise<WebCryptoAnima> {
  const enroll = await params.passkey.enroll(
    params.userId,
    params.displayName,
    params.challenge,
  );
  const remoteEnroll = await params.remote.enrollPasskey(params.userId, {
    attestationObject: enroll.attestationObject,
    clientDataJson: enroll.clientDataJson,
    credentialId: enroll.credentialId,
  });
  return new WebCryptoAnima({
    auth: params.passkey,
    wallet: params.remote,
    sessionCap: params.sessionCap,
    userId: params.userId,
    walletAddress: remoteEnroll.walletAddress,
    walletPubkey: remoteEnroll.walletPubkey,
  });
}

/**
 * High-level helper: load a previously-enrolled `WebCryptoAnima`.
 *
 * Pulls the cached credential from IndexedDB via `passkey.load`, then
 * fetches the wallet address + pubkey from `remote.getWalletAddress`
 * + `remote.getWalletPubkey`. No OS UI fires until the first signing
 * operation.
 *
 * Returns `null` if the user hasn't enrolled (caller must run
 * `enrollWebCryptoAnima` first).
 */
export async function loadWebCryptoAnima(params: {
  passkey: PasskeyOracle;
  remote: RemoteAnimaClient;
  sessionCap: SessionCap;
  userId: string;
}): Promise<WebCryptoAnima | null> {
  const loaded = await params.passkey.load(params.userId);
  if (!loaded) return null;

  const [walletAddress, walletPubkey] = await Promise.all([
    params.remote.getWalletAddress(params.userId),
    params.remote.getWalletPubkey(params.userId),
  ]);
  return new WebCryptoAnima({
    auth: params.passkey,
    wallet: params.remote,
    sessionCap: params.sessionCap,
    userId: params.userId,
    walletAddress,
    walletPubkey,
  });
}

// ── helpers ────────────────────────────────────────────────────────────

/** SHA-256 digest using WebCrypto. */
async function sha256(bytes: Uint8Array): Promise<Uint8Array> {
  const subtle = (globalThis as unknown as { crypto?: { subtle?: SubtleCrypto } }).crypto?.subtle;
  if (!subtle) {
    throw AnimaError.state("SHA-256 requires WebCrypto (crypto.subtle)");
  }
  // Copy into a fresh ArrayBuffer to satisfy `BufferSource` shape.
  const ab = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(ab).set(bytes);
  const hash = await subtle.digest("SHA-256", ab);
  return new Uint8Array(hash);
}

/** base64url (RFC 4648) without padding. */
function b64UrlEncode(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]!);
  let std: string;
  if (typeof btoa === "function") {
    std = btoa(bin);
  } else {
    const g = globalThis as unknown as {
      Buffer?: { from(s: string, enc: string): { toString(enc: string): string } };
    };
    if (!g.Buffer) throw AnimaError.crypto("no base64 encoder available");
    std = g.Buffer.from(bin, "binary").toString("base64");
  }
  return std.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
