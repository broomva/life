/**
 * WebSocket transport for `Agent.StreamSession`.
 *
 * The lifegw gateway upgrades `/v1/agent/stream` per Spec C₃ §6.2.
 * Frame envelope:
 *   `{ "kind": "...", ...rest }` where `kind` is the discriminator.
 *
 * Server frames (received here):
 *   - `agent_event` — `{ kind: "agent_event", seq_no: u64, record: <event>, agent_kind: str }`
 *   - `pong`        — `{ kind: "pong", seq_no: u64 }`
 *   - `closing`     — `{ kind: "closing", reason: str }` (always followed by close)
 *
 * Client frames (sent here):
 *   - `send_message`     — `{ kind: "send_message", content: str, attachment_blob_ref?: str }`
 *   - `approve_dispatch` — `{ kind: "approve_dispatch", dispatch_id: str }`
 *   - `cancel_dispatch`  — `{ kind: "cancel_dispatch", dispatch_id: str }`
 *   - `ping`             — `{ kind: "ping", seq_no?: u64 }`
 *   - `close`            — `{ kind: "close", reason?: str }`
 *
 * Reconnect-by-sequence (Spec C₃ §11.4):
 *   - On (re)connect, attach `?last_seq_no=<u64>` query OR
 *     `X-Life-Last-Seq-No: <u64>` header. lifed replays from `N+1`.
 *   - The connection stores the highest `seq_no` it has seen and
 *     uses it on the next reconnect attempt.
 *
 * Close codes: see {@link closeCodeToError} in `errors.ts` for the
 * full mapping. The connection emits a typed error via the
 * `error` event and the `close` event for graceful (1000) closes.
 */

import {
  closeCodeToError,
  TransportError,
  TlsNegotiationError,
  type LifeSdkError,
} from "./errors.js";

// ── Frame types ────────────────────────────────────────────────────

/**
 * Server → client frame envelope (Spec C₃ §6.2).
 */
export type OutboundFrame =
  | {
      kind: "agent_event";
      seq_no: string | number;
      record: unknown;
      agent_kind: string;
    }
  | { kind: "pong"; seq_no: string | number }
  | { kind: "closing"; reason: string };

/**
 * Client → server frame envelope.
 */
export type InboundFrame =
  | { kind: "send_message"; content: string; attachment_blob_ref?: string }
  | { kind: "approve_dispatch"; dispatch_id: string }
  | { kind: "cancel_dispatch"; dispatch_id: string }
  | { kind: "ping"; seq_no?: number }
  | { kind: "close"; reason?: string };

// ── WebSocket abstraction ──────────────────────────────────────────

/**
 * Minimal `WebSocket` interface — matches both the browser global and
 * the `ws` package's class so the SDK can run unchanged in either.
 */
export interface WebSocketLike {
  readonly readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  addEventListener(event: "open", handler: () => void): void;
  addEventListener(event: "message", handler: (e: { data: unknown }) => void): void;
  addEventListener(event: "error", handler: (e: { message?: string; error?: Error }) => void): void;
  addEventListener(
    event: "close",
    handler: (e: { code: number; reason: string; wasClean?: boolean }) => void,
  ): void;
  removeEventListener?(event: string, handler: (...args: unknown[]) => void): void;
}

/**
 * Factory for `WebSocket` instances.
 *
 * - Browser: pass `(url, protocols) => new WebSocket(url, protocols)`.
 * - Node: pass `(url, protocols) => new WebSocket(url, protocols)`
 *   from the `ws` package.
 *
 * Defaults to `globalThis.WebSocket` when available.
 */
export type WebSocketFactory = (url: string, protocols: string[]) => WebSocketLike;

function defaultWebSocketFactory(): WebSocketFactory {
  const g = globalThis as unknown as { WebSocket?: new (u: string, p?: string | string[]) => WebSocketLike };
  if (!g.WebSocket) {
    throw new TransportError(
      "no WebSocket implementation available — pass `webSocketFactory` to wire `ws` (Node) or run in a browser",
    );
  }
  const Ctor = g.WebSocket;
  return (url, protocols) => new Ctor(url, protocols);
}

// ── Connection options ─────────────────────────────────────────────

/**
 * Options for opening a `WsAgentSession`.
 */
export interface WsAgentSessionOptions {
  /**
   * lifegw base URL, e.g. `https://api.life.dev`. The SDK rewrites
   * the scheme to `wss://` automatically.
   */
  baseUrl: string;

  /**
   * Session id to attach to. Sent as `?sid=<sid>` query OR
   * `X-Life-Sid` header (server reads either; this SDK uses the
   * query form).
   */
  sid: string;

