"use client";

import { cn } from "../lib/utils";

interface ConnectionStatusProps {
  status: "connecting" | "connected" | "disconnected" | "error";
  tick: number;
  error: string | null;
}

export function ConnectionStatus({ status, tick, error }: ConnectionStatusProps) {
  return (
    <div className="flex items-center gap-2 text-xs font-mono">
      <div
        className={cn(
          "w-2 h-2 rounded-full",
          status === "connected" && "bg-emerald-400 animate-pulse",
          status === "connecting" && "bg-amber-400 animate-pulse",
          status === "disconnected" && "bg-slate-600",
          status === "error" && "bg-red-400",
        )}
      />
      <span className="text-slate-500">
        {status === "connected" && `tick:${tick}`}
        {status === "connecting" && "connecting..."}
        {status === "disconnected" && "offline"}
        {status === "error" && (error ?? "error")}
      </span>
    </div>
  );
}
