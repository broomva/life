"""Haima — Multi-framework x402 payment SDK for AI agents."""

from haima.client import HaimaClient
from haima.types import (
    ChainId,
    FacilitateResponse,
    FacilitationStatus,
    PaymentDecision,
    PaymentPolicy,
    PaymentScheme,
    SettlementReceipt,
    WalletInfo,
)
from haima.wallet import HaimaWallet
from haima.x402 import X402Middleware

__version__ = "0.1.0"

__all__ = [
    "HaimaClient",
    "HaimaWallet",
    "X402Middleware",
    "ChainId",
    "FacilitateResponse",
    "FacilitationStatus",
    "PaymentDecision",
    "PaymentPolicy",
    "PaymentScheme",
    "SettlementReceipt",
    "WalletInfo",
]
