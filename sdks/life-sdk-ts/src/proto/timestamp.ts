/**
 * `google.protobuf.Timestamp` JSON shape.
 *
 * The proto3 JSON mapping for Timestamp is an RFC 3339 string. This
 * SDK exposes both the canonical JSON form (string) and a JS-native
 * `Date` helper because most consumers want a `Date`.
 *
 * Wire format: `"2026-04-29T15:00:00Z"` (RFC 3339, UTC).
 */

export type Timestamp = string;

/**
 * Convert a proto3 JSON Timestamp string into a JS `Date`.
 *
 * Returns `null` when the input is empty or undefined — callers that
 * want a definite Date should null-check at the call site.
 */
export function timestampToDate(ts: Timestamp | null | undefined): Date | null {
  if (!ts) return null;
  const d = new Date(ts);
  return Number.isNaN(d.getTime()) ? null : d;
}

/**
 * Convert a `Date` to a proto3 JSON Timestamp string.
 */
export function dateToTimestamp(d: Date): Timestamp {
  return d.toISOString();
}
