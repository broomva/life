/**
 * `SessionCap` — Tier-User capability JWT lifecycle manager.
 *
 * Spec D §"Browser path (D-Sub-C in detail)" §5: "One passkey-mediated
 * handshake per browser session mints a short-lived in-memory
 * Tier-User capability that authorizes subsequent signing requests
 * against an in-tab signing oracle, so the user isn't prompted on
 * every JWT mint. Default TTL: 15 minutes."
 *
 * Stored in-memory ONLY — IndexedDB persistence is too long-lived for
 * the threat model. If the user closes the tab and reopens, they
 * complete a fresh passkey handshake to mint a new cap.
 *
 * Lifecycle:
 *   - On `getValidToken()`, check if the cached token has > 30s
 *     remaining. If yes, return cached.
 *   - Otherwise, mint a fresh cap by:
 *       1. Drawing a 32-byte random challenge from the lifegw mint
 *          endpoint (or a deterministic local nonce + timestamp shape).
 *       2. Calling `passkey.signWithAssertion(challenge)`.
 *       3. POSTing the assertion to `/anima/custody/mint_session_cap`.
 *       4. Caching the returned JWT + `expires_at`.
 *
 * Tracks the lifegw Tier-2 pattern from M7 Sub-phase B/C.
 */

import { AnimaError } from "./errors.js";
import type { PasskeyOracle } from "./passkey.js";
import type { RemoteAnimaClient } from "./remote.js";

/** Default refresh threshold — refresh when token has < this many seconds left. */
const DEFAULT_REFRESH_BEFORE_SECS = 30;

/**
 * Configuration for {@link SessionCap}.
 */
export interface SessionCapConfig {
  /** User id this cap is bound to. */
  userId: string;
  /** Passkey oracle used to sign the mint challenge. */
  passkey: PasskeyOracle;
  /** Remote client used to POST the assertion. */
  remote: RemoteAnimaClient;
  /**
   * Optional callback fired when a refresh is imminent (within
   * `refreshBeforeSecs`). UIs can use this to show a "click to
   * re-authenticate" prompt before the OS UI fires.
   */
  onExpiringSoon?: () => void;
  /**
   * Refresh when the token has fewer than this many seconds left.
   * Default 30s.
   */
  refreshBeforeSecs?: number;
  /**
   * Optional injection point for the current time (Unix-seconds). Tests
   * use this with vitest fake timers; production uses `Date.now()`.
   */
  now?: () => number;
  /**
   * Optional injection point for the random challenge. Defaults to a
   * 32-byte `crypto.getRandomValues`. Tests pin this to a deterministic
   * fixture so the test for the assertion shape is reproducible.
   */
  randomChallenge?: () => Uint8Array;
}

/**
 * Cached cap state.
 */
interface CapState {
  token: string;
  expiresAt: number; // Unix seconds
  issuedAt: number; // Unix seconds
}

/**
 * In-memory Tier-User capability lifecycle manager.
 *
 * One instance per (user, origin). The `getValidToken()` method is
 * the central entry point — pass it as `getToken` to
 * `RemoteAnimaClient` to get auto-refresh on the hot path.
 */
export class SessionCap {
  private readonly cfg: Required<
    Pick<SessionCapConfig, "userId" | "passkey" | "remote" | "refreshBeforeSecs" | "now" | "randomChallenge">
  > & { onExpiringSoon: (() => void) | undefined };

  private state: CapState | null = null;
  private inflight: Promise<string> | null = null;

  constructor(cfg: SessionCapConfig) {
    this.cfg = {
      userId: cfg.userId,
      passkey: cfg.passkey,
      remote: cfg.remote,
      refreshBeforeSecs: cfg.refreshBeforeSecs ?? DEFAULT_REFRESH_BEFORE_SECS,
      onExpiringSoon: cfg.onExpiringSoon,
      now: cfg.now ?? defaultNow,
      randomChallenge: cfg.randomChallenge ?? defaultRandomChallenge,
    };
  }

  /**
   * Return a Tier-User cap JWT, refreshing it if necessary.
   *
   * Concurrent callers that race during a refresh window share the
   * same in-flight promise — the OS auth prompt fires once.
   */
  async getValidToken(): Promise<string> {
    const now = this.cfg.now();

    if (this.state && this.state.expiresAt - now > this.cfg.refreshBeforeSecs) {
      return this.state.token;
    }

    if (this.inflight !== null) {
      // Another caller is already refreshing — share the promise.
      return await this.inflight;
    }

    this.inflight = this.mint().finally(() => {
      this.inflight = null;
    });
    return await this.inflight;
  }

  /**
   * Force a refresh now, regardless of cached state. Useful after
   * the user has clicked "renew session" in a UI prompt.
   */
  async forceRefresh(): Promise<string> {
    if (this.inflight !== null) {
      // Reuse the in-flight refresh.
      return await this.inflight;
    }
    this.inflight = this.mint().finally(() => {
      this.inflight = null;
    });
    return await this.inflight;
  }

  /** Drop the cached cap. Next `getValidToken` call mints a fresh one. */
  invalidate(): void {
    this.state = null;
  }

  /** Returns the cached cap state, or null if not minted yet. */
  current(): CapState | null {
    return this.state ? { ...this.state } : null;
  }

  private async mint(): Promise<string> {
    if (this.cfg.onExpiringSoon) {
      try {
        this.cfg.onExpiringSoon();
      } catch {
        // Don't propagate UI callback errors.
      }
    }

    const challenge = this.cfg.randomChallenge();
    if (challenge.byteLength !== 32) {
      throw AnimaError.state(
        `SessionCap challenge must be 32 bytes, got ${challenge.byteLength}`,
      );
    }

    const assertion = await this.cfg.passkey.signWithAssertion(challenge);

    // For first mint after enrollment we don't have a cap yet — pass
    // an empty bearer so RemoteAnima skips the Authorization header.
    // The mint endpoint is one of two on lifegw that accepts un-authed
    // requests (the other being `/anima/custody/enroll_passkey`).
    const result = await this.cfg.remote.mintSessionCap(
      this.cfg.userId,
      { assertion },
      this.state?.token, // pass current token if we have one (refresh path)
    );

    this.state = {
      token: result.token,
      expiresAt: result.expiresAt,
      issuedAt: result.issuedAt,
    };
    return result.token;
  }
}

function defaultNow(): number {
  return Math.floor(Date.now() / 1000);
}

function defaultRandomChallenge(): Uint8Array {
  const out = new Uint8Array(32);
  const g = globalThis as unknown as { crypto?: { getRandomValues?: (b: Uint8Array) => Uint8Array } };
  if (g.crypto?.getRandomValues) {
    g.crypto.getRandomValues(out);
    return out;
  }
  // Fallback for environments without WebCrypto — should be rare;
  // tests inject `randomChallenge` directly. Throw instead of
  // emitting weak randomness.
  throw AnimaError.state(
    "SessionCap: no crypto.getRandomValues available — pass `randomChallenge` config",
  );
}
