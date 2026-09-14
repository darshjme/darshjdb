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
      <h2 className="text-lg font-semibold text-ink mb-1">Settings</h2>
      <p className="text-sm text-ink-muted mb-6">
        Manage your DarshJDB deployment configuration
      </p>

      {/* Tabs */}
      <div className="flex items-center gap-1 mb-6 bg-surface-subtle rounded-lg p-0.5 border border-line w-fit">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            onClick={() => setActiveTab(tab.id)}
            className={cn(
              "flex items-center gap-2 px-3 py-1.5 rounded-md text-xs font-medium transition-colors",
              activeTab === tab.id
                ? "bg-surface-muted text-ink"
                : "text-ink-muted hover:text-ink-secondary",
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
            <p className="text-sm text-ink-secondary">
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
            <div className="glass-panel p-3 border-brand-500/30 flex items-center gap-2 text-xs text-brand-400">
              <AlertCircle className="w-3.5 h-3.5 flex-shrink-0" />
              <span>{systemError}</span>
            </div>
          )}

          {systemLoading && !healthData && (
            <div className="glass-panel p-8 flex items-center justify-center gap-2 text-sm text-ink-muted">
              <Loader2 className="w-4 h-4 animate-spin" />
              Loading system status...
            </div>
          )}

          {/* Health / Pool Stats */}
          {healthData && (
            <div className="glass-panel p-0 overflow-hidden">
              <div className="px-4 py-3 border-b border-line/60 flex items-center gap-2">
                <Activity className="w-4 h-4 text-emerald-700" />
                <h3 className="text-sm font-semibold text-ink">Server Health</h3>
                <Badge
                  variant={healthData.status === "ok" ? "emerald" : "red"}
                  className="text-[9px] ml-auto"
                >
                  {healthData.status}
                </Badge>
              </div>
              <div className="grid grid-cols-2 gap-px bg-surface-muted/40">
                {[
                  { label: "Version", value: healthData.version },
                  { label: "Uptime", value: formatUptime(healthData.uptime_secs) },
                  { label: "Database", value: healthData.database },
                  { label: "Total Triples", value: healthData.triples.toLocaleString() },
                  { label: "WebSocket Connections", value: String(healthData.websockets.active_connections) },
                  { label: "Service", value: healthData.service },
                ].map((item) => (
                  <div key={item.label} className="bg-surface px-4 py-3">
                    <p className="text-[10px] font-semibold uppercase tracking-wider text-ink-muted">
                      {item.label}
                    </p>
                    <p className="text-sm text-ink mt-0.5 font-mono">{item.value}</p>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Connection Pool */}
          {healthData && (
            <div className="glass-panel p-0 overflow-hidden">
              <div className="px-4 py-3 border-b border-line/60 flex items-center gap-2">
                <Database className="w-4 h-4 text-sky-700" />
                <h3 className="text-sm font-semibold text-ink">Connection Pool</h3>
              </div>
              <div className="px-4 py-4">
                <div className="grid grid-cols-4 gap-4 mb-4">
                  {[
                    { label: "Size", value: healthData.pool.size, color: "text-ink" },
                    { label: "Active", value: healthData.pool.active, color: "text-brand-400" },
                    { label: "Idle", value: healthData.pool.idle, color: "text-emerald-700" },
                    { label: "Max", value: healthData.pool.max, color: "text-ink-muted" },
                  ].map((item) => (
                    <div key={item.label} className="text-center">
                      <p className={cn("text-2xl font-bold font-mono", item.color)}>
                        {item.value}
                      </p>
                      <p className="text-[10px] text-ink-muted uppercase tracking-wider mt-1">
                        {item.label}
                      </p>
                    </div>
                  ))}
                </div>
                {/* Pool utilization bar */}
                <div className="space-y-1">
                  <div className="flex items-center justify-between text-[10px] text-ink-muted">
                    <span>Pool utilization</span>
                    <span>
                      {healthData.pool.active} / {healthData.pool.max} (
                      {Math.round((healthData.pool.active / healthData.pool.max) * 100)}%)
                    </span>
                  </div>
                  <div className="w-full h-2 bg-surface-muted rounded-full overflow-hidden">
                    <div
                      className={cn(
                        "h-full rounded-full transition-all",
                        healthData.pool.active / healthData.pool.max > 0.8
                          ? "bg-red-500"
                          : healthData.pool.active / healthData.pool.max > 0.5
                            ? "bg-brand-500"
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
                  <div className="mt-4 pt-4 border-t border-line/60">
                    <p className="text-[10px] font-semibold uppercase tracking-wider text-ink-muted mb-2">
                      Pool Metrics
                    </p>
                    <div className="grid grid-cols-2 gap-2">
                      {Object.entries(healthData.pool_stats).map(([key, value]) => (
                        <div key={key} className="flex items-center justify-between text-xs">
                          <span className="text-ink-muted font-mono">{key}</span>
                          <span className="text-ink-secondary font-mono">
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
              <div className="px-4 py-3 border-b border-line/60 flex items-center gap-2">
                <RefreshCw className="w-4 h-4 text-brand-400" />
                <h3 className="text-sm font-semibold text-ink">Query Cache</h3>
              </div>
              <div className="px-4 py-4">
                {typeof cacheStats.cache === "object" &&
                cacheStats.cache !== null &&
                Object.keys(cacheStats.cache).length > 0 ? (
                  <div className="grid grid-cols-2 gap-3">
                    {Object.entries(cacheStats.cache).map(([key, value]) => (
                      <div key={key} className="flex items-center justify-between text-xs">
                        <span className="text-ink-muted font-mono">{key}</span>
                        <span className="text-ink font-mono">
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
                  <p className="text-xs text-ink-muted italic">
                    Cache is empty or no stats available yet.
                  </p>
                )}
              </div>
            </div>
          )}

          {/* Audit Chain */}
          {auditChain && (
            <div className="glass-panel p-0 overflow-hidden">
              <div className="px-4 py-3 border-b border-line/60 flex items-center gap-2">
                <ShieldCheck className="w-4 h-4 text-emerald-700" />
                <h3 className="text-sm font-semibold text-ink">Audit Chain</h3>
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
                    <p className="text-[10px] font-semibold uppercase tracking-wider text-ink-muted">
                      Chain Status
                    </p>
                    <p className={cn(
                      "text-sm font-semibold mt-0.5",
                      auditChain.valid ? "text-emerald-700" : "text-red-700",
                    )}>
                      {auditChain.valid ? "All hashes valid" : "Chain integrity broken"}
                    </p>
                  </div>
                  <div>
                    <p className="text-[10px] font-semibold uppercase tracking-wider text-ink-muted">
                      Total Transactions
                    </p>
                    <p className="text-sm text-ink font-mono mt-0.5">
                      {auditChain.total_transactions.toLocaleString()}
                    </p>
                  </div>
                </div>
                {auditChain.first_broken_tx !== null && (
                  <div className="glass-panel p-3 border-red-500/20">
                    <p className="text-xs text-red-700">
                      First broken transaction: #{auditChain.first_broken_tx}
                    </p>
                  </div>
                )}
                {auditChain.detail && (
                  <p className="text-xs text-ink-muted">{auditChain.detail}</p>
                )}
              </div>
            </div>
          )}

          {/* Show placeholder when nothing loaded yet and no error */}
          {!systemLoading && !healthData && !cacheStats && !auditChain && !systemError && (
            <div className="glass-panel p-8 text-center text-sm text-ink-muted">
              No system data available. Is the server running?
            </div>
          )}
        </div>
      )}

      {activeTab === "operations" && <div className="space-y-5">
        <section className="glass-panel p-6"><h3 className="font-medium text-ink mb-2">Backups and restore</h3><p className="text-sm text-ink-secondary">Use PostgreSQL backup tooling on the server and back up the storage volume separately. This console does not create or restore backups. No backup history is reported by the server.</p></section>
        <section className="glass-panel p-6"><h3 className="font-medium text-ink mb-2">Server configuration</h3><p className="text-sm text-ink-secondary">Manage environment variables and rate limits in your deployment configuration. Webhook management is available through the authenticated /api/webhooks API; this console does not yet provide a webhook editor.</p></section>
      </div>}
    </div>
  );
}
