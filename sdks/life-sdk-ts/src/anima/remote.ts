/**
 * `RemoteAnimaClient` — HTTP client for lifegw's `/anima/custody/*`
 * routes.
 *
 * Spec D D-Sub-C ground truth (Stream R ships these routes in
 * parallel):
 *
 *   POST /anima/custody/sign_auth        { user_id, digest_b64 }
 *   POST /anima/custody/sign_wallet      { user_id, digest_b64 }
 *   GET  /anima/custody/get_auth_pubkey/:uid
 *   GET  /anima/custody/get_wallet_pubkey/:uid
 *   POST /anima/custody/mint_session_cap { user_id, attestation, assertion }
 *   POST /anima/custody/enroll_passkey   { user_id, attestation, client_data }
 *
 * Locked architectural decision (HTTP/JSON, NOT gRPC-web): keeping
 * custody RPC code separate from the existing tonic-web SDK paths
 * sidesteps M8.1 (Connect-vs-grpc-web mismatch). Custody calls land
 * regardless of M8.1's status — they're plain `fetch` to JSON
 * endpoints with bearer-token auth.
 *
 * The wallet half delegates to a server-side anima daemon (Spec D
 * §"Backend matrix" — `RemoteAnima` row) that holds secp256k1 in
 * `VaultTransitAnima`. The browser-side `WebCryptoAnima` composes
 * with this client; from the chain's perspective the signature is
 * still EOA-flavored.
 */

import { AnimaError } from "./errors.js";
import type { Eip712Domain, EvmSignature, TxRequest } from "./types.js";

/**
 * Configuration for {@link RemoteAnimaClient}.
 */
export interface RemoteAnimaClientConfig {
  /**
   * lifegw HTTPS endpoint, e.g. `https://api.life.dev`. Trailing
   * slashes are normalized away.
   *
   * Spec C₃ §5.1 — TLS 1.3 mandatory.
   */
  baseUrl: string;
  /**
   * Async producer for the Tier-User cap JWT — typically a
   * `SessionCap.getValidToken` function. Called on every RPC.
   */
  getToken: () => Promise<string>;
  /**
   * Optional `fetch` override (tests inject a deterministic shim;
   * production code lets it default to `globalThis.fetch`).
   */
  fetch?: typeof fetch;
  /**
   * I-2 review fix: per-request timeout in milliseconds. A hung
   * lifegw shouldn't block the browser indefinitely — bare `fetch`
   * has no built-in timeout. Defaults to 30 000 (30s); tune up for
   * slow upstream KMS providers, down for tighter UX. Set to `0` to
   * disable (NOT recommended in production).
   */
  requestTimeoutMs?: number;
}

/**
 * Default per-request timeout (30 s) — used when
 * `RemoteAnimaClientConfig.requestTimeoutMs` is unset.
 */
export const DEFAULT_REQUEST_TIMEOUT_MS = 30_000;

/**
 * Maximum number of characters of an upstream HTTP error body that
 * `RemoteAnimaClient` includes in `AnimaError.message`. I-1 review fix:
 * lifegw error bodies often echo back JWT claims / `request_id` /
 * upstream error blobs that leak under devtools, in stack traces shipped
 * to Sentry, or on uncaught-promise reports. Truncate aggressively.
 */
const MAX_REMOTE_ERROR_BODY_CHARS = 200;

/** Returned by `enrollPasskey`. */
export interface EnrollPasskeyResult {
  /** Server-issued user DID after enrollment (`did:key:zDn…`). */
  userDid: string;
  /**
   * Wallet address the server resolved/minted for this user. Browser
   * custody uses this on every `signEvmTx` call as the `from` address
   * sanity check.
   */
  walletAddress: string;
  /** SEC1-compressed secp256k1 wallet pubkey (33 bytes). */
  walletPubkey: Uint8Array;
}

/** Returned by `mintSessionCap`. */
export interface MintSessionCapResult {
  /** Tier-User capability JWT (ES256 over the user's auth key). */
  token: string;
  /** Unix-seconds expiry from the JWT `exp` claim. */
  expiresAt: number;
  /** Wall-clock issued-at (Unix seconds) from the JWT `iat` claim. */
  issuedAt: number;
}

/**
 * Thin `fetch` wrapper for lifegw's HTTP `/anima/custody/*` surface.
 *
 * One instance per origin; safe to share across an app. Uses
 * `getToken` to refresh the bearer on every call, so callers can wire
 * a `SessionCap` in front to centralise mint + refresh.
 */
export class RemoteAnimaClient {
  readonly baseUrl: string;
  private readonly fetchFn: typeof fetch;
  private readonly getToken: () => Promise<string>;
  private readonly requestTimeoutMs: number;

