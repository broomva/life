/**
 * Typed error classes raised by the Life SDK.
 *
 * The full close-code mapping is canonicalised in
 * `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md` (Spec C₃
 * §6.5). When the SDK observes a WS close, it consults
 * {@link closeCodeToError} to produce one of these typed errors.
 */

// ── Base error ─────────────────────────────────────────────────────

export class LifeSdkError extends Error {
  public readonly code: string;

  constructor(code: string, message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "LifeSdkError";
    this.code = code;
    // Preserve prototype chain in older runtimes.
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

// ── Transport errors ───────────────────────────────────────────────

/**
 * The lifegw HTTPS endpoint refused our TLS handshake. Most often
 * means the client could not negotiate TLS 1.3 — lifegw rejects
 * everything older per Spec C₃ §5.1 (TLS 1.3-only listener).
 *
 * In the browser this surfaces as a generic `TypeError: Failed to
 * fetch`; the SDK best-effort detects the case and remaps it.
 */
export class TlsNegotiationError extends LifeSdkError {
  constructor(message = "TLS 1.3 negotiation failed against lifegw") {
    super("tls_negotiation_failed", message);
    this.name = "TlsNegotiationError";
  }
}

/**
 * Transport-level error: the request failed before reaching lifegw
 * (DNS, connect, abort, etc.).
 */
export class TransportError extends LifeSdkError {
  constructor(message: string, options?: ErrorOptions) {
    super("transport", message, options);
    this.name = "TransportError";
  }
}

// ── Request / response errors ──────────────────────────────────────

/**
 * Tier-1 / Tier-2 token rejected by lifegw or upstream lifed
 * (`Unauthenticated` / `PermissionDenied`).
 *
 * Maps to WS close code 1008 per Spec C₃ §6.5.
 */
export class AuthError extends LifeSdkError {
  constructor(message = "auth token rejected") {
    super("unauthenticated", message);
    this.name = "AuthError";
  }
}

/**
 * Request rejected because the server's per-user / per-IP token
 * bucket is exhausted (Sub-phase D D1, mapped from
 * `tonic::Code::ResourceExhausted`).
 *
 * On the WS plane this is close code 4001.
 */
export class RateLimitError extends LifeSdkError {
  /**
   * The reason payload prefix from the server. Stable for runbook
   * purposes — `rate_limit:per_user` vs `rate_limit:per_ip`.
   */
  public readonly reasonPrefix: string;

  constructor(reasonPrefix = "rate_limit:per_user", message?: string) {
    super("resource_exhausted", message ?? `rate limit exceeded (${reasonPrefix})`);
    this.name = "RateLimitError";
    this.reasonPrefix = reasonPrefix;
  }
}

/**
 * Server closed the WS because the outbound mpsc(64) backed up
 * `STALLED_THRESHOLD` consecutive ticks. Close code 4002.
 */
export class BackpressureError extends LifeSdkError {
  constructor(message = "slow consumer / backpressure overflow") {
    super("slow_consumer", message);
    this.name = "BackpressureError";
  }
}

/**
 * Peer IP is on the in-process blocklist (admin RPC `BlocklistAdd`).
 * Close code 4003.
 */
export class IpBlockedError extends LifeSdkError {
  constructor(message = "peer IP is blocklisted") {
    super("ip_blocked", message);
    this.name = "IpBlockedError";
  }
}

/**
 * Upstream lifed UDS unreachable / circuit breaker open. Close code
 * 4004 (mapped from `tonic::Code::Unavailable`). Callers should
 * back off + retry.
 */
export class LifedUnavailableError extends LifeSdkError {
  constructor(message = "upstream lifed circuit open / unreachable") {
    super("lifed_unavailable", message);
    this.name = "LifedUnavailableError";
  }
}

/**
 * The `from_sequence` cursor sent on reconnect has been retired by
 * lifed (`tonic::Code::OutOfRange`). The client should drop its
 * cursor and reconnect with `from_sequence: 0` to resume from the
 * start of the live tail. Close code 4005.
 */
export class SequenceRetiredError extends LifeSdkError {
  constructor(message = "from_sequence cursor retired upstream") {
    super("sequence_retired", message);
    this.name = "SequenceRetiredError";
  }
}

/**
 * Server-side fault — typically `tonic::Code::Internal` or any tonic
 * code the gateway hasn't enumerated. Close code 1011. Heartbeat
 * timeouts (Spec C₃ §6.4) also surface as this error.
 */
export class InternalServerError extends LifeSdkError {
  constructor(message = "internal server error") {
    super("internal", message);
    this.name = "InternalServerError";
  }
}

/**
 * Server is shutting down (drain). Close code 1001. Callers should
 * reconnect once the server is up again.
 */
export class GoingAwayError extends LifeSdkError {
  constructor(message = "server going away (drain)") {
    super("going_away", message);
    this.name = "GoingAwayError";
  }
}

/**
 * Generic server-returned error with a gRPC status code. Used for
 * unary RPC errors that don't have a more specific class.
 */
export class GrpcError extends LifeSdkError {
  /**
   * gRPC status code (numeric, per Status Code Mappings).
   * @see https://grpc.github.io/grpc/core/md_doc_statuscodes.html
   */
  public readonly grpcCode: number;

  /**
   * gRPC status code string (e.g. `RESOURCE_EXHAUSTED`).
   */
  public readonly grpcStatus: string;

  constructor(grpcCode: number, grpcStatus: string, message: string) {
    super(grpcStatus.toLowerCase(), message);
    this.name = "GrpcError";
    this.grpcCode = grpcCode;
    this.grpcStatus = grpcStatus;
  }
}

// ── Close-code → typed error mapping ───────────────────────────────

/**
 * Translate a WS close code into the corresponding typed error.
 *
 * Per Spec C₃ §6.5 (see
 * `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md`):
 *
 * | Code | Error class            | Reason prefix                  |
 * |---:|------------------------|--------------------------------|
 * | 1000 | (no error — graceful)  | `normal`                       |
 * | 1001 | {@link GoingAwayError} | `going_away`                   |
 * | 1008 | {@link AuthError}      | `policy_violation:token_expired` |
 * | 1011 | {@link InternalServerError} | `internal_error`          |
 * | 4001 | {@link RateLimitError} | `rate_limit:per_user`          |
 * | 4002 | {@link BackpressureError} | `backpressure:slow_consumer` |
 * | 4003 | {@link IpBlockedError} | `ip_blocked`                   |
 * | 4004 | {@link LifedUnavailableError} | `lifed_circuit_open`    |
 * | 4005 | {@link SequenceRetiredError} | `sequence_retired`       |
 *
 * Codes 1000 and any unknown code return `null`. Callers that need
 * to surface an error for code 1000 should construct one explicitly.
 */
export function closeCodeToError(
  code: number,
  reason?: string,
): LifeSdkError | null {
  switch (code) {
    case 1000:
      return null;
    case 1001:
      return new GoingAwayError(reason || undefined);
    case 1008:
      return new AuthError(reason || undefined);
    case 1011:
      return new InternalServerError(reason || undefined);
    case 4001: {
      // Reason prefixes are stable for dashboards, e.g. `rate_limit:per_ip`.
      const prefix = reason?.split(":").slice(0, 2).join(":") ?? "rate_limit:per_user";
      return new RateLimitError(prefix);
    }
    case 4002:
      return new BackpressureError(reason || undefined);
    case 4003:
      return new IpBlockedError(reason || undefined);
    case 4004:
      return new LifedUnavailableError(reason || undefined);
    case 4005:
      return new SequenceRetiredError(reason || undefined);
    default:
      return null;
  }
}

/**
 * Translate a numeric gRPC status code to a typed SDK error.
 *
 * Used by the unary-RPC path so callers see the same error classes
 * regardless of whether the call failed via WS close or gRPC status.
 *
 * @param grpcCode gRPC numeric status code
 * @param grpcStatus gRPC status name (e.g. `RESOURCE_EXHAUSTED`)
 * @param message optional human-readable message
 */
export function grpcCodeToError(
  grpcCode: number,
  grpcStatus: string,
  message?: string,
): LifeSdkError {
  const msg = message || `gRPC ${grpcStatus}`;
  switch (grpcCode) {
    case 16: // UNAUTHENTICATED
    case 7: // PERMISSION_DENIED
      return new AuthError(msg);
    case 8: // RESOURCE_EXHAUSTED
      return new RateLimitError("rate_limit:per_user", msg);
    case 14: // UNAVAILABLE
      return new LifedUnavailableError(msg);
    case 11: // OUT_OF_RANGE
      return new SequenceRetiredError(msg);
    case 13: // INTERNAL
      return new InternalServerError(msg);
    case 1: // CANCELLED
    case 10: // ABORTED
      return new GrpcError(grpcCode, grpcStatus, msg);
    default:
      return new GrpcError(grpcCode, grpcStatus, msg);
  }
}

/**
 * gRPC status code constants for convenient reference.
 *
 * @see https://grpc.github.io/grpc/core/md_doc_statuscodes.html
 */
export const GrpcCode = {
  Ok: 0,
  Cancelled: 1,
  Unknown: 2,
  InvalidArgument: 3,
  DeadlineExceeded: 4,
  NotFound: 5,
  AlreadyExists: 6,
  PermissionDenied: 7,
  ResourceExhausted: 8,
  FailedPrecondition: 9,
  Aborted: 10,
  OutOfRange: 11,
  Unimplemented: 12,
  Internal: 13,
  Unavailable: 14,
  DataLoss: 15,
  Unauthenticated: 16,
} as const;

export type GrpcCodeValue = (typeof GrpcCode)[keyof typeof GrpcCode];
