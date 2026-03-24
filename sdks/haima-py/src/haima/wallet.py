"""Wallet management — secp256k1 keypair generation and signing."""

from __future__ import annotations

import base64
import json
import os
from typing import Optional

from eth_account import Account
from eth_account.messages import encode_defunct

from haima.types import ChainId, WalletInfo


class HaimaWallet:
    """Local secp256k1 wallet for x402 payment signing.

    Generates or loads an EVM-compatible wallet. Private keys are held in memory
    only — use environment variables or encrypted storage for persistence.
    """

    def __init__(self, private_key: Optional[str] = None, chain: ChainId = ChainId.BASE):
        if private_key:
            self._account = Account.from_key(private_key)
        else:
            self._account = Account.create(os.urandom(32).hex())
        self._chain = chain

    @property
    def address(self) -> str:
        return self._account.address

    @property
    def chain(self) -> ChainId:
        return self._chain

    @property
    def info(self) -> WalletInfo:
        return WalletInfo(address=self.address, chain=self._chain)

    def sign_message(self, message: bytes) -> bytes:
        """EIP-191 personal_sign."""
        msg = encode_defunct(primitive=message)
        signed = self._account.sign_message(msg)
        return signed.signature

    def sign_payment_header(
        self,
        scheme: str,
        network: str,
        resource_url: str,
        amount: str,
        recipient: str,
    ) -> str:
        """Sign a payment and return a base64-encoded PAYMENT-SIGNATURE header.

        Matches the Rust `PaymentSignatureHeader` format:
        base64(json({ scheme, network, payload }))
        """
        # Create the message to sign: hash of payment details
        msg_bytes = f"{resource_url}:{amount}:{recipient}".encode()
        signature = self.sign_message(msg_bytes)

        header = {
            "scheme": scheme,
            "network": network,
            "payload": signature.hex(),
        }
        return base64.b64encode(json.dumps(header).encode()).decode()

    @classmethod
    def from_env(cls, chain: ChainId = ChainId.BASE) -> "HaimaWallet":
        """Load wallet from HAIMA_PRIVATE_KEY environment variable."""
        key = os.environ.get("HAIMA_PRIVATE_KEY")
        if not key:
            raise ValueError("HAIMA_PRIVATE_KEY environment variable not set")
        return cls(private_key=key, chain=chain)

    @classmethod
    def generate(cls, chain: ChainId = ChainId.BASE) -> "HaimaWallet":
        """Generate a new wallet with a random keypair."""
        return cls(chain=chain)
