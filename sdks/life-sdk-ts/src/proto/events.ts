/**
 * `life.v1.Events` proto types.
 *
 * Hand-curated TypeScript mirror of `proto/life/v1/events.proto`.
 *
 * **Proto3 JSON canonical mapping** (per
 * https://protobuf.dev/programming-guides/proto3/#json):
 * - `int64` / `uint64` → JSON string (preserves precision past 2^53).
 * - `bytes` → base64 string (URL-safe or standard; both accepted on read).
 *
 * Pre-merge code-quality review I-1: this PR changes `sequence` /
 * `fromSequence` from `bigint` to `string` and `payload` / `data` from
 * `Uint8Array` to `string` (base64) so the type system reflects the
 * actual runtime values. The transport's `jsonReviver` was a no-op;
 * the previous typing was a type lie that would break every `payload
 * instanceof Uint8Array` consumer call.
 *
 * Convert to bigint or bytes at the call site via the helpers in
 * `../codec.js` (see {@link sequenceToBigInt}, {@link bytesFromBase64}).
 *
 * @see proto/life/v1/events.proto
 */

import type { SessionId } from "./aios.js";
import type { Timestamp } from "./timestamp.js";

export interface EventRecord {
  sessionId: SessionId;
  /** Monotonic event sequence. Proto3 JSON wire shape: string (int64). */
  sequence: string;
  at?: Timestamp;
  kind: string;
  /** Opaque event payload. Proto3 JSON wire shape: base64 string. */
  payload: string;
}

export interface ReadReq {
  sessionId: SessionId;
  /** Resume cursor. Proto3 JSON wire shape: string (int64) or omitted. */
  fromSequence?: string;
  limit?: number;
}

export interface SubscribeReq {
  sessionId: SessionId;
  /**
   * Free-form kind filter. Empty list means "all kinds".
   */
  kinds?: string[];
  /** Resume cursor. Proto3 JSON wire shape: string (int64) or omitted. */
  fromSequence?: string;
}

export interface BlobRef {
  namespace: string;
  /**
   * Hex SHA-256 digest.
   */
  sha256: string;
}

export interface Blob {
  /** Blob bytes. Proto3 JSON wire shape: base64 string. */
  data: string;
  contentType?: string;
}
