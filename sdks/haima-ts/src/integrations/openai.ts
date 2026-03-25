/**
 * OpenAI Agents SDK tool for Haima x402 payments.
 *
 * Usage with OpenAI Agents SDK:
 *   import { haimaPayTool, haimaCheckCreditTool } from "@haima/sdk/openai";
 *   import OpenAI from "openai";
 *
 *   const tools = [haimaPayTool(), haimaCheckCreditTool()];
 *   // Use with OpenAI function calling or Agents SDK
 *
 * Compatible with OpenAI's function calling format and the Agents SDK.
 */

import { HaimaClient } from "../client.js";
import type { HaimaConfig, FacilitateResponse } from "../types.js";

/** OpenAI function tool definition format. */
interface OpenAITool {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: Record<string, unknown>;
    strict?: boolean;
  };
}

/** Tool definition + execution handler pair. */
export interface HaimaTool {
  definition: OpenAITool;
  execute: (args: Record<string, unknown>) => Promise<string>;
}

/**
 * Create a Haima pay tool for OpenAI function calling / Agents SDK.
 *
 * Returns both the tool definition (for the API) and an execute function
 * (for handling tool calls).
 */
export function haimaPayTool(config?: HaimaConfig): HaimaTool {
  const client = new HaimaClient(config);

  return {
    definition: {
      type: "function",
      function: {
        name: "haima_pay",
        description:
          "Make a payment to another agent or service using the x402 protocol. " +
          "Payments settle on-chain in USDC via the Haima facilitator. " +
          "Amount is in micro-USD (1 USD = 1,000,000 micro-USD).",
        parameters: {
          type: "object",
          properties: {
            recipient: {
              type: "string",
              description: "Recipient wallet address (EVM hex, e.g., 0x...)",
            },
            amount_micro_usd: {
              type: "number",
              description: "Payment amount in micro-USD (1 USD = 1,000,000)",
            },
            task_id: {
              type: "string",
              description: "Optional task identifier for billing attribution",
            },
          },
          required: ["recipient", "amount_micro_usd"],
          additionalProperties: false,
        },
        strict: true,
      },
    },
    execute: async (args: Record<string, unknown>): Promise<string> => {
      const recipient = args.recipient as string;
      const amount = args.amount_micro_usd as number;
      const taskId = args.task_id as string | undefined;

      try {
        const result: FacilitateResponse = await client.pay(
          recipient,
          amount,
          taskId,
        );
        return JSON.stringify(result);
      } catch (err) {
        return JSON.stringify({
          status: "rejected",
          reason: (err as Error).message,
        });
      }
    },
  };
}

/**
 * Create a credit check tool for OpenAI function calling / Agents SDK.
 */
export function haimaCheckCreditTool(config?: HaimaConfig): HaimaTool {
  const client = new HaimaClient(config);

  return {
    definition: {
      type: "function",
      function: {
        name: "haima_check_credit",
        description:
          "Check if an agent has sufficient credit to make a payment. " +
          "Returns whether the specified amount can be spent.",
        parameters: {
          type: "object",
          properties: {
            agent_id: {
              type: "string",
              description: "Agent identifier to check credit for",
            },
            amount_micro_usd: {
              type: "number",
              description: "Amount in micro-USD to check",
            },
          },
          required: ["agent_id", "amount_micro_usd"],
          additionalProperties: false,
        },
        strict: true,
      },
    },
    execute: async (args: Record<string, unknown>): Promise<string> => {
      const agentId = args.agent_id as string;
      const amount = args.amount_micro_usd as number;

      try {
        const allowed = await client.checkCredit(agentId, amount);
        return JSON.stringify({ agent_id: agentId, amount, allowed });
      } catch (err) {
        return JSON.stringify({
          agent_id: agentId,
          amount,
          allowed: false,
          error: (err as Error).message,
        });
      }
    },
  };
}

/**
 * Handle an OpenAI tool call by dispatching to the correct Haima tool.
 *
 * Usage:
 *   const tools = [haimaPayTool(), haimaCheckCreditTool()];
 *   // After receiving a tool call from the API:
 *   const result = await handleToolCall(tools, toolCall.function.name, toolCall.function.arguments);
 */
export async function handleToolCall(
  tools: HaimaTool[],
  name: string,
  argsJson: string,
): Promise<string> {
  const tool = tools.find((t) => t.definition.function.name === name);
  if (!tool) {
    return JSON.stringify({ error: `Unknown tool: ${name}` });
  }
  const args = JSON.parse(argsJson) as Record<string, unknown>;
  return tool.execute(args);
}
