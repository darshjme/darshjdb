import { useState, useEffect, useCallback } from "react";
import {
  Shield,
  RefreshCw,
  Loader2,
  Database,
  Activity,
  ShieldCheck,
  AlertCircle,
  Server,
} from "lucide-react";
import { Badge } from "../components/Badge";
import {
  fetchCacheStats,
  fetchHealthDetailed,
  fetchAuditChain,
  ApiError,
} from "../lib/api";
import type {
  CacheStats,
  HealthResponse,
  AuditChainResult,
} from "../lib/api";
import { cn } from "../lib/utils";

type SettingsTab = "system" | "operations";

export function Settings() {
  const [activeTab, setActiveTab] = useState<SettingsTab>("system");

  // Live system data
  const [cacheStats, setCacheStats] = useState<CacheStats | null>(null);
  const [healthData, setHealthData] = useState<HealthResponse | null>(null);
  const [auditChain, setAuditChain] = useState<AuditChainResult | null>(null);
  const [systemLoading, setSystemLoading] = useState(true);
  const [systemError, setSystemError] = useState<string | null>(null);

  const loadSystemData = useCallback(async () => {
    setSystemLoading(true);
    setSystemError(null);

    const results = await Promise.allSettled([
      fetchHealthDetailed(),
      fetchCacheStats(),
      fetchAuditChain(),
    ]);

    if (results[0].status === "fulfilled") {
      setHealthData(results[0].value);
    }
    if (results[1].status === "fulfilled") {
      setCacheStats(results[1].value);
    }
    if (results[2].status === "fulfilled") {
      setAuditChain(results[2].value);
    }

    if (results.some((r) => r.status === "rejected")) {
      const err = results.find(r => r.status === "rejected")?.reason;
      setSystemError(
        err instanceof ApiError
          ? `Cannot connect to DarshJDB server (${err.status}). Is the server running?`
          : err instanceof Error ? err.message : "Some system metrics are unavailable.",
      );
    }

    setSystemLoading(false);
  }, []);

  useEffect(() => {
    loadSystemData();
  }, [loadSystemData]);

  const tabs: { id: SettingsTab; label: string; icon: typeof Shield }[] = [
    { id: "system", label: "System Status", icon: Server },
    { id: "operations", label: "Operations", icon: Shield },
  ];

  function formatUptime(secs: number): string {
    const d = Math.floor(secs / 86400);
    const h = Math.floor((secs % 86400) / 3600);
    const m = Math.floor((secs % 3600) / 60);
    if (d > 0) return `${d}d ${h}h ${m}m`;
    if (h > 0) return `${h}h ${m}m`;
    return `${m}m ${secs % 60}s`;
  }

  return (
    <div className="p-6 max-w-4xl">
      <h2 className="text-lg font-semibold text-zinc-100 mb-1">Settings</h2>
      <p className="text-sm text-zinc-500 mb-6">
        Manage your DarshJDB deployment configuration
      </p>

      {/* Tabs */}
      <div className="flex items-center gap-1 mb-6 bg-zinc-900 rounded-lg p-0.5 border border-zinc-800 w-fit">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            onClick={() => setActiveTab(tab.id)}
            className={cn(
              "flex items-center gap-2 px-3 py-1.5 rounded-md text-xs font-medium transition-colors",
              activeTab === tab.id
                ? "bg-zinc-800 text-zinc-100"
                : "text-zinc-500 hover:text-zinc-300",
            )}
          >
            <tab.icon className="w-3.5 h-3.5" />
            {tab.label}
          </button>
        ))}
      </div>

      {/* System Status (new live tab) */}
      {activeTab === "system" && (
        <div className="space-y-6">
          <div className="flex items-center justify-between">
            <p className="text-sm text-zinc-400">
              Live system status from server health, cache, and audit endpoints.
            </p>
            <button
              onClick={loadSystemData}
              className="btn-ghost text-xs"
            >
              <RefreshCw className={cn("w-3.5 h-3.5", systemLoading && "animate-spin")} />
              Refresh
            </button>
          </div>

          {systemError && (
            <div className="glass-panel p-3 border-amber-500/30 flex items-center gap-2 text-xs text-amber-400">
              <AlertCircle className="w-3.5 h-3.5 flex-shrink-0" />
              <span>{systemError}</span>
            </div>
          )}

          {systemLoading && !healthData && (
            <div className="glass-panel p-8 flex items-center justify-center gap-2 text-sm text-zinc-500">
              <Loader2 className="w-4 h-4 animate-spin" />
              Loading system status...
            </div>
          )}

          {/* Health / Pool Stats */}
          {healthData && (
            <div className="glass-panel p-0 overflow-hidden">
              <div className="px-4 py-3 border-b border-zinc-800/60 flex items-center gap-2">
                <Activity className="w-4 h-4 text-emerald-400" />
                <h3 className="text-sm font-semibold text-zinc-100">Server Health</h3>
                <Badge
                  variant={healthData.status === "ok" ? "emerald" : "red"}
                  className="text-[9px] ml-auto"
                >
                  {healthData.status}
                </Badge>
              </div>
              <div className="grid grid-cols-2 gap-px bg-zinc-800/40">
                {[
                  { label: "Version", value: healthData.version },
                  { label: "Uptime", value: formatUptime(healthData.uptime_secs) },
                  { label: "Database", value: healthData.database },
                  { label: "Total Triples", value: healthData.triples.toLocaleString() },
                  { label: "WebSocket Connections", value: String(healthData.websockets.active_connections) },
                  { label: "Service", value: healthData.service },
                ].map((item) => (
                  <div key={item.label} className="bg-zinc-950 px-4 py-3">
                    <p className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
                      {item.label}
                    </p>
                    <p className="text-sm text-zinc-200 mt-0.5 font-mono">{item.value}</p>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Connection Pool */}
          {healthData && (
            <div className="glass-panel p-0 overflow-hidden">
              <div className="px-4 py-3 border-b border-zinc-800/60 flex items-center gap-2">
                <Database className="w-4 h-4 text-sky-400" />
                <h3 className="text-sm font-semibold text-zinc-100">Connection Pool</h3>
              </div>
              <div className="px-4 py-4">
                <div className="grid grid-cols-4 gap-4 mb-4">
                  {[
                    { label: "Size", value: healthData.pool.size, color: "text-zinc-200" },
                    { label: "Active", value: healthData.pool.active, color: "text-amber-400" },
                    { label: "Idle", value: healthData.pool.idle, color: "text-emerald-400" },
                    { label: "Max", value: healthData.pool.max, color: "text-zinc-500" },
                  ].map((item) => (
                    <div key={item.label} className="text-center">
                      <p className={cn("text-2xl font-bold font-mono", item.color)}>
                        {item.value}
                      </p>
                      <p className="text-[10px] text-zinc-600 uppercase tracking-wider mt-1">
                        {item.label}
                      </p>
                    </div>
                  ))}
                </div>
                {/* Pool utilization bar */}
                <div className="space-y-1">
                  <div className="flex items-center justify-between text-[10px] text-zinc-500">
                    <span>Pool utilization</span>
                    <span>
                      {healthData.pool.active} / {healthData.pool.max} (
                      {Math.round((healthData.pool.active / healthData.pool.max) * 100)}%)
                    </span>
                  </div>
                  <div className="w-full h-2 bg-zinc-800 rounded-full overflow-hidden">
                    <div
                      className={cn(
                        "h-full rounded-full transition-all",
                        healthData.pool.active / healthData.pool.max > 0.8
                          ? "bg-red-500"
                          : healthData.pool.active / healthData.pool.max > 0.5
                            ? "bg-amber-500"
                            : "bg-emerald-500",
                      )}
                      style={{
                        width: `${(healthData.pool.active / healthData.pool.max) * 100}%`,
                      }}
                    />
                  </div>
                </div>

                {/* Extended pool stats if available */}
                {healthData.pool_stats && Object.keys(healthData.pool_stats).length > 0 && (
                  <div className="mt-4 pt-4 border-t border-zinc-800/60">
                    <p className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600 mb-2">
                      Pool Metrics
                    </p>
                    <div className="grid grid-cols-2 gap-2">
                      {Object.entries(healthData.pool_stats).map(([key, value]) => (
                        <div key={key} className="flex items-center justify-between text-xs">
                          <span className="text-zinc-500 font-mono">{key}</span>
                          <span className="text-zinc-300 font-mono">
                            {typeof value === "number"
                              ? Number.isInteger(value)
                                ? value.toLocaleString()
                                : (value as number).toFixed(2)
                              : typeof value === "object" ? JSON.stringify(value) : String(value)}
                          </span>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </div>
          )}

          {/* Cache Stats */}
          {cacheStats && (
            <div className="glass-panel p-0 overflow-hidden">
              <div className="px-4 py-3 border-b border-zinc-800/60 flex items-center gap-2">
                <RefreshCw className="w-4 h-4 text-amber-400" />
                <h3 className="text-sm font-semibold text-zinc-100">Query Cache</h3>
              </div>
              <div className="px-4 py-4">
                {typeof cacheStats.cache === "object" &&
                cacheStats.cache !== null &&
                Object.keys(cacheStats.cache).length > 0 ? (
                  <div className="grid grid-cols-2 gap-3">
                    {Object.entries(cacheStats.cache).map(([key, value]) => (
                      <div key={key} className="flex items-center justify-between text-xs">
                        <span className="text-zinc-500 font-mono">{key}</span>
                        <span className="text-zinc-200 font-mono">
                          {typeof value === "number"
                            ? Number.isInteger(value)
                              ? value.toLocaleString()
                              : (value as number).toFixed(2)
                            : typeof value === "object" ? JSON.stringify(value) : String(value)}
                        </span>
                      </div>
                    ))}
                  </div>
                ) : (
                  <p className="text-xs text-zinc-500 italic">
                    Cache is empty or no stats available yet.
                  </p>
                )}
              </div>
            </div>
          )}

          {/* Audit Chain */}
          {auditChain && (
            <div className="glass-panel p-0 overflow-hidden">
              <div className="px-4 py-3 border-b border-zinc-800/60 flex items-center gap-2">
                <ShieldCheck className="w-4 h-4 text-emerald-400" />
                <h3 className="text-sm font-semibold text-zinc-100">Audit Chain</h3>
                <Badge
                  variant={auditChain.valid ? "emerald" : "red"}
                  className="text-[9px] ml-auto"
                >
                  {auditChain.valid ? "verified" : "broken"}
                </Badge>
              </div>
              <div className="px-4 py-4 space-y-3">
                <div className="grid grid-cols-2 gap-4">
                  <div>
                    <p className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
                      Chain Status
                    </p>
                    <p className={cn(
                      "text-sm font-semibold mt-0.5",
                      auditChain.valid ? "text-emerald-400" : "text-red-400",
                    )}>
                      {auditChain.valid ? "All hashes valid" : "Chain integrity broken"}
                    </p>
                  </div>
                  <div>
                    <p className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
                      Total Transactions
                    </p>
                    <p className="text-sm text-zinc-200 font-mono mt-0.5">
                      {auditChain.total_transactions.toLocaleString()}
                    </p>
                  </div>
                </div>
                {auditChain.first_broken_tx !== null && (
                  <div className="glass-panel p-3 border-red-500/20">
                    <p className="text-xs text-red-400">
                      First broken transaction: #{auditChain.first_broken_tx}
                    </p>
                  </div>
                )}
                {auditChain.detail && (
                  <p className="text-xs text-zinc-500">{auditChain.detail}</p>
                )}
              </div>
            </div>
          )}

          {/* Show placeholder when nothing loaded yet and no error */}
          {!systemLoading && !healthData && !cacheStats && !auditChain && !systemError && (
            <div className="glass-panel p-8 text-center text-sm text-zinc-500">
              No system data available. Is the server running?
            </div>
          )}
        </div>
      )}

      {activeTab === "operations" && <div className="space-y-5">
        <section className="glass-panel p-6"><h3 className="font-medium text-zinc-100 mb-2">Backups and restore</h3><p className="text-sm text-zinc-400">Use PostgreSQL backup tooling on the server and back up the storage volume separately. This console does not create or restore backups. No backup history is reported by the server.</p></section>
        <section className="glass-panel p-6"><h3 className="font-medium text-zinc-100 mb-2">Server configuration</h3><p className="text-sm text-zinc-400">Manage environment variables and rate limits in your deployment configuration. Webhook management is available through the authenticated /api/webhooks API; this console does not yet provide a webhook editor.</p></section>
      </div>}
    </div>
  );
}
