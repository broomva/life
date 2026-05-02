/**
 * `WebCryptoAnima` composition tests — exercise the split-custody
 * flow with mocked passkey + mocked RemoteAnima.
 */

import "fake-indexeddb/auto";
import { describe, expect, it } from "vitest";

import { generateDidKeyP256 } from "../../src/anima/did.js";
import { AnimaError } from "../../src/anima/errors.js";
import { PasskeyOracle } from "../../src/anima/passkey.js";
import { RemoteAnimaClient } from "../../src/anima/remote.js";
import { SessionCap } from "../../src/anima/session_cap.js";
import {
  enrollWebCryptoAnima,
  loadWebCryptoAnima,
  WebCryptoAnima,
} from "../../src/anima/web_crypto.js";
import { MockPasskeyAuthenticator } from "./_mock_authenticator.js";

let testCounter = 0;
function uniqueDb(): string {
  testCounter += 1;
  return `web-crypto-test-${testCounter}`;
}

interface RecordedRequest {
  url: string;
  method: string;
  body: unknown;
}

function makeFakeFetch(
  routes: Array<{
    method: string;
    path: string;
    handler: (req: RecordedRequest) => unknown;
  }>,
): { fetchFn: typeof fetch; calls: RecordedRequest[] } {
  const calls: RecordedRequest[] = [];
  const fetchFn: typeof fetch = async (input, init) => {
    const url =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.toString()
          : (input as Request).url;
    const path = new URL(url).pathname;
    const method = (init?.method ?? "GET").toUpperCase();
    let body: unknown = undefined;
    if (init?.body) {
      const text =
        typeof init.body === "string"
          ? init.body
          : new TextDecoder().decode(init.body as Uint8Array);
      body = text ? JSON.parse(text) : undefined;
    }
    const recorded: RecordedRequest = { url, method, body };
    calls.push(recorded);
    const route = routes.find((r) => r.method === method && r.path === path);
    if (!route) {
      return new Response("not found", { status: 404 });
    }
    return new Response(JSON.stringify(route.handler(recorded)), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  };
  return { fetchFn, calls };
}

function bytesToBase64(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]!);
  return btoa(bin);
}

function makeWalletPubkey(): Uint8Array {
  const out = new Uint8Array(33);
  out[0] = 0x02;
  out[1] = 0xaa;
  return out;
}

describe("WebCryptoAnima — basic shape", () => {
  it("exposes user_did derived from authPubkey", () => {
    const auth = new MockPasskeyAuthenticator();
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: uniqueDb(),
    });

    const fakePubkey = new Uint8Array(33);
    fakePubkey[0] = 0x02;
    fakePubkey[5] = 0xff;
    // Inject the cached pubkey via the enroll path so authPubkey() works.
    auth.credentialIdFactory = () => new Uint8Array(16);

    return passkey.enroll("u-1", "U", new Uint8Array(32)).then(() => {
      const remote = new RemoteAnimaClient({
        baseUrl: "https://gw.test",
        getToken: async () => "tok",
      });
      const cap = new SessionCap({
        userId: "u-1",
        passkey,
        remote,
        now: () => 0,
        randomChallenge: () => new Uint8Array(32),
      });
      const handle = new WebCryptoAnima({
        auth: passkey,
        wallet: remote,
        sessionCap: cap,
        userId: "u-1",
        walletAddress: "0xabcdef",
        walletPubkey: makeWalletPubkey(),
      });

      expect(handle.userId()).toBe("u-1");
      expect(handle.walletAddress()).toBe("0xabcdef");
      expect(handle.backendKind()).toBe("web_crypto");
      expect(handle.authPubkey().byteLength).toBe(33);
      expect(handle.userDid()).toMatch(/^did:key:zDn/);
      expect(handle.userDid()).toBe(generateDidKeyP256(handle.authPubkey()));
    });
  });

  it("rejects construction with wrong-length walletPubkey", async () => {
    const auth = new MockPasskeyAuthenticator();
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: uniqueDb(),
    });
    await passkey.enroll("u-1", "U", new Uint8Array(32));
    const remote = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
    });
    const cap = new SessionCap({
      userId: "u-1",
      passkey,
      remote,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    expect(
      () =>
        new WebCryptoAnima({
          auth: passkey,
          wallet: remote,
          sessionCap: cap,
          userId: "u-1",
          walletAddress: "0xabc",
          walletPubkey: new Uint8Array(32),
        }),
    ).toThrow(/33 bytes/);
  });
});

