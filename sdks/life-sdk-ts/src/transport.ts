/**
 * gRPC-web transport over `fetch`.
 *
 * This module implements the **Connect over HTTP** wire shape (also
 * known as `application/connect+json`) — a gRPC-compatible protocol
 * that uses plain JSON request/response bodies plus a length-prefixed
 * frame framing for streaming.
 *
 * The lifegw edge gateway negotiates `grpc-web+json` and `connect+json`
 * via tonic-web; this SDK uses `connect+json` because it gives a
 * ergonomic plain-JSON request shape and doesn't need a protobuf
 * runtime in the client.
 *
 * Wire shape (Connect protocol simplified):
 *
 * Unary:
 *   POST /life.v1.Service/Method
 *   Content-Type: application/json
 *   { "request fields..." }
 *   →
 *   200 OK
 *   Content-Type: application/json
 *   { "response fields..." }
 *
 * Errors:
 *   non-2xx response with JSON body { code: "<status>", message: "..." }
 *
 * Server streaming:
 *   POST /life.v1.Service/Method
 *   Content-Type: application/connect+json
 *   {flag: 0, length: N, body: <json>}* {flag: 2, length: N, body: <end-stream-json>}
 *
 * For stream framing the wire is:
 *   [flag (1 byte)][length (4 bytes BE)][payload (length bytes)]
 *
 * - flag = 0   → message frame (UTF-8 JSON-encoded message)
 * - flag = 2   → end-stream frame (UTF-8 JSON, contains `error?`)
 *
 * This is sufficient for the four user-facing services. Larger
 * payloads (binary blobs) use `bytes` fields which Connect/JSON
 * encodes as base64.
 *
 * @see https://connectrpc.com/docs/protocol/
 */

import {
  GrpcCode,
  GrpcError,
  TlsNegotiationError,
  TransportError,
  grpcCodeToError,
} from "./errors.js";
import type { LifeSdkError } from "./errors.js";

/**
 * Map a Connect status name (string) to a numeric gRPC code.
 */
const CONNECT_CODE_MAP: Record<string, number> = {
  canceled: GrpcCode.Cancelled,
  unknown: GrpcCode.Unknown,
  invalid_argument: GrpcCode.InvalidArgument,
  deadline_exceeded: GrpcCode.DeadlineExceeded,
  not_found: GrpcCode.NotFound,
  already_exists: GrpcCode.AlreadyExists,
  permission_denied: GrpcCode.PermissionDenied,
  resource_exhausted: GrpcCode.ResourceExhausted,
  failed_precondition: GrpcCode.FailedPrecondition,
  aborted: GrpcCode.Aborted,
  out_of_range: GrpcCode.OutOfRange,
  unimplemented: GrpcCode.Unimplemented,
  internal: GrpcCode.Internal,
  unavailable: GrpcCode.Unavailable,
  data_loss: GrpcCode.DataLoss,
  unauthenticated: GrpcCode.Unauthenticated,
};

const CONNECT_CODE_REVERSE: Record<number, string> = Object.fromEntries(
  Object.entries(CONNECT_CODE_MAP).map(([k, v]) => [v, k.toUpperCase()]),
);

interface ConnectErrorBody {
  code?: string;
  message?: string;
}

/**
 * Options passed to every transport call.
 */
export interface TransportCallOptions {
  /**
   * Optional `AbortSignal` for cancellation.
   */
  signal?: AbortSignal;
  /**
   * Per-call request timeout in milliseconds. Implementation creates
   * an internal `AbortController` linked to `signal`.
   */
  timeoutMs?: number;
  /**
   * Extra headers to attach. Authorization is added automatically by
   * the {@link Transport} from its `getAuthToken` callback.
   */
  headers?: Record<string, string>;
}

/**
 * Transport configuration.
 */
export interface TransportConfig {
  /**
   * Base URL of the lifegw HTTPS endpoint, e.g. `https://api.life.dev`.
   * Trailing slashes are normalized away.
   */
  baseUrl: string;

  /**
   * Async producer for the Tier-1 bearer token. Called on every RPC;
   * implementations should cache + refresh internally to avoid
   * blocking the hot path.
   */
  getAuthToken?: () => Promise<string | undefined>;

  /**
   * Optional `fetch` override (for tests or non-browser hosts).
   */
  fetch?: typeof fetch;
}

/**
 * Lightweight transport over `fetch`. One transport instance can be
 * shared across all four services.
 */
export class Transport {
  readonly baseUrl: string;
  readonly fetchFn: typeof fetch;
  readonly getAuthToken?: () => Promise<string | undefined>;

  constructor(cfg: TransportConfig) {
    this.baseUrl = cfg.baseUrl.replace(/\/+$/, "");
    this.fetchFn = cfg.fetch ?? globalThis.fetch.bind(globalThis);
    this.getAuthToken = cfg.getAuthToken;
  }

