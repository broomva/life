/**
 * `PasskeyOracle` unit tests — exercises COSE_Key parsing, DER → JOSE
 * conversion, IndexedDB persistence, and the load/enroll/sign flows.
 */

import { p256 } from "@noble/curves/nist.js";
import "fake-indexeddb/auto";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { generateDidKeyP256 } from "../../src/anima/did.js";
import {
  derSignatureToJoseRaw,
  parsePubkeyFromAttestation,
  PasskeyOracle,
} from "../../src/anima/passkey.js";
import { MockPasskeyAuthenticator } from "./_mock_authenticator.js";

// Each test gets its own database name to avoid cross-test pollution.
let testCounter = 0;

function makeOracle(authenticator: MockPasskeyAuthenticator): PasskeyOracle {
  testCounter += 1;
  return new PasskeyOracle({
    rpId: "broomva.test",
    rpName: "Broomva Test",
    credentials: authenticator,
    indexedDB: globalThis.indexedDB,
    databaseName: `broomva-anima-passkeys-test-${testCounter}`,
  });
}

describe("PasskeyOracle.enroll", () => {
  it("returns a SEC1-compressed P-256 pubkey from the attestation object", async () => {
    const auth = new MockPasskeyAuthenticator();
    const oracle = makeOracle(auth);
    const challenge = new Uint8Array(32);
    challenge[0] = 0x42;

    const result = await oracle.enroll("user-1", "Test User", challenge);

    expect(result.pubkey.byteLength).toBe(33);
    expect([0x02, 0x03]).toContain(result.pubkey[0]);
    expect(result.attestationObject.byteLength).toBeGreaterThan(50);
    expect(result.clientDataJson.byteLength).toBeGreaterThan(20);
  });

  it("persists credentials so a fresh oracle can load them", async () => {
    const auth = new MockPasskeyAuthenticator();
    testCounter += 1;
    const dbName = `broomva-anima-passkeys-test-${testCounter}`;

    const oracleA = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: dbName,
    });
    const enroll = await oracleA.enroll("user-2", "Test User", new Uint8Array(32));

    // Brand-new oracle pointed at the same DB.
    const oracleB = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: dbName,
    });
    const loaded = await oracleB.load("user-2");
    expect(loaded).not.toBeNull();
    expect(loaded?.pubkey).toEqual(enroll.pubkey);
  });

  it("rejects challenges that are too short", async () => {
    const auth = new MockPasskeyAuthenticator();
    const oracle = makeOracle(auth);
    await expect(oracle.enroll("u", "U", new Uint8Array(8))).rejects.toThrow(
      /≥ 16 bytes/,
    );
  });

  it("propagates passkey-API failures as AnimaError", async () => {
    const auth = new MockPasskeyAuthenticator();
    auth.beforeSign = (): never => {
      throw new Error("user cancelled");
    };
    // Override `create` to also throw — replicates the cancel path.
    const cancelling = {
      ...auth,
      create: async () => {
        throw new Error("the user cancelled the prompt");
      },
      get: auth.get.bind(auth),
    };
    const oracle = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: cancelling,
      indexedDB: globalThis.indexedDB,
      databaseName: `cancel-test-${++testCounter}`,
    });
    await expect(oracle.enroll("u", "U", new Uint8Array(32))).rejects.toThrow(
      /WebAuthn create/,
    );
  });
});

describe("PasskeyOracle.load", () => {
  it("returns null when no credential exists", async () => {
    const auth = new MockPasskeyAuthenticator();
    const oracle = makeOracle(auth);
    const loaded = await oracle.load("never-enrolled");
    expect(loaded).toBeNull();
  });

  it("rejects when stored rpId differs from current oracle rpId", async () => {
    const auth = new MockPasskeyAuthenticator();
    testCounter += 1;
    const dbName = `mismatched-rp-test-${testCounter}`;

    const oracle1 = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: dbName,
    });
    await oracle1.enroll("u", "U", new Uint8Array(32));

    const oracle2 = new PasskeyOracle({
      rpId: "evil.test", // mismatched
      rpName: "Evil",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: dbName,
    });
    await expect(oracle2.load("u")).rejects.toThrow(/rpId/);
  });
});

