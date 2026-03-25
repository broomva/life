"""Tests for wallet generation and signing."""

import base64
import json

from haima.wallet import HaimaWallet
from haima.types import ChainId


def test_generate_wallet():
    wallet = HaimaWallet.generate()
    assert wallet.address.startswith("0x")
    assert len(wallet.address) == 42
    assert wallet.chain == ChainId.BASE


def test_generate_different_chain():
    wallet = HaimaWallet.generate(chain=ChainId.BASE_SEPOLIA)
    assert wallet.chain == ChainId.BASE_SEPOLIA


def test_deterministic_from_key():
    key = "0x" + "ab" * 32
    w1 = HaimaWallet(private_key=key)
    w2 = HaimaWallet(private_key=key)
    assert w1.address == w2.address


def test_different_keys_different_addresses():
    w1 = HaimaWallet.generate()
    w2 = HaimaWallet.generate()
    assert w1.address != w2.address


def test_sign_message():
    wallet = HaimaWallet.generate()
    sig = wallet.sign_message(b"hello world")
    assert isinstance(sig, bytes)
    assert len(sig) == 65  # r (32) + s (32) + v (1)


def test_sign_payment_header():
    wallet = HaimaWallet.generate()
    header = wallet.sign_payment_header(
        scheme="exact",
        network="eip155:8453",
        resource_url="https://api.example.com/data",
        amount="1000",
        recipient="0x742d35Cc6634C0532925a3b844Bc9e7595916Da2",
    )
    # Should be base64-encoded JSON
    decoded = json.loads(base64.b64decode(header))
    assert decoded["scheme"] == "exact"
    assert decoded["network"] == "eip155:8453"
    assert decoded["payload"].startswith("0x") or len(decoded["payload"]) > 0


def test_wallet_info():
    wallet = HaimaWallet.generate()
    info = wallet.info
    assert info.address == wallet.address
    assert info.chain == wallet.chain