  /**
   * Perform a unary RPC. The server is expected to return JSON on
   * 2xx and a Connect error body on non-2xx.
   *
   * @param service Fully-qualified proto service name, e.g. `life.v1.Agent`.
   * @param method Method name, e.g. `CreateSession`.
   * @param body Request payload (plain JS object — not yet stringified).
   * @param opts Per-call options.
   *
   * @returns Decoded response body.
   * @throws {LifeSdkError} subclasses on transport / RPC error.
   */
  async unary<TReq, TRes>(
    service: string,
    method: string,
    body: TReq,
    opts: TransportCallOptions = {},
  ): Promise<TRes> {
    const url = `${this.baseUrl}/${service}/${method}`;
    const headers = await this.buildHeaders(opts.headers);
    headers["Content-Type"] = "application/json";

    const controller = new AbortController();
    const linked = linkSignals(controller, opts.signal, opts.timeoutMs);

    let resp: Response;
    try {
      resp = await this.fetchFn(url, {
        method: "POST",
        headers,
        body: JSON.stringify(body, jsonReplacer),
        signal: controller.signal,
      });
    } catch (err) {
      throw mapFetchError(err);
    } finally {
      linked.cleanup();
    }

    if (!resp.ok) {
      throw await readConnectError(resp);
    }

    const text = await resp.text();
    return parseJsonResponse<TRes>(text);
  }

  /**
   * Perform a server-streaming RPC (Connect protocol framing).
   *
   * Yields decoded messages from the stream. Throws when the stream
   * ends with an error end-stream frame, or when transport fails.
   */
  async *serverStream<TReq, TRes>(
    service: string,
    method: string,
    body: TReq,
    opts: TransportCallOptions = {},
  ): AsyncIterable<TRes> {
    const url = `${this.baseUrl}/${service}/${method}`;
    const headers = await this.buildHeaders(opts.headers);
    headers["Content-Type"] = "application/connect+json";
    headers["Connect-Protocol-Version"] = "1";

    const controller = new AbortController();
    const linked = linkSignals(controller, opts.signal, opts.timeoutMs);

    let resp: Response;
    try {
      // Connect server-stream encodes the request as a single message frame.
      const reqFrame = encodeMessageFrame(JSON.stringify(body, jsonReplacer));
      // Cast to BodyInit — Uint8Array is a valid BodyInit at runtime
      // but TS lib.dom.d.ts only includes the older signature in some
      // versions.
      resp = await this.fetchFn(url, {
        method: "POST",
        headers,
        body: reqFrame as unknown as BodyInit,
        signal: controller.signal,
      });
    } catch (err) {
      linked.cleanup();
      throw mapFetchError(err);
    }

    if (!resp.ok) {
      linked.cleanup();
      throw await readConnectError(resp);
    }

    if (!resp.body) {
      linked.cleanup();
      throw new TransportError("server-stream response had no body");
    }

    try {
      for await (const frame of readConnectFrames(resp.body)) {
        if (frame.flag === 2) {
          // End-stream frame.
          const trailer = parseJsonResponse<ConnectErrorBody>(frame.body) as ConnectErrorBody;
          if (trailer && trailer.code) {
            throw connectErrorToSdkError(trailer);
          }
          return;
        }
        if (frame.flag === 0) {
          yield parseJsonResponse<TRes>(frame.body);
        }
        // Unknown flags are silently dropped per Connect spec.
      }
    } finally {
      linked.cleanup();
    }
  }

  private async buildHeaders(extra?: Record<string, string>): Promise<Record<string, string>> {
    const out: Record<string, string> = { ...(extra ?? {}) };
    if (this.getAuthToken) {
      const tok = await this.getAuthToken();
      if (tok) out.Authorization = `Bearer ${tok}`;
    }
    return out;
  }
}

// ── Frame encoding helpers ─────────────────────────────────────────

/**
 * Encode a message body as a Connect frame: `[flag][len BE][payload]`.
 */
function encodeMessageFrame(body: string): Uint8Array {
  const payload = new TextEncoder().encode(body);
  const out = new Uint8Array(5 + payload.byteLength);
  // flag = 0 (message)
  out[0] = 0;
  // 4-byte big-endian length
  const view = new DataView(out.buffer);
  view.setUint32(1, payload.byteLength, false);
  out.set(payload, 5);
  return out;
}

interface ConnectFrame {
  flag: number;
  body: string;
}

/**
 * Read Connect-protocol stream frames from a `ReadableStream<Uint8Array>`.
 */
async function* readConnectFrames(
  stream: ReadableStream<Uint8Array>,
): AsyncIterable<ConnectFrame> {
  const reader = stream.getReader();
  let buffer = new Uint8Array(0);
  const td = new TextDecoder();

  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    if (!value) continue;

    // Append to buffer.
    const next = new Uint8Array(buffer.byteLength + value.byteLength);
    next.set(buffer, 0);
    next.set(value, buffer.byteLength);
    buffer = next;

    // Parse all complete frames.
    while (buffer.byteLength >= 5) {
      const flag = buffer[0];
      if (flag === undefined) break;
      const view = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);
      const length = view.getUint32(1, false);
      if (buffer.byteLength < 5 + length) break;
      const payload = buffer.slice(5, 5 + length);
      const body = td.decode(payload);
      yield { flag, body };
      buffer = buffer.slice(5 + length);
    }
  }
}

