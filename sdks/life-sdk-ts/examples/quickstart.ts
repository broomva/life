/**
 * Quickstart: connect to a local lifegw, create a session, send a
 * message, watch events stream back, then read the wallet balance.
 *
 * Run with: `pnpm example:quickstart` (or `tsx examples/quickstart.ts`).
 *
 * Assumes lifegw is reachable at `https://localhost:443` with a dev
 * signer accepting `Bearer test-token-for-{user_id}`.
 */

import { LifeClient } from "../src/index.js";

const BASE_URL = process.env.LIFE_BASE_URL ?? "https://localhost:443";
const USER_ID = process.env.LIFE_USER_ID ?? "u-quickstart";
const PROJECT_ID = process.env.LIFE_PROJECT_ID ?? "p-quickstart";

async function main(): Promise<void> {
  const life = new LifeClient({
    baseUrl: BASE_URL,
    getAuthToken: async () => `test-token-for-${USER_ID}`,
  });

  // 1. Resolve identity to confirm auth works.
  const me = await life.identity.whoami();
  console.log("[whoami]", me);

  // 2. Open a session.
  const session = await life.agent.createSession({
    userId: USER_ID,
    projectId: PROJECT_ID,
    label: "quickstart-session",
  });
  console.log("[session]", session.sid.value);

  // 3. Send a message and stream events back.
  console.log("[stream] sending message…");
  for await (const event of life.agent.sendMessage({
    sid: session.sid,
    content: "Hello from @broomva/life-sdk quickstart",
  })) {
    console.log("[event]", event.kind, "seq=", event.record.sequence.toString());
    if (event.kind === "AGENT_EVENT_KIND_FINISH") break;
    if (event.kind === "AGENT_EVENT_KIND_ERROR") {
      console.error("server error event");
      break;
    }
  }

  // 4. Read wallet balance.
  const balance = await life.wallet.getBalance({
    userId: USER_ID,
    projectId: PROJECT_ID,
  });
  console.log("[balance]", balance.micros.toString(), balance.currency);

  // 5. Close the session.
  await life.agent.closeSession({ sid: session.sid });
  console.log("[done]");
}

main().catch((err: unknown) => {
  console.error("[error]", err);
  process.exit(1);
});
