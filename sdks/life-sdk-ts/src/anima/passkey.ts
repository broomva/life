/**
 * `PasskeyOracle` — WebAuthn passkey custody for the auth half of
 * `WebCryptoAnima` (Spec D D-Sub-C, "Browser path" §1-3).
 *
 * Wraps `navigator.credentials.create` (enrollment) and
 * `navigator.credentials.get` (signing). The underlying CryptoKey is
 * non-extractable per L4-D5 — we NEVER call `crypto.subtle.exportKey`,
 * the key is OS-managed (Touch ID / Windows Hello / iCloud Keychain).
 *
 * IndexedDB is used to persist the credentialId across browser
 * sessions. The cached SEC1-compressed pubkey is stored alongside so
 * we can derive the user's DID without prompting the OS auth UI on
 * every load.
 *
 * The DER-encoded ECDSA signature returned by WebAuthn is normalized
 * to JOSE compact form (raw 64 bytes, `r||s`) per RFC 7515 + RFC 7518.
 *
 * @see Spec D §"Browser path (D-Sub-C in detail)"
 * @see https://www.w3.org/TR/webauthn-3/
 */

import { decode as cborDecode } from "cbor-x";
import { type IDBPDatabase, openDB } from "idb";
import { AnimaError } from "./errors.js";

/** Database name for the passkey credentialId cache. */
const PASSKEY_DB_NAME = "broomva-anima-passkeys";
/** Schema version. Bump if the record shape changes. */
const PASSKEY_DB_VERSION = 1;
/** Object store name within the DB. Keyed by `userId`. */
const PASSKEY_STORE = "credentials";

/**
 * Stored passkey record — one per `userId`.
 *
 * `credentialId` is the opaque authenticator handle echoed back on
 * subsequent sign requests; `pubkey` is the SEC1-compressed P-256
 * point parsed once from the COSE_Key at enrollment time.
 */
interface StoredCredential {
  /** Caller-supplied user id (e.g. Clerk user id). Primary key. */
  userId: string;
  /** Credential ID bytes — passed to `allowCredentials` on signing. */
  credentialId: ArrayBuffer;
  /** SEC1-compressed P-256 public key (33 bytes). */
  pubkey: Uint8Array;
  /** WebAuthn relying-party id this credential was enrolled against. */
  rpId: string;
  /** Wall-clock when the credential was enrolled (ISO-8601). */
  createdAt: string;
}

/** Output of `PasskeyOracle.enroll`. */
export interface PasskeyEnrollResult {
  credentialId: ArrayBuffer;
  attestationObject: Uint8Array;
  clientDataJson: Uint8Array;
  /** SEC1-compressed P-256 public key (33 bytes). */
  pubkey: Uint8Array;
}

/** Output of `PasskeyOracle.load`. */
export interface PasskeyLoadResult {
  credentialId: ArrayBuffer;
  /** SEC1-compressed P-256 public key (33 bytes). */
  pubkey: Uint8Array;
}

/**
 * Configuration for {@link PasskeyOracle}.
 *
 * The defaults target broomva.tech; consumers running embedded in a
 * tenant deployment should override `rpId` + `rpName` to match their
 * domain (WebAuthn relying-party id MUST be the suffix of `origin`).
 */
export interface PasskeyOracleConfig {
  /** WebAuthn relying-party id (must be a suffix of the origin). */
  rpId: string;
  /** Human-readable RP name shown in the OS auth prompt. */
  rpName: string;
  /**
   * Optional injection point for the WebAuthn API — defaults to
   * `globalThis.navigator.credentials`. Tests override this with a
   * deterministic mock; production paths leave this undefined.
   */
  credentials?: CredentialsContainerLike;
  /**
   * Optional injection point for the IndexedDB DB factory — defaults
   * to `globalThis.indexedDB`. Tests use `fake-indexeddb` to avoid
   * a real-browser dependency.
   */
  indexedDB?: IDBFactory;
  /**
   * Optional injection point for the database name. Tests can pass a
   * unique name to isolate parallel test runs.
   */
  databaseName?: string;
}

/**
 * Subset of `CredentialsContainer` we use. Defining a structural type
 * lets tests mock the API without depending on lib.dom's heavyweight
 * `PublicKeyCredential` shape.
 */
