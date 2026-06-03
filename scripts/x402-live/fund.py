#!/usr/bin/env python3
"""Fund a Base Sepolia address with USDC (+ ETH) via the CDP faucet.

The public web faucets are all login/captcha-gated; the CDP faucet API
(authenticated by a CDP API key) is programmatic. The key is read by file
PATH from $CDP_KEY_FILE and never printed.

Exits non-zero if any requested token failed to fund, so callers/CI don't
treat a failed fund as success.

Usage:
  CDP_KEY_FILE="/path/to/CDP API Key.json" FUND_ADDRESS=0x... \
    python3 scripts/x402-live/fund.py [tokens...]   # default: usdc eth
"""
import asyncio
import json
import os
import sys

from cdp import CdpClient


async def main() -> int:
    key = json.load(open(os.environ["CDP_KEY_FILE"]))
    addr = os.environ["FUND_ADDRESS"]
    tokens = sys.argv[1:] or ["usdc", "eth"]
    failures = 0
    async with CdpClient(api_key_id=key["id"], api_key_secret=key["privateKey"]) as cdp:
        for token in tokens:
            try:
                r = await cdp.evm.request_faucet(
                    address=addr, network="base-sepolia", token=token
                )
                txh = getattr(r, "transaction_hash", None) or str(r)
                print(f"FAUCET_OK token={token} tx={txh}")
            except Exception as e:  # noqa: BLE001
                failures += 1
                print(f"FAUCET_ERR token={token} err={type(e).__name__}: {e}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
