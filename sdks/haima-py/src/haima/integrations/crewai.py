"""CrewAI tool integration for Haima x402 payments.

Usage:
    from haima.integrations.crewai import HaimaPayTool, HaimaCheckCreditTool
    from crewai import Agent, Task, Crew

    pay_tool = HaimaPayTool(facilitator_url="https://haima.broomva.tech")
    agent = Agent(
        role="Payment Agent",
        tools=[pay_tool],
        ...
    )
"""

from __future__ import annotations

import asyncio
from typing import Optional, Type

from pydantic import BaseModel, Field

from haima.client import HaimaClient
from haima.types import ChainId, PaymentPolicy
from haima.wallet import HaimaWallet

try:
    from crewai.tools import BaseTool as CrewAIBaseTool
except ImportError as e:
    raise ImportError(
        "crewai is required for CrewAI integration. "
        "Install with: pip install haima[crewai]"
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
    """Input for credit check."""

    agent_id: str = Field(description="Agent identifier")
    amount_micro_usd: int = Field(description="Amount in micro-USD to check", gt=0)


class HaimaPayTool(CrewAIBaseTool):
    """CrewAI tool for making x402 payments through Haima."""

    name: str = "haima_pay"
    description: str = (
        "Make a payment to another agent or service using the x402 protocol. "
        "Payments settle on-chain in USDC. Amount is in micro-USD "
        "(1 USD = 1,000,000 micro-USD)."
    )
    args_schema: Type[BaseModel] = PayInput

    facilitator_url: str = "http://localhost:3003"
    chain: ChainId = ChainId.BASE
    api_key: Optional[str] = None
    _wallet: Optional[HaimaWallet] = None
    _policy: Optional[PaymentPolicy] = None

    def __init__(
        self,
        facilitator_url: str = "http://localhost:3003",
        wallet: Optional[HaimaWallet] = None,
        policy: Optional[PaymentPolicy] = None,
        chain: ChainId = ChainId.BASE,
        api_key: Optional[str] = None,
        **kwargs,
    ):
        super().__init__(
            facilitator_url=facilitator_url,
            chain=chain,
            api_key=api_key,
            **kwargs,
        )
        self._wallet = wallet
        self._policy = policy

    def _run(
        self, recipient: str, amount_micro_usd: int, task_id: Optional[str] = None
    ) -> str:
        async def _do_pay():
            async with HaimaClient(
                facilitator_url=self.facilitator_url,
                wallet=self._wallet,
                policy=self._policy,
                chain=self.chain,
                api_key=self.api_key,
            ) as client:
                result = await client.pay(recipient, amount_micro_usd, task_id)
                return result.model_dump_json()

        try:
            asyncio.get_running_loop()
            import concurrent.futures

            with concurrent.futures.ThreadPoolExecutor() as pool:
                return pool.submit(asyncio.run, _do_pay()).result()
        except RuntimeError:
            return asyncio.run(_do_pay())


class HaimaCheckCreditTool(CrewAIBaseTool):
    """CrewAI tool for checking agent credit."""

    name: str = "haima_check_credit"
    description: str = (
        "Check if an agent has sufficient credit to make a payment. "
        "Returns whether the specified amount can be spent."
    )
    args_schema: Type[BaseModel] = CreditCheckInput

    facilitator_url: str = "http://localhost:3003"
    api_key: Optional[str] = None

    def _run(self, agent_id: str, amount_micro_usd: int) -> str:
        async def _do_check():
            async with HaimaClient(
                facilitator_url=self.facilitator_url,
                api_key=self.api_key,
            ) as client:
                allowed = await client.check_credit(agent_id, amount_micro_usd)
                return f"Agent {agent_id} can spend {amount_micro_usd} μc: {allowed}"

        try:
            asyncio.get_running_loop()
            import concurrent.futures

            with concurrent.futures.ThreadPoolExecutor() as pool:
                return pool.submit(asyncio.run, _do_check()).result()
        except RuntimeError:
            return asyncio.run(_do_check())
