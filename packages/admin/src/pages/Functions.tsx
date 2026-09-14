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
import { Badge } from "../components/Badge";
import { fetchFunctions, ApiError } from "../lib/api";
import type { ServerFunctionInfo } from "../lib/api";
import { cn } from "../lib/utils";
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
  const [functions, setFunctions] = useState<FunctionDef[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadFunctions = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await fetchFunctions();
      setFunctions(res.functions.map(serverToFunctionDef));
    } catch (err) {
      setFunctions([]);
      if (err instanceof ApiError) {
        setError(`Cannot connect to DarshJDB server (${err.status}). Is the server running?`);
      } else {
        setError("Cannot connect to DarshJDB server. Is the server running?");
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
        {/* Header */}
        <div className="p-6 border-b border-line">
          <div className="flex items-center justify-between mb-4">
            <div>
              <h2 className="text-lg font-semibold text-ink">Functions</h2>
              <p className="text-sm text-ink-muted mt-0.5 flex items-center gap-2">
                {loading ? (
                  <span className="flex items-center gap-1.5">
                    <Loader2 className="w-3 h-3 animate-spin" />
                    Loading...
                  </span>
                ) : (
                  <>{functions.length} registered functions</>
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
            </div>
          </div>

          {error && (
            <div className="glass-panel p-3 mb-4 border-red-500/30 flex items-center gap-2 text-xs text-red-700">
              <AlertCircle className="w-3.5 h-3.5 flex-shrink-0" />
              <span>{error}</span>
            </div>
          )}
        </div>

        {/* Function list */}
        <div className="p-6">
          {loading && (
            <div className="flex items-center justify-center gap-2 py-16 text-sm text-ink-muted">
              <Loader2 className="w-4 h-4 animate-spin" />
              Loading functions...
            </div>
          )}

          {!loading && !error && functions.length === 0 && (
            <div className="flex items-center justify-center py-16 text-sm text-ink-muted">
              No functions registered on the server yet.
            </div>
          )}

          {!loading && functions.length > 0 && (
            <>
              <div className="flex items-center gap-3 mb-4">
                <div className="relative flex-1 max-w-sm">
                  <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-ink-muted" />
                  <input
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    placeholder="Search functions..."
                    className="input-field pl-9 text-xs"
                  />
                </div>
                <div className="flex items-center gap-1 bg-surface-subtle rounded-lg p-0.5 border border-line">
                  {["all", "query", "mutation", "action", "cron"].map((t) => (
                    <button
                      key={t}
                      onClick={() => setTypeFilter(t)}
                      className={cn(
                        "px-2.5 py-1 rounded-md text-xs font-medium transition-colors capitalize",
                        typeFilter === t
                          ? "bg-surface-muted text-ink"
                          : "text-ink-muted hover:text-ink-secondary",
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
                      "w-full glass-panel p-0 text-left transition-all hover:border-line-strong",
                      selectedFn?.name === fn.name && "border-brand-500/40",
                    )}
                  >
                    <div className="flex items-center justify-between px-4 py-3">
                      <div className="flex items-center gap-3">
                        <Zap className={cn(
                          "w-4 h-4",
                          fn.type === "query" ? "text-sky-700" :
                          fn.type === "mutation" ? "text-brand-400" :
                          fn.type === "action" ? "text-purple-700" : "text-emerald-700",
                        )} />
                        <span className="font-mono text-sm text-ink">{fn.name}</span>
                        <Badge variant={typeBadgeVariant[fn.type]} className="text-[10px]">
                          {fn.type}
                        </Badge>
                      </div>
                      <div className="flex items-center gap-4 text-xs text-ink-muted">
                        {fn.avgDuration !== undefined && (
                          <span className="flex items-center gap-1">
                            <Clock className="w-3 h-3" />
                            {fn.avgDuration}ms
                          </span>
                        )}
                        {fn.errorRate !== undefined && fn.errorRate > 0 && (
                          <span className={cn(
                            "flex items-center gap-1",
                            fn.errorRate > 1 ? "text-red-700" : "text-ink-muted",
                          )}>
                            <AlertTriangle className="w-3 h-3" />
                            {fn.errorRate}%
                          </span>
                        )}
                      </div>
                    </div>

                    {selectedFn?.name === fn.name && (
                      <div className="px-4 pb-4 border-t border-line/60 pt-3">
                        <div className="grid grid-cols-2 gap-4">
                          <div>
                            <h4 className="text-xs font-semibold text-ink-muted mb-2">Arguments</h4>
                            <div className="space-y-1">
                              {Object.entries(fn.args).map(([name, type]) => (
                                <div key={name} className="flex items-center gap-2 text-xs">
                                  <span className="font-mono text-ink-secondary">{name}</span>
                                  <span className="text-ink-muted">:</span>
                                  <span className="font-mono text-sky-700">{type}</span>
                                </div>
                              ))}
                              {Object.keys(fn.args).length === 0 && (
                                <span className="text-xs text-ink-muted italic">No arguments</span>
                              )}
                            </div>
                          </div>
                          <div>
                            <h4 className="text-xs font-semibold text-ink-muted mb-2">Returns</h4>
                            <span className="font-mono text-xs text-emerald-700">{fn.returns}</span>
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
            </>
          )}
        </div>
      </div>

      {/* Execution history sidebar -- requires a real API endpoint */}
      <div className="w-80 flex-shrink-0 border-l border-line bg-surface/50 overflow-y-auto">
        <div className="px-4 py-3 border-b border-line">
          <h3 className="text-sm font-semibold text-ink">Recent Executions</h3>
        </div>
        <div className="flex items-center justify-center py-12 text-xs text-ink-muted italic">
          {error
            ? "Server unreachable"
            : "No execution history available yet."}
        </div>
      </div>
    </div>
  );
}
