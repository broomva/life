"use client";

import type { StateDomain, StateLine } from "../lib/types";
import { activityColor, cn, formatActivity, trendIndicator } from "../lib/utils";

interface StatePanelProps {
  stateLines: Map<StateDomain, StateLine>;
  selectedDomain: StateDomain | null;
  onSelectDomain: (domain: StateDomain | null) => void;
}

export function StatePanel({ stateLines, selectedDomain, onSelectDomain }: StatePanelProps) {
  const sorted = [...stateLines.values()].sort((a, b) => b.activity - a.activity);

  return (
    <div className="flex flex-col gap-1">
      <h2 className="text-xs font-semibold uppercase tracking-wider text-blue-400/70 mb-1">
        World State
      </h2>
      {sorted.map((line) => (
        <button
          key={line.domain}
          type="button"
          onClick={() => onSelectDomain(selectedDomain === line.domain ? null : line.domain)}
          className={cn(
            "flex items-center justify-between px-2 py-1 rounded text-xs font-mono transition-colors",
            "hover:bg-blue-500/10",
            selectedDomain === line.domain && "bg-blue-500/20 ring-1 ring-blue-500/30",
          )}
        >
          <span className="text-slate-300 truncate w-24 text-left">{line.domain}</span>
          <span className={cn("w-4 text-center", activityColor(line.activity))}>
            {trendIndicator(line.trend)}
          </span>
          <span className={cn("w-8 text-right tabular-nums", activityColor(line.activity))}>
            {formatActivity(line.activity)}
          </span>
        </button>
      ))}
    </div>
  );
}
