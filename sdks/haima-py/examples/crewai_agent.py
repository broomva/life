"""Example: CrewAI agent with Haima payment capabilities.

Add payments to your CrewAI agent in 5 minutes:
  1. pip install haima[crewai]
  2. Create the tool and add it to your agent
"""

import os

from haima.integrations.crewai import HaimaPayTool, HaimaCheckCreditTool

# --- Step 1: Create Haima tools ---
pay_tool = HaimaPayTool(
    facilitator_url=os.getenv("HAIMA_FACILITATOR_URL", "http://localhost:3003"),
)
credit_tool = HaimaCheckCreditTool(
    facilitator_url=os.getenv("HAIMA_FACILITATOR_URL", "http://localhost:3003"),
)

# --- Step 2: Wire into a CrewAI agent ---
# from crewai import Agent, Task, Crew
#
# payment_agent = Agent(
#     role="Payment Processor",
#     goal="Process x402 payments for API calls and services",
#     backstory="You manage payments for the agent team using the Haima x402 protocol.",
#     tools=[pay_tool, credit_tool],
#     verbose=True,
# )
#
# task = Task(
#     description="Pay 0x742d35Cc6634C0532925a3b844Bc9e7595916Da2 500 micro-usd for data analysis",
#     expected_output="Payment confirmation with transaction hash",
#     agent=payment_agent,
# )
#
# crew = Crew(agents=[payment_agent], tasks=[task])
# result = crew.kickoff()
# print(result)

# --- Step 3: Direct tool invocation (for testing) ---
if __name__ == "__main__":
    result = pay_tool._run(
        recipient="0x742d35Cc6634C0532925a3b844Bc9e7595916Da2",
        amount_micro_usd=100,
        task_id="example-data-analysis",
    )
    print(f"Payment result: {result}")