describe("PasskeyOracle.sign", () => {
  it("signs a 32-byte digest and returns 64-byte JOSE-raw bytes", async () => {
    const auth = new MockPasskeyAuthenticator();
    const oracle = makeOracle(auth);
    await oracle.enroll("u", "U", new Uint8Array(32));

    const digest = new Uint8Array(32);
    digest[0] = 0xab;
    const sig = await oracle.sign(digest);

    expect(sig.byteLength).toBe(64);
    expect(auth.lastChallenge).toEqual(digest);
  });

  it("throws when no credential is loaded", async () => {
    const auth = new MockPasskeyAuthenticator();
    const oracle = makeOracle(auth);
    await expect(oracle.sign(new Uint8Array(32))).rejects.toThrow(/no credential loaded/);
  });

  it("throws on wrong digest length", async () => {
    const auth = new MockPasskeyAuthenticator();
    const oracle = makeOracle(auth);
    await oracle.enroll("u", "U", new Uint8Array(32));
    await expect(oracle.sign(new Uint8Array(31))).rejects.toThrow(/32 bytes/);
  });

  it("signWithAssertion returns the full WebAuthn assertion bundle", async () => {
    const auth = new MockPasskeyAuthenticator();
    const oracle = makeOracle(auth);
    await oracle.enroll("u", "U", new Uint8Array(32));

    const digest = new Uint8Array(32);
    digest[0] = 0xcd;
    const bundle = await oracle.signWithAssertion(digest);

    expect(bundle.signature.byteLength).toBe(64);
    expect(bundle.clientDataJson.byteLength).toBeGreaterThan(0);
    expect(bundle.authenticatorData.byteLength).toBeGreaterThan(36);
  });

  it("derived DID matches generateDidKeyP256(authPubkey)", async () => {
    const auth = new MockPasskeyAuthenticator();
    const oracle = makeOracle(auth);
    const enroll = await oracle.enroll("u", "U", new Uint8Array(32));

    const didFromEnroll = generateDidKeyP256(enroll.pubkey);
    const didFromOracle = generateDidKeyP256(oracle.pubkey());
    expect(didFromOracle).toBe(didFromEnroll);
    expect(didFromOracle).toMatch(/^did:key:zDn/);
  });
});

describe("PasskeyOracle.forget", () => {
  it("removes the credential from IndexedDB", async () => {
    const auth = new MockPasskeyAuthenticator();
    testCounter += 1;
    const dbName = `forget-test-${testCounter}`;
    const oracle = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: dbName,
    });
    await oracle.enroll("u", "U", new Uint8Array(32));
    await oracle.forget("u");

    // A fresh oracle should not find the credential.
    const oracle2 = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: dbName,
    });
    expect(await oracle2.load("u")).toBeNull();
  });
});

describe("parsePubkeyFromAttestation — direct invocation", () => {
  it("extracts the same pubkey the authenticator registered", async () => {
    const auth = new MockPasskeyAuthenticator();
    const oracle = makeOracle(auth);
    const enroll = await oracle.enroll("u", "U", new Uint8Array(32));
    const directParse = parsePubkeyFromAttestation(enroll.attestationObject);
    expect(directParse).toEqual(enroll.pubkey);
  });
});

describe("derSignatureToJoseRaw", () => {
  it("round-trips an ECDSA signature produced by @noble/curves", () => {
    // Generate a real signature and verify the conversion.
    const priv = p256.utils.randomSecretKey();
    const message = new TextEncoder().encode("hello, mock authenticator");
    const der = p256.sign(message, priv, { prehash: true, format: "der" });

    const raw = derSignatureToJoseRaw(der);
    expect(raw.byteLength).toBe(64);

    // Verify we can convert back: the raw form is verifiable by p256.verify
    // with format: "compact".
    const valid = p256.verify(raw, message, p256.getPublicKey(priv, true), {
      prehash: true,
      format: "compact",
    });
    expect(valid).toBe(true);
  });

  it("strips a leading 0x00 from r when present (DER sign-disambiguation)", () => {
    // Build a DER signature with a high-bit-set r (forces the 0x00 sign byte)
    // by signing repeatedly until we see one. The probability is ~50% per try.
    const priv = p256.utils.randomSecretKey();
    let der: Uint8Array | null = null;
    for (let i = 0; i < 50; i++) {
      const msg = new Uint8Array(32);
      msg[0] = i;
      const candidate = p256.sign(msg, priv, { prehash: true, format: "der" });
      // DER format: 30 LL 02 RL <r_bytes> 02 SL <s_bytes>
      // Look for r_len > 32 (leading zero present).
      if (candidate[3]! > 32) {
        der = candidate;
        break;
      }
    }
    if (!der) {
      // Statistically very rare; just assert the basic property.
      console.warn("did not find a leading-zero r in 50 tries; skipping strict check");
      return;
    }
    const raw = derSignatureToJoseRaw(der);
    expect(raw.byteLength).toBe(64);
  });

  it("throws on malformed DER", () => {
    expect(() => derSignatureToJoseRaw(new Uint8Array([0x00]))).toThrow(/SEQUENCE tag/);
  });
});
