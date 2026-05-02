/**
 * `aios.v1` identifier types.
 *
 * Hand-curated TypeScript mirror of `proto/aios/v1/identifiers.proto`.
 * Each identifier is an opaque string newtype on the wire; in TS we
 * model them as `{ value: string }` to keep wire compatibility with
 * the canonical Rust types in `aios_protocol::ids`.
 *
 * @see proto/aios/v1/identifiers.proto
 */

export interface SessionId {
  value: string;
}

export interface AgentId {
  value: string;
}

export interface VmId {
  value: string;
}

export interface VmSnapshotId {
  value: string;
}

export interface BackendId {
  value: string;
}

/**
 * Construct a `SessionId` from a plain string. Convenience helper for
 * call sites that already have a string id.
 */
export function sessionId(value: string): SessionId {
  return { value };
}

/**
 * Construct an `AgentId` from a plain string.
 */
export function agentId(value: string): AgentId {
  return { value };
}