describe("WebCryptoAnima.signJws", () => {
  it("produces a 3-part compact JWS with ES256 header", async () => {
    const auth = new MockPasskeyAuthenticator();
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: uniqueDb(),
    });
    await passkey.enroll("u-1", "U", new Uint8Array(32));

    const remote = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
    });
    const cap = new SessionCap({
      userId: "u-1",
      passkey,
      remote,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    const handle = new WebCryptoAnima({
      auth: passkey,
      wallet: remote,
      sessionCap: cap,
      userId: "u-1",
      walletAddress: "0xabcdef",
      walletPubkey: makeWalletPubkey(),
    });

    const jws = await handle.signJws({}, { sub: "u-1", iss: "broomva", iat: 0 });
    const parts = jws.split(".");
    expect(parts).toHaveLength(3);

    // Decode header — should declare ES256 + JWT typ + a `did:key:zDn…` kid.
    const headerJson = atob(parts[0]!.replace(/-/g, "+").replace(/_/g, "/").padEnd(parts[0]!.length + ((4 - (parts[0]!.length % 4)) % 4), "="));
    const header = JSON.parse(headerJson);
    expect(header.alg).toBe("ES256");
    expect(header.typ).toBe("JWT");
    expect(header.kid).toMatch(/^did:key:zDn/);

    // Decode body — should contain our payload claims.
    const bodyJson = atob(parts[1]!.replace(/-/g, "+").replace(/_/g, "/").padEnd(parts[1]!.length + ((4 - (parts[1]!.length % 4)) % 4), "="));
    const body = JSON.parse(bodyJson);
    expect(body.sub).toBe("u-1");
    expect(body.iss).toBe("broomva");
  });
});

describe("WebCryptoAnima.signDigest", () => {
  it("returns 64-byte raw signature from passkey", async () => {
    const auth = new MockPasskeyAuthenticator();
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: uniqueDb(),
    });
    await passkey.enroll("u-1", "U", new Uint8Array(32));

    const remote = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
    });
    const cap = new SessionCap({
      userId: "u-1",
      passkey,
      remote,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    const handle = new WebCryptoAnima({
      auth: passkey,
      wallet: remote,
      sessionCap: cap,
      userId: "u-1",
      walletAddress: "0xabc",
      walletPubkey: makeWalletPubkey(),
    });

    const sig = await handle.signDigest(new Uint8Array(32));
    expect(sig.byteLength).toBe(64);
  });

  it("rejects digest of wrong length", async () => {
    const auth = new MockPasskeyAuthenticator();
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: uniqueDb(),
    });
    await passkey.enroll("u-1", "U", new Uint8Array(32));
    const remote = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
    });
    const cap = new SessionCap({
      userId: "u-1",
      passkey,
      remote,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    const handle = new WebCryptoAnima({
      auth: passkey,
      wallet: remote,
      sessionCap: cap,
      userId: "u-1",
      walletAddress: "0xabc",
      walletPubkey: makeWalletPubkey(),
    });
    await expect(handle.signDigest(new Uint8Array(31))).rejects.toThrow(/32 bytes/);
  });
});

describe("WebCryptoAnima.signEvmTx — wallet delegation", () => {
  it("delegates to RemoteAnimaClient.signEvmTx when from matches walletAddress", async () => {
    const auth = new MockPasskeyAuthenticator();
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: uniqueDb(),
    });
    await passkey.enroll("u-1", "U", new Uint8Array(32));

    const sigBytes = new Uint8Array(65);
    sigBytes[64] = 27;
    const { fetchFn, calls } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/sign_evm_tx",
        handler: () => ({ signature_b64: bytesToBase64(sigBytes) }),
      },
    ]);
    const remote = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    const cap = new SessionCap({
      userId: "u-1",
      passkey,
      remote,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    const handle = new WebCryptoAnima({
      auth: passkey,
      wallet: remote,
      sessionCap: cap,
      userId: "u-1",
      walletAddress: "0xWALLET",
      walletPubkey: makeWalletPubkey(),
    });

    const result = await handle.signEvmTx({
      from: "0xwallet", // case-insensitive comparison
      to: "0xrecv",
      valueWei: "1000",
      dataHex: "0x",
      nonce: 0,
      gasLimit: 21000,
      maxFeePerGasWei: "1",
      maxPriorityFeePerGasWei: "1",
      chain: "eip155:8453",
    });
    expect(result.bytes).toEqual(sigBytes);
    expect(calls).toHaveLength(1);
  });

  it("rejects tx.from not matching walletAddress (case-insensitive)", async () => {
    const auth = new MockPasskeyAuthenticator();
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: uniqueDb(),
    });
    await passkey.enroll("u-1", "U", new Uint8Array(32));

    const remote = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
    });
    const cap = new SessionCap({
      userId: "u-1",
      passkey,
      remote,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    const handle = new WebCryptoAnima({
      auth: passkey,
      wallet: remote,
      sessionCap: cap,
      userId: "u-1",
      walletAddress: "0xCORRECT",
      walletPubkey: makeWalletPubkey(),
    });
    await expect(
      handle.signEvmTx({
        from: "0xWRONG",
        to: "0xrecv",
        valueWei: "1",
        dataHex: "0x",
        nonce: 0,
        gasLimit: 21000,
        maxFeePerGasWei: "1",
        maxPriorityFeePerGasWei: "1",
        chain: "eip155:8453",
      }),
    ).rejects.toThrow(/does not match wallet address/);
  });
});

