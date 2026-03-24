# @haima/sdk

Multi-framework x402 payment SDK for AI agents. Add payments to your ElizaOS or OpenAI agent in 5 minutes.

## Install

```bash
npm install @haima/sdk
```

## Quick Start

```typescript
import { HaimaClient } from "@haima/sdk";

const client = new HaimaClient({
  facilitatorUrl: "https://haima.broomva.tech",
});

const receipt = await client.pay(
  "0x742d35Cc6634C0532925a3b844Bc9e7595916Da2",
  100,
  "translate-doc",
);
console.log(`Settled: ${receipt.receipt?.tx_hash}`);
```

## ElizaOS

```typescript
import { haimaPlugin } from "@haima/sdk/elizaos";

const agent = new AgentRuntime({
  plugins: [haimaPlugin({ facilitatorUrl: "https://haima.broomva.tech" })],
});
// HAIMA_PAY action is now available to the agent
```

## OpenAI Agents SDK

```typescript
import { haimaPayTool, handleToolCall } from "@haima/sdk/openai";

const tools = [haimaPayTool()];

// Use with OpenAI function calling
const response = await openai.chat.completions.create({
  model: "gpt-4o",
  tools: tools.map((t) => t.definition),
  messages: [{ role: "user", content: "Pay 0x742d... 500 for translation" }],
});

// Handle tool calls
for (const call of response.choices[0].message.tool_calls ?? []) {
  const result = await handleToolCall(tools, call.function.name, call.function.arguments);
}
```

## x402 Middleware

```typescript
import { HaimaWallet, X402Middleware } from "@haima/sdk";

const middleware = new X402Middleware(HaimaWallet.generate());
const response = await middleware.fetch("https://api.example.com/premium-data");
// 402 responses are auto-signed and retried
```

## Configuration

| Env Var | Description |
|---------|-------------|
| `HAIMA_PRIVATE_KEY` | Wallet private key (0x-prefixed hex) |
| `HAIMA_FACILITATOR_URL` | Facilitator endpoint |

## How It Works

1. Agent makes a payment request via `client.pay()`
2. SDK signs with the agent's secp256k1 wallet (via viem)
3. Signed payment submitted to Haima facilitator
4. Facilitator settles on-chain via USDC (Base L2)
5. Receipt returned with transaction hash

All amounts in **micro-USD** (1 USD = 1,000,000 μc).