  /**
   * Async producer for the bearer token.
   *
   * Browser `WebSocket` does NOT support custom request headers, so
   * this token is sent as a `Sec-WebSocket-Protocol` subprotocol
   * value: `bearer.<token>`. The lifegw upgrade handler accepts
   * this form via the auth middleware. In Node hosts that pass
   * a `webSocketFactory` honoring `headers`, the token is also
   * forwarded as `Authorization: Bearer <token>` for convenience.
   *
   * If undefined, no auth token is sent (only valid for dev mode).
   */
  getAuthToken?: () => Promise<string | undefined>;

  /**
   * Optional `WebSocket` factory. Defaults to `globalThis.WebSocket`.
   *
   * In Node: `import WebSocket from "ws"; { webSocketFactory: (u,p) => new WebSocket(u, p) }`.
   */
  webSocketFactory?: WebSocketFactory;

  /**
   * Initial `from_sequence` cursor — the server replays from
   * `cursor + 1`. Defaults to `0n` (fresh stream).
   */
  fromSequence?: bigint;

  /**
   * When `true`, the connection automatically reconnects on transient
   * close codes (4002 backpressure, 4004 lifed-unavailable, 1011
   * internal). Defaults to `true`. Permanent codes (1008 auth, 4003
   * IP-blocked, 4005 sequence-retired) never auto-reconnect.
   */
  autoReconnect?: boolean;

  /**
   * Maximum auto-reconnect attempts. Defaults to `5`.
   */
  maxReconnectAttempts?: number;

  /**
   * Base backoff in ms for reconnect attempts. Actual delay is
   * `backoffMs * 2^attempt` (capped at 30s). Defaults to `500`.
   */
  reconnectBackoffMs?: number;
}

// ── Event handlers ─────────────────────────────────────────────────

export interface WsAgentSessionHandlers {
  /**
   * Called on every successful connection (initial + each reconnect).
   */
  onOpen?: () => void;

  /**
   * Called for every `agent_event` frame.
   *
   * The `seqNo` is the lifed-assigned sequence number; the connection
   * stores it internally for resume-on-reconnect.
   */
  onAgentEvent?: (event: AgentEventEnvelope) => void;

  /**
   * Called when the server pongs in response to a client ping.
   */
  onPong?: (seqNo: bigint) => void;

  /**
   * Called when the server emits a pre-close diagnostic frame. The
   * actual close arrives next via `onClose` / `onError`.
   */
  onClosing?: (reason: string) => void;

  /**
   * Called on transport / protocol error. Auto-reconnect (when
   * enabled) takes care of retrying transient errors before
   * surfacing here, so by the time `onError` fires the connection
   * is permanently closed.
   */
  onError?: (err: LifeSdkError) => void;

  /**
   * Called when the connection closes cleanly (graceful 1000).
   */
  onClose?: () => void;
}

export interface AgentEventEnvelope {
  seqNo: bigint;
  record: unknown;
  agentKind: string;
}

// ── Implementation ─────────────────────────────────────────────────

/**
 * `WS_OPEN`, etc. — readyState constants. Defined here so the SDK
 * doesn't import the browser `WebSocket` constructor (which doesn't
 * exist in Node without `ws`).
 */
const WS_CONNECTING = 0;
const WS_OPEN = 1;
const WS_CLOSING = 2;
const WS_CLOSED = 3;

const PERMANENT_CLOSE_CODES = new Set<number>([
  1000, // normal — caller-initiated close, do not reconnect
  1008, // auth violation — token won't fix itself
  4001, // rate-limit — let caller back off explicitly
  4003, // ip-blocked
  4005, // sequence-retired (caller must reset cursor first)
]);

/**
 * A single Agent.StreamSession over WebSocket.
 *
 * Lifecycle:
 *   1. Construct with options + handlers.
 *   2. Call `open()` (or `await openWsAgentSession(...)` helper).
 *   3. Attach handlers BEFORE `open()` to avoid event races.
 *   4. Call `close()` when done.
 */
export class WsAgentSession {
  readonly options: Required<
    Pick<
      WsAgentSessionOptions,
      "baseUrl" | "sid" | "autoReconnect" | "maxReconnectAttempts" | "reconnectBackoffMs"
    >
  > &
    WsAgentSessionOptions;
  private handlers: WsAgentSessionHandlers = {};
  private ws: WebSocketLike | null = null;
  private factory: WebSocketFactory;
  private fromSequence: bigint;
  private reconnectAttempts = 0;
  private closing = false;
  private closed = false;

  constructor(options: WsAgentSessionOptions) {
    this.options = {
      autoReconnect: true,
      maxReconnectAttempts: 5,
      reconnectBackoffMs: 500,
      ...options,
    };
    this.factory = options.webSocketFactory ?? defaultWebSocketFactory();
    this.fromSequence = options.fromSequence ?? 0n;
  }

