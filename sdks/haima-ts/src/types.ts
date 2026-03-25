/**
 * Core types mirroring haima-core Rust types.
 */

/** CAIP-2 chain identifiers. */
export enum ChainId {
  Base = "eip155:8453",
  BaseSepolia = "eip155:84532",
  Ethereum = "eip155:1",
}

/** 1 USDC = 1,000,000 micro-credits (USDC has 6 decimals). */
export const USDC_TO_MICRO_CREDITS = 1_000_000;

/** USDC contract addresses by chain. */
export const USDC_CONTRACTS: Record<ChainId, `0x${string}`> = {
  [ChainId.Base]: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
  [ChainId.BaseSepolia]: "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
  [ChainId.Ethereum]: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
};

/** Wallet address with chain. */
export interface WalletInfo {
  address: string;
  chain: ChainId;
}

/** x402 payment scheme (currently only "exact"). */
export interface PaymentScheme {
  scheme: "exact";
  network: string;
  token: string;
  amount: string;
  recipient: string;
  facilitator: string;
}

/** Payment policy verdict. */
export type PaymentDecision = "approved" | "requires_approval" | "denied";

/** Payment policy thresholds (micro-credits). */
export interface PaymentPolicy {
  autoApproveCap: number;
  hardCapPerTx: number;
  sessionSpendCap: number;
  maxTxPerMinute: number;
  enabled: boolean;
}

/** Default payment policy matching Rust defaults. */
export const DEFAULT_POLICY: PaymentPolicy = {
  autoApproveCap: 100,
  hardCapPerTx: 1_000_000,
  sessionSpendCap: 10_000_000,
  maxTxPerMinute: 10,
  enabled: true,
};

/** Evaluate a payment amount against policy. */
export function evaluatePolicy(
  policy: PaymentPolicy,
  microCredits: number,
): PaymentDecision {
  if (!policy.enabled) return "denied";
  if (microCredits > policy.hardCapPerTx) return "denied";
  if (microCredits <= policy.autoApproveCap) return "approved";
  return "requires_approval";
}

/** Facilitator settlement status. */
export type FacilitationStatus = "settled" | "rejected" | "pending";

/** On-chain settlement receipt. */
export interface SettlementReceipt {
  tx_hash: string;
  payer: string;
  payee: string;
  amount_micro_usd: number;
  chain: string;
  settled_at: string;
}

/** Response from POST /v1/facilitate. */
export interface FacilitateResponse {
  status: FacilitationStatus;
  receipt?: SettlementReceipt;
  facilitator_fee_bps?: number;
  trust_attestation?: Record<string, unknown>;
  reason?: string;
  details?: string;
}

/** Agent credit score. */
export interface CreditScore {
  agent_id: string;
  score: number;
  tier: string;
  max_credit_line: number;
}

/** Facilitator stats. */
export interface FacilitatorStats {
  total_transactions: number;
  total_volume_micro_usd: number;
  total_fees_micro_usd: number;
  total_rejected: number;
}

/** Request to facilitate a payment. */
export interface FacilitateRequest {
  payment_header: string;
  resource_url: string;
  amount_micro_usd: number;
  agent_id?: string;
}

/** Haima client configuration. */
export interface HaimaConfig {
  facilitatorUrl?: string;
  privateKey?: `0x${string}`;
  chain?: ChainId;
  policy?: Partial<PaymentPolicy>;
  apiKey?: string;
}
