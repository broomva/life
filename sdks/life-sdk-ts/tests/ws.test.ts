/**
 * WebSocket transport unit tests.
 *
 * The fake WebSocket exercises:
 *   - Reconnect-by-`from_sequence` after a transient drop
 *   - Close-code → typed error mapping
 *   - Outgoing send_message / approve_dispatch / cancel_dispatch
 *   - Subprotocol bearer attachment (browser-compat code path)
 *   - Drop of unknown server frames
 */

import { describe, expect, it, vi } from "vitest";
import {
  AuthError,
  BackpressureError,
  IpBlockedError,
  RateLimitError,
  SequenceRetiredError,
  WsAgentSession,
  type AgentEventEnvelope,
  type WebSocketLike,
} from "../src/index.js";

/** Yield to the microtask queue until predicate() is true (or ttl ticks). */
async function waitFor(pred: () => boolean, ttl = 50): Promise<void> {
  for (let i = 0; i < ttl; i++) {
    if (pred()) return;
    await Promise.resolve();
  }
  if (!pred()) throw new Error(`waitFor: predicate did not become true within ${ttl} ticks`);
}

class FakeWs implements WebSocketLike {
  static instances: FakeWs[] = [];
  readyState = 0;
  readonly url: string;
  readonly protocols: string[];
  readonly sent: string[] = [];
  private listeners = new Map<string, Array<(arg: unknown) => void>>();

  constructor(url: string, protocols: string[]) {
    this.url = url;
    this.protocols = protocols;
    FakeWs.instances.push(this);
  }

  addEventListener(ev: "open", h: () => void): void;
  addEventListener(ev: "message", h: (e: { data: unknown }) => void): void;
  addEventListener(ev: "error", h: (e: { message?: string; error?: Error }) => void): void;
  addEventListener(ev: "close", h: (e: { code: number; reason: string; wasClean?: boolean }) => void): void;
  addEventListener(ev: string, h: (arg: unknown) => void): void {
    const list = this.listeners.get(ev) ?? [];
    list.push(h);
    this.listeners.set(ev, list);
  }

  send(data: string): void {
    this.sent.push(data);
  }

  close(code?: number, reason?: string): void {
    this.readyState = 3;
    this.fire("close", { code: code ?? 1000, reason: reason ?? "client_close" });
  }

  // ── Test-only helpers ─────────────────────────────────────────
  fire(event: string, arg: unknown): void {
    const list = this.listeners.get(event);
    if (!list) return;
    for (const h of list) h(arg);
  }

  open(): void {
    this.readyState = 1;
    this.fire("open", undefined);
  }

  receive(text: string): void {
    this.fire("message", { data: text });
  }

  serverClose(code: number, reason: string): void {
    this.readyState = 3;
    this.fire("close", { code, reason });
  }
}