export interface CredentialsContainerLike {
  create(options: { publicKey: PublicKeyCredentialCreationOptions }): Promise<PublicKeyCredentialLike | null>;
  get(options: { publicKey: PublicKeyCredentialRequestOptions }): Promise<PublicKeyCredentialLike | null>;
}

/**
 * Subset of `PublicKeyCredential` we read. Browser implementations
 * carry many more fields; for SDK purposes we only need the minimum
 * surface.
 */
export interface PublicKeyCredentialLike {
  rawId: ArrayBuffer;
  response:
    | AuthenticatorAttestationResponseLike
    | AuthenticatorAssertionResponseLike;
}

export interface AuthenticatorAttestationResponseLike {
  clientDataJSON: ArrayBuffer;
  attestationObject: ArrayBuffer;
}

export interface AuthenticatorAssertionResponseLike {
  clientDataJSON: ArrayBuffer;
  authenticatorData: ArrayBuffer;
  signature: ArrayBuffer;
  userHandle?: ArrayBuffer | null;
}

// Discriminator helpers for the union response type.
function isAttestationResponse(
  resp: AuthenticatorAttestationResponseLike | AuthenticatorAssertionResponseLike,
): resp is AuthenticatorAttestationResponseLike {
  return "attestationObject" in resp;
}

function isAssertionResponse(
  resp: AuthenticatorAttestationResponseLike | AuthenticatorAssertionResponseLike,
): resp is AuthenticatorAssertionResponseLike {
  return "signature" in resp && "authenticatorData" in resp;
}

/**
 * `PasskeyOracle` — passkey-managed P-256 keypair custody.
 *
 * Lifecycle:
 *   1. Construct with `rpId` + `rpName` (matching your origin).
 *   2. Either `enroll(userId, displayName, challenge)` once OR
 *      `load(userId)` to pick up a previously-enrolled credential.
 *   3. Call `sign(digest)` to produce a 64-byte JOSE-compact ECDSA
 *      signature over the supplied digest.
 *
 * Note: the WebAuthn `clientDataJSON.challenge` is the digest. The
 * authenticator hashes (clientDataJSON || authenticatorData) with
 * SHA-256 before signing, so the signed bytes are NOT exactly the
 * caller's digest. Callers wanting a JWS over the digest itself must
 * verify the chain (digest → clientDataJSON.challenge →
 * authenticatorData → signature). For the SDK's purposes (signing the
 * Tier-User cap mint challenge), this chain is exactly what lifegw's
 * `/anima/custody/mint_session_cap` endpoint validates.
 */
export class PasskeyOracle {
  private readonly cfg: PasskeyOracleConfig;
  private readonly credentials: CredentialsContainerLike;
  private readonly databaseName: string;

  // In-memory cache populated by enroll() / load(). Cleared by reset().
  private credentialId: ArrayBuffer | null = null;
  private cachedPubkey: Uint8Array | null = null;
  private cachedUserId: string | null = null;

  constructor(cfg: PasskeyOracleConfig) {
    this.cfg = cfg;
    const credentials = cfg.credentials ?? this.resolveCredentials();
    if (!credentials) {
      throw AnimaError.state(
        "PasskeyOracle: no WebAuthn API available — pass `credentials` config or run in a browser",
      );
    }
    this.credentials = credentials;

    const idb = cfg.indexedDB ?? this.resolveIndexedDB();
    if (!idb) {
      throw AnimaError.state(
        "PasskeyOracle: no IndexedDB available — pass `indexedDB` config (e.g. fake-indexeddb)",
      );
    }
    // Set the global IDB factory so `idb`'s `openDB()` picks it up.
    // Tests use `fake-indexeddb` which is registered as the global
    // before any oracle is constructed.
    this.databaseName = cfg.databaseName ?? PASSKEY_DB_NAME;
  }

