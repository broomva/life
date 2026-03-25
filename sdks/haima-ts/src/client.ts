/**
 * HaimaClient — high-level SDK for x402 payments through the Haima facilitator.
 *
 * Usage:
 *   const client = new HaimaClient({ facilitatorUrl: "https://haima.broomva.tech" });
 *   const receipt = await client.pay("0xRecipient...", 100, "translate-doc");
 */

import type { Hex } from "viem";
import type {
  FacilitateRequest,
  FacilitateResponse,
  CreditScore,
  FacilitatorStats,
  HaimaConfig,
  PaymentPolicy,
  WalletInfo,
} from "./types.js";
import {
  ChainId,
  DEFAULT_POLICY,
  USDC_CONTRACTS,
  evaluatePolicy,
} from "./types.js";
import { HaimaWallet } from "./wallet.js";
import { X402Middleware } from "./x402.js";

export class HaimaClient {
  readonly facilitatorUrl: string;
  readonly wallet: HaimaWallet;
  readonly policy: PaymentPolicy;
  readonly chain: ChainId;
  readonly x402: X402Middleware;

  private apiKey?: string;

  constructor(config: HaimaConfig = {}) {
    this.facilitatorUrl = (config.facilitatorUrl ?? "http://localhost:3003").replace(/\/$/, "");
    this.wallet = config.privateKey
      ? new HaimaWallet(config.privateKey, config.chain)
      : HaimaWallet.generate(config.chain);
    this.chain = config.chain ?? ChainId.Base;
    this.policy = { ...DEFAULT_POLICY, ...config.policy };
    this.apiKey = config.apiKey;
    this.x402 = new X402Middleware(this.wallet, this.policy);
  }

  get address(): string {
    return this.wallet.address;
  }

  get walletInfo(): WalletInfo {
    return this.wallet.info;
  }

  private headers(): Record<string, string> {
    const h: Record<string, string> = { "Content-Type": "application/json" };
    if (this.apiKey) h["Authorization"] = `Bearer ${this.apiKey}`;
    return h;
  }

  /**
   * Submit a payment through the Haima facilitator.
   *
   * This is the primary API: `haima.pay(recipient, amount, taskId)`
   */
  async pay(
    recipient: string,
    amountMicroUsd: number,
    taskId?: string,
    agentId?: string,
  ): Promise<FacilitateResponse> {
    // Check policy locally first
    const decision = evaluatePolicy(this.policy, amountMicroUsd);
    if (decision === "denied") {
      return {
        status: "rejected",
        reason: `Payment denied by local policy: ${amountMicroUsd} μc exceeds hard cap`,
      };
    }

    const signature = await this.wallet.signPaymentHeader({
      scheme: "exact",
      network: this.chain,
      resourceUrl: `haima://${taskId ?? "payment"}`,
      amount: String(amountMicroUsd),
      recipient,
    });

    const body: FacilitateRequest = {
      payment_header: signature,
      resource_url: `haima://${taskId ?? "payment"}`,
      amount_micro_usd: amountMicroUsd,
      ...(agentId ? { agent_id: agentId } : {}),
    };

    const response = await fetch(`${this.facilitatorUrl}/v1/facilitate`, {
      method: "POST",
      headers: this.headers(),
      body: JSON.stringify(body),
    });

    if (!response.ok) {
      throw new Error(`Facilitate request failed: ${response.status} ${await response.text()}`);
    }
    return response.json() as Promise<FacilitateResponse>;
  }

  /** Check if an agent can spend a given amount. */
  async checkCredit(agentId: string, amount: number): Promise<boolean> {
    const response = await fetch(
      `${this.facilitatorUrl}/v1/credit/${agentId}/check`,
      {
        method: "POST",
        headers: this.headers(),
        body: JSON.stringify({ amount_micro_usd: amount }),
      },
    );
    if (!response.ok) return false;
    const data = (await response.json()) as { allowed?: boolean };
    return data.allowed ?? false;
  }

  /** Get an agent's credit score. */
  async getCreditScore(agentId: string): Promise<CreditScore> {
    const response = await fetch(
      `${this.facilitatorUrl}/v1/credit/${agentId}`,
      { headers: this.headers() },
    );
    if (!response.ok) {
      throw new Error(`Credit score request failed: ${response.status}`);
    }
    return response.json() as Promise<CreditScore>;
  }

  /** Get facilitator statistics. */
  async stats(): Promise<FacilitatorStats> {
    const response = await fetch(
      `${this.facilitatorUrl}/v1/facilitator/stats`,
      { headers: this.headers() },
    );
    if (!response.ok) {
      throw new Error(`Stats request failed: ${response.status}`);
    }
    return response.json() as Promise<FacilitatorStats>;
  }

  /** Check facilitator health. */
  async health(): Promise<boolean> {
    try {
      const response = await fetch(`${this.facilitatorUrl}/health`);
      return response.ok;
    } catch {
      return false;
    }
  }
}