  constructor(cfg: RemoteAnimaClientConfig) {
    this.baseUrl = cfg.baseUrl.replace(/\/+$/, "");
    this.fetchFn = cfg.fetch ?? globalThis.fetch.bind(globalThis);
    this.getToken = cfg.getToken;
    this.requestTimeoutMs =
      cfg.requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS;
  }

  /**
   * Build an `AbortSignal` that fires after `requestTimeoutMs` if the
   * timeout is positive. Returns `undefined` when the timeout is `0`
   * (caller explicitly opted out). I-2 review fix.
   */
  private buildTimeoutSignal(): AbortSignal | undefined {
    if (this.requestTimeoutMs <= 0) return undefined;
    // `AbortSignal.timeout` is widely supported (Chrome 103+, FF 100+,
    // Safari 16+, Node 17.3+). We need it for browser custody and Node
    // tests; the fake-fetch shim in tests ignores `signal` so this
    // remains harmless there.
    if (typeof AbortSignal !== "undefined" && "timeout" in AbortSignal) {
      return AbortSignal.timeout(this.requestTimeoutMs);
    }
    // Fallback for ancient runtimes — manual controller + setTimeout.
    const ctrl = new AbortController();
    setTimeout(() => ctrl.abort(), this.requestTimeoutMs);
    return ctrl.signal;
  }

  // ── auth half (P-256) ─────────────────────────────────────────────

  /**
   * Forward a digest to the server-side anima for the auth-half
   * signature. Used internally by `WebCryptoAnima.signJws` only when
   * the browser is running in a delegated-auth shape (rare).
   */
  async signAuth(userId: string, digest: Uint8Array): Promise<Uint8Array> {
    const json = await this.postJson<{ signature_b64: string }>(
      "/anima/custody/sign_auth",
      {
        user_id: userId,
        digest_b64: bytesToB64(digest),
      },
    );
    return b64ToBytes(json.signature_b64);
  }

  /**
   * SEC1-compressed P-256 auth pubkey for the given user. Cached by
   * `WebCryptoAnima` at construction time.
   */
  async getAuthPubkey(userId: string): Promise<Uint8Array> {
    const json = await this.getJson<{ pubkey_b64: string }>(
      `/anima/custody/get_auth_pubkey/${encodeURIComponent(userId)}`,
    );
    return b64ToBytes(json.pubkey_b64);
  }

  // ── wallet half (secp256k1) ───────────────────────────────────────

  /**
   * Forward a 32-byte digest for the wallet-half signature. The
   * server applies the signing key (Vault Transit secp256k1) and
   * returns the 65-byte `r||s||v` recoverable signature.
   *
   * Used by `WebCryptoAnima.signEvmTx` and `signEip712`.
   */
  async signWallet(userId: string, digest: Uint8Array): Promise<Uint8Array> {
    const json = await this.postJson<{ signature_b64: string }>(
      "/anima/custody/sign_wallet",
      {
        user_id: userId,
        digest_b64: bytesToB64(digest),
      },
    );
    return b64ToBytes(json.signature_b64);
  }

  /**
   * Forward an EIP-1559 transaction request. The server computes the
   * keccak256 RLP digest server-side (so we don't have to ship a
   * keccak implementation in the SDK) and signs it.
   *
   * Returns the 65-byte `r||s||v` legacy recoverable signature
   * (`v ∈ {27, 28}`) — same shape as Rust's `EvmSignature.bytes`.
   */
  async signEvmTx(userId: string, tx: TxRequest): Promise<EvmSignature> {
    const json = await this.postJson<{ signature_b64: string }>(
      "/anima/custody/sign_evm_tx",
      {
        user_id: userId,
        tx: {
          from: tx.from,
          to: tx.to,
          value_wei: tx.valueWei,
          data_hex: tx.dataHex,
          nonce: tx.nonce,
          gas_limit: tx.gasLimit,
          max_fee_per_gas_wei: tx.maxFeePerGasWei,
          max_priority_fee_per_gas_wei: tx.maxPriorityFeePerGasWei,
          chain: tx.chain,
        },
      },
    );
    return { bytes: b64ToBytes(json.signature_b64) };
  }

  /**
   * Forward an EIP-712 typed-data signing request (used for x402 +
   * USDC `transferWithAuthorization`).
   *
   * `types` and `message` are passed through opaquely as JSON; lifegw
   * runs the canonical EIP-712 encoder server-side.
   */
  async signEip712(
    userId: string,
    domain: Eip712Domain,
    types: Record<string, unknown>,
    message: Record<string, unknown>,
  ): Promise<EvmSignature> {
    const json = await this.postJson<{ signature_b64: string }>(
      "/anima/custody/sign_eip712",
      {
        user_id: userId,
        domain: {
          name: domain.name,
          version: domain.version,
          chain_id: domain.chainId,
          verifying_contract: domain.verifyingContract,
        },
        types,
        message,
      },
    );
    return { bytes: b64ToBytes(json.signature_b64) };
  }