  /**
   * First-time enrollment. Calls `navigator.credentials.create` with
   * alg = -7 (ES256), parses the resulting attestation object to
   * extract the COSE_Key public key, persists `(credentialId, pubkey)`
   * to IndexedDB, and returns the raw bits for the caller to forward
   * to `RemoteAnimaClient.enrollPasskey`.
   *
   * @param userId Caller-supplied user id (Clerk user id, etc.).
   * @param displayName Human-readable name shown in the OS auth UI.
   * @param challenge 32-byte challenge from lifegw's enrollment flow
   *                  (typically a random nonce).
   */
  async enroll(
    userId: string,
    displayName: string,
    challenge: Uint8Array,
  ): Promise<PasskeyEnrollResult> {
    if (challenge.byteLength < 16) {
      throw AnimaError.state(`enroll challenge must be ≥ 16 bytes, got ${challenge.byteLength}`);
    }

    let credential: PublicKeyCredentialLike | null;
    try {
      credential = await this.credentials.create({
        publicKey: {
          challenge: cloneBuffer(challenge),
          rp: { id: this.cfg.rpId, name: this.cfg.rpName },
          user: {
            id: new TextEncoder().encode(userId),
            name: userId,
            displayName,
          },
          pubKeyCredParams: [{ type: "public-key", alg: -7 /* ES256 */ }],
          authenticatorSelection: {
            // Resident key + user verification — passkey defaults.
            residentKey: "preferred",
            userVerification: "required",
          },
          attestation: "none",
          timeout: 60_000,
        },
      });
    } catch (err) {
      throw AnimaError.passkey(
        `WebAuthn create() failed: ${(err as Error).message}`,
        err,
      );
    }
    if (!credential) {
      throw AnimaError.passkey("WebAuthn create() returned null");
    }

    const response = credential.response;
    if (!isAttestationResponse(response)) {
      throw AnimaError.passkey("expected attestation response, got assertion shape");
    }

    const attestationBytes = new Uint8Array(response.attestationObject);
    const clientDataBytes = new Uint8Array(response.clientDataJSON);
    const pubkey = parsePubkeyFromAttestation(attestationBytes);

    // Persist for future sessions.
    await this.persist({
      userId,
      credentialId: cloneBuffer(credential.rawId),
      pubkey,
      rpId: this.cfg.rpId,
      createdAt: new Date().toISOString(),
    });

    // Hot path: cache in memory.
    this.credentialId = cloneBuffer(credential.rawId);
    this.cachedPubkey = pubkey;
    this.cachedUserId = userId;

    return {
      credentialId: cloneBuffer(credential.rawId),
      attestationObject: attestationBytes,
      clientDataJson: clientDataBytes,
      pubkey,
    };
  }

  /**
   * Subsequent sessions. Loads the cached `(credentialId, pubkey)`
   * from IndexedDB and primes the in-memory cache.
   *
   * Returns `null` (NOT throws) if the user hasn't enrolled yet. The
   * caller decides whether to call `enroll()` automatically or to ask
   * the user to confirm enrollment (recommended).
   */
  async load(userId: string): Promise<PasskeyLoadResult | null> {
    const stored = await this.read(userId);
    if (!stored) return null;
    if (stored.rpId !== this.cfg.rpId) {
      throw AnimaError.state(
        `passkey was enrolled against rpId="${stored.rpId}" but current oracle uses "${this.cfg.rpId}"`,
      );
    }
    this.credentialId = cloneBuffer(stored.credentialId);
    this.cachedPubkey = stored.pubkey;
    this.cachedUserId = userId;
    return {
      credentialId: cloneBuffer(stored.credentialId),
      pubkey: stored.pubkey,
    };
  }

  /**
   * Sign a 32-byte digest. Returns the IEEE-P1363 64-byte form (`r||s`,
   * NO recovery byte) — same shape as Rust's
   * `EcdsaP256Identity::sign_digest`.
   *
   * The challenge passed to `navigator.credentials.get` is the digest
   * itself. The authenticator returns a DER-encoded ECDSA signature
   * over `SHA-256(authenticatorData || SHA-256(clientDataJSON))`. This
   * method returns the JOSE-compact form (raw 64 bytes) which
   * downstream `verify_jws_with_pubkey` consumers expect.
   *
   * @param digest 32-byte challenge to sign.
   */
  async sign(digest: Uint8Array): Promise<Uint8Array> {
    if (this.credentialId === null) {
      throw AnimaError.state(
        "PasskeyOracle.sign: no credential loaded — call enroll() or load() first",
      );
    }
    if (digest.byteLength !== 32) {
      throw AnimaError.state(`sign digest must be 32 bytes, got ${digest.byteLength}`);
    }

    let assertion: PublicKeyCredentialLike | null;
    try {
      assertion = await this.credentials.get({
        publicKey: {
          challenge: cloneBuffer(digest),
          rpId: this.cfg.rpId,
          allowCredentials: [
            {
              id: cloneBuffer(this.credentialId),
              type: "public-key",
            },
          ],
          userVerification: "required",
          timeout: 60_000,
        },
      });
    } catch (err) {
      throw AnimaError.passkey(
        `WebAuthn get() failed: ${(err as Error).message}`,
        err,
      );
    }
    if (!assertion) {
      throw AnimaError.passkey("WebAuthn get() returned null");
    }

    const response = assertion.response;
    if (!isAssertionResponse(response)) {
      throw AnimaError.passkey("expected assertion response, got attestation shape");
    }

    return derSignatureToJoseRaw(new Uint8Array(response.signature));
  }

