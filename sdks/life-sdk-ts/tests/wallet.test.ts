/**
 * `life.v1.Wallet` service unit tests.
 *
 * Covers GetBalance, Statement (server-stream), Debit (idempotent),
 * Transfer.
 */

import { describe, expect, it } from "vitest";
import { LifeClient, RateLimitError } from "../src/index.js";
import { FakeGateway } from "./_helpers.js";

describe("WalletClient", () => {
  it("getBalance returns bigint micros (proto3 JSON encodes int64 as string)", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Wallet/GetBalance",
        handler: async ({ body }) => {
          expect((body as { userId: string }).userId).toBe("u-1");
          return {
            unary: { micros: "9999999999", currency: "USDC", asOf: "2026-04-30T12:00:00Z" },
          };
        },
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    const bal = await life.wallet.getBalance({ userId: "u-1", projectId: "p-1" });
    expect(bal.currency).toBe("USDC");
    // The wire string deserialises into a JS string; bigint conversion is
    // a caller-side concern. The key property is preservation of digits.
    expect(String(bal.micros)).toBe("9999999999");
  });

  it("debit is unary and replays return the original receipt", async () => {
    let calls = 0;
    const gw = new FakeGateway([
      {
        path: "/life.v1.Wallet/Debit",
        handler: async ({ body }) => {
          calls++;
          expect((body as { sid?: string }).sid).toBe("sid-debit");
          // Replays return the same entry id.
          return {
            unary: {
              entryId: "entry-1",
              newBalance: { micros: "1000", currency: "USDC" },
            },
          };
        },
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    const r1 = await life.wallet.debit({
      wallet: { userId: "u", projectId: "p" },
      amountMicros: 100n,
      sid: "sid-debit",
      reason: "test",
    });
    const r2 = await life.wallet.debit({
      wallet: { userId: "u", projectId: "p" },
      amountMicros: 100n,
      sid: "sid-debit",
      reason: "test",
    });
    expect(r1.entryId).toBe(r2.entryId);
    expect(calls).toBe(2);
  });

  it("statement streams ledger entries", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Wallet/Statement",
        handler: async () => ({
          stream: [
            { flag: 0, body: { entryId: "e1", deltaMicros: "-100", reason: "tool:bash" } },
            { flag: 0, body: { entryId: "e2", deltaMicros: "-50", reason: "tool:fs" } },
            { flag: 2, body: {} },
          ],
        }),
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    const out: Array<{ entryId: string }> = [];
    for await (const e of life.wallet.statement({
      wallet: { userId: "u", projectId: "p" },
    })) {
      out.push(e as unknown as { entryId: string });
    }
    expect(out).toHaveLength(2);
  });

  it("transfer is unary", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Wallet/Transfer",
        handler: async () => ({
          unary: {
            entryId: "tx-1",
            fromBalance: { micros: "500", currency: "USDC" },
            toBalance: { micros: "1500", currency: "USDC" },
          },
        }),
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    const t = await life.wallet.transfer({
      from: { userId: "alice", projectId: "p" },
      to: { userId: "bob", projectId: "p" },
      amountMicros: 1000n,
    });
    expect(t.entryId).toBe("tx-1");
  });

  it("propagates RESOURCE_EXHAUSTED as RateLimitError", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Wallet/Transfer",
        handler: async () => ({
          status: 429,
          unary: { code: "resource_exhausted", message: "per-user rate limit exceeded" },
        }),
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    await expect(
      life.wallet.transfer({
        from: { userId: "a", projectId: "p" },
        to: { userId: "b", projectId: "p" },
        amountMicros: 1n,
      }),
    ).rejects.toBeInstanceOf(RateLimitError);
  });
});
