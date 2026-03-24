"""Example: Using x402 middleware for transparent payment handling.

The X402Middleware wraps HTTP requests to automatically handle 402 responses.
When a server returns 402 Payment Required, the middleware signs the payment
and retries — invisible to the calling code.
"""

import asyncio
import os

from haima import HaimaWallet, X402Middleware, PaymentPolicy


async def main():
    # Create wallet and middleware
    wallet = HaimaWallet()
    middleware = X402Middleware(
        wallet=wallet,
        policy=PaymentPolicy(auto_approve_cap=1000),  # Auto-approve up to 1000 μc
    )

    print(f"Wallet: {wallet.address}")

    # Make requests through the middleware — 402s are handled automatically
    # response = await middleware.get("https://api.example.com/premium-data")
    # The middleware will:
    #   1. Send GET request
    #   2. If 402 → parse PAYMENT-REQUIRED header
    #   3. Evaluate against policy
    #   4. Sign with wallet
    #   5. Retry with PAYMENT-SIGNATURE header
    #   6. Return the final response

    print("X402Middleware ready for transparent payment handling")
    print(f"  Auto-approve cap: {middleware.policy.auto_approve_cap} μc")
    print(f"  Session spend cap: {middleware.policy.session_spend_cap} μc")

    await middleware.close()


if __name__ == "__main__":
    asyncio.run(main())
