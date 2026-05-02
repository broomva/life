/**
 * `LifeClient` — high-level entry point.
 *
 * Aggregates the four user-facing services (`life.v1.{Agent, Events,
 * Wallet, Identity}`) over a shared transport. Construct one
 * `LifeClient` per origin / user; the same instance is safe to
 * share across an entire app.
 *
 * @example
 * ```ts
 * import { LifeClient } from "@broomva/life-sdk";
 *
 * const life = new LifeClient({
 *   baseUrl: "https://api.life.dev",
 *   getAuthToken: async () => myClerkToken,
 * });
 *
 * const session = await life.agent.createSession({ userId: "u-1" });
 * for await (const event of life.agent.sendMessage({
 *   sid: session.sid,
 *   content: "hello",
 * })) {
 *   console.log(event);
 * }
 * ```
 */

import { AgentClient } from "./services/agent.js";
import { EventsClient } from "./services/events.js";
import { WalletClient } from "./services/wallet.js";
import { IdentityClient } from "./services/identity.js";
import { Transport } from "./transport.js";
import {
  WsAgentSession,
  type WsAgentSessionHandlers,
  type WsAgentSessionOptions,
  type WebSocketFactory,
} from "./ws.js";

/**
 * Configuration for {@link LifeClient}.
 */
export interface LifeClientConfig {
  /**
   * lifegw HTTPS endpoint, e.g. `https://api.life.dev`. Trailing
   * slashes normalized away.
   *
   * Spec C₃ §5.1 mandates TLS 1.3 — clients that can't negotiate
   * 1.3 receive a {@link TlsNegotiationError} on the first call.
   */
  baseUrl: string;

  /**
   * Async callback that produces a Tier-1 bearer JWT (the "user"
   * token issued by your auth provider — Clerk, Auth.js, custom).
   *
   * The callback is invoked on every RPC, so implementations should
   * cache + refresh internally to avoid blocking the hot path.
   *
   * Pass `undefined` for dev-mode workflows that target lifegw with
   * its dev signer enabled; production lifegw rejects un-bearered
   * traffic.
   */
  getAuthToken?: () => Promise<string | undefined>;

  /**
   * Override `fetch`. Defaults to `globalThis.fetch`.
   *
   * Useful for tests and for hosts that want to inject custom
   * retry/instrumentation logic.
   */
  fetch?: typeof fetch;

  /**
   * Override the `WebSocket` factory used by {@link LifeClient.streamSession}.
   *
   * In Node, pass `import WebSocket from "ws"; (u, p) => new WebSocket(u, p)`.
   * In the browser, leave undefined to pick up `globalThis.WebSocket`.
   */
  webSocketFactory?: WebSocketFactory;
}

export class LifeClient {
  readonly transport: Transport;
  readonly agent: AgentClient;
  readonly events: EventsClient;
  readonly wallet: WalletClient;
  readonly identity: IdentityClient;

  private readonly cfg: LifeClientConfig;

  constructor(cfg: LifeClientConfig) {
    this.cfg = cfg;
    this.transport = new Transport({
      baseUrl: cfg.baseUrl,
      getAuthToken: cfg.getAuthToken,
      fetch: cfg.fetch,
    });
    this.agent = new AgentClient(this.transport);
    this.events = new EventsClient(this.transport);
    this.wallet = new WalletClient(this.transport);
    this.identity = new IdentityClient(this.transport);
  }

  /**
   * Open a long-lived `Agent.StreamSession` over WebSocket with
   * reconnect-by-`from_sequence` resume semantics (Spec C₃ §6).
   *
   * The returned `WsAgentSession` is already `open()`-ed; attach
   * handlers BEFORE the await resolves to avoid event races.
   *
   * @example
   * ```ts
   * const session = await life.streamSession("sid-123", {
   *   onAgentEvent: (e) => render(e),
   *   onError: (err) => console.error(err),
   * });
   *
   * session.sendMessage("hello");
   * // …later…
   * session.close();
   * ```
   */
  async streamSession(
    sid: string,
    handlers?: WsAgentSessionHandlers,
    extraOptions?: Partial<Omit<WsAgentSessionOptions, "baseUrl" | "sid" | "getAuthToken">>,
  ): Promise<WsAgentSession> {
    const opts: WsAgentSessionOptions = {
      baseUrl: this.cfg.baseUrl,
      sid,
      ...(this.cfg.getAuthToken && { getAuthToken: this.cfg.getAuthToken }),
      ...(this.cfg.webSocketFactory && { webSocketFactory: this.cfg.webSocketFactory }),
      ...extraOptions,
    };
    const session = new WsAgentSession(opts);
    if (handlers) session.on(handlers);
    await session.open();
    return session;
  }
}
