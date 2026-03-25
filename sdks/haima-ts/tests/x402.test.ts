import { describe, it, expect } from "vitest";
import { parsePaymentRequired, parsePaymentResponse } from "../src/x402.js";

describe("parsePaymentRequired", () => {
  it("parses a valid header", () => {
    const header = {
      schemes: [
        {
          scheme: "exact",
          network: "eip155:8453",
          token: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
          amount: "1000",
          recipient: "0x742d35Cc6634C0532925a3b844Bc9e7595916Da2",
          facilitator: "https://haima.broomva.tech",
        },
      ],
      version: "v2",
    };
    const encoded = btoa(JSON.stringify(header));
    const schemes = parsePaymentRequired(encoded);

    expect(schemes).toHaveLength(1);
    expect(schemes[0].scheme).toBe("exact");
    expect(schemes[0].network).toBe("eip155:8453");
    expect(schemes[0].amount).toBe("1000");
  });

  it("returns empty for no schemes", () => {
    const encoded = btoa(JSON.stringify({ schemes: [] }));
    expect(parsePaymentRequired(encoded)).toHaveLength(0);
  });
});

describe("parsePaymentResponse", () => {
  it("parses a settlement response", () => {
    const header = {
      tx_hash: "0xabc123",
      network: "eip155:8453",
      settled: true,
    };
    const encoded = btoa(JSON.stringify(header));
    const result = parsePaymentResponse(encoded);

    expect(result.tx_hash).toBe("0xabc123");
    expect(result.settled).toBe(true);
  });
});
