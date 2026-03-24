"""Core types mirroring haima-core Rust types."""

from __future__ import annotations

from datetime import datetime
from enum import Enum
from typing import Optional

from pydantic import BaseModel, Field


class ChainId(str, Enum):
    """CAIP-2 chain identifiers."""

    BASE = "eip155:8453"
    BASE_SEPOLIA = "eip155:84532"
    ETHEREUM = "eip155:1"


# 1 USDC = 1,000,000 micro-credits (USDC has 6 decimals)
USDC_TO_MICRO_CREDITS = 1_000_000

# Default USDC contract addresses
USDC_CONTRACTS = {
    ChainId.BASE: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    ChainId.BASE_SEPOLIA: "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
    ChainId.ETHEREUM: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
}


class WalletInfo(BaseModel):
    """Wallet address with chain."""

    address: str
    chain: ChainId = ChainId.BASE


class PaymentScheme(BaseModel):
    """x402 payment scheme (currently only 'exact')."""

    scheme: str = "exact"
    network: str
    token: str
    amount: str
    recipient: str
    facilitator: str


class PaymentDecision(str, Enum):
    """Payment policy verdict."""

    APPROVED = "approved"
    REQUIRES_APPROVAL = "requires_approval"
    DENIED = "denied"


class PaymentPolicy(BaseModel):
    """Payment policy thresholds (micro-credits)."""

    auto_approve_cap: int = 100
    hard_cap_per_tx: int = 1_000_000
    session_spend_cap: int = 10_000_000
    max_tx_per_minute: int = 10
    enabled: bool = True

    def evaluate(self, micro_credits: int) -> PaymentDecision:
        if not self.enabled:
            return PaymentDecision.DENIED
        if micro_credits > self.hard_cap_per_tx:
            return PaymentDecision.DENIED
        if micro_credits <= self.auto_approve_cap:
            return PaymentDecision.APPROVED
        return PaymentDecision.REQUIRES_APPROVAL


class FacilitationStatus(str, Enum):
    """Facilitator settlement status."""

    SETTLED = "settled"
    REJECTED = "rejected"
    PENDING = "pending"


class SettlementReceipt(BaseModel):
    """On-chain settlement receipt."""

    tx_hash: str
    payer: str
    payee: str
    amount_micro_usd: int
    chain: str
    settled_at: datetime


class FacilitateResponse(BaseModel):
    """Response from POST /v1/facilitate."""

    status: FacilitationStatus
    receipt: Optional[SettlementReceipt] = None
    facilitator_fee_bps: Optional[int] = None
    trust_attestation: Optional[dict] = None
    reason: Optional[str] = None
    details: Optional[str] = None


class CreditScore(BaseModel):
    """Agent credit score from the bureau."""

    agent_id: str
    score: int = Field(ge=0, le=1000)
    tier: str
    max_credit_line: int = 0


class FacilitatorStats(BaseModel):
    """Facilitator dashboard statistics."""

    total_transactions: int = 0
    total_volume_micro_usd: int = 0
    total_fees_micro_usd: int = 0
    total_rejected: int = 0
