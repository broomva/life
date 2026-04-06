"use client";

import { useEffect, useRef } from "react";
import type { StateDomain, StateLine } from "../lib/types";
import { DEFAULT_DOMAINS } from "../lib/utils";

interface TimelineProps {
  stateLines: Map<StateDomain, StateLine>;
  /** History of activity levels per domain. Each entry is one tick. */
  history: Map<StateDomain, number[]>;
  tick: number;
}

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

const TRACK_HEIGHT = 18;
const LABEL_WIDTH = 90;

export function Timeline({ stateLines, history, tick }: TimelineProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const container = containerRef.current;
    if (!canvas || !container) return;

    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const width = container.clientWidth - LABEL_WIDTH;
    const height = DEFAULT_DOMAINS.length * TRACK_HEIGHT;

    canvas.width = width * devicePixelRatio;
    canvas.height = height * devicePixelRatio;
    canvas.style.width = `${width}px`;
    canvas.style.height = `${height}px`;
    ctx.scale(devicePixelRatio, devicePixelRatio);

    ctx.clearRect(0, 0, width, height);

    DEFAULT_DOMAINS.forEach((domain, i) => {
      const y = i * TRACK_HEIGHT;
      const samples = history.get(domain) ?? [];
      const color = DOMAIN_COLORS[domain] ?? "#64748b";

      // Draw track background.
      ctx.fillStyle = "rgba(30, 41, 59, 0.3)";
      ctx.fillRect(0, y, width, TRACK_HEIGHT - 1);

      if (samples.length < 2) return;

      // Draw waveform.
      ctx.beginPath();
      ctx.strokeStyle = color;
      ctx.lineWidth = 1.5;
      ctx.globalAlpha = 0.8;

      const maxSamples = Math.min(samples.length, width);
      const startIdx = Math.max(0, samples.length - maxSamples);

      for (let j = startIdx; j < samples.length; j++) {
        const x = ((j - startIdx) / maxSamples) * width;
        const val = samples[j] ?? 0;
        const barY = y + TRACK_HEIGHT - 1 - val * (TRACK_HEIGHT - 2);

        if (j === startIdx) {
          ctx.moveTo(x, barY);
        } else {
          ctx.lineTo(x, barY);
        }
      }

      ctx.stroke();
      ctx.globalAlpha = 1.0;
    });
  }, [history]);

  return (
    <div ref={containerRef} className="flex w-full">
      <div className="shrink-0" style={{ width: LABEL_WIDTH }}>
        {DEFAULT_DOMAINS.map((domain) => (
          <div
            key={domain}
            className="text-[10px] font-mono text-slate-500 truncate px-1 flex items-center"
            style={{ height: TRACK_HEIGHT }}
          >
            {domain}
          </div>
        ))}
      </div>
      <canvas ref={canvasRef} className="flex-1" />
    </div>
  );
}
