/**
 * `life.v1.Events` service client.
 *
 * @see proto/life/v1/events.proto
 */

import type { Transport, TransportCallOptions } from "../transport.js";
import type {
  Blob,
  BlobRef,
  EventRecord,
  ReadReq,
  SubscribeReq,
} from "../proto/events.js";

const SERVICE = "life.v1.Events";

export class EventsClient {
  constructor(private readonly transport: Transport) {}

  /**
   * Read historical events from a session, server-streamed.
   *
   * Pass `req.fromSequence` to scan from a specific point; pass
   * `req.limit` to bound the response.
   */
  read(req: ReadReq, opts?: TransportCallOptions): AsyncIterable<EventRecord> {
    return this.transport.serverStream<ReadReq, EventRecord>(
      SERVICE,
      "Read",
      req,
      opts,
    );
  }

  /**
   * Subscribe to live events with optional kind filter.
   */
  subscribe(req: SubscribeReq, opts?: TransportCallOptions): AsyncIterable<EventRecord> {
    return this.transport.serverStream<SubscribeReq, EventRecord>(
      SERVICE,
      "Subscribe",
      req,
      opts,
    );
  }

  /**
   * Fetch a content-addressed blob.
   *
   * Convenience: the Connect-protocol JSON wire encodes `bytes` as
   * base64; the SDK handles the conversion in `Transport`.
   */
  getBlob(req: BlobRef, opts?: TransportCallOptions): Promise<Blob> {
    return this.transport.unary<BlobRef, Blob>(SERVICE, "GetBlob", req, opts);
  }

  /**
   * Convenience helper: read a blob and return its raw bytes.
   *
   * The proto `Blob.data` field is the proto3-JSON canonical encoding
   * of `bytes`, which is a base64-encoded string on the wire (see the
   * I-1 fix at top of `../proto/events.ts`). This helper decodes the
   * base64 to the raw `Uint8Array` payload.
   */
  async getBlobBytes(req: BlobRef, opts?: TransportCallOptions): Promise<Uint8Array> {
    const blob = await this.getBlob(req, opts);
    if (typeof blob.data !== "string" || blob.data.length === 0) {
      return new Uint8Array();
    }
    return base64ToBytes(blob.data);
  }
}

/**
 * Decode a proto3-JSON `bytes` field (base64 string) to raw bytes.
 * Exported for downstream consumers who hold an `EventRecord.payload`
 * or `CatalogEntry.manifest` and need the binary form.
 */
export function base64ToBytes(b64: string): Uint8Array {
  if (typeof atob === "function") {
    const bin = atob(b64);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  // Node fallback.
  const g = globalThis as unknown as { Buffer?: { from(s: string, enc: string): Uint8Array } };
  if (g.Buffer) return new Uint8Array(g.Buffer.from(b64, "base64"));
  throw new Error("no base64 decoder available");
}
