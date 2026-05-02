/**
 * DID `did:key:zDn…` (P-256) cross-language compatibility tests.
 *
 * Pinned against fixtures generated from Rust's
 * `crates/anima/anima-identity/src/did.rs::generate_did_key_p256`.
 * Any divergence here indicates a multicodec or base58 encoding bug
 * that would break interop with the server-side anima daemon.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { describe, expect, it } from "vitest";

import {
  generateDidKeyP256,
  resolveDidKeyP256,
  verifyDidKeyP256,
} from "../../src/anima/did.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const FIXTURES_PATH = resolve(__dirname, "../fixtures/did_p256_vectors.json");

interface Fixture {
  description: string;
  pubkey_compressed_hex: string;
  expected_did: string;
}

function loadFixtures(): Fixture[] {
  const raw = readFileSync(FIXTURES_PATH, "utf8");
  return JSON.parse(raw) as Fixture[];
}

function hexToBytes(hex: string): Uint8Array {
  if (hex.length % 2 !== 0) throw new Error("hex length not even");
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.byteLength; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

describe("generateDidKeyP256 — cross-language fixtures", () => {
  const fixtures = loadFixtures();

  it.each(fixtures)("$description matches Rust output", (fixture) => {
    const pubkey = hexToBytes(fixture.pubkey_compressed_hex);
    const did = generateDidKeyP256(pubkey);
    expect(did).toBe(fixture.expected_did);
  });

  it("loaded at least 5 fixtures (smoke check)", () => {
    expect(fixtures.length).toBeGreaterThanOrEqual(5);
  });
});

describe("generateDidKeyP256 — input validation", () => {
  it("rejects pubkey of wrong length", () => {
    expect(() => generateDidKeyP256(new Uint8Array(32))).toThrow(/33 bytes/);
    expect(() => generateDidKeyP256(new Uint8Array(34))).toThrow(/33 bytes/);
  });

  it("rejects pubkey not starting with 0x02 or 0x03", () => {
    const bad = new Uint8Array(33);
    bad[0] = 0x04; // uncompressed marker — disallowed for SEC1 compressed
    expect(() => generateDidKeyP256(bad)).toThrow(/0x02 or 0x03/);
  });

  it("accepts both 0x02 and 0x03 parity bytes", () => {
    const k02 = new Uint8Array(33);
    k02[0] = 0x02;
    k02[1] = 0x42;
    const did02 = generateDidKeyP256(k02);
    expect(did02).toMatch(/^did:key:zDn/);

    const k03 = new Uint8Array(33);
    k03[0] = 0x03;
    k03[1] = 0x42;
    const did03 = generateDidKeyP256(k03);
    expect(did03).toMatch(/^did:key:zDn/);

    expect(did02).not.toBe(did03);
  });
});

describe("resolveDidKeyP256 — round trip with generate", () => {
  const fixtures = loadFixtures();

  it.each(fixtures)("$description: resolve recovers original pubkey", (fixture) => {
    const original = hexToBytes(fixture.pubkey_compressed_hex);
    const did = generateDidKeyP256(original);
    const recovered = resolveDidKeyP256(did);
    expect(recovered).toEqual(original);
  });

  it("rejects DIDs with wrong scheme", () => {
    expect(() => resolveDidKeyP256("did:web:example.com")).toThrow(/missing prefix/);
  });

  it("rejects DIDs with wrong multicodec prefix", () => {
    // Use Ed25519 multicodec (0xed01) — should be rejected by the
    // strict P-256 resolver.
    const ed25519Did = "did:key:z6MkpTHR8VNsBxYAAWHut2Geadd9jSshBHqcxz4QEEfwfPQM";
    expect(() => resolveDidKeyP256(ed25519Did)).toThrow(/multicodec prefix/);
  });

  it("rejects malformed base58", () => {
    // 'O' (capital o) is not in the base58 alphabet.
    expect(() => resolveDidKeyP256("did:key:zDnaeOOO")).toThrow(/base58/);
  });
});

describe("verifyDidKeyP256", () => {
  const fixtures = loadFixtures();

  it.each(fixtures)("$description: verify accepts the canonical DID", (fixture) => {
    const pubkey = hexToBytes(fixture.pubkey_compressed_hex);
    expect(verifyDidKeyP256(fixture.expected_did, pubkey)).toBe(true);
  });

  it("verify rejects mismatched pubkey", () => {
    const pubkey = new Uint8Array(33);
    pubkey[0] = 0x02;
    const did = generateDidKeyP256(pubkey);
    const wrongPubkey = new Uint8Array(33);
    wrongPubkey[0] = 0x03; // different parity byte
    expect(verifyDidKeyP256(did, wrongPubkey)).toBe(false);
  });

  it("verify returns false (not throws) on malformed input", () => {
    expect(verifyDidKeyP256("not-a-did", new Uint8Array(33))).toBe(false);
  });
});
