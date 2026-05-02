/**
 * `life.v1.Events` proto types.
 *
 * Hand-curated TypeScript mirror of `proto/life/v1/events.proto`.
 *
 * @see proto/life/v1/events.proto
 */

import type { SessionId } from "./aios.js";
import type { Timestamp } from "./timestamp.js";

export interface EventRecord {
  sessionId: SessionId;
  sequence: bigint;
  at?: Timestamp;
  kind: string;
  payload: Uint8Array;
}

export interface ReadReq {
  sessionId: SessionId;
  fromSequence?: bigint;
  limit?: number;
}

export interface SubscribeReq {
  sessionId: SessionId;
  /**
   * Free-form kind filter. Empty list means "all kinds".
   */
  kinds?: string[];
  fromSequence?: bigint;
}

export interface BlobRef {
  namespace: string;
  /**
   * Hex SHA-256 digest.
   */
  sha256: string;
}

export interface Blob {
  data: Uint8Array;
  contentType?: string;
}
