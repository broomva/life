"""Tests for HaimaClient — local policy evaluation and construction."""

import pytest

from haima.client import HaimaClient
from haima.types import FacilitationStatus, PaymentPolicy


@pytest.mark.asyncio
async def test_pay_denied_by_policy_returns_proper_status():
    """Verify that local policy denial returns FacilitationStatus.REJECTED (not a raw string)."""
    policy = PaymentPolicy(hard_cap_per_tx=1000)
    client = HaimaClient(policy=policy)
    try:
        resp = await client.pay(
            recipient="0x742d35Cc6634C0532925a3b844Bc9e7595916Da2",
            amount_micro_usd=2000,  # exceeds hard_cap_per_tx
        )
        assert resp.status == FacilitationStatus.REJECTED
        assert resp.reason is not None
        assert "denied" in resp.reason.lower()
    finally:
        await client.close()


@pytest.mark.asyncio
async def test_client_wallet_address():
    """Client generates a wallet with a valid address."""
    client = HaimaClient()
    try:
        assert client.address.startswith("0x")
        assert len(client.address) == 42
    finally:
        await client.close()


@pytest.mark.asyncio
async def test_client_wallet_info():
    """Client wallet_info returns correct WalletInfo."""
    from haima.types import ChainId

    client = HaimaClient(chain=ChainId.BASE_SEPOLIA)
    try:
        info = client.wallet_info
        assert info.chain == ChainId.BASE_SEPOLIA
        assert info.address == client.address
    finally:
        await client.close()