// ── Error helpers ──────────────────────────────────────────────────

async function readConnectError(resp: Response): Promise<LifeSdkError> {
  const text = await resp.text();
  if (!text) {
    return new GrpcError(GrpcCode.Unknown, "UNKNOWN", `HTTP ${resp.status}`);
  }
  try {
    const body = JSON.parse(text) as ConnectErrorBody;
    return connectErrorToSdkError(body);
  } catch {
    return new GrpcError(GrpcCode.Unknown, "UNKNOWN", text);
  }
}

function connectErrorToSdkError(body: ConnectErrorBody): LifeSdkError {
  const codeKey = (body.code ?? "unknown").toLowerCase();
  const numeric = CONNECT_CODE_MAP[codeKey] ?? GrpcCode.Unknown;
  const status = CONNECT_CODE_REVERSE[numeric] ?? "UNKNOWN";
  return grpcCodeToError(numeric, status, body.message);
}

function mapFetchError(err: unknown): LifeSdkError {
  if (err instanceof Error) {
    const msg = err.message.toLowerCase();
    // Best-effort TLS-negotiation detection. Browser-side fetch
    // collapses TLS errors into a generic TypeError("Failed to
    // fetch") so the heuristic is intentionally lenient.
    if (
      msg.includes("ssl") ||
      msg.includes("tls") ||
      msg.includes("eproto") ||
      msg.includes("handshake") ||
      msg.includes("err_ssl") ||
      msg.includes("ssl3")
    ) {
      return new TlsNegotiationError(`TLS negotiation failed: ${err.message}`);
    }
    if (err.name === "AbortError") {
      return new GrpcError(GrpcCode.Cancelled, "CANCELLED", err.message);
    }
    return new TransportError(err.message, { cause: err });
  }
  return new TransportError(String(err));
}

function parseJsonResponse<T>(text: string): T {
  if (!text) return {} as T;
  try {
    return JSON.parse(text, jsonReviver) as T;
  } catch (err) {
    throw new TransportError(`malformed response JSON: ${(err as Error).message}`, {
      cause: err as Error,
    });
  }
}

// ── BigInt-safe JSON helpers ───────────────────────────────────────

/**
 * `JSON.stringify` replacer that converts `bigint` values to strings
 * (proto3 JSON canonical form for `int64` / `uint64`).
 */
function jsonReplacer(_key: string, value: unknown): unknown {
  if (typeof value === "bigint") return value.toString();
  if (value instanceof Uint8Array) {
    // proto3 JSON encodes `bytes` as base64.
    return uint8ArrayToBase64(value);
  }
  return value;
}

/**
 * `JSON.parse` reviver that leaves number-like strings alone (callers
 * choose whether to widen to `bigint`). We intentionally do NOT
 * auto-bigint here because the proto schema is the source of truth.
 */
function jsonReviver(_key: string, value: unknown): unknown {
  return value;
}

function uint8ArrayToBase64(u8: Uint8Array): string {
  // Browser-compat: prefer `btoa` over Node `Buffer` so the SDK runs
  // unchanged in the browser.
  let bin = "";
  for (let i = 0; i < u8.byteLength; i++) {
    bin += String.fromCharCode(u8[i] as number);
  }
  if (typeof btoa === "function") return btoa(bin);
  // Node fallback: TextEncoder + globalThis.Buffer
  const g = globalThis as unknown as { Buffer?: { from(s: string, enc: string): { toString(enc: string): string } } };
  if (g.Buffer) return g.Buffer.from(bin, "binary").toString("base64");
  throw new TransportError("no base64 encoder available");
}

// ── Signal linking ─────────────────────────────────────────────────

interface LinkedSignals {
  cleanup: () => void;
}

function linkSignals(
  controller: AbortController,
  external?: AbortSignal,
  timeoutMs?: number,
): LinkedSignals {
  const handlers: Array<() => void> = [];
  let timer: ReturnType<typeof setTimeout> | undefined;

  if (external) {
    if (external.aborted) {
      controller.abort(external.reason);
    } else {
      const onAbort = (): void => controller.abort(external.reason);
      external.addEventListener("abort", onAbort, { once: true });
      handlers.push(() => external.removeEventListener("abort", onAbort));
    }
  }

  if (timeoutMs && timeoutMs > 0) {
    timer = setTimeout(() => controller.abort(new Error("request timeout")), timeoutMs);
  }

  return {
    cleanup() {
      if (timer) clearTimeout(timer);
      for (const h of handlers) h();
    },
  };
}
