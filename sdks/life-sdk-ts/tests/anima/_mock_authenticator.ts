/**
 * `MockPasskeyAuthenticator` — deterministic WebAuthn authenticator
 * for unit tests.
 *
 * Generates a real P-256 keypair (via `@noble/curves/p256`), produces
 * structurally-valid attestation objects (CBOR with COSE_Key), and
 * signs assertion challenges with real ECDSA. This lets the
 * passkey-oracle tests exercise the full attestation parser + DER →
 * JOSE conversion paths without a real browser.
 */

import { p256 } from "@noble/curves/nist.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { encode as cborEncode } from "cbor-x";
import type {
  AuthenticatorAssertionResponseLike,
  AuthenticatorAttestationResponseLike,
  CredentialsContainerLike,
  PublicKeyCredentialLike,
} from "../../src/anima/passkey.js";

interface RegisteredCredential {
  /** Opaque credential id bytes (we generate random 16-byte ids). */
  credentialId: Uint8Array;
  /** Private scalar (32 bytes BE). */
  priv: Uint8Array;
  /** Public point — extracted x/y as 32-byte BE big-ints. */
  pubX: Uint8Array;
  pubY: Uint8Array;
  /** SEC1 compressed pubkey (33 bytes). */
  compressed: Uint8Array;
  /** RP id this credential was enrolled against. */
  rpId: string;
  /** Sign counter (incremented on every assertion). */
  signCount: number;
}

/**
 * Build a synthetic `attestationObject` CBOR blob containing the
 * given credential's public key. Mirrors WebAuthn's "none" attestation
 * format (the simplest, used by passkey-style enrollments).
 */
function buildAttestationObject(cred: RegisteredCredential): Uint8Array {
  // authData = rpIdHash(32) || flags(1) || signCount(4) || attestedCredentialData
  const rpIdHash = sha256(new TextEncoder().encode(cred.rpId));
  const flags = 0x40 | 0x01 | 0x04; // AT (attested credential data) + UP + UV
  const signCountBE = new Uint8Array(4); // 0
  const aaguid = new Uint8Array(16); // all zeros (generic passkey)
  const credIdLenBE = new Uint8Array(2);
  credIdLenBE[0] = (cred.credentialId.byteLength >> 8) & 0xff;
  credIdLenBE[1] = cred.credentialId.byteLength & 0xff;

  // COSE_Key shape (Map):
  //   1 (kty)  → 2 (EC2)
  //   3 (alg)  → -7 (ES256)
  //   -1 (crv) → 1 (P-256)
  //   -2 (x)   → 32 bytes
  //   -3 (y)   → 32 bytes
  const coseKey = new Map<number, number | Uint8Array>([
    [1, 2],
    [3, -7],
    [-1, 1],
    [-2, cred.pubX],
    [-3, cred.pubY],
  ]);
  const coseBytes = cborEncode(coseKey);

  const authData = concat(
    rpIdHash,
    new Uint8Array([flags]),
    signCountBE,
    aaguid,
    credIdLenBE,
    cred.credentialId,
    coseBytes,
  );

  // attestationObject = { fmt: "none", attStmt: {}, authData: bytes }
  const attestationMap = new Map<string, unknown>([
    ["fmt", "none"],
    ["attStmt", new Map()],
    ["authData", authData],
  ]);
  return cborEncode(attestationMap);
}

function concat(...arrays: Uint8Array[]): Uint8Array {
  const total = arrays.reduce((acc, a) => acc + a.byteLength, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const a of arrays) {
    out.set(a, offset);
    offset += a.byteLength;
  }
  return out;
}

function bytesEq(a: Uint8Array, b: Uint8Array): boolean {
  if (a.byteLength !== b.byteLength) return false;
  for (let i = 0; i < a.byteLength; i++) if (a[i] !== b[i]) return false;
  return true;
}

function arrayBufferToUint8(ab: ArrayBuffer | Uint8Array): Uint8Array {
  if (ab instanceof Uint8Array) return ab;
  return new Uint8Array(ab);
}

/**
 * `MockPasskeyAuthenticator` — implements `CredentialsContainerLike`.
 *
 * Each `create()` call produces a fresh keypair and registers it.
 * `get()` calls look up the matching credential and produce a real
 * ECDSA signature with the registered private key.
 */
export class MockPasskeyAuthenticator implements CredentialsContainerLike {
  readonly registered: RegisteredCredential[] = [];

  /**
   * Last challenge signed — useful for tests that want to verify the
   * authenticator signed exactly the bytes the caller passed in.
   */
  lastChallenge: Uint8Array | null = null;

  /**
   * Hook fired before signing — tests can throw from here to simulate
   * user-cancelled prompts, etc.
   */
  beforeSign?: () => void;

  /**
   * Optional override for credential id generation. Tests can pin
   * this to make IDs deterministic.
   */
  credentialIdFactory?: () => Uint8Array;

