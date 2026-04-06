"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import type { OpsisEvent } from "../lib/types";
import { cn } from "../lib/utils";

interface AgentTerminalProps {
  agentId: string;
  /** Recent agent observations/alerts to display as history. */
  recentEvents: OpsisEvent[];
  /** Arcan daemon URL (default: http://localhost:3000). */
  arcanUrl?: string;
  onClose?: () => void;
}

interface TerminalLine {
  type: "input" | "output" | "tool" | "error" | "system";
  text: string;
  timestamp?: string;
}

export function AgentTerminal({
  agentId,
  recentEvents,
  arcanUrl = "http://localhost:3000",
  onClose,
}: AgentTerminalProps) {
  const [lines, setLines] = useState<TerminalLine[]>([
    { type: "system", text: `> connected to ${agentId}` },
    { type: "system", text: "> type a message to interact with the agent" },
  ]);
  const [input, setInput] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [history, setHistory] = useState<string[]>([]);
  const [historyIdx, setHistoryIdx] = useState(-1);
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Auto-scroll on new lines.
  useEffect(() => {
    scrollRef.current?.scrollTo(0, scrollRef.current.scrollHeight);
  }, [lines]);

  // Focus input on mount.
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Seed terminal with recent agent events.
  useEffect(() => {
    if (recentEvents.length === 0) return;
    const eventLines: TerminalLine[] = recentEvents
      .slice(0, 5)
      .reverse()
      .map((e) => {
        if (e.kind.type === "AgentObservation") {
          const kind = e.kind as { type: "AgentObservation"; insight: string };
          return { type: "output" as const, text: kind.insight };
        }
        if (e.kind.type === "AgentAlert") {
          const kind = e.kind as { type: "AgentAlert"; message: string };
          return { type: "error" as const, text: `ALERT: ${kind.message}` };
        }
        return { type: "output" as const, text: "agent event" };
      });
    setLines((prev) => [...prev, ...eventLines]);
  }, []); // Only on mount

  const handleSubmit = useCallback(async () => {
    const msg = input.trim();
    if (!msg || isStreaming) return;

    // Built-in commands.
    if (msg === "clear") {
      setLines([{ type: "system", text: `> terminal cleared` }]);
      setInput("");
      return;
    }
    if (msg === "help") {
      setLines((prev) => [
        ...prev,
        { type: "input", text: msg },
        { type: "system", text: "> commands: clear, help, <any message>" },
        {
          type: "system",
          text: "> messages are sent to the agent for processing",
        },
      ]);
      setInput("");
      return;
    }

    setLines((prev) => [...prev, { type: "input", text: msg }]);
    setHistory((prev) => [msg, ...prev].slice(0, 50));
    setHistoryIdx(-1);
    setInput("");
    setIsStreaming(true);

    try {
      const res = await fetch(`${arcanUrl}/sessions/default/runs`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ objective: msg }),
      });

      if (!res.ok) {
        setLines((prev) => [
          ...prev,
          {
            type: "error",
            text: `error: ${res.status} ${res.statusText}`,
          },
        ]);
        setIsStreaming(false);
        return;
      }

      const data = await res.json();
      const answer =
        data.final_answer ?? data.answer ?? JSON.stringify(data).slice(0, 500);

      setLines((prev) => [...prev, { type: "output", text: answer }]);
    } catch (err) {
      setLines((prev) => [
        ...prev,
        {
          type: "error",
          text: `connection failed: ${err instanceof Error ? err.message : "unknown"}`,
        },
      ]);
    } finally {
      setIsStreaming(false);
    }
  }, [input, isStreaming, arcanUrl]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter") {
        handleSubmit();
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        if (history.length > 0) {
          const newIdx = Math.min(historyIdx + 1, history.length - 1);
          setHistoryIdx(newIdx);
          setInput(history[newIdx] ?? "");
        }
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        if (historyIdx > 0) {
          const newIdx = historyIdx - 1;
          setHistoryIdx(newIdx);
          setInput(history[newIdx] ?? "");
        } else {
          setHistoryIdx(-1);
          setInput("");
        }
      }
    },
    [handleSubmit, history, historyIdx],
  );

  return (
    <div className="agent-terminal rounded-lg overflow-hidden flex flex-col max-h-80">
      {/* Header bar */}
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-[#0d9aff33]">
        <div className="flex items-center gap-2">
          <span className="text-[10px] font-bold tracking-wider uppercase agent-text">
            {agentId}
          </span>
          {isStreaming && (
            <span className="text-[9px] text-amber-400 animate-pulse">
              PROCESSING
            </span>
          )}
        </div>
        {onClose && (
          <button
            type="button"
            onClick={onClose}
            className="text-[#1a3a5c] hover:text-[#4fc3f7] text-xs transition-colors"
          >
            x
          </button>
        )}
      </div>

      {/* Output area */}
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto px-3 py-2 space-y-0.5 min-h-0 text-[11px] leading-relaxed"
      >
        {lines.map((line, i) => (
          <div key={i} className="flex gap-1.5">
            {line.type === "input" && (
              <>
                <span className="text-[#0d9aff] shrink-0">&gt;_</span>
                <span className="text-[#4fc3f7]">{line.text}</span>
              </>
            )}
            {line.type === "output" && (
              <span className="text-[#4fc3f7cc]">{line.text}</span>
            )}
            {line.type === "tool" && (
              <>
                <span className="text-violet-400 shrink-0">&#x25C8;</span>
                <span className="text-violet-300">{line.text}</span>
              </>
            )}
            {line.type === "error" && (
              <span className="text-amber-400">{line.text}</span>
            )}
            {line.type === "system" && (
              <span className="text-[#1a3a5c]">{line.text}</span>
            )}
          </div>
        ))}
        {isStreaming && <span className="agent-streaming" />}
      </div>

      {/* Input */}
      <div className="flex items-center gap-1.5 px-3 py-2 border-t border-[#0d9aff33]">
        <span className="text-[#0d9aff] text-[11px] shrink-0">&gt;_</span>
        <input
          ref={inputRef}
          type="text"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={isStreaming ? "processing..." : "type a message"}
          disabled={isStreaming}
          className="flex-1 text-[11px] bg-transparent disabled:opacity-50"
        />
      </div>
    </div>
  );
}
