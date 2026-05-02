/**
 * `life.v1.Agent` proto types.
 *
 * Hand-curated TypeScript mirror of `proto/life/v1/agent.proto` (the
 * full Spec C₂ §3.1 surface). Wire format follows the proto3 JSON
 * mapping when serialized via grpc-web; see the gRPC documentation
 * for the canonical conversion rules.
 *
 * Only the user-facing `life.v1.Agent` service is exposed — the
 * admin plane (`life.admin.v1.*` / `life.admin.gw.v1.*`) is UDS-only
 * and never reachable through lifegw, so it is intentionally absent
 * from this SDK.
 *
 * @see proto/life/v1/agent.proto
 */

import type { AgentId, SessionId } from "./aios.js";
import type { Timestamp } from "./timestamp.js";

// ── Enums ──────────────────────────────────────────────────────────

/**
 * Agent event kinds emitted on the `Agent.SendMessage` /
 * `Agent.StreamSession` server stream. Mirrors `AgentEventKind` in
 * `proto/life/v1/agent.proto`.
 */
export const AgentEventKind = {
  Unspecified: "AGENT_EVENT_KIND_UNSPECIFIED",
  Token: "AGENT_EVENT_KIND_TOKEN",
  ToolCallPending: "AGENT_EVENT_KIND_TOOL_CALL_PENDING",
  ToolResult: "AGENT_EVENT_KIND_TOOL_RESULT",
  ApprovalRequired: "AGENT_EVENT_KIND_APPROVAL_REQUIRED",
  Finish: "AGENT_EVENT_KIND_FINISH",
  Error: "AGENT_EVENT_KIND_ERROR",
  Hibernate: "AGENT_EVENT_KIND_HIBERNATE",
} as const;

export type AgentEventKind = (typeof AgentEventKind)[keyof typeof AgentEventKind];

// ── Messages ───────────────────────────────────────────────────────

export interface ChildPolicy {
  inheritSkills?: boolean;
  inheritModels?: boolean;
  depthCap?: number;
  fanoutCap?: number;
}

export interface CreateSessionReq {
  userId?: string;
  projectId?: string;
  label?: string;
  resumeSid?: SessionId;
  inheritPolicy?: ChildPolicy;
}

export interface Session {
  sid: SessionId;
  agentId?: AgentId;
  userId?: string;
  projectId?: string;
  createdAt?: Timestamp;
}

export interface SessionRef {
  sid: SessionId;
  /**
   * Resume cursor — when set, `Agent.StreamSession` replays from
   * `from_sequence + 1`. `0` (or unset) means "fresh stream". See
   * Spec C₃ §11.4 for the WebSocket-side reconnect semantics.
   */
  fromSequence?: bigint;
}

export interface SendMessageReq {
  sid: SessionId;
  content: string;
  /**
   * Optional reference to a previously-uploaded blob, e.g.
   * `"sha256:<hex>"`. Forwarded verbatim to lifed.
   */
  attachmentBlobRef?: Uint8Array;
}

/**
 * Re-exported from `events.ts` to match the proto layout where
 * `EventRecord` lives in `events.proto` even though `AgentEvent`
 * carries it.
 */
export interface EventRecord {
  sessionId: SessionId;
  sequence: bigint;
  at?: Timestamp;
  kind: string;
  payload: Uint8Array;
}

export interface AgentEvent {
  record: EventRecord;
  kind: AgentEventKind;
}

export interface ApprovalReq {
  sid: SessionId;
  dispatchId: string;
}

export interface DispatchRef {
  sid: SessionId;
  dispatchId: string;
}

export interface ListSkillsReq {
  projectId?: string;
}

export interface ListModelsReq {
  projectId?: string;
}

export interface ListToolsReq {
  projectId?: string;
}

export interface CatalogEntry {
  id: string;
  version: string;
  manifest: Uint8Array;
}

export interface SkillCatalog {
  items: CatalogEntry[];
}

export interface ModelCatalog {
  items: CatalogEntry[];
}

export interface ToolCatalog {
  items: CatalogEntry[];
}

export interface SpawnChildReq {
  parentSid: SessionId;
  spec?: CreateSessionReq;
  budgetCapMicros?: bigint;
  inheritPolicy?: ChildPolicy;
}

export interface SpawnChildResp {
  childSid: SessionId;
  address: string;
}

export interface Empty {}
