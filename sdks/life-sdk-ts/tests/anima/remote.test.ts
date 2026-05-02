/**
 * `RemoteAnimaClient` unit tests — verify the HTTP wire shapes against
 * the routes Stream R is shipping in parallel.
 *
 * Mock pattern: rather than pulling msw's full request-interception
 * machinery (which adds ~MB of test deps), we inject a deterministic
 * `fetch` shim that records every call. The wire-shape contract is
 * documented inline.
 */

import { describe, expect, it } from "vitest";
import { AnimaError } from "../../src/anima/errors.js";
import { RemoteAnimaClient } from "../../src/anima/remote.js";

interface RecordedRequest {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: unknown;
}

function makeFakeFetch(
  routes: Array<{
    method: string;
    path: string;
    handler: (req: RecordedRequest) => { status?: number; json?: unknown };
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
    const headers: Record<string, string> = {};
    if (init?.headers) {
      for (const [k, v] of Object.entries(init.headers as Record<string, string>)) {
        headers[k.toLowerCase()] = v;
      }
    }
    let body: unknown = undefined;
    if (init?.body) {
      const text =
        typeof init.body === "string"
          ? init.body
          : new TextDecoder().decode(init.body as Uint8Array);
      body = text ? JSON.parse(text) : undefined;
    }
    const recorded: RecordedRequest = { url, method, headers, body };
    calls.push(recorded);

    const route = routes.find((r) => r.method === method && r.path === path);
    if (!route) {
      return new Response(JSON.stringify({ error: `no route for ${method} ${path}` }), {
        status: 404,
        headers: { "Content-Type": "application/json" },
      });
    }
    const result = route.handler(recorded);
    return new Response(JSON.stringify(result.json ?? {}), {
      status: result.status ?? 200,
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

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

describe("RemoteAnimaClient.signAuth", () => {
  it("POSTs digest_b64 + user_id and decodes signature_b64", async () => {
    const expected = new Uint8Array(64);
    expected.fill(0xab);
    const { fetchFn, calls } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/sign_auth",
        handler: (req) => {
          const body = req.body as { user_id: string; digest_b64: string };
          expect(body.user_id).toBe("u-1");
          expect(base64ToBytes(body.digest_b64).byteLength).toBe(32);
          return { json: { signature_b64: bytesToBase64(expected) } };
        },
      },
    ]);

    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "user-cap-token",
      fetch: fetchFn,
    });
    const digest = new Uint8Array(32);
    digest[0] = 1;
    const sig = await client.signAuth("u-1", digest);
    expect(sig).toEqual(expected);

    expect(calls).toHaveLength(1);
    expect(calls[0]?.headers["authorization"]).toBe("Bearer user-cap-token");
    expect(calls[0]?.headers["content-type"]).toBe("application/json");
  });
});

describe("RemoteAnimaClient.signWallet", () => {
  it("POSTs digest_b64 and returns the 65-byte EvmSignature", async () => {
    const expected = new Uint8Array(65);
    expected.fill(0xff);
    const { fetchFn, calls } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/sign_wallet",
        handler: () => ({ json: { signature_b64: bytesToBase64(expected) } }),
      },
    ]);

    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    const sig = await client.signWallet("u-1", new Uint8Array(32));
    expect(sig).toEqual(expected);
    expect(calls[0]?.url).toBe("https://gw.test/anima/custody/sign_wallet");
  });
});

describe("RemoteAnimaClient.signEvmTx", () => {
  it("serialises camelCase fields to snake_case wire shape", async () => {
    const expected = new Uint8Array(65);
    expected[64] = 27; // legacy v
    const { fetchFn, calls } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/sign_evm_tx",
        handler: (req) => {
          const body = req.body as { user_id: string; tx: Record<string, unknown> };
          expect(body.user_id).toBe("u-1");
          expect(body.tx).toEqual({
            from: "0xabc",
            to: "0xdef",
            value_wei: "1000",
            data_hex: "0x",
            nonce: 5,
            gas_limit: 21000,
            max_fee_per_gas_wei: "200",
            max_priority_fee_per_gas_wei: "100",
            chain: "eip155:8453",
          });
          return { json: { signature_b64: bytesToBase64(expected) } };
        },
      },
    ]);

    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    const sig = await client.signEvmTx("u-1", {
      from: "0xabc",
      to: "0xdef",
      valueWei: "1000",
      dataHex: "0x",
      nonce: 5,
      gasLimit: 21000,
      maxFeePerGasWei: "200",
      maxPriorityFeePerGasWei: "100",
      chain: "eip155:8453",
    });
    expect(sig.bytes).toEqual(expected);
    expect(calls).toHaveLength(1);
  });
});

