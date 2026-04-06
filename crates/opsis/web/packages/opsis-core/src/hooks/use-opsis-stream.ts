"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import type { AgentPresence, AgentState, GaiaState, OpsisEvent, StateDomain, StateLine, WorldDelta, WorldState } from "../lib/types";
import { DEFAULT_DOMAINS } from "../lib/utils";

const MAX_EVENTS = 500;
const MAX_GAIA_INSIGHTS = 20;
const MAX_AGENT_OBSERVATIONS = 20;
const MAX_AGENT_ALERTS = 10;
const MAX_UNROUTED = 50;

function createInitialGaiaState(): GaiaState {
  return { recentInsights: [], tensionScore: 0, activeCorrelations: 0 };
}

function createInitialAgentState(): AgentState {
  return { activeAgents: [], recentObservations: [], recentAlerts: [] };
}

function isAgentEvent(event: OpsisEvent): boolean {
  return typeof event.source === "object" && "Agent" in event.source;
}

function getAgentId(event: OpsisEvent): string | null {
  if (typeof event.source === "object" && "Agent" in event.source) {
    return (event.source as { Agent: string }).Agent;
  }
  return null;
}

function computeAgentState(
  prev: AgentState,
  newEvents: OpsisEvent[],
  tick: number,
): AgentState {
  const agentEvents = newEvents.filter(isAgentEvent);
  if (agentEvents.length === 0) return prev;

  const agents = new Map<string, AgentPresence>(
    prev.activeAgents.map((a) => [a.agentId, { ...a }]),
  );

  const newObs: OpsisEvent[] = [];
  const newAlerts: OpsisEvent[] = [];

  for (const event of agentEvents) {
    const agentId = getAgentId(event);
    if (!agentId) continue;

    const existing = agents.get(agentId) ?? {
      agentId,
      lastSeenTick: 0,
      observationCount: 0,
      alertCount: 0,
    };
    existing.lastSeenTick = tick;

    if (event.kind.type === "AgentObservation") {
      existing.observationCount++;
      newObs.push(event);
    } else if (event.kind.type === "AgentAlert") {
      existing.alertCount++;
      newAlerts.push(event);
    } else {
      existing.observationCount++;
      newObs.push(event);
    }

    agents.set(agentId, existing);
  }

  return {
    activeAgents: [...agents.values()].sort(
      (a, b) => b.lastSeenTick - a.lastSeenTick,
    ),
    recentObservations: [...newObs, ...prev.recentObservations].slice(
      0,
      MAX_AGENT_OBSERVATIONS,
    ),
    recentAlerts: [...newAlerts, ...prev.recentAlerts].slice(
      0,
      MAX_AGENT_ALERTS,
    ),
  };
}

function computeGaiaState(insights: OpsisEvent[]): GaiaState {
  const correlations = insights.filter(
    (e) => e.kind.type === "GaiaCorrelation",
  );
  const tensionScore =
    correlations.length > 0
      ? Math.round(
          correlations.reduce((sum, e) => {
            const kind = e.kind as { type: "GaiaCorrelation"; confidence: number };
            return sum + kind.confidence * 100;
          }, 0) / correlations.length,
        )
      : 0;
  return {
    recentInsights: insights,
    tensionScore,
    activeCorrelations: correlations.length,
  };
}

function createInitialState(): WorldState {
  const stateLines = new Map<StateDomain, StateLine>();
  for (const domain of DEFAULT_DOMAINS) {
    stateLines.set(domain, {
      domain,
      activity: 0,
      trend: "Stable",
      hotspots: [],
      recentEvents: [],
    });
  }
  return { tick: 0, stateLines, allEvents: [] };
}

export interface UseOpsisStreamOptions {
  /** Opsis server URL (default: http://localhost:3010). */
  url?: string;
  /** Auto-connect on mount (default: true). */
  autoConnect?: boolean;
}

export interface UseOpsisStreamReturn {
  /** Current accumulated world state. */
  worldState: WorldState;
  /** Accumulated Gaia intelligence state. */
  gaiaState: GaiaState;
  /** Accumulated agent state (presence, observations, alerts). */
  agentState: AgentState;
  /** Unrouted events (domain: null) for debug/pattern discovery. */
  unroutedEvents: OpsisEvent[];
  /** Connection status. */
  status: "connecting" | "connected" | "disconnected" | "error";
  /** Last error message, if any. */
  error: string | null;
  /** Manually connect. */
  connect: () => void;
  /** Manually disconnect. */
  disconnect: () => void;
}

