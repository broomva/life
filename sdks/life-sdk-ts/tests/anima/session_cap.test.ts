/**
 * `SessionCap` lifecycle tests — enrollment + mint, refresh-on-expiry,
 * concurrent in-flight handling, force refresh, invalidate.
 *
 * Uses vitest fake timers + injected `now` to drive the lifecycle
 * deterministically.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PasskeyOracle } from "../../src/anima/passkey.js";
import { RemoteAnimaClient } from "../../src/anima/remote.js";
import { SessionCap } from "../../src/anima/session_cap.js";

/** Build a SessionCap with hand-rolled stubs for passkey + remote. */
function makeHarness(opts?: {
  refreshBeforeSecs?: number;
  initialNow?: number;
}): {
  cap: SessionCap;
  passkeyCalls: number;
  remoteCalls: number;
  expiresAt: () => number;
  setExpiresAt: (n: number) => void;
  now: () => number;
  setNow: (n: number) => void;
} {
  let now = opts?.initialNow ?? 1_000_000;
  let expiresAt = now + 900; // 15-minute default

  const harnessInternal = {
    passkeyCalls: 0,
    remoteCalls: 0,
  };

  // Stub PasskeyOracle.signWithAssertion via a structurally-typed
  // shim — we only need the one method.
  const passkeyStub = {
    signWithAssertion: async (_digest: Uint8Array) => {
      harnessInternal.passkeyCalls += 1;
      return {
        signature: new Uint8Array(64),
        clientDataJson: new Uint8Array(8),
        authenticatorData: new Uint8Array(40),
        credentialId: new ArrayBuffer(16),
      };
    },
  } as unknown as PasskeyOracle;

  // Stub RemoteAnimaClient.mintSessionCap.
  const remoteStub = {
    mintSessionCap: async (
      _userId: string,
      _params: unknown,
      _bearerOverride: string | undefined,
    ) => {
      harnessInternal.remoteCalls += 1;
      return {
        token: `tok-${harnessInternal.remoteCalls}`,
        expiresAt,
        issuedAt: now,
      };
    },
  } as unknown as RemoteAnimaClient;

  const cap = new SessionCap({
    userId: "u-1",
    passkey: passkeyStub,
    remote: remoteStub,
    refreshBeforeSecs: opts?.refreshBeforeSecs ?? 30,
    now: () => now,
    randomChallenge: () => {
      const c = new Uint8Array(32);
      c[0] = 0x42;
      return c;
    },
  });

  return {
    cap,
    get passkeyCalls() {
      return harnessInternal.passkeyCalls;
    },
    get remoteCalls() {
      return harnessInternal.remoteCalls;
    },
    expiresAt: () => expiresAt,
    setExpiresAt: (n: number) => {
      expiresAt = n;
    },
    now: () => now,
    setNow: (n: number) => {
      now = n;
    },
  } as unknown as ReturnType<typeof makeHarness>;
}

describe("SessionCap.getValidToken — initial mint", () => {
  it("triggers a passkey + remote call on first invocation", async () => {
    const h = makeHarness();
    const tok = await h.cap.getValidToken();
    expect(tok).toBe("tok-1");
    expect(h.passkeyCalls).toBe(1);
    expect(h.remoteCalls).toBe(1);
  });

  it("subsequent calls within TTL return cached token without prompting", async () => {
    const h = makeHarness();
    const tok1 = await h.cap.getValidToken();
    const tok2 = await h.cap.getValidToken();
    const tok3 = await h.cap.getValidToken();
    expect(tok1).toBe(tok2);
    expect(tok1).toBe(tok3);
    expect(h.passkeyCalls).toBe(1);
    expect(h.remoteCalls).toBe(1);
  });
});

describe("SessionCap.getValidToken — refresh on expiry", () => {
  it("refreshes when remaining < refreshBeforeSecs", async () => {
    const h = makeHarness({ refreshBeforeSecs: 30 });
    const tok1 = await h.cap.getValidToken();
    expect(tok1).toBe("tok-1");

    // Advance time so token has 20s remaining (< 30s threshold).
    h.setNow(h.expiresAt() - 20);
    const tok2 = await h.cap.getValidToken();
    expect(tok2).toBe("tok-2");
    expect(h.remoteCalls).toBe(2);
  });

  it("does not refresh while > threshold remains", async () => {
    const h = makeHarness({ refreshBeforeSecs: 30 });
    await h.cap.getValidToken();
    h.setNow(h.expiresAt() - 60); // 60s remaining > 30s threshold
    await h.cap.getValidToken();
    expect(h.remoteCalls).toBe(1);
  });
});

describe("SessionCap.forceRefresh", () => {
  it("forces a fresh mint regardless of cached state", async () => {
    const h = makeHarness();
    const t1 = await h.cap.getValidToken();
    const t2 = await h.cap.forceRefresh();
    expect(t1).toBe("tok-1");
    expect(t2).toBe("tok-2");
    expect(h.remoteCalls).toBe(2);
  });
});

