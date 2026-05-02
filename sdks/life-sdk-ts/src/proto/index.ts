/**
 * Public proto type re-exports.
 *
 * Only `life.v1.*` (user-facing) and `aios.v1.*` (canonical
 * identifiers) are re-exported. The admin plane (`life.admin.v1.*`,
 * `life.admin.gw.v1.*`) is intentionally absent — it is UDS-only and
 * never reachable from the public-plane SDK.
 */

export * from "./aios.js";
export * from "./timestamp.js";
export * from "./agent.js";
export type {
  ReadReq,
  SubscribeReq,
  BlobRef,
  Blob,
} from "./events.js";
export * from "./wallet.js";
export * from "./identity.js";
