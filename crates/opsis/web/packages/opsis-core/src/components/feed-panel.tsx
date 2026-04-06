"use client";

import { useState } from "react";
import type { OpsisEvent } from "../lib/types";
import { cn, eventSummary } from "../lib/utils";

interface FeedPanelProps {
  events: OpsisEvent[];
  onEventClick?: (event: OpsisEvent) => void;
}

const TABS = ["All", "Emergency", "Weather", "Finance", "Trade"] as const;

export function FeedPanel({ events, onEventClick }: FeedPanelProps) {
  const [activeTab, setActiveTab] = useState<string>("All");

  const filtered =
    activeTab === "All" ? events : events.filter((e) => e.domain === activeTab);

  const recent = filtered.slice(-50).reverse();

  return (
    <div className="flex flex-col gap-1 min-h-0">
      <h2 className="text-xs font-semibold uppercase tracking-wider text-blue-400/70">Feeds</h2>

      <div className="flex gap-1 mb-1">
        {TABS.map((tab) => (
          <button
            key={tab}
            type="button"
            onClick={() => setActiveTab(tab)}
            className={cn(
              "px-2 py-0.5 text-[10px] rounded transition-colors",
              activeTab === tab
                ? "bg-blue-500/20 text-blue-300"
                : "text-slate-500 hover:text-slate-300",
            )}
          >
            {tab}
          </button>
        ))}
      </div>

      <div className="flex-1 overflow-y-auto space-y-0.5 min-h-0">
        {recent.length === 0 ? (
          <p className="text-slate-600 text-xs italic">Waiting for events...</p>
        ) : (
          recent.map((event) => (
            <button
              key={event.id}
              type="button"
              onClick={() => onEventClick?.(event)}
              className="w-full text-left px-2 py-1 rounded text-xs hover:bg-blue-500/10 transition-colors"
            >
              <div className="flex items-center gap-1.5">
                <span
                  className={cn(
                    "w-1.5 h-1.5 rounded-full shrink-0",
                    (event.severity ?? 0) >= 0.7
                      ? "bg-red-400"
                      : (event.severity ?? 0) >= 0.4
                        ? "bg-amber-400"
                        : "bg-emerald-400",
                  )}
                />
                <span className="text-slate-500 shrink-0">{event.domain ?? "System"}</span>
                <span className="text-slate-300 truncate">{eventSummary(event)}</span>
              </div>
            </button>
          ))
        )}
      </div>
    </div>
  );
}
