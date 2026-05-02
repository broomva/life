/**
 * `life.v1.Wallet` proto types.
 *
 * Hand-curated TypeScript mirror of `proto/life/v1/wallet.proto`.
 *
 * **Proto3 JSON canonical mapping for `int64`/`uint64`:** the wire
 * format is a JSON string (e.g. `"9999999999"`), NOT a JS number or
 * bigint, because IEEE-754 doubles lose precision past 2^53. To keep
 * round-trips honest, currency-amount fields here are typed as
 * `string` matching the wire shape exactly. Convert to `bigint` at
 * the call site via the {@link microsToBigInt} helper when arithmetic
 * is needed.
 *
 * Pre-merge code-quality review I-1: this PR changes `micros` /
 * `amountMicros` / `deltaMicros` from `bigint` to `string` so the
 * type system reflects the actual runtime values. The earlier
 * `bigint` typing was a "type lie" — the JSON reviver was a no-op,
 * so consumers calling `bal.micros - 1n` would have crashed with
 * `Cannot mix BigInt and other types`.
 *
 * @see proto/life/v1/wallet.proto
 * @see {@link microsToBigInt} / {@link bigIntToMicros} in `../codec.js`
 */

import type { Timestamp } from "./timestamp.js";

export interface WalletRef {
  userId: string;
  projectId: string;
}

export interface Balance {
  /** μ-units / micros. Proto3 JSON wire shape: string. */
  micros: string;
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
   * Proto3 JSON wire shape: string (an int64 on the proto side; JSON
   * encodes int64 as string per the canonical mapping).
   */
  deltaMicros: string;
  reason: string;
  sid?: string;
  skill?: string;
  model?: string;
  tool?: string;
}

export interface DebitReq {
  wallet: WalletRef;
  /** μ-units / micros. Proto3 JSON wire shape: string. */
  amountMicros: string;
  /**
   * Idempotency key. Spec C₂ §3.3 declares the dedup tuple as
   * `(wallet, sid)`. Two `Debit` calls with the same `(wallet, sid)`
   * deduplicate to one ledger entry. For one-shot debits without a
   * session context, generate a stable opaque sid (e.g. a ULID).
   */
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
  /** μ-units / micros. Proto3 JSON wire shape: string. */
  amountMicros: string;
  /**
   * Idempotency key for `Wallet.Transfer`. Spec C₂ §3.3 + M5 Sub-phase
   * D bundled `Wallet.Transfer` idempotency: the server uses `memo` as
   * the dedup key (the proto schema does not yet carry a dedicated
   * idempotency field). Pass a stable opaque value if you want
   * idempotency; pass `undefined` for fire-and-forget transfers.
   *
   * Pre-merge spec-compliance review I2 doc fix.
   */
  memo?: string;
}

export interface TransferReceipt {
  entryId: string;
  fromBalance: Balance;
  toBalance: Balance;
}