  async create(options: {
    publicKey: PublicKeyCredentialCreationOptions;
  }): Promise<PublicKeyCredentialLike | null> {
    const rpId = options.publicKey.rp.id ?? "localhost";
    const credId =
      this.credentialIdFactory?.() ?? randomBytes(16);

    // Generate a fresh P-256 keypair using a random scalar.
    const priv = p256.utils.randomSecretKey();
    const pubUncompressed = p256.getPublicKey(priv, false); // 0x04 || x(32) || y(32)
    if (pubUncompressed.byteLength !== 65) {
      throw new Error(`unexpected uncompressed pubkey length ${pubUncompressed.byteLength}`);
    }
    const pubX = pubUncompressed.slice(1, 33);
    const pubY = pubUncompressed.slice(33, 65);
    const compressed = p256.getPublicKey(priv, true);

    const cred: RegisteredCredential = {
      credentialId: credId,
      priv,
      pubX,
      pubY,
      compressed,
      rpId,
      signCount: 0,
    };
    this.registered.push(cred);

    const attestationObject = buildAttestationObject(cred);
    const clientData = JSON.stringify({
      type: "webauthn.create",
      challenge: bytesToB64Url(arrayBufferToUint8(options.publicKey.challenge as ArrayBuffer)),
      origin: `https://${rpId}`,
    });

    const response: AuthenticatorAttestationResponseLike = {
      clientDataJSON: utf8ToBuffer(clientData),
      attestationObject: bufferOf(attestationObject),
    };
    return {
      rawId: bufferOf(credId),
      response,
    };
  }

  async get(options: {
    publicKey: PublicKeyCredentialRequestOptions;
  }): Promise<PublicKeyCredentialLike | null> {
    if (this.beforeSign) this.beforeSign();

    const rpId = options.publicKey.rpId ?? "localhost";
    const allowed = options.publicKey.allowCredentials ?? [];
    if (allowed.length === 0) {
      throw new Error("MockPasskeyAuthenticator.get: allowCredentials cannot be empty");
    }
    let cred: RegisteredCredential | undefined;
    for (const allow of allowed) {
      const allowId = arrayBufferToUint8(allow.id as ArrayBuffer);
      cred = this.registered.find((c) => c.rpId === rpId && bytesEq(c.credentialId, allowId));
      if (cred) break;
    }
    if (!cred) {
      throw new Error("MockPasskeyAuthenticator.get: no registered credential matches");
    }

    cred.signCount += 1;
    const challengeBytes = arrayBufferToUint8(options.publicKey.challenge as ArrayBuffer);
    this.lastChallenge = challengeBytes;

    // Build authenticator data: rpIdHash || flags || signCount
    const rpIdHash = sha256(new TextEncoder().encode(rpId));
    const flags = 0x01 | 0x04; // UP + UV (no AT, no extensions on assertion)
    const signCountBE = new Uint8Array(4);
    new DataView(signCountBE.buffer).setUint32(0, cred.signCount, false);
    const authenticatorData = concat(rpIdHash, new Uint8Array([flags]), signCountBE);

    // ClientDataJSON
    const clientData = JSON.stringify({
      type: "webauthn.get",
      challenge: bytesToB64Url(challengeBytes),
      origin: `https://${rpId}`,
    });
    const clientDataBytes = new TextEncoder().encode(clientData);

    // WebAuthn signs ECDSA over (authenticatorData || SHA-256(clientDataJSON)).
    // The curve's internal SHA-256 prehash applies to the concatenation;
    // we let `prehash: true` (the default) handle the final SHA-256 step.
    const clientDataHash = sha256(clientDataBytes);
    const signedBytes = concat(authenticatorData, clientDataHash);

    const derSig = p256.sign(signedBytes, cred.priv, {
      prehash: true,
      format: "der",
    });

    const response: AuthenticatorAssertionResponseLike = {
      clientDataJSON: bufferOf(clientDataBytes),
      authenticatorData: bufferOf(authenticatorData),
      signature: bufferOf(derSig),
    };
    return {
      rawId: bufferOf(cred.credentialId),
      response,
    };
  }
}

function randomBytes(n: number): Uint8Array {
  const out = new Uint8Array(n);
  const g = globalThis as unknown as { crypto?: { getRandomValues?: (b: Uint8Array) => Uint8Array } };
  if (g.crypto?.getRandomValues) {
    g.crypto.getRandomValues(out);
    return out;
  }
  // Fallback for ancient environments — pseudo-random is OK for tests.
  for (let i = 0; i < n; i++) out[i] = Math.floor(Math.random() * 256);
  return out;
}

function utf8ToBuffer(s: string): ArrayBuffer {
  const bytes = new TextEncoder().encode(s);
  return bufferOf(bytes);
}

function bufferOf(bytes: Uint8Array): ArrayBuffer {
  // Always copy into a fresh ArrayBuffer to avoid SharedArrayBuffer
  // type issues across runtimes.
  const out = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(out).set(bytes);
  return out;
}

function bytesToB64Url(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.byteLength; i++) bin += String.fromCharCode(bytes[i]!);
  const std = btoa(bin);
  return std.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
