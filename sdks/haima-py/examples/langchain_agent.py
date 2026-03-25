"""Example: LangChain agent with Haima payment capabilities.

Add payments to your LangChain agent in 5 minutes:
  1. pip install haima[langchain]
  2. Set HAIMA_PRIVATE_KEY or let Haima generate a wallet
  3. Create the tool and add it to your agent
"""

import asyncio
import os

# --- Step 1: Create Haima payment tools ---
from haima.integrations.langchain import haima_pay_tool, haima_check_credit_tool

pay = haima_pay_tool(
    facilitator_url=os.getenv("HAIMA_FACILITATOR_URL", "http://localhost:3003"),
)
check = haima_check_credit_tool(
    facilitator_url=os.getenv("HAIMA_FACILITATOR_URL", "http://localhost:3003"),
)

# --- Step 2: Wire into a LangChain agent ---
from langchain_core.messages import HumanMessage

# With LangGraph (recommended):
# from langgraph.prebuilt import create_react_agent
# from langchain_openai import ChatOpenAI
#
# llm = ChatOpenAI(model="gpt-4o")
# agent = create_react_agent(llm, [pay, check])
#
# result = agent.invoke({
#     "messages": [HumanMessage(content="Pay 0x742d35Cc6634C0532925a3b844Bc9e7595916Da2 500 micro-usd for data-fetch")]
# })

# --- Step 3: Direct tool invocation (for testing) ---
async def main():
    print(f"Wallet: {pay._client.address}")
    print(f"Chain: {pay._client.chain.value}")

    # Check health
    healthy = await pay._client.health()
    print(f"Facilitator healthy: {healthy}")

    if healthy:
        # Make a payment
        result = await pay._arun(
            recipient="0x742d35Cc6634C0532925a3b844Bc9e7595916Da2",
            amount_micro_usd=100,
            task_id="example-translation",
        )
        print(f"Payment result: {result}")


if __name__ == "__main__":
    asyncio.run(main())
