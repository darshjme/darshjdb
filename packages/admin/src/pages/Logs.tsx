import { useState, useRef, useEffect } from "react";
import {
  Search,
  Radio,
  ChevronDown,
  ChevronRight,
  AlertCircle,
  AlertTriangle,
  Info,
  Bug,
  Download,
  Trash2,
} from "lucide-react";
import { Badge } from "../components/Badge";
import { mockLogs } from "../lib/mock-data";
import { cn, formatTimestamp } from "../lib/utils";
import type { LogEntry } from "../types";

const levelConfig: Record<
  LogEntry["level"],
  { icon: typeof Info; color: string; badge: "red" | "amber" | "sky" | "zinc" }
> = {
  error: { icon: AlertCircle, color: "text-red-400", badge: "red" },
  warn: { icon: AlertTriangle, color: "text-amber-400", badge: "amber" },
  info: { icon: Info, color: "text-sky-400", badge: "sky" },
  debug: { icon: Bug, color: "text-zinc-500", badge: "zinc" },
};

export function Logs() {
  const [search, setSearch] = useState("");
  const [levelFilter, setLevelFilter] = useState<string>("all");
  const [liveMode, setLiveMode] = useState(false);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const logsEndRef = useRef<HTMLDivElement>(null);

  const filtered = mockLogs.filter((log) => {
    if (levelFilter !== "all" && log.level !== levelFilter) return false;
    if (search) {
      const q = search.toLowerCase();
      return (
        log.message.toLowerCase().includes(q) ||
        log.function?.toLowerCase().includes(q) ||
        log.userId?.toLowerCase().includes(q)
      );
    }
    return true;
  });

  const toggleExpand = (id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  useEffect(() => {
    if (liveMode) {
      logsEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [liveMode, filtered.length]);

  const levelCounts = mockLogs.reduce(
    (acc, log) => {
      acc[log.level] = (acc[log.level] || 0) + 1;
      return acc;
    },
    {} as Record<string, number>,
  );

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="px-6 py-4 border-b border-zinc-800">
        <div className="flex items-center justify-between mb-4">
          <div>
            <h2 className="text-lg font-semibold text-zinc-100">Logs</h2>
            <p className="text-sm text-zinc-500 mt-0.5">
              {filtered.length} entries
            </p>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={() => setLiveMode(!liveMode)}
              className={cn(
                "btn-ghost text-xs gap-1.5",
                liveMode && "text-emerald-400",
              )}
            >
              <Radio className={cn("w-3.5 h-3.5", liveMode && "animate-pulse-slow")} />
              {liveMode ? "Live" : "Paused"}
            </button>
            <button className="btn-ghost text-xs">
              <Download className="w-3.5 h-3.5" />
              Export
            </button>
            <button className="btn-ghost text-xs text-red-400 hover:text-red-300">
              <Trash2 className="w-3.5 h-3.5" />
              Clear
            </button>
          </div>
        </div>

        {/* Filters */}
        <div className="flex items-center gap-3">
          <div className="relative flex-1 max-w-sm">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-zinc-500" />
            <input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search logs..."
              className="input-field pl-9 text-xs"
            />
          </div>
          <div className="flex items-center gap-1 bg-zinc-900 rounded-lg p-0.5 border border-zinc-800">
            {["all", "error", "warn", "info", "debug"].map((level) => (
              <button
                key={level}
                onClick={() => setLevelFilter(level)}
                className={cn(
                  "px-2.5 py-1 rounded-md text-xs font-medium transition-colors capitalize flex items-center gap-1.5",
                  levelFilter === level
                    ? "bg-zinc-800 text-zinc-100"
                    : "text-zinc-500 hover:text-zinc-300",
                )}
              >
                {level}
                {level !== "all" && (
                  <span className="text-[10px] text-zinc-600">
                    {levelCounts[level] || 0}
                  </span>
                )}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Log entries */}
      <div className="flex-1 overflow-y-auto font-mono text-xs">
        {filtered.map((log) => {
          const config = levelConfig[log.level];
          const isExpanded = expanded.has(log.id);
          return (
            <div
              key={log.id}
              className={cn(
                "border-b border-zinc-800/40 hover:bg-zinc-900/50 transition-colors",
                log.level === "error" && "bg-red-500/[0.02]",
              )}
            >
              <button
                onClick={() => log.data && toggleExpand(log.id)}
                className="flex items-start gap-3 w-full px-6 py-2 text-left"
              >
                <span className="text-[10px] text-zinc-600 mt-0.5 w-32 flex-shrink-0">
                  {formatTimestamp(log.timestamp)}
                </span>
                <config.icon className={cn("w-3.5 h-3.5 mt-0.5 flex-shrink-0", config.color)} />
                <Badge variant={config.badge} className="text-[9px] w-12 justify-center flex-shrink-0">
                  {log.level}
                </Badge>
                <span className="text-zinc-300 flex-1">{log.message}</span>
                {log.function && (
                  <span className="text-zinc-600 flex-shrink-0">{log.function}</span>
                )}
                {log.data && (
                  isExpanded ? (
                    <ChevronDown className="w-3 h-3 text-zinc-600 flex-shrink-0 mt-0.5" />
                  ) : (
                    <ChevronRight className="w-3 h-3 text-zinc-600 flex-shrink-0 mt-0.5" />
                  )
                )}
              </button>
              {isExpanded && log.data && (
                <div className="px-6 pb-3 ml-[11.5rem]">
                  <pre className="bg-zinc-900/80 border border-zinc-800 rounded-lg p-3 text-[11px] text-zinc-400 overflow-x-auto">
                    {JSON.stringify(log.data, null, 2)}
                  </pre>
                </div>
              )}
            </div>
          );
        })}
        <div ref={logsEndRef} />
      </div>
    </div>
  );
}
