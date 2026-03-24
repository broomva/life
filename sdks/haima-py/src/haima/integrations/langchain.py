"""LangChain/LangGraph tool integration for Haima x402 payments.

Usage with LangChain:
    from haima.integrations.langchain import HaimaPayTool
    tool = HaimaPayTool(facilitator_url="https://haima.broomva.tech")
    agent = create_react_agent(llm, [tool])

Usage with LangGraph:
    from haima.integrations.langchain import haima_pay_tool, haima_check_credit_tool
    tools = [haima_pay_tool(), haima_check_credit_tool()]
    graph = create_react_agent(llm, tools)
"""

from __future__ import annotations

import asyncio
from typing import Optional, Type

from pydantic import BaseModel, Field

from haima.client import HaimaClient
from haima.types import ChainId, PaymentPolicy
from haima.wallet import HaimaWallet

try:
    from langchain_core.tools import BaseTool
except ImportError as e:
    raise ImportError(
        "langchain-core is required for LangChain integration. "
        "Install with: pip install haima[langchain]"
    ) from e


class PayInput(BaseModel):
    """Input schema for the Haima pay tool."""

    recipient: str = Field(description="Recipient wallet address (EVM hex, e.g., 0x...)")
    amount_micro_usd: int = Field(
        description="Payment amount in micro-USD (1 USD = 1,000,000 micro-USD)",
        gt=0,
    )
    task_id: Optional[str] = Field(
        default=None,
        description="Task identifier for billing attribution",
    )


class CreditCheckInput(BaseModel):
    """Input schema for the credit check tool."""

    agent_id: str = Field(description="Agent identifier to check credit for")
    amount_micro_usd: int = Field(
        description="Amount in micro-USD to check if agent can spend",
        gt=0,
    )


class HaimaPayTool(BaseTool):
    """LangChain tool for making x402 payments through Haima.

    Allows AI agents to pay for services, APIs, and resources using
    the x402 payment protocol. Payments settle on-chain via USDC.
    """

    name: str = "haima_pay"
    description: str = (
        "Make a payment to another agent or service using the x402 protocol. "
        "Payments are settled on-chain in USDC via the Haima facilitator. "
        "Use this when you need to pay for an API call, service, or resource. "
        "Amount is in micro-USD (1 USD = 1,000,000 micro-USD)."
    )
    args_schema: Type[BaseModel] = PayInput

    _client: HaimaClient

    def __init__(
        self,
        facilitator_url: str = "http://localhost:3003",
        wallet: Optional[HaimaWallet] = None,
        policy: Optional[PaymentPolicy] = None,
        chain: ChainId = ChainId.BASE,
        api_key: Optional[str] = None,
        **kwargs,
    ):
        super().__init__(**kwargs)
        self._client = HaimaClient(
            facilitator_url=facilitator_url,
            wallet=wallet,
            policy=policy,
            chain=chain,
            api_key=api_key,
        )

    def _run(self, recipient: str, amount_micro_usd: int, task_id: Optional[str] = None) -> str:
        """Synchronous wrapper for the pay operation."""
        loop = asyncio.get_event_loop()
        if loop.is_running():
            import concurrent.futures

            with concurrent.futures.ThreadPoolExecutor() as pool:
                result = pool.submit(
                    asyncio.run,
                    self._client.pay(recipient, amount_micro_usd, task_id),
                ).result()
        else:
            result = asyncio.run(self._client.pay(recipient, amount_micro_usd, task_id))
        return result.model_dump_json()

    async def _arun(
        self, recipient: str, amount_micro_usd: int, task_id: Optional[str] = None
    ) -> str:
        """Async pay operation."""
        result = await self._client.pay(recipient, amount_micro_usd, task_id)
        return result.model_dump_json()


class HaimaCheckCreditTool(BaseTool):
    """LangChain tool for checking agent credit before spending."""

    name: str = "haima_check_credit"
    description: str = (
        "Check if an agent has sufficient credit to make a payment. "
        "Returns whether the specified amount can be spent."
    )
    args_schema: Type[BaseModel] = CreditCheckInput

    _client: HaimaClient

    def __init__(
        self,
        facilitator_url: str = "http://localhost:3003",
        api_key: Optional[str] = None,
        **kwargs,
    ):
        super().__init__(**kwargs)
        self._client = HaimaClient(facilitator_url=facilitator_url, api_key=api_key)

    def _run(self, agent_id: str, amount_micro_usd: int) -> str:
        result = asyncio.get_event_loop().run_until_complete(
            self._client.check_credit(agent_id, amount_micro_usd)
        )
        return f"Agent {agent_id} can spend {amount_micro_usd} μc: {result}"

    async def _arun(self, agent_id: str, amount_micro_usd: int) -> str:
        result = await self._client.check_credit(agent_id, amount_micro_usd)
        return f"Agent {agent_id} can spend {amount_micro_usd} μc: {result}"


def haima_pay_tool(
    facilitator_url: str = "http://localhost:3003",
    wallet: Optional[HaimaWallet] = None,
    policy: Optional[PaymentPolicy] = None,
    chain: ChainId = ChainId.BASE,
    api_key: Optional[str] = None,
) -> HaimaPayTool:
    """Factory function for creating a Haima pay tool."""
    return HaimaPayTool(
        facilitator_url=facilitator_url,
        wallet=wallet,
        policy=policy,
        chain=chain,
        api_key=api_key,
    )


def haima_check_credit_tool(
    facilitator_url: str = "http://localhost:3003",
    api_key: Optional[str] = None,
) -> HaimaCheckCreditTool:
    """Factory function for creating a credit check tool."""
    return HaimaCheckCreditTool(facilitator_url=facilitator_url, api_key=api_key)