  /**
   * Sign a digest and return the full WebAuthn assertion bundle. Used
   * by `mintSessionCap` flows where lifegw needs to verify the
   * signature against `clientDataJSON` + `authenticatorData` rather
   * than the raw digest.
   */
  async signWithAssertion(digest: Uint8Array): Promise<{
    signature: Uint8Array;
    clientDataJson: Uint8Array;
    authenticatorData: Uint8Array;
    credentialId: ArrayBuffer;
  }> {
    if (this.credentialId === null) {
      throw AnimaError.state(
        "PasskeyOracle.sign: no credential loaded — call enroll() or load() first",
      );
    }
    if (digest.byteLength !== 32) {
      throw AnimaError.state(`sign digest must be 32 bytes, got ${digest.byteLength}`);
    }

    let assertion: PublicKeyCredentialLike | null;
    try {
      assertion = await this.credentials.get({
        publicKey: {
          challenge: cloneBuffer(digest),
          rpId: this.cfg.rpId,
          allowCredentials: [
            {
              id: cloneBuffer(this.credentialId),
              type: "public-key",
            },
          ],
          userVerification: "required",
          timeout: 60_000,
        },
      });
    } catch (err) {
      throw AnimaError.passkey(
        `WebAuthn get() failed: ${(err as Error).message}`,
        err,
      );
    }
    if (!assertion) {
      throw AnimaError.passkey("WebAuthn get() returned null");
    }

    const response = assertion.response;
    if (!isAssertionResponse(response)) {
      throw AnimaError.passkey("expected assertion response, got attestation shape");
    }

    return {
      signature: derSignatureToJoseRaw(new Uint8Array(response.signature)),
      clientDataJson: new Uint8Array(response.clientDataJSON),
      authenticatorData: new Uint8Array(response.authenticatorData),
      credentialId: cloneBuffer(assertion.rawId),
    };
  }

  /**
   * Returns the SEC1-compressed P-256 pubkey loaded for the active
   * credential.
   *
   * @throws if no credential has been enrolled or loaded yet.
   */
  pubkey(): Uint8Array {
    if (this.cachedPubkey === null) {
      throw AnimaError.state(
        "PasskeyOracle.pubkey: no credential loaded — call enroll() or load() first",
      );
    }
    return this.cachedPubkey;
  }

  /**
   * The user id this oracle is bound to (set by `enroll` / `load`).
   *
   * @throws if no credential has been loaded yet.
   */
  userId(): string {
    if (this.cachedUserId === null) {
      throw AnimaError.state("PasskeyOracle.userId: no credential loaded");
    }
    return this.cachedUserId;
  }

  /**
   * Drop the in-memory cache. Useful when the user logs out — the
   * IndexedDB record persists so re-login picks the same credential.
   */
  reset(): void {
    this.credentialId = null;
    this.cachedPubkey = null;
    this.cachedUserId = null;
  }

  /**
   * Forget the credential entirely (in-memory + IndexedDB). After
   * calling this the user must `enroll()` again on next login.
   */
  async forget(userId: string): Promise<void> {
    const db = await this.openDb();
    try {
      const tx = db.transaction(PASSKEY_STORE, "readwrite");
      await tx.store.delete(userId);
      await tx.done;
    } finally {
      db.close();
    }
    if (this.cachedUserId === userId) this.reset();
  }

