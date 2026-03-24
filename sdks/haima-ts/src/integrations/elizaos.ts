/**
 * ElizaOS plugin for Haima x402 payments.
 *
 * Usage:
 *   import { haimaPlugin } from "@haima/sdk/elizaos";
 *   const agent = new AgentRuntime({ plugins: [haimaPlugin()] });
 *
 * ElizaOS plugin interface:
 *   - Plugin: { name, description, actions, providers }
 *   - Action: { name, description, handler, validate, similes, examples }
 *   - Provider: { get }
 */

import { HaimaClient } from "../client.js";
import type { HaimaConfig, FacilitateResponse } from "../types.js";

// ElizaOS types (peer dependency — not imported at compile time)
interface ElizaAction {
  name: string;
  description: string;
  similes: string[];
  examples: Array<Array<{ user: string; content: { text: string; action?: string } }>>;
  validate: (runtime: unknown, message: unknown) => Promise<boolean>;
  handler: (
    runtime: unknown,
    message: unknown,
    state: unknown,
    options: unknown,
    callback: (response: { text: string; action?: string }) => void,
  ) => Promise<void>;
}

interface ElizaProvider {
  get: (runtime: unknown, message: unknown) => Promise<string>;
}

interface ElizaPlugin {
  name: string;
  description: string;
  actions: ElizaAction[];
  providers: ElizaProvider[];
}

function extractPayParams(text: string): {
  recipient?: string;
  amount?: number;
  taskId?: string;
} {
  // Parse "pay 0x... 1000 for task-123" style messages
  const addrMatch = text.match(/0x[a-fA-F0-9]{40}/);
  const amountMatch = text.match(/(\d+)\s*(?:micro[-_]?usd|μc|usd)/i);
  const taskMatch = text.match(/(?:for|task[_-]?id?)\s+([^\s,]+)/i);
  return {
    recipient: addrMatch?.[0],
    amount: amountMatch ? parseInt(amountMatch[1], 10) : undefined,
    taskId: taskMatch?.[1],
  };
}

/** Create the Haima pay action for ElizaOS. */
function createPayAction(client: HaimaClient): ElizaAction {
  return {
    name: "HAIMA_PAY",
    description:
      "Make a payment to another agent or service using the x402 protocol. " +
      "Payments settle on-chain in USDC via the Haima facilitator.",
    similes: [
      "PAY_AGENT",
      "SEND_PAYMENT",
      "TRANSFER_USDC",
      "X402_PAY",
      "MAKE_PAYMENT",
    ],
    examples: [
      [
        {
          user: "{{user1}}",
          content: {
            text: "Pay 0x1234567890abcdef1234567890abcdef12345678 1000 micro-usd for translation",
          },
        },
        {
          user: "{{agent}}",
          content: {
            text: "Payment of 1000 μc settled successfully. TX: 0xabc123...",
            action: "HAIMA_PAY",
          },
        },
      ],
    ],
    validate: async (_runtime: unknown, message: unknown) => {
      const text = (message as { content?: { text?: string } })?.content?.text ?? "";
      const params = extractPayParams(text);
      return params.recipient !== undefined && params.amount !== undefined;
    },
    handler: async (
      _runtime: unknown,
      message: unknown,
      _state: unknown,
      _options: unknown,
      callback: (response: { text: string }) => void,
    ) => {
      const text = (message as { content?: { text?: string } })?.content?.text ?? "";
      const params = extractPayParams(text);

      if (!params.recipient || !params.amount) {
        callback({
          text: "Could not parse payment details. Use format: pay 0x... <amount> micro-usd [for <task-id>]",
        });
        return;
      }

      try {
        const result: FacilitateResponse = await client.pay(
          params.recipient,
          params.amount,
          params.taskId,
        );

        if (result.status === "settled" && result.receipt) {
          callback({
            text: `Payment of ${params.amount} μc settled. TX: ${result.receipt.tx_hash} on ${result.receipt.chain}`,
          });
        } else if (result.status === "rejected") {
          callback({ text: `Payment rejected: ${result.reason ?? "unknown"}` });
        } else {
          callback({ text: `Payment pending: ${result.details ?? "awaiting settlement"}` });
        }
      } catch (err) {
        callback({ text: `Payment failed: ${(err as Error).message}` });
      }
    },
  };
}

/** Create the Haima wallet info provider for ElizaOS. */
function createWalletProvider(client: HaimaClient): ElizaProvider {
  return {
    get: async () => {
      const info = client.walletInfo;
      const healthy = await client.health();
      return (
        `Haima Wallet: ${info.address} (${info.chain})\n` +
        `Facilitator: ${client.facilitatorUrl} (${healthy ? "healthy" : "unreachable"})`
      );
    },
  };
}

/**
 * Create the Haima ElizaOS plugin.
 *
 * @example
 *   import { haimaPlugin } from "@haima/sdk/elizaos";
 *   const agent = new AgentRuntime({
 *     plugins: [haimaPlugin({ facilitatorUrl: "https://haima.broomva.tech" })],
 *   });
 */
export function haimaPlugin(config?: HaimaConfig): ElizaPlugin {
  const client = new HaimaClient(config);

  return {
    name: "haima",
    description: "x402 payment capabilities via Haima facilitator",
    actions: [createPayAction(client)],
    providers: [createWalletProvider(client)],
  };
}
