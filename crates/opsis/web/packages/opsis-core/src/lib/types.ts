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

/** A normalized event that drives state lines. */
export interface StateEvent {
  readonly id: string;
  readonly tick: number;
  readonly domain: StateDomain;
  readonly location: GeoPoint | null;
  readonly severity: number;
  readonly summary: string;
  readonly source: string;
  readonly tags: string[];
  readonly raw_ref: string;
}

/** Change in a single state line during one tick. */
export interface StateLineDelta {
  readonly domain: StateDomain;
  readonly activity: number;
  readonly trend: Trend;
  readonly new_events: StateEvent[];
  readonly hotspots: GeoHotspot[];
}

/** Broadcast payload sent every tick via SSE. */
export interface WorldDelta {
  readonly tick: number;
  readonly timestamp: string;
  readonly state_line_deltas: StateLineDelta[];
}

/** Per-domain state line (client-side accumulated state). */
export interface StateLine {
  domain: StateDomain;
  activity: number;
  trend: Trend;
  hotspots: GeoHotspot[];
  recentEvents: StateEvent[];
}

/** Full client-side world state. */
export interface WorldState {
  tick: number;
  stateLines: Map<StateDomain, StateLine>;
  allEvents: StateEvent[];
}

/** Health check response from opsisd. */
export interface HealthResponse {
  readonly status: string;
  readonly service: string;
  readonly version: string;
  readonly uptime_seconds: number;
  readonly connected_clients: number;
}