  // ── IndexedDB helpers ────────────────────────────────────────────

  private async openDb(): Promise<IDBPDatabase<PasskeyDb>> {
    const storeName = PASSKEY_STORE;
    return openDB<PasskeyDb>(this.databaseName, PASSKEY_DB_VERSION, {
      upgrade(db) {
        if (!db.objectStoreNames.contains(storeName)) {
          db.createObjectStore(storeName, { keyPath: "userId" });
        }
      },
      // openDB picks up the configured factory if one is registered as
      // the global IDB; our test path injects via `globalThis.indexedDB`.
    });
  }

  private async persist(record: StoredCredential): Promise<void> {
    const db = await this.openDb();
    try {
      const tx = db.transaction(PASSKEY_STORE, "readwrite");
      await tx.store.put(record);
      await tx.done;
    } finally {
      db.close();
    }
  }

  private async read(userId: string): Promise<StoredCredential | null> {
    const db = await this.openDb();
    try {
      const tx = db.transaction(PASSKEY_STORE, "readonly");
      const got = await tx.store.get(userId);
      await tx.done;
      return (got as StoredCredential | undefined) ?? null;
    } finally {
      db.close();
    }
  }

  private resolveCredentials(): CredentialsContainerLike | null {
    const nav = (globalThis as { navigator?: { credentials?: CredentialsContainerLike } })
      .navigator;
    return nav?.credentials ?? null;
  }

  private resolveIndexedDB(): IDBFactory | null {
    const g = globalThis as unknown as { indexedDB?: IDBFactory };
    return g.indexedDB ?? null;
  }
}

interface PasskeyDb {
  [PASSKEY_STORE]: {
    key: string;
    value: StoredCredential;
  };
}

// ── COSE_Key parser ───────────────────────────────────────────────────

/**
 * Parse a SEC1-compressed P-256 public key out of a WebAuthn
 * attestation object.
 *
 * The object shape (CBOR):
 *   { fmt: <text>, attStmt: <map>, authData: <bytes> }
 *
 * `authData` is bit-flagged binary:
 *   [rpIdHash 32][flags 1][signCount 4][attestedCredentialData?][extensions?]
 *
 * `attestedCredentialData` (present iff flags bit 6 is set):
 *   [aaguid 16][credentialIdLen 2 BE][credentialId][credentialPublicKey CBOR]
 *
 * `credentialPublicKey` is COSE_Key:
 *   { 1: 2 (kty=EC2), 3: -7 (alg=ES256), -1: 1 (crv=P-256),
 *     -2: <x bytes 32>, -3: <y bytes 32> }
 *
 * We extract `(x, y)` and return the SEC1-compressed encoding:
 *   [(0x02 if y even else 0x03)] || x   (33 bytes)
 *
 * @throws if any field is missing or malformed.
 */
export function parsePubkeyFromAttestation(attestationObject: Uint8Array): Uint8Array {
  let outer: unknown;
  try {
    outer = cborDecode(attestationObject);
  } catch (err) {
    throw AnimaError.crypto(
      `attestationObject CBOR decode failed: ${(err as Error).message}`,
      err,
    );
  }
  const authDataRaw = readAuthData(outer);

  if (authDataRaw.byteLength < 37) {
    throw AnimaError.crypto(
      `authData too short (need 37+ bytes for header, got ${authDataRaw.byteLength})`,
    );
  }
  const flags = authDataRaw[32]!;
  const attestedCredentialDataPresent = (flags & 0x40) !== 0;
  if (!attestedCredentialDataPresent) {
    throw AnimaError.crypto("authData has no attestedCredentialData (AT flag clear)");
  }

  // Skip rpIdHash (32) + flags (1) + signCount (4) = 37 bytes.
  let cursor = 37;
  // attestedCredentialData = aaguid (16) + credentialIdLen (2 BE) + credentialId + credPubKey
  if (authDataRaw.byteLength < cursor + 18) {
    throw AnimaError.crypto("authData too short for attestedCredentialData header");
  }
  cursor += 16; // skip aaguid
  const credIdLen = (authDataRaw[cursor]! << 8) | authDataRaw[cursor + 1]!;
  cursor += 2;
  if (authDataRaw.byteLength < cursor + credIdLen) {
    throw AnimaError.crypto(
      `authData short for credentialId (need ${credIdLen}, have ${authDataRaw.byteLength - cursor})`,
    );
  }
  cursor += credIdLen; // skip credentialId

  // Remaining bytes are the COSE_Key CBOR.
  const coseBytes = authDataRaw.slice(cursor);
  let cose: unknown;
  try {
    cose = cborDecode(coseBytes);
  } catch (err) {
    throw AnimaError.crypto(
      `COSE_Key CBOR decode failed: ${(err as Error).message}`,
      err,
    );
  }
  return coseKeyToSec1(cose);
}

