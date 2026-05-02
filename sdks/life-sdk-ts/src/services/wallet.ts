/**
 * `life.v1.Wallet` service client.
 *
 * Currency amounts are bigint micros throughout. Idempotency
 * semantics:
 *   - `Debit`: idempotent on the `(wallet, sid)` tuple — replays
 *     return the original receipt.
 *   - `Transfer`: idempotency lands in M5 Sub-phase D follow-up
 *     (haima per-task billing alignment); callers should still
 *     pass a stable `memo` to aid operator reconciliation.
 *
 * @see proto/life/v1/wallet.proto
 */

import type { Transport, TransportCallOptions } from "../transport.js";
import type {
  Balance,
  DebitReceipt,
  DebitReq,
  LedgerEntry,
  StatementReq,
  TransferReceipt,
  TransferReq,
  WalletRef,
} from "../proto/wallet.js";

const SERVICE = "life.v1.Wallet";

export class WalletClient {
  constructor(private readonly transport: Transport) {}

  /**
   * Read the current balance of a wallet.
   *
   * The returned `micros` is a bigint — convert to USDC by dividing
   * by 1_000_000n.
   */
  getBalance(req: WalletRef, opts?: TransportCallOptions): Promise<Balance> {
    return this.transport.unary<WalletRef, Balance>(
      SERVICE,
      "GetBalance",
      req,
      opts,
    );
  }

  /**
   * Stream the wallet's ledger entries since `since` (inclusive).
   */
  statement(req: StatementReq, opts?: TransportCallOptions): AsyncIterable<LedgerEntry> {
    return this.transport.serverStream<StatementReq, LedgerEntry>(
      SERVICE,
      "Statement",
      req,
      opts,
    );
  }

  /**
   * Debit the wallet. Idempotent on `(wallet, sid)` — replays return
   * the original receipt.
   */
  debit(req: DebitReq, opts?: TransportCallOptions): Promise<DebitReceipt> {
    return this.transport.unary<DebitReq, DebitReceipt>(
      SERVICE,
      "Debit",
      req,
      opts,
    );
  }

  /**
   * Transfer micros between two wallets.
   */
  transfer(req: TransferReq, opts?: TransportCallOptions): Promise<TransferReceipt> {
    return this.transport.unary<TransferReq, TransferReceipt>(
      SERVICE,
      "Transfer",
      req,
      opts,
    );
  }
}