describe("RemoteAnimaClient.signEip712", () => {
  it("serialises domain to snake_case + passes types/message opaquely", async () => {
    const expected = new Uint8Array(65);
    expected[0] = 0xee;
    const { fetchFn } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/sign_eip712",
        handler: (req) => {
          const body = req.body as {
            user_id: string;
            domain: Record<string, unknown>;
            types: Record<string, unknown>;
            message: Record<string, unknown>;
          };
          expect(body.user_id).toBe("u-1");
          expect(body.domain).toEqual({
            name: "USD Coin",
            version: "2",
            chain_id: "8453",
            verifying_contract: "0xUSDC",
          });
          expect(body.types).toEqual({ TransferWithAuthorization: [] });
          expect(body.message).toEqual({ from: "0xabc", value: "100" });
          return { json: { signature_b64: bytesToBase64(expected) } };
        },
      },
    ]);

    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    const sig = await client.signEip712(
      "u-1",
      {
        name: "USD Coin",
        version: "2",
        chainId: "8453",
        verifyingContract: "0xUSDC",
      },
      { TransferWithAuthorization: [] },
      { from: "0xabc", value: "100" },
    );
    expect(sig.bytes).toEqual(expected);
  });
});

describe("RemoteAnimaClient pubkey + address GETs", () => {
  it("getAuthPubkey URL-encodes the user_id path segment", async () => {
    const expected = new Uint8Array(33);
    expected[0] = 0x02;
    const { fetchFn, calls } = makeFakeFetch([
      {
        method: "GET",
        path: "/anima/custody/get_auth_pubkey/u%3A1",
        handler: () => ({ json: { pubkey_b64: bytesToBase64(expected) } }),
      },
    ]);

    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    const pk = await client.getAuthPubkey("u:1");
    expect(pk).toEqual(expected);
    expect(calls[0]?.method).toBe("GET");
  });

  it("getWalletPubkey returns the secp256k1 SEC1-compressed bytes", async () => {
    const expected = new Uint8Array(33);
    expected[0] = 0x03;
    const { fetchFn } = makeFakeFetch([
      {
        method: "GET",
        path: "/anima/custody/get_wallet_pubkey/u-1",
        handler: () => ({ json: { pubkey_b64: bytesToBase64(expected) } }),
      },
    ]);
    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    expect(await client.getWalletPubkey("u-1")).toEqual(expected);
  });

  it("getWalletAddress returns the address string", async () => {
    const { fetchFn } = makeFakeFetch([
      {
        method: "GET",
        path: "/anima/custody/wallet_address/u-1",
        handler: () => ({ json: { address: "0x1234567890abcdef" } }),
      },
    ]);
    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    expect(await client.getWalletAddress("u-1")).toBe("0x1234567890abcdef");
  });
});

describe("RemoteAnimaClient.enrollPasskey", () => {
  it("POSTs base64-encoded attestation bundles + returns DID + wallet info", async () => {
    const walletPk = new Uint8Array(33);
    walletPk[0] = 0x02;
    walletPk[1] = 0xaa;
    const { fetchFn, calls } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/enroll_passkey",
        handler: (req) => {
          const body = req.body as {
            user_id: string;
            attestation_object_b64: string;
            client_data_json_b64: string;
            credential_id_b64: string;
          };
          expect(body.user_id).toBe("u-1");
          expect(base64ToBytes(body.attestation_object_b64).byteLength).toBeGreaterThan(0);
          expect(base64ToBytes(body.credential_id_b64).byteLength).toBe(16);
          return {
            json: {
              user_did: "did:key:zDnNew",
              wallet_address: "0xWALLET",
              wallet_pubkey_b64: bytesToBase64(walletPk),
            },
          };
        },
      },
    ]);
    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "enroll-tok",
      fetch: fetchFn,
    });
    const result = await client.enrollPasskey("u-1", {
      attestationObject: new Uint8Array([1, 2, 3, 4, 5]),
      clientDataJson: new Uint8Array([6, 7, 8]),
      credentialId: new ArrayBuffer(16),
    });
    expect(result.userDid).toBe("did:key:zDnNew");
    expect(result.walletAddress).toBe("0xWALLET");
    expect(result.walletPubkey).toEqual(walletPk);
    expect(calls).toHaveLength(1);
  });
});

