import { describe, it, expect } from "vitest";
import {
  ChainId,
  USDC_TO_MICRO_CREDITS,
  USDC_CONTRACTS,
  DEFAULT_POLICY,
  evaluatePolicy,
} from "../src/types.js";

describe("ChainId", () => {
  it("has correct CAIP-2 values", () => {
    expect(ChainId.Base).toBe("eip155:8453");
    expect(ChainId.BaseSepolia).toBe("eip155:84532");
    expect(ChainId.Ethereum).toBe("eip155:1");
  });
});

describe("USDC constants", () => {
  it("has correct micro-credit conversion", () => {
    expect(USDC_TO_MICRO_CREDITS).toBe(1_000_000);
  });

  it("has contracts for all chains", () => {
    expect(USDC_CONTRACTS[ChainId.Base]).toMatch(/^0x/);
    expect(USDC_CONTRACTS[ChainId.BaseSepolia]).toMatch(/^0x/);
    expect(USDC_CONTRACTS[ChainId.Ethereum]).toMatch(/^0x/);
  });
});

describe("evaluatePolicy", () => {
  it("auto-approves below cap", () => {
    expect(evaluatePolicy(DEFAULT_POLICY, 50)).toBe("approved");
    expect(evaluatePolicy(DEFAULT_POLICY, 100)).toBe("approved");
  });

  it("requires approval between caps", () => {
    expect(evaluatePolicy(DEFAULT_POLICY, 101)).toBe("requires_approval");
    expect(evaluatePolicy(DEFAULT_POLICY, 999_999)).toBe("requires_approval");
  });

  it("denies above hard cap", () => {
    expect(evaluatePolicy(DEFAULT_POLICY, 1_000_001)).toBe("denied");
  });

  it("denies when disabled", () => {
    const policy = { ...DEFAULT_POLICY, enabled: false };
    expect(evaluatePolicy(policy, 1)).toBe("denied");
  });

  it("respects custom caps", () => {
    const policy = { ...DEFAULT_POLICY, autoApproveCap: 500, hardCapPerTx: 5000 };
    expect(evaluatePolicy(policy, 500)).toBe("approved");
    expect(evaluatePolicy(policy, 501)).toBe("requires_approval");
    expect(evaluatePolicy(policy, 5001)).toBe("denied");
  });
});
