/**
 * Example: ElizaOS agent with Haima payment capabilities.
 *
 * Add payments to your ElizaOS agent in 5 minutes:
 *   1. npm install @haima/sdk
 *   2. Import the plugin and add to your agent
 */

import { haimaPlugin } from "@haima/sdk/elizaos";

// --- Step 1: Create the Haima plugin ---
const plugin = haimaPlugin({
  facilitatorUrl: process.env.HAIMA_FACILITATOR_URL ?? "http://localhost:3003",
  // privateKey: process.env.HAIMA_PRIVATE_KEY as `0x${string}`,
});

console.log("Plugin:", plugin.name);
console.log("Actions:", plugin.actions.map((a) => a.name));

// --- Step 2: Add to ElizaOS agent ---
// import { AgentRuntime } from "@elizaos/core";
//
// const agent = new AgentRuntime({
//   plugins: [plugin],
//   // ... other config
// });
//
// The HAIMA_PAY action is now available to the agent.
// When a user says "Pay 0x... 1000 micro-usd for translation",
// the agent will automatically sign and submit the payment.

// --- Step 3: Test the action directly ---
async function main() {
  const payAction = plugin.actions[0];

  // Simulate a message
  const mockMessage = {
    content: {
      text: "Pay 0x742d35Cc6634C0532925a3b844Bc9e7595916Da2 100 micro-usd for test",
    },
  };

  // Validate
  const valid = await payAction.validate(null, mockMessage);
  console.log("Valid:", valid);

  // Get wallet info from provider
  const info = await plugin.providers[0].get(null, null);
  console.log(info);
}

main().catch(console.error);