  /**
   * SEC1-compressed secp256k1 wallet pubkey for the given user.
   * Cached by `WebCryptoAnima` at construction time so we don't
   * re-fetch it on every `signEvmTx`.
   */
  async getWalletPubkey(userId: string): Promise<Uint8Array> {
    const json = await this.getJson<{ pubkey_b64: string }>(
      `/anima/custody/get_wallet_pubkey/${encodeURIComponent(userId)}`,
    );
    return b64ToBytes(json.pubkey_b64);
  }

  /**
   * Resolve the user's wallet address — `0x…` hex for Ethereum, may be
   * a chain-specific shape for non-EVM in the future.
   */
  async getWalletAddress(userId: string): Promise<string> {
    const json = await this.getJson<{ address: string }>(
      `/anima/custody/wallet_address/${encodeURIComponent(userId)}`,
    );
    return json.address;
  }

  // ── lifecycle ─────────────────────────────────────────────────────

  /**
   * One-shot enrollment: send the WebAuthn attestation to lifegw which
   * provisions per-user Vault keys (or returns existing keys if the
   * user already has them) and returns the user's DID + wallet
   * address.
   */
  async enrollPasskey(
    userId: string,
    params: {
      attestationObject: Uint8Array;
      clientDataJson: Uint8Array;
      credentialId: ArrayBuffer;
    },
  ): Promise<EnrollPasskeyResult> {
    const json = await this.postJson<{
      user_did: string;
      wallet_address: string;
      wallet_pubkey_b64: string;
    }>("/anima/custody/enroll_passkey", {
      user_id: userId,
      attestation_object_b64: bytesToB64(params.attestationObject),
      client_data_json_b64: bytesToB64(params.clientDataJson),
      credential_id_b64: bytesToB64(new Uint8Array(params.credentialId)),
    });
    return {
      userDid: json.user_did,
      walletAddress: json.wallet_address,
      walletPubkey: b64ToBytes(json.wallet_pubkey_b64),
    };
  }

  /**
   * Mint a Tier-User capability JWT. Spec D §"Browser path" §5
   * — one passkey-mediated handshake per session mints a short-lived
   * cap that authorizes subsequent RPCs without re-prompting the OS
   * UI. Default TTL 15 minutes.
   *
   * `attestationObject` is sent on the FIRST mint after enrollment
   * (verified server-side against the credentialId). Subsequent mints
   * within the cap's lifetime omit it.
   */
  async mintSessionCap(
    userId: string,
    params: {
      assertion: {
        signature: Uint8Array;
        clientDataJson: Uint8Array;
        authenticatorData: Uint8Array;
        credentialId: ArrayBuffer;
      };
    },
    bearerOverride?: string,
  ): Promise<MintSessionCapResult> {
    const body: Record<string, unknown> = {
      user_id: userId,
      assertion: {
        signature_b64: bytesToB64(params.assertion.signature),
        client_data_json_b64: bytesToB64(params.assertion.clientDataJson),
        authenticator_data_b64: bytesToB64(params.assertion.authenticatorData),
        credential_id_b64: bytesToB64(new Uint8Array(params.assertion.credentialId)),
      },
    };

    // The mint endpoint is reachable both with the existing cap (for
    // refresh) and pre-cap (using a one-shot enrollment cookie or a
    // public mint endpoint). Callers that have just enrolled and don't
    // yet have a cap pass `bearerOverride: ""` to skip the bearer.
    const json = await this.postJsonWithExplicitToken<{
      token: string;
      expires_at: number;
      issued_at: number;
    }>("/anima/custody/mint_session_cap", body, bearerOverride);

    return {
      token: json.token,
      expiresAt: json.expires_at,
      issuedAt: json.issued_at,
    };
  }

  // ── HTTP plumbing ─────────────────────────────────────────────────

  private async postJson<T>(path: string, body: unknown): Promise<T> {
    const token = await this.getToken();
    return this.postJsonWithExplicitToken<T>(path, body, token);
  }

  private async postJsonWithExplicitToken<T>(
    path: string,
    body: unknown,
    token: string | undefined,
  ): Promise<T> {
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
    };
    if (token) headers.Authorization = `Bearer ${token}`;