export function useOpsisStream(options: UseOpsisStreamOptions = {}): UseOpsisStreamReturn {
  const { url = "http://localhost:3010", autoConnect = true } = options;

  const [worldState, setWorldState] = useState<WorldState>(createInitialState);
  const [gaiaState, setGaiaState] = useState<GaiaState>(createInitialGaiaState);
  const [agentState, setAgentState] = useState<AgentState>(createInitialAgentState);
  const [unroutedEvents, setUnroutedEvents] = useState<OpsisEvent[]>([]);
  const [status, setStatus] = useState<"connecting" | "connected" | "disconnected" | "error">(
    "disconnected",
  );
  const [error, setError] = useState<string | null>(null);
  const eventSourceRef = useRef<EventSource | null>(null);

  const applyDelta = useCallback((delta: WorldDelta) => {
    setWorldState((prev) => {
      const newLines = new Map(prev.stateLines);
      const newEvents = [...prev.allEvents];

      for (const sld of delta.state_line_deltas) {
        const existing = newLines.get(sld.domain);
        const recentEvents = [...(existing?.recentEvents ?? []), ...sld.new_events].slice(-50);

        newLines.set(sld.domain, {
          domain: sld.domain,
          activity: sld.activity,
          trend: sld.trend,
          hotspots: sld.hotspots,
          recentEvents,
        });

        newEvents.push(...sld.new_events);
      }

      // Cap total events.
      if (newEvents.length > MAX_EVENTS) {
        newEvents.splice(0, newEvents.length - MAX_EVENTS);
      }

      return { tick: delta.tick, stateLines: newLines, allEvents: newEvents };
    });

    // Accumulate Gaia insights separately.
    const incoming = delta.gaia_insights ?? [];
    if (incoming.length > 0) {
      setGaiaState((prev) => {
        const insights = [...incoming, ...prev.recentInsights].slice(
          0,
          MAX_GAIA_INSIGHTS,
        );
        return computeGaiaState(insights);
      });
    }

    // Accumulate agent state from all events.
    const allNewEvents = delta.state_line_deltas.flatMap((sld) => sld.new_events);
    const unrouted = delta.unrouted_events ?? [];
    const combined = [...allNewEvents, ...unrouted];
    if (combined.some(isAgentEvent)) {
      setAgentState((prev) => computeAgentState(prev, combined, delta.tick));
    }

    // Collect unrouted events.
    if (unrouted.length > 0) {
      setUnroutedEvents((prev) =>
        [...unrouted, ...prev].slice(0, MAX_UNROUTED),
      );
    }
  }, []);

  const connect = useCallback(() => {
    if (eventSourceRef.current) return;

    setStatus("connecting");
    setError(null);

    const es = new EventSource(`${url}/stream`);
    eventSourceRef.current = es;

    es.addEventListener("open", () => {
      setStatus("connected");
    });

    es.addEventListener("world_delta", (event) => {
      try {
        const delta: WorldDelta = JSON.parse(event.data);
        applyDelta(delta);
      } catch {
        // Ignore malformed events.
      }
    });

    es.addEventListener("lagged", () => {
      setError("Stream lagged — some events were dropped");
    });

    es.addEventListener("error", () => {
      setStatus("error");
      setError("Connection lost — retrying...");
      eventSourceRef.current = null;
      es.close();

      // Reconnect after 3 seconds.
      setTimeout(() => {
        setStatus("disconnected");
        connect();
      }, 3000);
    });
  }, [url, applyDelta]);

  const disconnect = useCallback(() => {
    if (eventSourceRef.current) {
      eventSourceRef.current.close();
      eventSourceRef.current = null;
    }
    setStatus("disconnected");
  }, []);

  useEffect(() => {
    if (autoConnect) {
      connect();
    }
    return () => {
      disconnect();
    };
  }, [autoConnect, connect, disconnect]);

  return { worldState, gaiaState, agentState, unroutedEvents, status, error, connect, disconnect };
}
