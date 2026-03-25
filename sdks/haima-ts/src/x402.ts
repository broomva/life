/**
 * x402 protocol helpers — header parsing and auto-pay middleware.
 */

import type { PaymentScheme, PaymentPolicy } from "./types.js";
import { DEFAULT_POLICY, evaluatePolicy } from "./types.js";
import { HaimaWallet } from "./wallet.js";

/** Parse a base64-encoded PAYMENT-REQUIRED header. */
export function parsePaymentRequired(headerValue: string): PaymentScheme[] {
  const decoded = JSON.parse(atob(headerValue));
  return decoded.schemes ?? [];
}

/** Parse a base64-encoded PAYMENT-RESPONSE header. */
export function parsePaymentResponse(headerValue: string): {
  tx_hash: string;
  network: string;
  settled: boolean;
} {
  return JSON.parse(atob(headerValue));
}

/**
 * HTTP fetch wrapper that auto-handles x402 payment flows.
 *
 * Intercepts 402 responses, evaluates against policy,
 * signs with wallet, and retries — transparent to the caller.
 */
export class X402Middleware {
  private wallet: HaimaWallet;
  private policy: PaymentPolicy;
  private autoApprove: boolean;
  private sessionSpend = 0;

  constructor(
    wallet: HaimaWallet,
    policy?: Partial<PaymentPolicy>,
    autoApprove = true,
  ) {
    this.wallet = wallet;
    this.policy = { ...DEFAULT_POLICY, ...policy };
    this.autoApprove = autoApprove;
  }

  async fetch(url: string, init?: RequestInit): Promise<Response> {
    const response = await fetch(url, init);

    if (response.status !== 402) {
      return response;
    }

    const prHeader = response.headers.get("payment-required");
    if (!prHeader) return response;

    const schemes = parsePaymentRequired(prHeader);
    if (!schemes.length) return response;

    // Select first compatible scheme (EVM "exact" only)
    const scheme = schemes.find((s) => s.scheme === "exact");
    if (!scheme) return response;

    const microCredits = parseInt(scheme.amount, 10);
    const decision = evaluatePolicy(this.policy, microCredits);

    if (decision === "denied") return response;
    if (decision === "requires_approval" && !this.autoApprove) return response;

    // Check session spend cap
    if (this.sessionSpend + microCredits > this.policy.sessionSpendCap) {
      return response;
    }

    // Sign and retry
    const signature = await this.wallet.signPaymentHeader({
      scheme: scheme.scheme,
      network: scheme.network,
      resourceUrl: url,
      amount: scheme.amount,
      recipient: scheme.recipient,
    });

    const headers = new Headers(init?.headers);
    headers.set("payment-signature", signature);

    const retryResponse = await fetch(url, { ...init, headers });
    if (retryResponse.ok) {
      this.sessionSpend += microCredits;
    }
    return retryResponse;
  }

  resetSession(): void {
    this.sessionSpend = 0;
  }
}
