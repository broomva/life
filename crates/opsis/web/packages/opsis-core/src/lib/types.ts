/** Monotonic world tick counter. */
export interface WorldTick {
  readonly value: number;
}

/** A point on the globe (WGS84). */
export interface GeoPoint {
  readonly lat: number;
  readonly lon: number;
}

/** Axis-aligned bounding box. */
export interface Bbox {
  readonly sw: GeoPoint;
  readonly ne: GeoPoint;
}

/** Geographic cluster of activity. */
export interface GeoHotspot {
  readonly center: GeoPoint;
  readonly radius_km: number;
  readonly intensity: number;
  readonly event_count: number;
}

/** Domains of world activity. */
export type StateDomain =
  | "Emergency"
  | "Health"
  | "Finance"
  | "Trade"
  | "Conflict"
  | "Politics"
  | "Weather"
  | "Space"
  | "Ocean"
  | "Technology"
  | "Personal"
  | "Infrastructure"
  | string; // Custom domains

/** Activity trend direction. */
export type Trend = "Rising" | "Falling" | "Stable" | "Spike" | "Crash";

// ═══ OpsisEvent Protocol (mirrors Rust opsis-core) ═══

/** Who produced this event. */
export type EventSource =
  | { Feed: string }
  | { Agent: string }
  | "Gaia"
  | "System"
  | { Universe: string };

/** What happened — tagged union matching Rust OpsisEventKind. */
export type OpsisEventKind =
  | { type: "WorldObservation"; summary: string }
  | { type: "GaiaCorrelation"; domains: StateDomain[]; description: string; confidence: number }
  | { type: "GaiaAnomaly"; domain: StateDomain; sigma: number; description: string }
  | { type: "AgentObservation"; insight: string; confidence: number }
  | { type: "AgentAlert"; message: string }
  | { type: "Custom"; event_type: string; data: unknown };

/** Universal event envelope for all Opsis events. */
export interface OpsisEvent {
  readonly id: string;
  readonly tick: number;
  readonly timestamp: string;
  readonly source: EventSource;
  readonly kind: OpsisEventKind;
  readonly location: GeoPoint | null;
  readonly domain: StateDomain | null;
  readonly severity: number | null;
  readonly schema_key: string;
  readonly tags: string[];
}

/** Change in a single state line during one tick. */
export interface StateLineDelta {
  readonly domain: StateDomain;
  readonly activity: number;
  readonly trend: Trend;
  readonly new_events: OpsisEvent[];
  readonly hotspots: GeoHotspot[];
}

/** Broadcast payload sent every tick via SSE. */
export interface WorldDelta {
  readonly tick: number;
  readonly timestamp: string;
  readonly state_line_deltas: StateLineDelta[];
  /** Gaia-generated cross-domain insights for this tick (may be empty). */
  readonly gaia_insights: OpsisEvent[];
  /** Events without a domain — exposed for pattern discovery (may be empty). */
  readonly unrouted_events?: OpsisEvent[];
}

/** Agent presence tracking (client-side). */
export interface AgentPresence {
  agentId: string;
  lastSeenTick: number;
  observationCount: number;
  alertCount: number;
}

/** Accumulated agent state (client-side). */
export interface AgentState {
  activeAgents: AgentPresence[];
  recentObservations: OpsisEvent[];
  recentAlerts: OpsisEvent[];
}

/** Accumulated Gaia intelligence state (client-side). */
export interface GaiaState {
  /** Recent Gaia insights (last 20), newest first. */
  recentInsights: OpsisEvent[];
  /** Global tension score derived from recent GaiaCorrelation confidence (0–100). */
  tensionScore: number;
  /** Number of GaiaCorrelation events in recent insights. */
  activeCorrelations: number;
}

/** Per-domain state line (client-side accumulated state). */
export interface StateLine {
  domain: StateDomain;
  activity: number;
  trend: Trend;
  hotspots: GeoHotspot[];
  recentEvents: OpsisEvent[];
}

/** Full client-side world state. */
export interface WorldState {
  tick: number;
  stateLines: Map<StateDomain, StateLine>;
  allEvents: OpsisEvent[];
}

/** Health check response from opsisd. */
export interface HealthResponse {
  readonly status: string;
  readonly service: string;
  readonly version: string;
  readonly uptime_seconds: number;
  readonly connected_clients: number;
}

// ═══ Legacy alias for backward compatibility ═══
/** @deprecated Use OpsisEvent instead */
export type StateEvent = OpsisEvent;
