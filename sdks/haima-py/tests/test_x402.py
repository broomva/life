"""Tests for x402 protocol helpers."""

import base64
import json

from haima.x402 import parse_payment_required, parse_payment_response


def test_parse_payment_required():
    header = {
        "schemes": [
            {
                "scheme": "exact",
                "network": "eip155:8453",
                "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "amount": "1000",
                "recipient": "0x742d35Cc6634C0532925a3b844Bc9e7595916Da2",
                "facilitator": "https://haima.broomva.tech",
            }
        ],
        "version": "v2",
    }
    encoded = base64.b64encode(json.dumps(header).encode()).decode()
    schemes = parse_payment_required(encoded)

    assert len(schemes) == 1
    assert schemes[0].scheme == "exact"
    assert schemes[0].network == "eip155:8453"
    assert schemes[0].amount == "1000"


def test_parse_payment_required_empty():
    header = {"schemes": []}
    encoded = base64.b64encode(json.dumps(header).encode()).decode()
    schemes = parse_payment_required(encoded)
    assert len(schemes) == 0


def test_parse_payment_response():
    header = {
        "tx_hash": "0xabc123",
        "network": "eip155:8453",
        "settled": True,
    }
    encoded = base64.b64encode(json.dumps(header).encode()).decode()
    result = parse_payment_response(encoded)

    assert result["tx_hash"] == "0xabc123"
    assert result["settled"] is True