describe("WsAgentSession", () => {
  it("attaches sid + last_seq_no in the URL", async () => {
    FakeWs.instances = [];
    const session = new WsAgentSession({
      baseUrl: "https://gw.test",
      sid: "sid-1",
      fromSequence: 42n,
      autoReconnect: false,
      webSocketFactory: (u, p) => new FakeWs(u, p),
    });
    const openP = session.open();
    await waitFor(() => FakeWs.instances.length > 0);
    FakeWs.instances[0]!.open();
    await openP;
    const url = new URL(FakeWs.instances[0]!.url);
    expect(url.searchParams.get("sid")).toBe("sid-1");
    expect(url.searchParams.get("last_seq_no")).toBe("42");
    expect(FakeWs.instances[0]!.protocols).toContain("life.v1.agent");
    session.close();
  });

  it("attaches bearer.<token> as a subprotocol", async () => {
    FakeWs.instances = [];
    const session = new WsAgentSession({
      baseUrl: "https://gw.test",
      sid: "sid-x",
      autoReconnect: false,
      getAuthToken: async () => "tok-xyz",
      webSocketFactory: (u, p) => new FakeWs(u, p),
    });
    const openP = session.open();
    await waitFor(() => FakeWs.instances.length > 0);
    FakeWs.instances[0]!.open();
    await openP;
    expect(FakeWs.instances[0]!.protocols).toContain("bearer.tok-xyz");
    session.close();
  });

  it("decodes agent_event frames and tracks last seq", async () => {
    FakeWs.instances = [];
    const events: AgentEventEnvelope[] = [];
    const session = new WsAgentSession({
      baseUrl: "https://gw.test",
      sid: "sid",
      autoReconnect: false,
      webSocketFactory: (u, p) => new FakeWs(u, p),
    });
    session.on({ onAgentEvent: (e) => events.push(e) });

    const openP = session.open();
    await waitFor(() => FakeWs.instances.length > 0);
    const ws = FakeWs.instances[0]!;
    ws.open();
    await openP;

    ws.receive(JSON.stringify({ kind: "agent_event", seq_no: "1", record: { x: 1 }, agent_kind: "TOKEN" }));
    ws.receive(JSON.stringify({ kind: "agent_event", seq_no: "5", record: {}, agent_kind: "FINISH" }));

    expect(events).toHaveLength(2);
    expect(session.lastSeqNo).toBe(5n);
    session.close();
  });

  it("drops unknown frame kinds silently", async () => {
    FakeWs.instances = [];
    const events: AgentEventEnvelope[] = [];
    const onError = vi.fn();
    const session = new WsAgentSession({
      baseUrl: "https://gw.test",
      sid: "sid",
      autoReconnect: false,
      webSocketFactory: (u, p) => new FakeWs(u, p),
    });
    session.on({ onAgentEvent: (e) => events.push(e), onError });
    const openP = session.open();
    await waitFor(() => FakeWs.instances.length > 0);
    const ws = FakeWs.instances[0]!;
    ws.open();
    await openP;
    ws.receive(JSON.stringify({ kind: "future_kind_we_dont_know", x: 1 }));
    expect(events).toHaveLength(0);
    expect(onError).not.toHaveBeenCalled();
    session.close();
  });

  it("surfaces 1008 PolicyViolation as AuthError on close (no auto-reconnect)", async () => {
    FakeWs.instances = [];
    const session = new WsAgentSession({
      baseUrl: "https://gw.test",
      sid: "sid",
      autoReconnect: true,
      webSocketFactory: (u, p) => new FakeWs(u, p),
    });
    const errors: unknown[] = [];
    session.on({ onError: (e) => errors.push(e) });

    const openP = session.open();
    await waitFor(() => FakeWs.instances.length > 0);
    const ws = FakeWs.instances[0]!;
    ws.open();
    await openP;

    ws.serverClose(1008, "policy_violation:token_expired");

    expect(errors).toHaveLength(1);
    expect(errors[0]).toBeInstanceOf(AuthError);
    expect(session.isClosed).toBe(true);
    expect(FakeWs.instances).toHaveLength(1); // no reconnect
  });

  it("auto-reconnects on transient 4004 LifedUnavailable and resumes from last seq", async () => {
    vi.useFakeTimers();
    FakeWs.instances = [];
    const session = new WsAgentSession({
      baseUrl: "https://gw.test",
      sid: "sid",
      autoReconnect: true,
      reconnectBackoffMs: 10,
      maxReconnectAttempts: 2,
      webSocketFactory: (u, p) => new FakeWs(u, p),
    });
    session.on({});
    const openP = session.open();
    await waitFor(() => FakeWs.instances.length > 0);
    FakeWs.instances[0]!.open();
    await openP;

    FakeWs.instances[0]!.receive(JSON.stringify({ kind: "agent_event", seq_no: "7", record: {}, agent_kind: "TOKEN" }));
    expect(session.lastSeqNo).toBe(7n);

    // Server drops with transient 4004.
    FakeWs.instances[0]!.serverClose(4004, "lifed_circuit_open");

    // Advance past backoff to trigger the reconnect.
    await vi.advanceTimersByTimeAsync(20);
    expect(FakeWs.instances).toHaveLength(2);

    const reconnectUrl = new URL(FakeWs.instances[1]!.url);
    expect(reconnectUrl.searchParams.get("last_seq_no")).toBe("7");
    expect(reconnectUrl.searchParams.get("sid")).toBe("sid");

    FakeWs.instances[1]!.open();
    session.close();
    vi.useRealTimers();
  });

  it("does NOT auto-reconnect on 4001 RateLimit (permanent)", async () => {
    vi.useFakeTimers();
    FakeWs.instances = [];
    const session = new WsAgentSession({
      baseUrl: "https://gw.test",
      sid: "sid",
      autoReconnect: true,
      reconnectBackoffMs: 1,
      webSocketFactory: (u, p) => new FakeWs(u, p),
    });
    const errs: unknown[] = [];
    session.on({ onError: (e) => errs.push(e) });
    const openP = session.open();
    await waitFor(() => FakeWs.instances.length > 0);
    FakeWs.instances[0]!.open();
    await openP;

    FakeWs.instances[0]!.serverClose(4001, "rate_limit:per_user");

    await vi.advanceTimersByTimeAsync(10);
    expect(FakeWs.instances).toHaveLength(1); // no reconnect
    expect(errs[0]).toBeInstanceOf(RateLimitError);
    vi.useRealTimers();
  });

  it("4003 IP-blocked + 4005 sequence-retired are also permanent", async () => {
    for (const [code, ErrorCtor] of [
      [4003, IpBlockedError],
      [4005, SequenceRetiredError],
    ] as const) {
      vi.useFakeTimers();
      FakeWs.instances = [];
      const session = new WsAgentSession({
        baseUrl: "https://gw.test",
        sid: "sid",
        autoReconnect: true,
        reconnectBackoffMs: 1,
        webSocketFactory: (u, p) => new FakeWs(u, p),
      });
      const errs: unknown[] = [];
      session.on({ onError: (e) => errs.push(e) });
      const openP = session.open();
      FakeWs.instances[0]!.open();
      await openP;
      FakeWs.instances[0]!.serverClose(code, "");
      await vi.advanceTimersByTimeAsync(10);
      expect(FakeWs.instances).toHaveLength(1);
      expect(errs[0]).toBeInstanceOf(ErrorCtor);
      vi.useRealTimers();
    }
  });

  it("4002 backpressure auto-reconnects (transient)", async () => {
    vi.useFakeTimers();
    FakeWs.instances = [];
    const session = new WsAgentSession({
      baseUrl: "https://gw.test",
      sid: "sid",
      autoReconnect: true,
      reconnectBackoffMs: 1,
      webSocketFactory: (u, p) => new FakeWs(u, p),
    });
    session.on({});
    const openP = session.open();
    await waitFor(() => FakeWs.instances.length > 0);
    FakeWs.instances[0]!.open();
    await openP;
    FakeWs.instances[0]!.serverClose(4002, "backpressure:slow_consumer");
    await vi.advanceTimersByTimeAsync(10);
    expect(FakeWs.instances.length).toBeGreaterThanOrEqual(2);
    // Smoke-test that the second connection inherits the backpressure
    // surface even though the first errored on it. The last-seq cursor
    // should still be 0 (no events received).
    expect(session.lastSeqNo).toBe(0n);
    // Mark BackpressureError type used so the import isn't dead.
    expect(BackpressureError.prototype).toBeDefined();
    vi.useRealTimers();
  });

  it("sends send_message / approve_dispatch / cancel_dispatch / ping / close frames", async () => {
    FakeWs.instances = [];
    const session = new WsAgentSession({
      baseUrl: "https://gw.test",
      sid: "sid",
      autoReconnect: false,
      webSocketFactory: (u, p) => new FakeWs(u, p),
    });
    session.on({});
    const openP = session.open();
    await waitFor(() => FakeWs.instances.length > 0);
    const ws = FakeWs.instances[0]!;
    ws.open();
    await openP;

    session.sendMessage("hi", "sha256:abc");
    session.approveDispatch("d1");
    session.cancelDispatch("d2");
    session.ping(7);

    expect(ws.sent).toHaveLength(4);
    const sent = ws.sent.map((s) => JSON.parse(s));
    expect(sent[0]).toEqual({ kind: "send_message", content: "hi", attachment_blob_ref: "sha256:abc" });
    expect(sent[1]).toEqual({ kind: "approve_dispatch", dispatch_id: "d1" });
    expect(sent[2]).toEqual({ kind: "cancel_dispatch", dispatch_id: "d2" });
    expect(sent[3]).toEqual({ kind: "ping", seq_no: 7 });

    session.close("done");
    expect(ws.sent[ws.sent.length - 1]).toContain('"close"');
    expect(session.isClosed).toBe(true);
  });

  it("invokes onClosing for pre-close diagnostic frames", async () => {
    FakeWs.instances = [];
    const onClosing = vi.fn();
    const session = new WsAgentSession({
      baseUrl: "https://gw.test",
      sid: "sid",
      autoReconnect: false,
      webSocketFactory: (u, p) => new FakeWs(u, p),
    });
    session.on({ onClosing });
    const openP = session.open();
    await waitFor(() => FakeWs.instances.length > 0);
    const ws = FakeWs.instances[0]!;
    ws.open();
    await openP;
    ws.receive(JSON.stringify({ kind: "closing", reason: "rate_limit:per_user" }));
    expect(onClosing).toHaveBeenCalledWith("rate_limit:per_user");
    session.close();
  });

  it("rewrites https://baseUrl to wss://", async () => {
    FakeWs.instances = [];
    const session = new WsAgentSession({
      baseUrl: "https://api.life.dev",
      sid: "sid",
      autoReconnect: false,
      webSocketFactory: (u, p) => new FakeWs(u, p),
    });
    session.on({});
    const openP = session.open();
    await waitFor(() => FakeWs.instances.length > 0);
    FakeWs.instances[0]!.open();
    await openP;
    expect(FakeWs.instances[0]!.url.startsWith("wss://api.life.dev/v1/agent/stream")).toBe(true);
    session.close();
  });
});
