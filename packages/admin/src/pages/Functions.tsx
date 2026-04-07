import { useState, useEffect, useCallback } from "react";
import {
  Search,
  Zap,
  GitCommit,
  Clock,
  AlertTriangle,
  Play,
  Loader2,
  AlertCircle,
  RefreshCw,
} from "lucide-react";
import {
  AreaChart,
  Area,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import { Badge } from "../components/Badge";
import { mockFunctions, mockExecutions, mockExecutionHistory } from "../lib/mock-data";
import { fetchFunctions, ApiError } from "../lib/api";
import type { ServerFunctionInfo } from "../lib/api";
import { cn, formatRelativeTime } from "../lib/utils";
import type { FunctionDef } from "../types";

const typeBadgeVariant: Record<FunctionDef["type"], "amber" | "emerald" | "purple" | "sky"> = {
  query: "sky",
  mutation: "amber",
  action: "purple",
  cron: "emerald",
};

/** Map server FunctionKind (e.g. "Query", "Scheduled") to UI type. */
function kindToType(kind: string): FunctionDef["type"] {
  const k = kind.toLowerCase();
  if (k === "query") return "query";
  if (k === "mutation") return "mutation";
  if (k === "action") return "action";
  if (k === "scheduled") return "cron";
  return "query";
}

/** Convert a server function info into the UI's FunctionDef shape. */
function serverToFunctionDef(fn: ServerFunctionInfo): FunctionDef {
  const module = fn.file_path
    ? fn.file_path.replace(/\.[^.]+$/, "")
    : fn.name.split(":")[0] ?? "default";

  // Convert args_schema into a simple name→type map for display.
  const args: Record<string, string> = {};
  if (fn.args_schema && typeof fn.args_schema === "object") {
    const schema = fn.args_schema as Record<string, unknown>;
    const props = (schema.properties ?? schema.fields ?? schema) as Record<string, unknown>;
    if (props && typeof props === "object") {
      for (const [key, val] of Object.entries(props)) {
        if (typeof val === "object" && val !== null && "type" in val) {
          args[key] = String((val as Record<string, unknown>).type);
        } else {
          args[key] = String(val);
        }
      }
    }
  }

  return {
    name: fn.name,
    type: kindToType(fn.kind),
    module,
    args,
    returns: fn.description ?? "unknown",
    avgDuration: undefined,
    errorRate: undefined,
  };
}

export function Functions() {
  const [search, setSearch] = useState("");
  const [typeFilter, setTypeFilter] = useState<string>("all");
  const [selectedFn, setSelectedFn] = useState<FunctionDef | null>(null);

  // Live data state
  const [functions, setFunctions] = useState<FunctionDef[]>(mockFunctions);
  const [loading, setLoading] = useState(true);
  const [isLive, setIsLive] = useState(false);
  const [apiMessage, setApiMessage] = useState<string | null>(null);

  const loadFunctions = useCallback(async () => {
    setLoading(true);
    setApiMessage(null);
    try {
      const res = await fetchFunctions();
      if (res.functions.length === 0) {
        // Server endpoint exists but returns empty (TODO on server side)
        setFunctions(mockFunctions);
        setIsLive(false);
        setApiMessage(
          "Functions endpoint connected but no functions registered on server yet. Showing demo data.",
        );
      } else {
        setFunctions(res.functions.map(serverToFunctionDef));
        setIsLive(true);
      }
    } catch (err) {
      console.warn("[Functions] API unavailable, using mock data:", err);
      setFunctions(mockFunctions);
      setIsLive(false);
      if (err instanceof ApiError) {
        setApiMessage(`API ${err.status}: ${err.body} -- showing demo data`);
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadFunctions();
  }, [loadFunctions]);

  const filtered = functions.filter((fn) => {
    if (typeFilter !== "all" && fn.type !== typeFilter) return false;
    if (search && !fn.name.toLowerCase().includes(search.toLowerCase())) return false;
    return true;
  });

  return (
    <div className="flex h-full">
      <div className="flex-1 overflow-auto">
        {/* Execution chart */}
        <div className="p-6 border-b border-zinc-800">
          <div className="flex items-center justify-between mb-4">
            <div>
              <h2 className="text-lg font-semibold text-zinc-100">Functions</h2>
              <p className="text-sm text-zinc-500 mt-0.5 flex items-center gap-2">
                {loading ? (
                  <span className="flex items-center gap-1.5">
                    <Loader2 className="w-3 h-3 animate-spin" />
                    Loading...
                  </span>
                ) : (
                  <>
                    {functions.length} registered functions
                    {isLive ? (
                      <Badge variant="emerald" className="text-[9px]">live</Badge>
                    ) : (
                      <Badge variant="zinc" className="text-[9px]">demo</Badge>
                    )}
                  </>
                )}
              </p>
            </div>
            <div className="flex items-center gap-3">
              <button
                onClick={loadFunctions}
                className="btn-ghost text-xs"
                title="Refresh"
              >
                <RefreshCw className={cn("w-3.5 h-3.5", loading && "animate-spin")} />
              </button>
              <div className="flex items-center gap-4 text-xs text-zinc-500">
                <div className="flex items-center gap-1.5">
                  <div className="w-2.5 h-2.5 rounded-full bg-sky-500" />
                  Queries
                </div>
                <div className="flex items-center gap-1.5">
                  <div className="w-2.5 h-2.5 rounded-full bg-amber-500" />
                  Mutations
                </div>
                <div className="flex items-center gap-1.5">
                  <div className="w-2.5 h-2.5 rounded-full bg-red-500" />
                  Errors
                </div>
              </div>
            </div>
          </div>

          {apiMessage && (
            <div className="glass-panel p-3 mb-4 border-amber-500/30 flex items-center gap-2 text-xs text-amber-400">
              <AlertCircle className="w-3.5 h-3.5 flex-shrink-0" />
              <span>{apiMessage}</span>
            </div>
          )}

          <div className="h-48 glass-panel p-4">
            <ResponsiveContainer width="100%" height="100%">
              <AreaChart data={mockExecutionHistory}>
                <defs>
                  <linearGradient id="queryGradient" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="5%" stopColor="#0EA5E9" stopOpacity={0.2} />
                    <stop offset="95%" stopColor="#0EA5E9" stopOpacity={0} />
                  </linearGradient>
                  <linearGradient id="mutationGradient" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="5%" stopColor="#F59E0B" stopOpacity={0.2} />
                    <stop offset="95%" stopColor="#F59E0B" stopOpacity={0} />
                  </linearGradient>
                </defs>
                <CartesianGrid strokeDasharray="3 3" stroke="#27272a" />
                <XAxis dataKey="hour" tick={{ fill: "#71717a", fontSize: 10 }} tickLine={false} axisLine={false} />
                <YAxis tick={{ fill: "#71717a", fontSize: 10 }} tickLine={false} axisLine={false} />
                <Tooltip
                  contentStyle={{
                    backgroundColor: "#18181b",
                    border: "1px solid #27272a",
                    borderRadius: "8px",
                    fontSize: "12px",
                  }}
                />
                <Area type="monotone" dataKey="queries" stroke="#0EA5E9" fill="url(#queryGradient)" strokeWidth={1.5} />
                <Area type="monotone" dataKey="mutations" stroke="#F59E0B" fill="url(#mutationGradient)" strokeWidth={1.5} />
                <Area type="monotone" dataKey="errors" stroke="#EF4444" fill="transparent" strokeWidth={1} strokeDasharray="4 4" />
              </AreaChart>
            </ResponsiveContainer>
          </div>
        </div>

        {/* Function list */}
        <div className="p-6">
          <div className="flex items-center gap-3 mb-4">
            <div className="relative flex-1 max-w-sm">
              <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-zinc-500" />
              <input
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Search functions..."
                className="input-field pl-9 text-xs"
              />
            </div>
            <div className="flex items-center gap-1 bg-zinc-900 rounded-lg p-0.5 border border-zinc-800">
              {["all", "query", "mutation", "action", "cron"].map((t) => (
                <button
                  key={t}
                  onClick={() => setTypeFilter(t)}
                  className={cn(
                    "px-2.5 py-1 rounded-md text-xs font-medium transition-colors capitalize",
                    typeFilter === t
                      ? "bg-zinc-800 text-zinc-100"
                      : "text-zinc-500 hover:text-zinc-300",
                  )}
                >
                  {t}
                </button>
              ))}
            </div>
          </div>

          <div className="space-y-2">
            {filtered.map((fn) => (
              <button
                key={fn.name}
                onClick={() => setSelectedFn(selectedFn?.name === fn.name ? null : fn)}
                className={cn(
                  "w-full glass-panel p-0 text-left transition-all hover:border-zinc-700",
                  selectedFn?.name === fn.name && "border-amber-500/40",
                )}
              >
                <div className="flex items-center justify-between px-4 py-3">
                  <div className="flex items-center gap-3">
                    <Zap className={cn(
                      "w-4 h-4",
                      fn.type === "query" ? "text-sky-400" :
                      fn.type === "mutation" ? "text-amber-400" :
                      fn.type === "action" ? "text-purple-400" : "text-emerald-400",
                    )} />
                    <span className="font-mono text-sm text-zinc-200">{fn.name}</span>
                    <Badge variant={typeBadgeVariant[fn.type]} className="text-[10px]">
                      {fn.type}
                    </Badge>
                  </div>
                  <div className="flex items-center gap-4 text-xs text-zinc-500">
                    {fn.avgDuration !== undefined && (
                      <span className="flex items-center gap-1">
                        <Clock className="w-3 h-3" />
                        {fn.avgDuration}ms
                      </span>
                    )}
                    {fn.errorRate !== undefined && fn.errorRate > 0 && (
                      <span className={cn(
                        "flex items-center gap-1",
                        fn.errorRate > 1 ? "text-red-400" : "text-zinc-500",
                      )}>
                        <AlertTriangle className="w-3 h-3" />
                        {fn.errorRate}%
                      </span>
                    )}
                  </div>
                </div>

                {selectedFn?.name === fn.name && (
                  <div className="px-4 pb-4 border-t border-zinc-800/60 pt-3">
                    <div className="grid grid-cols-2 gap-4">
                      <div>
                        <h4 className="text-xs font-semibold text-zinc-500 mb-2">Arguments</h4>
                        <div className="space-y-1">
                          {Object.entries(fn.args).map(([name, type]) => (
                            <div key={name} className="flex items-center gap-2 text-xs">
                              <span className="font-mono text-zinc-300">{name}</span>
                              <span className="text-zinc-600">:</span>
                              <span className="font-mono text-sky-400">{type}</span>
                            </div>
                          ))}
                          {Object.keys(fn.args).length === 0 && (
                            <span className="text-xs text-zinc-600 italic">No arguments</span>
                          )}
                        </div>
                      </div>
                      <div>
                        <h4 className="text-xs font-semibold text-zinc-500 mb-2">Returns</h4>
                        <span className="font-mono text-xs text-emerald-400">{fn.returns}</span>
                      </div>
                    </div>
                    <div className="mt-3 flex gap-2">
                      <button className="btn-primary text-xs py-1.5">
                        <Play className="w-3 h-3" />
                        Execute
                      </button>
                      <button className="btn-secondary text-xs py-1.5">
                        <GitCommit className="w-3 h-3" />
                        View History
                      </button>
                    </div>
                  </div>
                )}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Execution history sidebar */}
      <div className="w-80 flex-shrink-0 border-l border-zinc-800 bg-zinc-950/50 overflow-y-auto">
        <div className="px-4 py-3 border-b border-zinc-800">
          <h3 className="text-sm font-semibold text-zinc-100">Recent Executions</h3>
        </div>
        <div className="divide-y divide-zinc-800/60">
          {mockExecutions.slice(0, 15).map((exec) => (
            <div key={exec.id} className="px-4 py-2.5 hover:bg-zinc-800/30 transition-colors">
              <div className="flex items-center justify-between">
                <span className="font-mono text-xs text-zinc-300 truncate max-w-[180px]">
                  {exec.functionName}
                </span>
                <Badge
                  variant={
                    exec.status === "success" ? "emerald" :
                    exec.status === "error" ? "red" : "amber"
                  }
                  className="text-[9px]"
                >
                  {exec.status}
                </Badge>
              </div>
              <div className="flex items-center gap-3 mt-1 text-[10px] text-zinc-600">
                <span>{exec.duration}ms</span>
                <span>{formatRelativeTime(exec.timestamp)}</span>
              </div>
              {exec.error && (
                <p className="mt-1 text-[10px] text-red-400 truncate">{exec.error}</p>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
