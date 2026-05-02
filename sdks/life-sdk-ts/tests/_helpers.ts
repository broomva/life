/**
 * Test helpers — fake `fetch`, fake gateway routing, and a minimal
 * Connect-protocol server-stream encoder.
 */

import { expect } from "vitest";

export interface FakeRoute {
  /**
   * Fully-qualified path — `/<service>/<method>`.
   */
  path: string;
  /**
   * Async handler — receives the parsed JSON request body and the
   * raw `Request`. May return either a JSON-serializable response
   * (unary) or an array of frames for server-streaming.
   */
  handler: (req: { body: unknown; raw: Request }) => Promise<FakeResponse>;
}

export interface FakeResponse {
  status?: number;
  unary?: unknown;
  stream?: Array<{ flag: 0 | 2; body: unknown }>;
  /**
   * Map of HTTP headers to assert in the test body.
   */
  recordedHeaders?: Record<string, string | undefined>;
}

export class FakeGateway {
  recordedRequests: Array<{ url: string; headers: Record<string, string>; body: unknown }> = [];

  constructor(private routes: FakeRoute[]) {}

  /**
   * Build a `fetch`-compatible function that routes to the configured
   * fake handlers.
   */
  fetch: typeof fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(url).pathname;
    const route = this.routes.find((r) => r.path === path);
    const headers: Record<string, string> = {};
    if (init?.headers) {
      for (const [k, v] of Object.entries(init.headers as Record<string, string>)) {
        headers[k.toLowerCase()] = v;
      }
    }
    let body: unknown = undefined;
    if (init?.body) {
      // Connect server-stream wraps the body in [flag][len][payload];
      // unary sends raw JSON.
      const rawBytes =
        init.body instanceof Uint8Array
          ? init.body
          : typeof init.body === "string"
            ? new TextEncoder().encode(init.body)
            : new Uint8Array();
      const ct = headers["content-type"] ?? "";
      if (ct.includes("connect")) {
        body = decodeFirstConnectFrame(rawBytes);
      } else {
        body = JSON.parse(new TextDecoder().decode(rawBytes));
      }
    }
    this.recordedRequests.push({ url, headers, body });

    if (!route) {
      return new Response(JSON.stringify({ code: "not_found", message: `no route for ${path}` }), {
        status: 404,
        headers: { "Content-Type": "application/json" },
      });
    }
    const fakeReq = { body, raw: new Request(url, init as RequestInit | undefined) };
    const resp = await route.handler(fakeReq);
    if (resp.stream) {
      const encoded = encodeConnectStream(resp.stream);
      return new Response(encoded, {
        status: resp.status ?? 200,
        headers: { "Content-Type": "application/connect+json" },
      });
    }
    if (resp.status && resp.status >= 400) {
      return new Response(JSON.stringify(resp.unary ?? { code: "unknown" }), {
        status: resp.status,
        headers: { "Content-Type": "application/json" },
      });
    }
    return new Response(JSON.stringify(resp.unary ?? {}), {
      status: resp.status ?? 200,
      headers: { "Content-Type": "application/json" },
    });
  };
}

function decodeFirstConnectFrame(bytes: Uint8Array): unknown {
  if (bytes.byteLength < 5) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const length = view.getUint32(1, false);
  const payload = bytes.slice(5, 5 + length);
  const text = new TextDecoder().decode(payload);
  return text ? JSON.parse(text) : null;
}

function encodeConnectStream(frames: Array<{ flag: 0 | 2; body: unknown }>): Uint8Array {
  // Total length first
  const encoded = frames.map(({ flag, body }) => {
    const text = JSON.stringify(body ?? {});
    const payload = new TextEncoder().encode(text);
    const out = new Uint8Array(5 + payload.byteLength);
    out[0] = flag;
    new DataView(out.buffer).setUint32(1, payload.byteLength, false);
    out.set(payload, 5);
    return out;
  });
  const totalLen = encoded.reduce((acc, e) => acc + e.byteLength, 0);
  const merged = new Uint8Array(totalLen);
  let offset = 0;
  for (const e of encoded) {
    merged.set(e, offset);
    offset += e.byteLength;
  }
  return merged;
}

export function expectAuthHeader(
  headers: Record<string, string>,
  expectedToken: string,
): void {
  expect(headers["authorization"] ?? headers["Authorization"]).toBe(`Bearer ${expectedToken}`);
}
