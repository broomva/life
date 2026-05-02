/**
 * @broomva/life-sdk — public TypeScript SDK over lifegw.
 *
 * Implements the four user-facing services exposed by lifegw:
 *   - `life.v1.Agent`    — session lifecycle + chat + tool dispatch
 *   - `life.v1.Events`   — event tail + content-addressed blobs
 *   - `life.v1.Wallet`   — balance, debit, transfer
 *   - `life.v1.Identity` — whoami, profile, sessions
 *
 * The admin plane (`life.admin.*`) is intentionally NOT re-exported —
 * it is UDS-only on the gateway and never reachable from the public
 * SDK.
 *
 * Spec ground truth:
 *   - Spec C₂ (lifed facade): `docs/superpowers/specs/2026-04-26-spec-c2-lifed-facade.md`
 *   - Spec C₃ (lifegw edge gateway): Linear BRO-922 + amendments in
 *     `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md`
 *   - Master spec: `docs/superpowers/specs/2026-04-25-life-runtime-architecture-spec.md`
 */

export { LifeClient, type LifeClientConfig } from "./client.js";

export { Transport, type TransportConfig, type TransportCallOptions } from "./transport.js";

export {
  AgentClient,
  EventsClient,
  WalletClient,
  IdentityClient,
} from "./services/index.js";

// WebSocket primitives.
export {
  WsAgentSession,
  openWsAgentSession,
  WebSocketReadyState,
  type WsAgentSessionOptions,
  type WsAgentSessionHandlers,
  type WebSocketFactory,
  type WebSocketLike,
  type AgentEventEnvelope,
  type InboundFrame,
  type OutboundFrame,
} from "./ws.js";

// Typed errors.
export {
  LifeSdkError,
  AuthError,
  RateLimitError,
  BackpressureError,
  IpBlockedError,
  LifedUnavailableError,
  SequenceRetiredError,
  InternalServerError,
  GoingAwayError,
  TlsNegotiationError,
  TransportError,
  GrpcError,
  GrpcCode,
  type GrpcCodeValue,
  closeCodeToError,
  grpcCodeToError,
} from "./errors.js";

// Proto types — re-exported so consumers don't need a deep import.
export * as proto from "./proto/index.js";
