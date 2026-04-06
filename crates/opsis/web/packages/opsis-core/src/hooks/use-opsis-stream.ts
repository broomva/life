"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import type { GaiaState, OpsisEvent, StateDomain, StateLine, WorldDelta, WorldState } from "../lib/types";
import { DEFAULT_DOMAINS } from "../lib/utils";

const MAX_EVENTS = 500;
const MAX_GAIA_INSIGHTS = 20;

function createInitialGaiaState(): GaiaState {
  return { recentInsights: [], tensionScore: 0, activeCorrelations: 0 };
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

  return { worldState, gaiaState, status, error, connect, disconnect };
}
