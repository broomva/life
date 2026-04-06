"use client";

import type { AgentState } from "../lib/types";
import { cn } from "../lib/utils";

interface AgentPresencePanelProps {
  agentState: AgentState;
  onAgentClick?: (agentId: string) => void;
}

export function AgentPresencePanel({
  agentState,
  onAgentClick,
}: AgentPresencePanelProps) {
  const { activeAgents, recentObservations, recentAlerts } = agentState;
  const hasAgents = activeAgents.length > 0;

  return (
    <div className="glass-deep p-2.5 bracket-tr bracket-br agent-panel">
      {/* Header */}
      <div className="flex items-center justify-between mb-2">
        <span className="text-[10px] font-bold tracking-wider uppercase agent-header">
          Agent Link
        </span>
        <div className="flex items-center gap-1">
          <span
            className={cn(
              "w-1.5 h-1.5 rounded-full",
              hasAgents ? "agent-dot-active" : "bg-slate-600",
            )}
          />
          <span
            className={cn(
              "text-[9px] font-bold uppercase",
              hasAgents ? "agent-text" : "text-[var(--color-text-muted)]",
            )}
          >
            {hasAgents ? "ESTABLISHED" : "NO AGENTS"}
          </span>
        </div>
      </div>

      {!hasAgents ? (
        <p className="text-[10px] text-[var(--color-text-muted)] text-center py-2 font-mono">
          &gt; awaiting agent connection...
        </p>
      ) : (
        <>
          {/* Agent list */}
          <div className="space-y-1 mb-2">
            {activeAgents.map((agent) => (
              <button
                key={agent.agentId}
                type="button"
                onClick={() => onAgentClick?.(agent.agentId)}
                className="w-full flex items-center gap-1.5 py-0.5 px-1 rounded text-[10px] hover:bg-[rgba(13,154,255,0.1)] transition-colors font-mono"
              >
                <span className="agent-diamond">&#x25C6;</span>
                <span className="agent-text truncate flex-1 text-left">
                  {agent.agentId}
                </span>
                <span className="text-[var(--color-text-muted)] tabular-nums">
                  {agent.observationCount}obs
                </span>
                {agent.alertCount > 0 && (
                  <span className="text-amber-400 tabular-nums">
                    {agent.alertCount}!
                  </span>
                )}
              </button>
            ))}
          </div>

          {/* Recent alerts */}
          {recentAlerts.length > 0 && (
            <div className="border-t border-[rgba(13,154,255,0.15)] pt-1.5">
              <span className="text-[9px] text-amber-400/70 uppercase tracking-wider font-bold">
                Alerts
              </span>
              {recentAlerts.slice(0, 3).map((event) => (
                <div
                  key={event.id}
                  className="flex gap-1 py-0.5 text-[10px] font-mono"
                >
                  <span className="text-amber-400 shrink-0">&gt;</span>
                  <span className="text-[var(--color-text-dim)] truncate">
                    {event.kind.type === "AgentAlert"
                      ? (event.kind as { type: "AgentAlert"; message: string })
                          .message
                      : "alert"}
                  </span>
                </div>
              ))}
            </div>
          )}

          {/* Recent observations preview */}
          {recentObservations.length > 0 && recentAlerts.length === 0 && (
            <div className="border-t border-[rgba(13,154,255,0.15)] pt-1.5">
              {recentObservations.slice(0, 2).map((event) => (
                <div
                  key={event.id}
                  className="flex gap-1 py-0.5 text-[10px] font-mono"
                >
                  <span className="agent-text shrink-0">&gt;</span>
                  <span className="text-[var(--color-text-dim)] truncate">
                    {event.kind.type === "AgentObservation"
                      ? (
                          event.kind as {
                            type: "AgentObservation";
                            insight: string;
                          }
                        ).insight
                      : "observation"}
                  </span>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
}
