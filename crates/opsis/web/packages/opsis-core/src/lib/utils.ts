import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** Map activity level (0-1) to a CSS color class. */
export function activityColor(activity: number): string {
  if (activity >= 0.7) return "text-red-400";
  if (activity >= 0.4) return "text-amber-400";
  return "text-emerald-400";
}

/** Map trend to display character. */
export function trendIndicator(trend: string): string {
  switch (trend) {
    case "Spike":
    case "Rising":
      return "\u25B2"; // ▲
    case "Crash":
    case "Falling":
      return "\u25BC"; // ▼
    default:
      return "\u2500"; // ─
  }
}

/** Format activity as percentage string. */
export function formatActivity(activity: number): string {
  return (activity * 100).toFixed(0);
}

/** Extract a display summary from an OpsisEvent. */
export function eventSummary(event: { kind: import("./types").OpsisEventKind }): string {
  const k = event.kind;
  switch (k.type) {
    case "WorldObservation":
      return k.summary;
    case "GaiaCorrelation":
      return k.description;
    case "GaiaAnomaly":
      return `${k.domain}: ${k.description} (${k.sigma.toFixed(1)}σ)`;
    case "AgentObservation":
      return k.insight;
    case "AgentAlert":
      return k.message;
    case "Custom":
      return k.event_type;
    default:
      return "Unknown event";
  }
}

/** Extract a display source label from an EventSource. */
export function eventSourceLabel(source: import("./types").EventSource): string {
  if (typeof source === "string") return source.toLowerCase();
  if ("Feed" in source) return source.Feed;
  if ("Agent" in source) return `agent:${source.Agent}`;
  if ("Universe" in source) return `universe:${source.Universe}`;
  return "unknown";
}

/** Default state domains in display order. */
export const DEFAULT_DOMAINS = [
  "Emergency",
  "Health",
  "Finance",
  "Trade",
  "Conflict",
  "Politics",
  "Weather",
  "Space",
  "Ocean",
  "Technology",
  "Personal",
  "Infrastructure",
] as const;