  /**
   * Set the event handler bag. Replace once at startup; in-flight
   * frames may have been delivered before the swap so callers should
   * not rely on perfect ordering when changing handlers mid-flight.
   */
  on(handlers: WsAgentSessionHandlers): this {
    this.handlers = handlers;
    return this;
  }

  /**
   * Highest `seq_no` observed on this session. Persisted across
   * reconnects (used as `last_seq_no` on the next handshake).
   */
  get lastSeqNo(): bigint {
    return this.fromSequence;
  }

  /**
   * `true` once the connection has been closed gracefully or the
   * permanent-error budget has been exhausted.
   */
  get isClosed(): boolean {
    return this.closed;
  }

  /**
   * Open the connection. Resolves once the underlying `WebSocket`
   * fires `open`; rejects if the initial handshake fails.
   *
   * Subsequent reconnects (if enabled) happen automatically without
   * resolving / rejecting this promise — observe via `onError` /
   * `onOpen` handlers.
   */
  async open(): Promise<void> {
    return this.connect();
  }

  private async connect(): Promise<void> {
    const url = this.buildUrl();
    // Resolve protocols synchronously when no token producer is set so
    // tests + dev callers can attach WS instances on the same task tick.
    const protocols = this.options.getAuthToken
      ? await this.buildProtocols()
      : ["life.v1.agent"];

    return new Promise<void>((resolve, reject) => {
      let ws: WebSocketLike;
      try {
        ws = this.factory(url, protocols);
      } catch (err) {
        const sdkErr = err instanceof Error
          ? new TransportError(`failed to create WebSocket: ${err.message}`, { cause: err })
          : new TransportError(String(err));
        reject(sdkErr);
        return;
      }
      this.ws = ws;

      let opened = false;

      ws.addEventListener("open", () => {
        opened = true;
        this.reconnectAttempts = 0;
        this.handlers.onOpen?.();
        resolve();
      });

      ws.addEventListener("message", (e: { data: unknown }) => {
        const text = coerceData(e.data);
        if (text === null) return;
        let frame: OutboundFrame;
        try {
          frame = JSON.parse(text) as OutboundFrame;
        } catch {
          return; // drop malformed
        }
        this.handleFrame(frame);
      });

      ws.addEventListener("error", (e) => {
        // Browser WebSocket `error` events don't carry useful detail;
        // the close event has the code. We surface a transport error
        // only if no `close` follows (handled by close handler).
        if (!opened) {
          // Initial handshake failure. Heuristic: TLS errors often
          // surface here on browsers.
          const msg = e.message ?? "ws connection error";
          const lower = msg.toLowerCase();
          if (lower.includes("tls") || lower.includes("ssl") || lower.includes("handshake")) {
            reject(new TlsNegotiationError(msg));
          } else {
            reject(new TransportError(msg, e.error ? { cause: e.error } : undefined));
          }
        }
      });

      ws.addEventListener("close", (e) => {
        const err = closeCodeToError(e.code, e.reason);
        if (!opened) {
          // Connect failed before opening; reject via the close mapping.
          reject(err ?? new TransportError(`ws closed before open: ${e.code} ${e.reason}`));
          return;
        }

        if (this.closing || e.code === 1000) {
          // Caller-initiated graceful close.
          this.closed = true;
          this.handlers.onClose?.();
          return;
        }

        if (
          this.options.autoReconnect &&
          !PERMANENT_CLOSE_CODES.has(e.code) &&
          this.reconnectAttempts < this.options.maxReconnectAttempts
        ) {
          this.scheduleReconnect();
          return;
        }

        // Permanent error or budget exhausted.
        this.closed = true;
        if (err) {
          this.handlers.onError?.(err);
        } else {
          this.handlers.onClose?.();
        }
      });
    });
  }

  private scheduleReconnect(): void {
    const attempt = this.reconnectAttempts++;
    const delay = Math.min(
      this.options.reconnectBackoffMs * 2 ** attempt,
      30_000,
    );
    setTimeout(() => {
      if (this.closing) return;
      this.connect().catch((err: unknown) => {
        // If the synchronous handshake fails outright, surface and stop.
        this.closed = true;
        this.handlers.onError?.(err as LifeSdkError);
      });
    }, delay);
  }

  private handleFrame(frame: OutboundFrame): void {
    switch (frame.kind) {
      case "agent_event": {
        const seq = parseSeq(frame.seq_no);
        // Track the highest seen sequence so reconnect resumes
        // from `seq + 1` on the next handshake.
        if (seq > this.fromSequence) {
          this.fromSequence = seq;
        }
        this.handlers.onAgentEvent?.({
          seqNo: seq,
          record: frame.record,
          agentKind: frame.agent_kind,
        });
        return;
      }
      case "pong": {
        this.handlers.onPong?.(parseSeq(frame.seq_no));
        return;
      }
      case "closing": {
        this.handlers.onClosing?.(frame.reason);
        return;
      }
      // Unknown kinds dropped silently per Spec C₃ §6.2.
      default:
        return;
    }
  }