    let resp: Response;
    try {
      resp = await this.fetchFn(`${this.baseUrl}${path}`, {
        method: "POST",
        headers,
        body: JSON.stringify(body),
        signal: this.buildTimeoutSignal(),
      });
    } catch (err) {
      throw mapFetchFailure(path, err);
    }
    return await this.parseResponse<T>(path, resp);
  }

  private async getJson<T>(path: string): Promise<T> {
    const token = await this.getToken();
    const headers: Record<string, string> = {};
    if (token) headers.Authorization = `Bearer ${token}`;

    let resp: Response;
    try {
      resp = await this.fetchFn(`${this.baseUrl}${path}`, {
        method: "GET",
        headers,
        signal: this.buildTimeoutSignal(),
      });
    } catch (err) {
      throw mapFetchFailure(path, err);
    }
    return await this.parseResponse<T>(path, resp);
  }

  private async parseResponse<T>(path: string, resp: Response): Promise<T> {
    const text = await resp.text();
    if (!resp.ok) {
      // I-1 review fix: lifegw error bodies often echo back JWT
      // claims, request IDs, or upstream KMS error blobs. Truncate
      // aggressively so the error string never leaks an entire
      // upstream payload into devtools / Sentry / uncaught-promise
      // reports. Prefer parsed `{ code, message }` JSON shape when
      // present; fall back to a length-capped raw string.
      const safeBody = sanitizeErrorBody(text, resp.statusText, resp.status);
      throw AnimaError.remote(resp.status, `${path}: ${safeBody}`);
    }
    if (!text) return {} as T;
    try {
      return JSON.parse(text) as T;
    } catch (err) {
      throw AnimaError.crypto(
        `malformed JSON from ${path}: ${(err as Error).message}`,
        err,
      );
    }
  }
}

// ── base64 helpers (mirror codec.ts but kept local to avoid coupling) ─

function bytesToB64(bytes: Uint8Array): string {
  if (typeof btoa === "function") {
    let bin = "";
    for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]!);
    return btoa(bin);
  }
  const g = globalThis as unknown as {
    Buffer?: { from(b: Uint8Array): { toString(enc: string): string } };
  };
  if (g.Buffer) return g.Buffer.from(bytes).toString("base64");
  throw AnimaError.crypto("no base64 encoder available");
}

function b64ToBytes(b64: string): Uint8Array {
  // Accept both standard and URL-safe variants for forward-compat.
  const normalized = b64.replace(/-/g, "+").replace(/_/g, "/");
  const padded = normalized + "===".slice((normalized.length + 3) % 4);
  if (typeof atob === "function") {
    const bin = atob(padded);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  const g = globalThis as unknown as {
    Buffer?: { from(s: string, enc: string): Uint8Array };
  };
  if (g.Buffer) return new Uint8Array(g.Buffer.from(padded, "base64"));
  throw AnimaError.crypto("no base64 decoder available");
}

/**
 * I-1 review fix: turn an upstream error body into a length-capped,
 * leak-resistant message. If the body parses as JSON with `{ code,
 * message }` shape, prefer those fields and drop everything else
 * (avoids accidentally surfacing JWT claims / request IDs / upstream
 * KMS errors). Otherwise truncate to `MAX_REMOTE_ERROR_BODY_CHARS`.
 */
function sanitizeErrorBody(
  body: string,
  statusText: string,
  status: number,
): string {
  if (!body) return statusText || `HTTP ${status}`;
  // Try to extract a structured `{ code, message }` shape — these are
  // safe by convention (lifegw doesn't put PII in `code`/`message`).
  try {
    const parsed = JSON.parse(body) as Record<string, unknown>;
    const code = typeof parsed.code === "string" ? parsed.code : undefined;
    const msg = typeof parsed.message === "string" ? parsed.message : undefined;
    if (code && msg) return `${code}: ${truncate(msg, MAX_REMOTE_ERROR_BODY_CHARS)}`;
    if (code) return code;
    if (msg) return truncate(msg, MAX_REMOTE_ERROR_BODY_CHARS);
  } catch {
    // Not JSON — fall through to raw truncation.
  }
  return truncate(body, MAX_REMOTE_ERROR_BODY_CHARS);
}

function truncate(s: string, max: number): string {
  return s.length <= max ? s : `${s.slice(0, max)}…`;
}

/**
 * I-2 review fix: convert a `fetch`-rejection error into a typed
 * `AnimaError.remote`, distinguishing timeouts (AbortError /
 * TimeoutError) from generic transport failures. Useful for callers
 * that want to retry only on timeouts.
 */
function mapFetchFailure(path: string, err: unknown): AnimaError {
  const e = err as { name?: string; message?: string };
  const name = e?.name ?? "";
  const message = e?.message ?? String(err);
  if (name === "TimeoutError" || name === "AbortError") {
    return AnimaError.remote(0, `request timeout for ${path}`);
  }
  return AnimaError.remote(0, `fetch failed for ${path}: ${message}`);
}