describe("RemoteAnimaClient.mintSessionCap", () => {
  it("POSTs assertion bundle and returns expiry/issued metadata", async () => {
    const { fetchFn, calls } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/mint_session_cap",
        handler: (req) => {
          const body = req.body as {
            user_id: string;
            assertion: Record<string, string>;
          };
          expect(body.user_id).toBe("u-1");
          expect(body.assertion["signature_b64"]).toBeTruthy();
          expect(body.assertion["client_data_json_b64"]).toBeTruthy();
          expect(body.assertion["authenticator_data_b64"]).toBeTruthy();
          expect(body.assertion["credential_id_b64"]).toBeTruthy();
          return {
            json: {
              token: "TIER-USER-JWT",
              expires_at: 1_700_000_000,
              issued_at: 1_699_999_100,
            },
          };
        },
      },
    ]);
    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "ignored",
      fetch: fetchFn,
    });
    const result = await client.mintSessionCap(
      "u-1",
      {
        assertion: {
          signature: new Uint8Array(64),
          clientDataJson: new Uint8Array(8),
          authenticatorData: new Uint8Array(40),
          credentialId: new ArrayBuffer(16),
        },
      },
      "explicit-bearer",
    );
    expect(result.token).toBe("TIER-USER-JWT");
    expect(result.expiresAt).toBe(1_700_000_000);
    expect(result.issuedAt).toBe(1_699_999_100);
    // The explicit override takes precedence over `getToken`.
    expect(calls[0]?.headers["authorization"]).toBe("Bearer explicit-bearer");
  });

  it("omits Authorization header when bearerOverride is empty string", async () => {
    const { fetchFn, calls } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/mint_session_cap",
        handler: () => ({
          json: { token: "T", expires_at: 1, issued_at: 0 },
        }),
      },
    ]);
    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "outer-tok",
      fetch: fetchFn,
    });
    await client.mintSessionCap(
      "u-1",
      {
        assertion: {
          signature: new Uint8Array(64),
          clientDataJson: new Uint8Array(8),
          authenticatorData: new Uint8Array(40),
          credentialId: new ArrayBuffer(16),
        },
      },
      "", // explicit empty → unauthed
    );
    expect(calls[0]?.headers["authorization"]).toBeUndefined();
  });
});

describe("RemoteAnimaClient error handling", () => {
  it("throws AnimaError on non-2xx response", async () => {
    const { fetchFn } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/sign_auth",
        handler: () => ({ status: 401, json: { error: "missing token" } }),
      },
    ]);
    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    let caught: unknown;
    try {
      await client.signAuth("u-1", new Uint8Array(32));
    } catch (err) {
      caught = err;
    }
    expect(caught).toBeInstanceOf(AnimaError);
    expect((caught as AnimaError).code).toBe("remote_anima_401");
    expect((caught as AnimaError).message).toContain("missing token");
  });

  it("throws AnimaError on network failure", async () => {
    const fetchFn: typeof fetch = async () => {
      throw new Error("network down");
    };
    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    await expect(client.getWalletAddress("u-1")).rejects.toThrow(/fetch failed/);
  });

  it("normalizes baseUrl trailing slashes", () => {
    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test///",
      getToken: async () => "tok",
    });
    expect(client.baseUrl).toBe("https://gw.test");
  });

  // I-1 review fix: structured `{ code, message }` errors surface
  // those fields only — never the raw body.
  it("surfaces only code+message from structured error bodies (I-1)", async () => {
    const { fetchFn } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/sign_auth",
        handler: () => ({
          status: 403,
          json: {
            code: "tier_user_revoked",
            message: "session cap revoked",
            // Sensitive fields that MUST NOT leak into AnimaError.message.
            jwt: "eyJhbGciOiJFUzI1NiI.dot.dot",
            request_id: "req_abc123",
            upstream_kms_error: "vault: 403 lease expired",
          },
        }),
      },
    ]);
    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    let caught: unknown;
    try {
      await client.signAuth("u-1", new Uint8Array(32));
    } catch (err) {
      caught = err;
    }
    const msg = (caught as AnimaError).message;
    expect(msg).toContain("tier_user_revoked");
    expect(msg).toContain("session cap revoked");
    // Critical: none of these sensitive fields leak.
    expect(msg).not.toContain("eyJhbGciOiJFUzI1NiI");
    expect(msg).not.toContain("req_abc123");
    expect(msg).not.toContain("vault: 403 lease expired");
  });

  // I-1 review fix: long unstructured bodies get truncated to
  // MAX_REMOTE_ERROR_BODY_CHARS (200) + ellipsis. The fake-fetch
  // helper always JSON-encodes responses; an enormous unstructured
  // string lands as a JSON-string body, which sanitizeErrorBody falls
  // through to truncate (no `code`/`message` shape).
  it("truncates oversized error bodies to ~200 chars + ellipsis (I-1)", async () => {
    const huge = "x".repeat(2_000);
    const { fetchFn } = makeFakeFetch([
      {
        method: "POST",
        path: "/anima/custody/sign_auth",
        // `json: huge` serializes to '"xxxx..."' (~2002 chars total)
        handler: () => ({ status: 500, json: huge }),
      },
    ]);
    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    let caught: unknown;
    try {
      await client.signAuth("u-1", new Uint8Array(32));
    } catch (err) {
      caught = err;
    }
    const msg = (caught as AnimaError).message;
    expect(msg.length).toBeLessThan(huge.length);
    expect(msg).toContain("…");
  });

  // I-2 review fix: fetch failures with TimeoutError/AbortError name
  // map to a typed "request timeout" AnimaError.
  it("maps AbortError/TimeoutError to a request-timeout error (I-2)", async () => {
    const fetchFn: typeof fetch = async () => {
      const e = new Error("aborted") as Error & { name: string };
      e.name = "TimeoutError";
      throw e;
    };
    const client = new RemoteAnimaClient({
      baseUrl: "https://gw.test",
      getToken: async () => "tok",
      fetch: fetchFn,
    });
    await expect(client.getAuthPubkey("u-1")).rejects.toThrow(
      /request timeout/,
    );
  });
});
