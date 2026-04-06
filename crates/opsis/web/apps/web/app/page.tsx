"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  useOpsisStream,
  ConnectionStatus,
  Globe,
  DEFAULT_DOMAINS,
  activityColor,
  trendIndicator,
  formatActivity,
  eventSummary,
  eventSourceLabel,
  cn,
} from "@opsis/core";
import type { StateDomain, OpsisEvent } from "@opsis/core";

export default function OpsisPage() {
  const { worldState, status, error } = useOpsisStream();
  const [selectedDomain, setSelectedDomain] = useState<StateDomain | null>(null);
  const [showLegend, setShowLegend] = useState(true);

  // Accumulate timeline history.
  const historyRef = useRef<Map<StateDomain, number[]>>(new Map());
  for (const domain of DEFAULT_DOMAINS) {
    const line = worldState.stateLines.get(domain);
    if (!line) continue;
    const arr = historyRef.current.get(domain) ?? [];
    arr.push(line.activity);
    if (arr.length > 600) arr.splice(0, arr.length - 600);
    historyRef.current.set(domain, arr);
  }

  // Compute global tension score (average of all activity levels).
  const allActivities = [...worldState.stateLines.values()].map((l) => l.activity);
  const globalTension = allActivities.length > 0
    ? Math.round((allActivities.reduce((a, b) => a + b, 0) / allActivities.length) * 100)
    : 0;
  const tensionLevel = globalTension >= 60 ? "SEVERE" : globalTension >= 30 ? "ELEVATED" : "NORMAL";
  const tensionColor = globalTension >= 60 ? "text-red-400" : globalTension >= 30 ? "text-amber-400" : "text-emerald-400";

  // Recent events for the feed.
  const recentEvents = worldState.allEvents.slice(-100).reverse();
  const filteredEvents = selectedDomain
    ? recentEvents.filter((e) => e.domain === selectedDomain)
    : recentEvents;

  return (
    <div className="h-screen w-screen overflow-hidden relative">
      {/* ═══ FULL-BLEED GLOBE ═══ */}
      <Globe
        events={worldState.allEvents}
        selectedDomain={selectedDomain}
        googleApiKey={process.env.NEXT_PUBLIC_GOOGLE_MAPS_API_KEY}
        cesiumIonToken={process.env.NEXT_PUBLIC_CESIUM_ION_TOKEN}
      />

      {/* ═══ TOP BAR — Logo + Ticker + Status ═══ */}
      <div className="absolute top-0 left-0 right-0 z-20">
        {/* News ticker */}
        <div className="h-6 bg-[oklch(0.08_0.01_250_/_0.9)] border-b border-[var(--color-border)] overflow-hidden flex items-center">
          <div className="ticker-scroll whitespace-nowrap flex gap-8 text-[10px]">
            {recentEvents.slice(0, 20).map((e, i) => (
              <span key={`${e.id}-${i}`} className="flex items-center gap-1.5">
                <span className={cn(
                  "px-1.5 py-0 rounded text-[9px] font-bold uppercase",
                  (e.severity ?? 0) >= 0.7 ? "bg-red-500/20 text-red-400" :
                  (e.severity ?? 0) >= 0.4 ? "bg-amber-500/20 text-amber-400" :
                  "bg-cyan-500/10 text-cyan-600"
                )}>
                  {(e.severity ?? 0) >= 0.7 ? "HIGH" : (e.severity ?? 0) >= 0.4 ? "MED" : "LOW"}
                </span>
                <span className="text-[var(--color-text-dim)]">{eventSummary(e)}</span>
              </span>
            ))}
            {recentEvents.length === 0 && (
              <span className="text-[var(--color-text-muted)]">
                Awaiting world state events...
              </span>
            )}
            {/* Duplicate for seamless loop */}
            {recentEvents.slice(0, 20).map((e, i) => (
              <span key={`dup-${e.id}-${i}`} className="flex items-center gap-1.5">
                <span className={cn(
                  "px-1.5 py-0 rounded text-[9px] font-bold uppercase",
                  (e.severity ?? 0) >= 0.7 ? "bg-red-500/20 text-red-400" :
                  (e.severity ?? 0) >= 0.4 ? "bg-amber-500/20 text-amber-400" :
                  "bg-cyan-500/10 text-cyan-600"
                )}>
                  {(e.severity ?? 0) >= 0.7 ? "HIGH" : (e.severity ?? 0) >= 0.4 ? "MED" : "LOW"}
                </span>
                <span className="text-[var(--color-text-dim)]">{eventSummary(e)}</span>
              </span>
            ))}
          </div>
        </div>

        {/* Header */}
        <div className="flex items-center justify-between px-4 py-2">
          <div className="flex items-center gap-4">
            <h1 className="text-lg font-bold tracking-[0.25em] text-[var(--color-cyan)] glow-cyan">
              OPSIS
            </h1>
            <span className="text-[10px] text-[var(--color-text-muted)] tracking-wider">
              WORLD STATE ENGINE
            </span>
          </div>

          {/* Global tension indicator (Glint style) */}
          <div className="hud-panel px-3 py-1 flex items-center gap-2">
            <span className={cn("w-2 h-2 rounded-full", globalTension >= 60 ? "severity-critical" : globalTension >= 30 ? "severity-high" : "severity-low")} />
            <span className="text-[10px] text-[var(--color-text-dim)] uppercase tracking-wider">
              Global Tension
            </span>
            <span className={cn("text-sm font-bold tabular-nums", tensionColor)}>
              {globalTension}
            </span>
            <span className={cn("text-[10px] font-bold uppercase tracking-wider", tensionColor)}>
              {tensionLevel}
            </span>
          </div>

          <div className="flex items-center gap-4">
            <div className="flex items-center gap-2">
              <span className="text-[10px] text-[var(--color-text-muted)]">
                {recentEvents.length}
              </span>
              <span className="text-[10px] text-[var(--color-text-muted)] uppercase">
                Active Signals
              </span>
            </div>
            <ConnectionStatus status={status} tick={worldState.tick} error={error} />
          </div>
        </div>
      </div>

      {/* ═══ LEFT HUD — State Legend (WorldView style) ═══ */}
      {showLegend && (
        <div className="absolute top-24 left-4 z-20 glass-deep p-3 w-44 bracket-tl bracket-bl">
          <div className="flex items-center justify-between mb-2">
            <span className="text-[10px] font-bold tracking-wider text-[var(--color-cyan-dim)] uppercase">
              State Lines
            </span>
            <button
              type="button"
              onClick={() => setShowLegend(false)}
              className="text-[var(--color-text-muted)] hover:text-[var(--color-text)] text-xs"
            >
              x
            </button>
          </div>
          {[...worldState.stateLines.values()]
            .sort((a, b) => b.activity - a.activity)
            .map((line) => (
              <button
                key={line.domain}
                type="button"
                onClick={() =>
                  setSelectedDomain(selectedDomain === line.domain ? null : line.domain)
                }
                className={cn(
                  "w-full flex items-center gap-2 py-0.5 px-1 rounded text-[11px] transition-colors",
                  "hover:bg-[oklch(0.20_0.03_250_/_0.5)]",
                  selectedDomain === line.domain && "bg-[oklch(0.20_0.04_250_/_0.6)]",
                )}
              >
                <span
                  className="w-2 h-2 rounded-full shrink-0"
                  style={{ backgroundColor: DOMAIN_COLORS[line.domain] ?? "#64748b" }}
                />
                <span className="text-[var(--color-text-dim)] flex-1 text-left truncate">
                  {line.domain}
                </span>
                <span className={cn("text-[10px] tabular-nums", activityColor(line.activity))}>
                  {trendIndicator(line.trend)}
                </span>
              </button>
            ))}
        </div>
      )}

      {/* ═══ TOP-LEFT HUD — Coordinates & Info (WorldView style) ═══ */}
      <div className="absolute bottom-16 left-4 z-20 text-[10px] font-mono text-[var(--color-cyan-dim)]">
        <div>TICK: {worldState.tick.toString().padStart(6, "0")}</div>
        <UtcClock />
      </div>

      {/* ═══ RIGHT HUD — Feed Panel (Glint style) ═══ */}
      <div className="absolute top-24 right-4 bottom-16 z-20 w-80 flex flex-col gap-2">
        {/* Feed header */}
        <div className="hud-panel p-2 bracket-tr">
          <div className="flex items-center justify-between mb-2">
            <span className="text-[10px] font-bold tracking-wider text-[var(--color-cyan-dim)] uppercase">
              Feed
            </span>
            <div className="flex items-center gap-1.5">
              <span className="w-1.5 h-1.5 rounded-full bg-[var(--color-live)] animate-pulse" />
              <span className="text-[10px] text-[var(--color-live)]">LIVE</span>
            </div>
          </div>
          {/* Filter pills */}
          <div className="flex flex-wrap gap-1">
            {["All", "Emergency", "Weather", "Finance", "Conflict"].map((cat) => (
              <button
                key={cat}
                type="button"
                onClick={() => setSelectedDomain(cat === "All" ? null : cat)}
                className={cn(
                  "pill",
                  (cat === "All" && !selectedDomain) || selectedDomain === cat
                    ? "pill-active"
                    : "pill-inactive",
                )}
              >
                {cat}
              </button>
            ))}
          </div>
        </div>

        {/* Feed items */}
        <div className="hud-panel flex-1 min-h-0 overflow-y-auto p-2 space-y-1">
          {filteredEvents.length === 0 ? (
            <div className="text-center py-8">
              <p className="text-[var(--color-text-muted)] text-xs">
                Waiting for events...
              </p>
            </div>
          ) : (
            filteredEvents.slice(0, 50).map((event) => (
              <div
                key={event.id}
                className="flex gap-2 py-1.5 px-1 rounded hover:bg-[oklch(0.15_0.02_250_/_0.5)] transition-colors border-b border-[var(--color-border)] last:border-0"
              >
                <div className="shrink-0 mt-1">
                  <span className={cn(
                    "w-2 h-2 rounded-full block",
                    (event.severity ?? 0) >= 0.7 ? "severity-critical" :
                    (event.severity ?? 0) >= 0.4 ? "severity-high" :
                    (event.severity ?? 0) >= 0.2 ? "severity-medium" : "severity-low",
                  )} />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-1.5 mb-0.5">
                    <span className={cn(
                      "text-[9px] px-1 rounded font-bold uppercase",
                      (event.severity ?? 0) >= 0.7 ? "bg-red-500/15 text-red-400" :
                      (event.severity ?? 0) >= 0.4 ? "bg-amber-500/15 text-amber-400" :
                      "bg-slate-500/15 text-slate-400",
                    )}>
                      {event.domain}
                    </span>
                    <span className="text-[9px] text-[var(--color-text-muted)]">
                      {eventSourceLabel(event.source)}
                    </span>
                  </div>
                  <p className="text-[11px] text-[var(--color-text-dim)] leading-tight truncate">
                    {eventSummary(event)}
                  </p>
                </div>
              </div>
            ))
          )}
        </div>
      </div>

      {/* ═══ BOTTOM BAR — Timeline + Controls (WorldView style) ═══ */}
      <div className="absolute bottom-0 left-0 right-0 z-20">
        <div className="bg-[oklch(0.06_0.01_250_/_0.92)] border-t border-[var(--color-border)] backdrop-blur-sm px-4 py-2">
          {/* Category filter tags */}
          <div className="flex items-center gap-2 flex-wrap">
            {DEFAULT_DOMAINS.map((domain) => {
              const line = worldState.stateLines.get(domain);
              const activity = line?.activity ?? 0;
              return (
                <button
                  key={domain}
                  type="button"
                  onClick={() =>
                    setSelectedDomain(selectedDomain === domain ? null : domain)
                  }
                  className={cn(
                    "flex items-center gap-1.5 text-[10px] py-0.5 px-2 rounded transition-colors",
                    selectedDomain === domain
                      ? "bg-[var(--color-cyan-glow)] text-[var(--color-cyan)]"
                      : "text-[var(--color-text-muted)] hover:text-[var(--color-text-dim)]",
                  )}
                >
                  <span
                    className="w-1.5 h-1.5 rounded-full"
                    style={{
                      backgroundColor: DOMAIN_COLORS[domain] ?? "#64748b",
                      opacity: 0.5 + activity * 0.5,
                    }}
                  />
                  {domain}
                </button>
              );
            })}
            <span className="ml-auto text-[10px] text-[var(--color-text-muted)] tabular-nums">
              {worldState.tick > 0
                ? `${Math.floor(worldState.tick / 60)}m ${worldState.tick % 60}s`
                : "---"}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}

/* ═══ UTC Clock (avoids hydration mismatch) ═══ */
function UtcClock() {
  const [time, setTime] = useState("");
  useEffect(() => {
    const update = () => setTime(new Date().toISOString().replace("T", " ").slice(0, 19));
    update();
    const id = setInterval(update, 1000);
    return () => clearInterval(id);
  }, []);
  return <div>{time ? `${time} UTC` : ""}</div>;
}

/* ═══ Domain color map ═══ */
const DOMAIN_COLORS: Record<string, string> = {
  Emergency: "#ef4444",
  Health: "#22c55e",
  Finance: "#3b82f6",
  Trade: "#f59e0b",
  Conflict: "#dc2626",
  Politics: "#8b5cf6",
  Weather: "#06b6d4",
  Space: "#a78bfa",
  Ocean: "#0ea5e9",
  Technology: "#10b981",
  Personal: "#f472b6",
  Infrastructure: "#64748b",
};