describe("WebCryptoAnima.signEip712 — wallet delegation", () => {
  it("forwards the typed-data payload to RemoteAnimaClient.signEip712", async () => {
    const auth = new MockPasskeyAuthenticator();
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: uniqueDb(),
    });
    await passkey.enroll("u-1", "U", new Uint8Array(32));

    const sigBytes = new Uint8Array(65);
    const { fetchFn, calls } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/sign_eip712",
        handler: () => ({ signature_b64: bytesToBase64(sigBytes) }),
      },
    ]);
    const remote = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    const cap = new SessionCap({
      userId: "u-1",
      passkey,
      remote,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    const handle = new WebCryptoAnima({
      auth: passkey,
      wallet: remote,
      sessionCap: cap,
      userId: "u-1",
      walletAddress: "0xwallet",
      walletPubkey: makeWalletPubkey(),
    });

    await handle.signEip712(
      {
        name: "USD Coin",
        version: "2",
        chainId: "8453",
        verifyingContract: "0xUSDC",
      },
      { TransferWithAuthorization: [] },
      { from: "0xwallet", value: "100" },
    );
    expect(calls).toHaveLength(1);
  });
});

describe("WebCryptoAnima.rotate", () => {
  it("rejects with not_supported error", async () => {
    const auth = new MockPasskeyAuthenticator();
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: uniqueDb(),
    });
    await passkey.enroll("u-1", "U", new Uint8Array(32));
    const remote = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
    });
    const cap = new SessionCap({
      userId: "u-1",
      passkey,
      remote,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    const handle = new WebCryptoAnima({
      auth: passkey,
      wallet: remote,
      sessionCap: cap,
      userId: "u-1",
      walletAddress: "0xabc",
      walletPubkey: makeWalletPubkey(),
    });

    let caught: unknown;
    try {
      await handle.rotate();
    } catch (err) {
      caught = err;
    }
    expect(caught).toBeInstanceOf(AnimaError);
    expect((caught as AnimaError).code).toBe("not_supported");
  });
});

describe("enrollWebCryptoAnima — full flow", () => {
  it("calls passkey.enroll then remote.enrollPasskey and returns a configured handle", async () => {
    const auth = new MockPasskeyAuthenticator();
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: uniqueDb(),
    });

    const walletPk = makeWalletPubkey();
    const { fetchFn, calls } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/enroll_passkey",
        handler: () => ({
          user_did: "did:key:zDnSERVER",
          wallet_address: "0xWALLET",
          wallet_pubkey_b64: bytesToBase64(walletPk),
        }),
      },
    ]);
    const remote = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    const cap = new SessionCap({
      userId: "u-1",
      passkey,
      remote,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    const handle = await enrollWebCryptoAnima({
      passkey,
      remote,
      sessionCap: cap,
      userId: "u-1",
      displayName: "Test User",
      challenge: new Uint8Array(32),
    });
    expect(handle.walletAddress()).toBe("0xWALLET");
    expect(handle.walletPubkey()).toEqual(walletPk);
    expect(calls).toHaveLength(1);
  });
});

describe("loadWebCryptoAnima — load existing", () => {
  it("returns null when not enrolled", async () => {
    const auth = new MockPasskeyAuthenticator();
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: uniqueDb(),
    });
    const remote = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
    });
    const cap = new SessionCap({
      userId: "u-never",
      passkey,
      remote,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    const handle = await loadWebCryptoAnima({
      passkey,
      remote,
      sessionCap: cap,
      userId: "u-never",
    });
    expect(handle).toBeNull();
  });

  it("loads stored passkey + fetches wallet info from remote", async () => {
    const auth = new MockPasskeyAuthenticator();
    testCounter += 1;
    const dbName = `load-test-${testCounter}`;
    const passkey = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: dbName,
    });
    await passkey.enroll("u-load", "U", new Uint8Array(32));
    passkey.reset();

    const walletPk = makeWalletPubkey();
    const { fetchFn } = makeFakeFetch([
      {
        method: "GET",
        path: "/anima/custody/wallet_address/u-load",
        handler: () => ({ address: "0xCACHED" }),
      },
      {
        method: "GET",
        path: "/anima/custody/get_wallet_pubkey/u-load",
        handler: () => ({ pubkey_b64: bytesToBase64(walletPk) }),
      },
    ]);
    const remote = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });

    // Use a fresh oracle with same DB to validate the load path.
    const passkey2 = new PasskeyOracle({
      rpId: "broomva.test",
      rpName: "Broomva Test",
      credentials: auth,
      indexedDB: globalThis.indexedDB,
      databaseName: dbName,
    });
    const cap = new SessionCap({
      userId: "u-load",
      passkey: passkey2,
      remote,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    const handle = await loadWebCryptoAnima({
      passkey: passkey2,
      remote,
      sessionCap: cap,
      userId: "u-load",
    });
    expect(handle).not.toBeNull();
    expect(handle?.walletAddress()).toBe("0xCACHED");
    expect(handle?.walletPubkey()).toEqual(walletPk);
  });
});