  private buildUrl(): string {
    let base = this.options.baseUrl.replace(/\/+$/, "");
    if (base.startsWith("https://")) base = "wss://" + base.slice("https://".length);
    else if (base.startsWith("http://")) base = "ws://" + base.slice("http://".length);
    const params = new URLSearchParams();
    params.set("sid", this.options.sid);
    if (this.fromSequence > 0n) {
      params.set("last_seq_no", this.fromSequence.toString());
    }
    return `${base}/v1/agent/stream?${params.toString()}`;
  }

  /**
   * Build the subprotocol list. lifegw negotiates `life.v1.agent`;
   * the auth token (if any) is appended as `bearer.<token>` so the
   * browser can forward it without custom headers.
   */
  private async buildProtocols(): Promise<string[]> {
    const protos: string[] = ["life.v1.agent"];
    if (this.options.getAuthToken) {
      const tok = await this.options.getAuthToken();
      if (tok) protos.push(`bearer.${tok}`);
    }
    return protos;
  }

  // ── Outgoing frames ─────────────────────────────────────────────

  /**
   * Send a chat message on this session. Mapped to upstream
   * `Agent.SendMessage`.
   */
  sendMessage(content: string, attachmentBlobRef?: string): void {
    this.send({
      kind: "send_message",
      content,
      ...(attachmentBlobRef ? { attachment_blob_ref: attachmentBlobRef } : {}),
    });
  }

  /**
   * Approve a pending dispatch identified by id.
   */
  approveDispatch(dispatchId: string): void {
    this.send({ kind: "approve_dispatch", dispatch_id: dispatchId });
  }

  /**
   * Cancel a pending dispatch identified by id.
   */
  cancelDispatch(dispatchId: string): void {
    this.send({ kind: "cancel_dispatch", dispatch_id: dispatchId });
  }

  /**
   * Send a ping frame; the server replies with `pong`. The server
   * also runs its own heartbeat (Spec C₃ §6.4 — 30 s interval, 60 s
   * pong-deadline) so client-initiated pings are optional.
   */
  ping(seqNo?: number): void {
    this.send({ kind: "ping", ...(seqNo !== undefined ? { seq_no: seqNo } : {}) });
  }

  /**
   * Close the connection gracefully (1000). Disables auto-reconnect.
   */
  close(reason?: string): void {
    this.closing = true;
    if (this.ws && (this.ws.readyState === WS_OPEN || this.ws.readyState === WS_CONNECTING)) {
      this.send({ kind: "close", ...(reason ? { reason } : {}) });
      this.ws.close(1000, reason ?? "client_close");
    }
  }

  private send(frame: InboundFrame): void {
    if (!this.ws || this.ws.readyState !== WS_OPEN) {
      throw new TransportError(`cannot send frame in readyState=${this.ws?.readyState ?? "null"}`);
    }
    this.ws.send(JSON.stringify(frame));
  }
}

/**
 * Convenience helper — open + return the session in one call.
 *
 * @example
 * ```ts
 * const session = await openWsAgentSession({
 *   baseUrl: "https://api.life.dev",
 *   sid: "sid-123",
 *   getAuthToken: async () => myToken,
 * });
 * session.on({ onAgentEvent: (e) => console.log(e) });
 * session.sendMessage("hello");
 * ```
 */
export async function openWsAgentSession(
  opts: WsAgentSessionOptions,
  handlers?: WsAgentSessionHandlers,
): Promise<WsAgentSession> {
  const s = new WsAgentSession(opts);
  if (handlers) s.on(handlers);
  await s.open();
  return s;
}

// ── Helpers ────────────────────────────────────────────────────────

function coerceData(data: unknown): string | null {
  if (typeof data === "string") return data;
  if (data instanceof ArrayBuffer) {
    return new TextDecoder().decode(new Uint8Array(data));
  }
  if (data && typeof data === "object" && "byteLength" in (data as object)) {
    return new TextDecoder().decode(data as ArrayBuffer);
  }
  return null;
}

function parseSeq(v: string | number | bigint): bigint {
  if (typeof v === "bigint") return v;
  if (typeof v === "number") return BigInt(v);
  if (typeof v === "string") {
    try {
      return BigInt(v);
    } catch {
      return 0n;
    }
  }
  return 0n;
}

// ── Re-exported readyState constants for convenience ───────────────
export const WebSocketReadyState = {
  Connecting: WS_CONNECTING,
  Open: WS_OPEN,
  Closing: WS_CLOSING,
  Closed: WS_CLOSED,
} as const;