describe("SessionCap.invalidate", () => {
  it("clears the cached cap so next call mints", async () => {
    const h = makeHarness();
    await h.cap.getValidToken();
    h.cap.invalidate();
    expect(h.cap.current()).toBeNull();
    await h.cap.getValidToken();
    expect(h.remoteCalls).toBe(2);
  });
});

describe("SessionCap concurrent callers", () => {
  it("multiple callers during a refresh share the same in-flight promise", async () => {
    let releaseRemote!: (token: { token: string; expiresAt: number; issuedAt: number }) => void;
    const remotePending = new Promise<{ token: string; expiresAt: number; issuedAt: number }>(
      (resolve) => {
        releaseRemote = resolve;
      },
    );
    let mintCalls = 0;
    const passkeyStub = {
      signWithAssertion: async () => ({
        signature: new Uint8Array(64),
        clientDataJson: new Uint8Array(8),
        authenticatorData: new Uint8Array(40),
        credentialId: new ArrayBuffer(16),
      }),
    } as unknown as PasskeyOracle;
    const remoteStub = {
      mintSessionCap: async () => {
        mintCalls += 1;
        return await remotePending;
      },
    } as unknown as RemoteAnimaClient;
    const cap = new SessionCap({
      userId: "u-1",
      passkey: passkeyStub,
      remote: remoteStub,
      refreshBeforeSecs: 30,
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });

    // Three concurrent callers — only one mintSessionCap fires.
    const p1 = cap.getValidToken();
    const p2 = cap.getValidToken();
    const p3 = cap.getValidToken();

    // Resolve the one in-flight call.
    releaseRemote({ token: "shared-tok", expiresAt: 999_999, issuedAt: 0 });

    const [r1, r2, r3] = await Promise.all([p1, p2, p3]);
    expect(r1).toBe("shared-tok");
    expect(r2).toBe("shared-tok");
    expect(r3).toBe("shared-tok");
    expect(mintCalls).toBe(1);
  });
});

describe("SessionCap.onExpiringSoon", () => {
  it("fires the callback before a refresh", async () => {
    let fired = 0;
    const passkeyStub = {
      signWithAssertion: async () => ({
        signature: new Uint8Array(64),
        clientDataJson: new Uint8Array(8),
        authenticatorData: new Uint8Array(40),
        credentialId: new ArrayBuffer(16),
      }),
    } as unknown as PasskeyOracle;
    const remoteStub = {
      mintSessionCap: async () => ({ token: "T", expiresAt: 999, issuedAt: 0 }),
    } as unknown as RemoteAnimaClient;
    const cap = new SessionCap({
      userId: "u-1",
      passkey: passkeyStub,
      remote: remoteStub,
      onExpiringSoon: () => {
        fired += 1;
      },
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    await cap.getValidToken();
    expect(fired).toBe(1);
  });

  it("does not propagate callback errors", async () => {
    const passkeyStub = {
      signWithAssertion: async () => ({
        signature: new Uint8Array(64),
        clientDataJson: new Uint8Array(8),
        authenticatorData: new Uint8Array(40),
        credentialId: new ArrayBuffer(16),
      }),
    } as unknown as PasskeyOracle;
    const remoteStub = {
      mintSessionCap: async () => ({ token: "T", expiresAt: 999, issuedAt: 0 }),
    } as unknown as RemoteAnimaClient;
    const cap = new SessionCap({
      userId: "u-1",
      passkey: passkeyStub,
      remote: remoteStub,
      onExpiringSoon: () => {
        throw new Error("ui blew up");
      },
      now: () => 0,
      randomChallenge: () => new Uint8Array(32),
    });
    // Should NOT throw despite the broken callback.
    await expect(cap.getValidToken()).resolves.toBe("T");
  });
});

describe("SessionCap input validation", () => {
  it("throws if randomChallenge returns wrong length", async () => {
    const passkeyStub = {
      signWithAssertion: async () => ({
        signature: new Uint8Array(64),
        clientDataJson: new Uint8Array(8),
        authenticatorData: new Uint8Array(40),
        credentialId: new ArrayBuffer(16),
      }),
    } as unknown as PasskeyOracle;
    const remoteStub = {
      mintSessionCap: async () => ({ token: "T", expiresAt: 999, issuedAt: 0 }),
    } as unknown as RemoteAnimaClient;
    const cap = new SessionCap({
      userId: "u-1",
      passkey: passkeyStub,
      remote: remoteStub,
      now: () => 0,
      randomChallenge: () => new Uint8Array(16),
    });
    await expect(cap.getValidToken()).rejects.toThrow(/32 bytes/);
  });
});
