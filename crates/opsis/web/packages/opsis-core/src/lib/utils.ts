import { type ClassValue, clsx } from "clsx";
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
