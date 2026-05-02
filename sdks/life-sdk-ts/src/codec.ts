/**
 * Codec helpers for proto3-JSON canonical type conversions.
 *
 * Per the proto3-JSON canonical mapping
 * (https://protobuf.dev/programming-guides/proto3/#json):
 *
 * | Proto type    | JSON wire shape | This SDK type |
 * |---------------|-----------------|---------------|
 * | `int64`       | string (number) | `string`      |
 * | `uint64`      | string (number) | `string`      |
 * | `bytes`       | base64 string   | `string`      |
 *
 * Pre-merge code-quality review I-1: the previous SDK shipped these
 * proto types as `bigint` and `Uint8Array`, but the JSON reviver was
 * a no-op so runtime values were always strings. Consumers calling
 * `bal.micros - 1n` would have crashed with "Cannot mix BigInt and
 * other types".
 *
 * The fix typed the proto fields as `string` (the actual wire shape)
 * and provides this module of conversion helpers for callers that
 * need bigint arithmetic or raw bytes.
 */

/**
 * Convert a proto3-JSON `int64`/`uint64` field to a `bigint`.
 *
 * @example
 * const bal = await wallet.getBalance({ ... });
 * const remaining = microsToBigInt(bal.micros) - 1_000_000n;
 */
export function microsToBigInt(s: string): bigint {
  return BigInt(s);
}

/**
 * Convert a `bigint` to the proto3-JSON `int64`/`uint64` wire shape.
 *
 * @example
 * await wallet.debit({ ..., amountMicros: bigIntToMicros(500_000n) });
 */
export function bigIntToMicros(n: bigint): string {
  return n.toString();
}

/**
 * Convert a proto3-JSON `int64` (sequence number) field to a `bigint`.
 * Convenience alias for {@link microsToBigInt} with intent-revealing
 * naming for `EventRecord.sequence` / `SessionRef.fromSequence`.
 *
 * @example
 * for await (const evt of events.subscribe({ ... })) {
 *   if (sequenceToBigInt(evt.sequence) > 1000n) break;
 * }
 */
export function sequenceToBigInt(s: string): bigint {
  return BigInt(s);
}

/**
 * Convert a `bigint` cursor to the proto3-JSON `int64` wire shape.
 *
 * @example
 * await events.read({ ..., fromSequence: bigIntToSequence(lastSeen + 1n) });
 */
export function bigIntToSequence(n: bigint): string {
  return n.toString();
}

/**
 * Decode a proto3-JSON `bytes` field (base64 string) to raw bytes.
 *
 * Proto3-JSON encodes `bytes` fields as either standard or URL-safe
 * base64; this helper accepts both shapes.
 *
 * @example
 * const blob = await events.getBlob({ namespace, sha256 });
 * const bytes = bytesFromBase64(blob.data);
 */
export function bytesFromBase64(b64: string): Uint8Array {
  // Normalize URL-safe variants to standard base64.
  const normalized = b64.replace(/-/g, "+").replace(/_/g, "/");
  // Pad to 4-char boundary (atob is strict on padding).
  const padded = normalized + "===".slice((normalized.length + 3) % 4);
  if (typeof atob === "function") {
    const bin = atob(padded);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  // Node fallback (Node 18+ has atob globally; this is for very old hosts).
  const g = globalThis as unknown as {
    Buffer?: { from(s: string, enc: string): Uint8Array };
  };
  if (g.Buffer) return new Uint8Array(g.Buffer.from(padded, "base64"));
  throw new Error("no base64 decoder available");
}

/**
 * Encode raw bytes to a proto3-JSON `bytes` field (standard base64).
 *
 * @example
 * await client.agent.sendMessage({
 *   sid,
 *   content: "...",
 *   attachmentBlobRef: bytesToBase64(new Uint8Array([0xde, 0xad])),
 * });
 */
export function bytesToBase64(bytes: Uint8Array): string {
  if (typeof btoa === "function") {
    let bin = "";
    for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]!);
    return btoa(bin);
  }
  const g = globalThis as unknown as {
    Buffer?: { from(b: Uint8Array): { toString(enc: string): string } };
  };
  if (g.Buffer) return g.Buffer.from(bytes).toString("base64");
  throw new Error("no base64 encoder available");
}