/** Read `authData` from a CBOR-decoded attestationObject. */
function readAuthData(decoded: unknown): Uint8Array {
  if (!isCborMap(decoded)) {
    throw AnimaError.crypto("attestationObject is not a CBOR map");
  }
  const auth = mapGet(decoded, "authData");
  if (auth === undefined) {
    throw AnimaError.crypto("attestationObject missing authData");
  }
  if (auth instanceof Uint8Array) return auth;
  if (auth instanceof ArrayBuffer) return new Uint8Array(auth);
  if (Array.isArray(auth)) return new Uint8Array(auth as number[]);
  throw AnimaError.crypto(`authData has unexpected type: ${typeof auth}`);
}

/** Convert a COSE_Key map to SEC1 compressed P-256 bytes. */
function coseKeyToSec1(cose: unknown): Uint8Array {
  if (!isCborMap(cose)) {
    throw AnimaError.crypto("credentialPublicKey is not a CBOR map");
  }
  // Required: kty == 2 (EC2), alg == -7 (ES256), crv == 1 (P-256).
  const kty = mapGet(cose, 1);
  const alg = mapGet(cose, 3);
  const crv = mapGet(cose, -1);
  const x = mapGet(cose, -2);
  const y = mapGet(cose, -3);

  if (kty !== 2) {
    throw AnimaError.crypto(`COSE_Key kty must be 2 (EC2), got ${String(kty)}`);
  }
  if (alg !== -7) {
    throw AnimaError.crypto(`COSE_Key alg must be -7 (ES256), got ${String(alg)}`);
  }
  if (crv !== 1) {
    throw AnimaError.crypto(`COSE_Key crv must be 1 (P-256), got ${String(crv)}`);
  }
  const xBytes = coerceBytes(x, "COSE_Key x");
  const yBytes = coerceBytes(y, "COSE_Key y");

  if (xBytes.byteLength !== 32) {
    throw AnimaError.crypto(`COSE_Key x must be 32 bytes, got ${xBytes.byteLength}`);
  }
  if (yBytes.byteLength !== 32) {
    throw AnimaError.crypto(`COSE_Key y must be 32 bytes, got ${yBytes.byteLength}`);
  }

  // SEC1 compressed: 0x02 if y even, 0x03 if y odd.
  const yIsOdd = (yBytes[31]! & 0x01) === 0x01;
  const out = new Uint8Array(33);
  out[0] = yIsOdd ? 0x03 : 0x02;
  out.set(xBytes, 1);
  return out;
}

function coerceBytes(v: unknown, ctx: string): Uint8Array {
  if (v instanceof Uint8Array) return v;
  if (v instanceof ArrayBuffer) return new Uint8Array(v);
  if (Array.isArray(v)) return new Uint8Array(v as number[]);
  throw AnimaError.crypto(`${ctx} unexpected type: ${typeof v}`);
}

/** True if the value is a Map (cbor-x default for canonical maps) or a plain object. */
function isCborMap(v: unknown): v is Map<unknown, unknown> | Record<string | number, unknown> {
  return v instanceof Map || (typeof v === "object" && v !== null);
}

/** Read a value from either a Map or plain-object CBOR-decoded shape. */
function mapGet(m: Map<unknown, unknown> | Record<string | number, unknown>, key: unknown): unknown {
  if (m instanceof Map) return m.get(key);
  return (m as Record<string | number, unknown>)[key as string | number];
}

// ── DER → JOSE-compact signature conversion ───────────────────────────

