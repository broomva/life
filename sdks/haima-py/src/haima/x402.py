"""x402 protocol helpers — header parsing, middleware, and auto-pay."""

from __future__ import annotations

import base64
import json
import logging
from typing import Optional

import httpx

from haima.types import PaymentPolicy, PaymentScheme
from haima.wallet import HaimaWallet

logger = logging.getLogger("haima.x402")


def parse_payment_required(header_value: str) -> list[PaymentScheme]:
    """Parse a base64-encoded PAYMENT-REQUIRED header into scheme requirements."""
    decoded = json.loads(base64.b64decode(header_value))
    schemes = decoded.get("schemes", [])
    return [PaymentScheme(**s) for s in schemes]


def parse_payment_response(header_value: str) -> dict:
    """Parse a base64-encoded PAYMENT-RESPONSE header."""
    return json.loads(base64.b64decode(header_value))


class X402Middleware:
    """HTTP transport middleware that auto-handles x402 payment flows.

    Wraps httpx to intercept 402 responses, evaluate against policy,
    sign with wallet, and retry — transparent to the caller.
    """

    def __init__(
        self,
        wallet: HaimaWallet,
        policy: Optional[PaymentPolicy] = None,
        auto_approve: bool = True,
    ):
        self.wallet = wallet
        self.policy = policy or PaymentPolicy()
        self.auto_approve = auto_approve
        self._session_spend = 0
        self._client = httpx.AsyncClient()

    async def request(
        self,
        method: str,
        url: str,
        **kwargs,
    ) -> httpx.Response:
        """Make an HTTP request with automatic x402 handling."""
        response = await self._client.request(method, url, **kwargs)

        if response.status_code != 402:
            return response

        # Parse 402 response
        pr_header = response.headers.get("payment-required")
        if not pr_header:
            return response

        schemes = parse_payment_required(pr_header)
        if not schemes:
            return response

        # Select first compatible scheme (EVM "exact" only)
        scheme = next((s for s in schemes if s.scheme == "exact"), None)
        if not scheme:
            logger.warning("No compatible payment scheme found")
            return response

        # Evaluate against policy
        micro_credits = int(scheme.amount)
        decision = self.policy.evaluate(micro_credits)

        if decision.value == "denied":
            logger.warning("Payment denied by policy: %d μc", micro_credits)
            return response

        if decision.value == "requires_approval" and not self.auto_approve:
            logger.info("Payment requires approval: %d μc", micro_credits)
            return response

        # Check session spend cap
        if self._session_spend + micro_credits > self.policy.session_spend_cap:
            logger.warning("Session spend cap exceeded")
            return response

        # Sign and retry
        signature = self.wallet.sign_payment_header(
            scheme=scheme.scheme,
            network=scheme.network,
            resource_url=url,
            amount=scheme.amount,
            recipient=scheme.recipient,
        )

        headers = dict(kwargs.get("headers", {}))
        headers["payment-signature"] = signature
        kwargs["headers"] = headers

        retry_response = await self._client.request(method, url, **kwargs)
        if retry_response.status_code == 200:
            self._session_spend += micro_credits
            logger.info("Payment settled: %d μc (session total: %d μc)", micro_credits, self._session_spend)

        return retry_response

    async def get(self, url: str, **kwargs) -> httpx.Response:
        return await self.request("GET", url, **kwargs)

    async def post(self, url: str, **kwargs) -> httpx.Response:
        return await self.request("POST", url, **kwargs)

    def reset_session(self) -> None:
        self._session_spend = 0

    async def close(self) -> None:
        await self._client.aclose()
