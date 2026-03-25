# haima

Multi-framework x402 payment SDK for AI agents. Add payments to your LangChain, LangGraph, or CrewAI agent in 5 minutes.

## Install

```bash
pip install haima                    # Core SDK
pip install haima[langchain]         # + LangChain/LangGraph
pip install haima[crewai]            # + CrewAI
pip install haima[all]               # Everything
```

## Quick Start

```python
from haima import HaimaClient

async with HaimaClient(facilitator_url="https://haima.broomva.tech") as client:
    receipt = await client.pay(
        recipient="0x742d35Cc6634C0532925a3b844Bc9e7595916Da2",
        amount_micro_usd=100,
        task_id="translate-doc",
    )
    print(f"Settled: {receipt.receipt.tx_hash}")
```

## LangChain / LangGraph

```python
from haima.integrations.langchain import haima_pay_tool
from langgraph.prebuilt import create_react_agent

tool = haima_pay_tool(facilitator_url="https://haima.broomva.tech")
agent = create_react_agent(llm, [tool])
```

## CrewAI

```python
from haima.integrations.crewai import HaimaPayTool
from crewai import Agent

tool = HaimaPayTool(facilitator_url="https://haima.broomva.tech")
agent = Agent(role="Payment Processor", tools=[tool])
```

## x402 Middleware

Transparent payment handling for any HTTP request:

```python
from haima import HaimaWallet, X402Middleware

middleware = X402Middleware(wallet=HaimaWallet.generate())
response = await middleware.get("https://api.example.com/premium-data")
# 402 responses are auto-signed and retried
```

## Configuration

| Env Var | Description |
|---------|-------------|
| `HAIMA_PRIVATE_KEY` | Wallet private key (hex) |
| `HAIMA_FACILITATOR_URL` | Facilitator endpoint |

## How It Works

1. Agent makes a payment request via `client.pay()`
2. SDK signs the payment with the agent's secp256k1 wallet
3. Signed payment submitted to Haima facilitator
4. Facilitator settles on-chain via USDC (Base L2)
5. Receipt returned with transaction hash

All amounts in **micro-USD** (1 USD = 1,000,000 μc).
