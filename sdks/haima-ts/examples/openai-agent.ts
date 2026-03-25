/**
 * Example: OpenAI Agents SDK with Haima payment capabilities.
 *
 * Add payments to your OpenAI agent in 5 minutes:
 *   1. npm install @haima/sdk openai
 *   2. Create tools and wire into function calling
 */

import {
  haimaPayTool,
  haimaCheckCreditTool,
  handleToolCall,
} from "@haima/sdk/openai";

// --- Step 1: Create Haima tools ---
const payTool = haimaPayTool({
  facilitatorUrl: process.env.HAIMA_FACILITATOR_URL ?? "http://localhost:3003",
});
const creditTool = haimaCheckCreditTool({
  facilitatorUrl: process.env.HAIMA_FACILITATOR_URL ?? "http://localhost:3003",
});

const tools = [payTool, creditTool];

// --- Step 2: Use with OpenAI function calling ---
// import OpenAI from "openai";
//
// const openai = new OpenAI();
//
// const response = await openai.chat.completions.create({
//   model: "gpt-4o",
//   messages: [
//     { role: "user", content: "Pay 0x742d... 500 micro-usd for translation" },
//   ],
//   tools: tools.map((t) => t.definition),
// });
//
// // Handle tool calls
// for (const toolCall of response.choices[0].message.tool_calls ?? []) {
//   const result = await handleToolCall(
//     tools,
//     toolCall.function.name,
//     toolCall.function.arguments,
//   );
//   console.log(`Tool ${toolCall.function.name}:`, result);
// }

// --- Step 3: Direct execution (for testing) ---
async function main() {
  console.log("Tool definitions:");
  for (const tool of tools) {
    console.log(`  ${tool.definition.function.name}: ${tool.definition.function.description.slice(0, 60)}...`);
  }

  // Execute pay tool directly
  const result = await payTool.execute({
    recipient: "0x742d35Cc6634C0532925a3b844Bc9e7595916Da2",
    amount_micro_usd: 100,
    task_id: "example-translation",
  });
  console.log("Pay result:", result);

  // Use the generic handler
  const creditResult = await handleToolCall(
    tools,
    "haima_check_credit",
    JSON.stringify({ agent_id: "agent-001", amount_micro_usd: 500 }),
  );
  console.log("Credit check:", creditResult);
}

main().catch(console.error);