/**
 * Convert a DER-encoded ECDSA signature to JOSE-compact form
 * (raw 64 bytes: r || s, each padded to 32 bytes big-endian, stripped
 * of leading zero bytes inserted by ASN.1 INTEGER encoding).
 *
 * DER shape (RFC 3279):
 *   30 <total_len> 02 <r_len> <r_bytes> 02 <s_len> <s_bytes>
 *
 * This is the inverse of what `p256::ecdsa::Signature::to_bytes`
 * returns from the Rust side.
 */
export function derSignatureToJoseRaw(der: Uint8Array): Uint8Array {
  if (der.byteLength < 8 || der[0] !== 0x30) {
    throw AnimaError.crypto(
      `DER signature: invalid SEQUENCE tag at offset 0 (got 0x${der[0]?.toString(16) ?? "??"})`,
    );
  }
  let cursor = 1;
  // Total length — handle short or long-form. For ECDSA P-256 sigs
  // the total payload is < 128 bytes so short-form is universal, but
  // we allow long-form to be safe.
  cursor = skipDerLength(der, cursor).cursor;

  // r INTEGER
  if (der[cursor] !== 0x02) {
    throw AnimaError.crypto(
      `DER signature: expected INTEGER tag for r at offset ${cursor}`,
    );
  }
  cursor++;
  const rLen = readDerLength(der, cursor);
  cursor = rLen.cursor;
  let rBytes = der.slice(cursor, cursor + rLen.length);
  cursor += rLen.length;

  // s INTEGER
  if (der[cursor] !== 0x02) {
    throw AnimaError.crypto(
      `DER signature: expected INTEGER tag for s at offset ${cursor}`,
    );
  }
  cursor++;
  const sLen = readDerLength(der, cursor);
  cursor = sLen.cursor;
  let sBytes = der.slice(cursor, cursor + sLen.length);

  // ASN.1 INTEGER may have a leading 0x00 byte to disambiguate sign;
  // strip it for raw form.
  const rTrimmed = stripLeadingZero(rBytes);
  const sTrimmed = stripLeadingZero(sBytes);

  if (rTrimmed.byteLength > 32 || sTrimmed.byteLength > 32) {
    throw AnimaError.crypto(
      `DER signature: r/s longer than 32 bytes after sign-strip (r=${rTrimmed.byteLength}, s=${sTrimmed.byteLength})`,
    );
  }

  // Pad to 32 bytes big-endian.
  const out = new Uint8Array(64);
  out.set(rTrimmed, 32 - rTrimmed.byteLength);
  out.set(sTrimmed, 64 - sTrimmed.byteLength);
  return out;
}

function stripLeadingZero(b: Uint8Array): Uint8Array {
  if (b.byteLength > 0 && b[0] === 0x00) {
    const tail = new Uint8Array(b.byteLength - 1);
    tail.set(b.subarray(1));
    return tail;
  }
  return b;
}

function readDerLength(buf: Uint8Array, cursor: number): { cursor: number; length: number } {
  const first = buf[cursor];
  if (first === undefined) {
    throw AnimaError.crypto(`DER length: unexpected EOF at offset ${cursor}`);
  }
  if ((first & 0x80) === 0) {
    return { cursor: cursor + 1, length: first };
  }
  const numBytes = first & 0x7f;
  if (numBytes > 4) {
    throw AnimaError.crypto(`DER length: long-form too wide (${numBytes} bytes)`);
  }
  let length = 0;
  for (let i = 0; i < numBytes; i++) {
    const b = buf[cursor + 1 + i];
    if (b === undefined) {
      throw AnimaError.crypto(`DER length: unexpected EOF in long-form at offset ${cursor + 1 + i}`);
    }
    length = (length << 8) | b;
  }
  return { cursor: cursor + 1 + numBytes, length };
}

function skipDerLength(buf: Uint8Array, cursor: number): { cursor: number; length: number } {
  return readDerLength(buf, cursor);
}

// ── ArrayBuffer/Uint8Array helper ─────────────────────────────────────

function cloneBuffer(input: ArrayBuffer | Uint8Array): ArrayBuffer {
  if (input instanceof ArrayBuffer) {
    return input.slice(0);
  }
  // Uint8Array — copy into a fresh ArrayBuffer of the exact byte length.
  const out = new ArrayBuffer(input.byteLength);
  new Uint8Array(out).set(input);
  return out;
}
