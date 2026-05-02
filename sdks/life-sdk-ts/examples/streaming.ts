/**
 * Streaming: open a long-lived `Agent.StreamSession` over WebSocket,
 * print every event, automatically reconnect on transient drops.
 *
 * Run with: `pnpm example:streaming` (or `tsx examples/streaming.ts`).
 *
 * In Node, `globalThis.WebSocket` is provided by Node 22+. On older
 * runtimes pass `webSocketFactory` from the `ws` package.
 */

import { LifeClient } from "../src/index.js";

const BASE_URL = process.env.LIFE_BASE_URL ?? "https://localhost:443";
const SID = process.env.LIFE_SID ?? "sid-streaming-demo";
const USER_ID = process.env.LIFE_USER_ID ?? "u-streaming";

async function main(): Promise<void> {
  // Optional: use the `ws` package on older Node hosts. Node 22+ has
  // a built-in `WebSocket` so the dynamic import is best-effort.
  type LifeWsFactory = NonNullable<ConstructorParameters<typeof LifeClient>[0]["webSocketFactory"]>;
  let webSocketFactory: LifeWsFactory | undefined;
  try {
    const wsMod = (await import("ws")) as unknown as {
      default: new (u: string, p: string[]) => unknown;
    };
    webSocketFactory = (url, protocols) =>
      new wsMod.default(url, protocols) as ReturnType<LifeWsFactory>;
  } catch {
    // Fall back to globalThis.WebSocket
    webSocketFactory = undefined;
  }

  const life = new LifeClient({
    baseUrl: BASE_URL,
    getAuthToken: async () => `test-token-for-${USER_ID}`,
    webSocketFactory,
  });

  console.log(`[ws] connecting to ${BASE_URL} sid=${SID}…`);

  const session = await life.streamSession(SID, {
    onOpen: () => console.log("[ws] open"),
    onAgentEvent: (e) => {
      console.log("[event] seq=", e.seqNo.toString(), "kind=", e.agentKind);
    },
    onClosing: (reason) => console.log("[ws] closing:", reason),
    onError: (err) => console.error("[ws] error:", err.code, err.message),
    onClose: () => console.log("[ws] closed"),
  });

  // Send a chat message — server streams the response back.
  session.sendMessage("Hello over WebSocket");

  // Keep the process alive until the session closes (or Ctrl-C).
  process.on("SIGINT", () => {
    console.log("[ws] SIGINT — closing");
    session.close("sigint");
    process.exit(0);
  });

  // Park forever — the WS event loop keeps Node alive.
  await new Promise(() => {
    // never resolves
  });
}

main().catch((err: unknown) => {
  console.error("[error]", err);
  process.exit(1);
});
