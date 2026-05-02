/**
 * `life.v1.Wallet` proto types.
 *
 * Hand-curated TypeScript mirror of `proto/life/v1/wallet.proto`.
 * Currency amounts are represented as `bigint` (μ-units / micros) to
 * preserve precision — proto3 `uint64` does not fit in a JS `number`
 * for very large balances.
 *
 * @see proto/life/v1/wallet.proto
 */

import type { Timestamp } from "./timestamp.js";

export interface WalletRef {
  userId: string;
  projectId: string;
}

export interface Balance {
  micros: bigint;
  currency: string;
  asOf?: Timestamp;
}

export interface StatementReq {
  wallet: WalletRef;
  since?: Timestamp;
  until?: Timestamp;
  limit?: number;
}

export interface LedgerEntry {
  entryId: string;
  at?: Timestamp;
  /**
   * Signed delta in micros. Negative for debits, positive for credits.
   */
  deltaMicros: bigint;
  reason: string;
  sid?: string;
  skill?: string;
  model?: string;
  tool?: string;
}

export interface DebitReq {
  wallet: WalletRef;
  amountMicros: bigint;
  sid?: string;
  reason?: string;
}

export interface DebitReceipt {
  entryId: string;
  newBalance: Balance;
}

export interface TransferReq {
  from: WalletRef;
  to: WalletRef;
  amountMicros: bigint;
  memo?: string;
}

export interface TransferReceipt {
  entryId: string;
  fromBalance: Balance;
  toBalance: Balance;
}
