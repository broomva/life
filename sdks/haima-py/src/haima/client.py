"""HaimaClient — high-level SDK for x402 payments through the Haima facilitator."""

from __future__ import annotations

import logging
from typing import Optional

import httpx

from haima.types import (
    ChainId,
    CreditScore,
    FacilitateResponse,
    FacilitatorStats,
    PaymentPolicy,
    USDC_CONTRACTS,
    WalletInfo,
)
from haima.wallet import HaimaWallet
from haima.x402 import X402Middleware

logger = logging.getLogger("haima")


class HaimaClient:
    """High-level client for Haima x402 payments.

    Usage:
        client = HaimaClient(facilitator_url="https://haima.broomva.tech")
        receipt = await client.pay("0xRecipient...", 100, task_id="translate-doc")
    """

    def __init__(
        self,
        facilitator_url: str = "http://localhost:3003",
        wallet: Optional[HaimaWallet] = None,
        policy: Optional[PaymentPolicy] = None,
        chain: ChainId = ChainId.BASE,
        api_key: Optional[str] = None,
    ):
        self.facilitator_url = facilitator_url.rstrip("/")
        self.wallet = wallet or HaimaWallet(chain=chain)
        self.policy = policy or PaymentPolicy()
        self.chain = chain
        self._api_key = api_key
        self._http = httpx.AsyncClient(
            base_url=self.facilitator_url,
            headers=self._auth_headers(),
        )
        self.x402 = X402Middleware(wallet=self.wallet, policy=self.policy)

    def _auth_headers(self) -> dict[str, str]:
        if self._api_key:
            return {"Authorization": f"Bearer {self._api_key}"}
        return {}

    @property
    def address(self) -> str:
        return self.wallet.address

    @property
    def wallet_info(self) -> WalletInfo:
        return self.wallet.info

    async def pay(
        self,
        recipient: str,
        amount_micro_usd: int,
        task_id: Optional[str] = None,
        agent_id: Optional[str] = None,
    ) -> FacilitateResponse:
        """Submit a payment through the Haima facilitator.

        This is the primary API: `haima.pay(recipient, amount, task_id)`

        Args:
            recipient: Payee wallet address (EVM hex).
            amount_micro_usd: Amount in micro-USD (1 USD = 1,000,000).
            task_id: Optional task identifier for billing attribution.
            agent_id: Optional agent ID for credit gating.

        Returns:
            FacilitateResponse with settlement receipt on success.
        """
        # Check policy before submitting
        decision = self.policy.evaluate(amount_micro_usd)
        if decision.value == "denied":
            return FacilitateResponse(
                status="rejected",
                reason=f"Payment denied by local policy: {amount_micro_usd} μc exceeds hard cap",
            )

        # Sign the payment
        usdc_contract = USDC_CONTRACTS.get(self.chain, USDC_CONTRACTS[ChainId.BASE])
        signature = self.wallet.sign_payment_header(
            scheme="exact",
            network=self.chain.value,
            resource_url=f"haima://{task_id or 'payment'}",
            amount=str(amount_micro_usd),
            recipient=recipient,
        )

        payload = {
            "payment_header": signature,
            "resource_url": f"haima://{task_id or 'payment'}",
            "amount_micro_usd": amount_micro_usd,
        }
        if agent_id:
            payload["agent_id"] = agent_id

        response = await self._http.post("/v1/facilitate", json=payload)
        response.raise_for_status()
        return FacilitateResponse(**response.json())

    async def check_credit(self, agent_id: str, amount: int) -> bool:
        """Check if an agent can spend a given amount."""
        response = await self._http.post(
            f"/v1/credit/{agent_id}/check",
            json={"amount_micro_usd": amount},
        )
        response.raise_for_status()
        return response.json().get("allowed", False)

    async def get_credit_score(self, agent_id: str) -> CreditScore:
        """Get an agent's credit score."""
        response = await self._http.get(f"/v1/credit/{agent_id}")
        response.raise_for_status()
        return CreditScore(**response.json())

    async def stats(self) -> FacilitatorStats:
        """Get facilitator statistics."""
        response = await self._http.get("/v1/facilitator/stats")
        response.raise_for_status()
        return FacilitatorStats(**response.json())

    async def health(self) -> bool:
        """Check facilitator health."""
        try:
            response = await self._http.get("/health")
            return response.status_code == 200
        except httpx.HTTPError:
            return False

    async def close(self) -> None:
        await self._http.aclose()
        await self.x402.close()

    async def __aenter__(self) -> "HaimaClient":
        return self

    async def __aexit__(self, *args) -> None:
        await self.close()
