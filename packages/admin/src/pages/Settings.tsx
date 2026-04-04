import { useState } from "react";
import {
  Eye,
  EyeOff,
  Plus,
  Trash2,
  Save,
  Download,
  Upload,
  Shield,
  Bell,
  Globe,
  Clock,
  Copy,
  Check,
  RefreshCw,
} from "lucide-react";
import { Badge } from "../components/Badge";
import { mockEnvVars } from "../lib/mock-data";
import { cn, formatRelativeTime } from "../lib/utils";

type SettingsTab = "env" | "backup" | "rate-limits" | "webhooks";

export function Settings() {
  const [activeTab, setActiveTab] = useState<SettingsTab>("env");
  const [showSecrets, setShowSecrets] = useState<Set<string>>(new Set());
  const [copied, setCopied] = useState<string | null>(null);

  const toggleSecret = (key: string) => {
    setShowSecrets((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const copyValue = (key: string, value: string) => {
    navigator.clipboard.writeText(value);
    setCopied(key);
    setTimeout(() => setCopied(null), 2000);
  };

  const tabs: { id: SettingsTab; label: string; icon: typeof Shield }[] = [
    { id: "env", label: "Environment Variables", icon: Globe },
    { id: "backup", label: "Backup & Restore", icon: Download },
    { id: "rate-limits", label: "Rate Limits", icon: Shield },
    { id: "webhooks", label: "Webhooks", icon: Bell },
  ];

  return (
    <div className="p-6 max-w-4xl">
      <h2 className="text-lg font-semibold text-zinc-100 mb-1">Settings</h2>
      <p className="text-sm text-zinc-500 mb-6">
        Manage your DarshanDB deployment configuration
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

      {/* Environment Variables */}
      {activeTab === "env" && (
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <p className="text-sm text-zinc-400">
              Environment variables are encrypted at rest and available to your functions.
            </p>
            <button className="btn-primary text-xs">
              <Plus className="w-3.5 h-3.5" />
              Add Variable
            </button>
          </div>

          <div className="glass-panel p-0 overflow-hidden">
            {mockEnvVars.map((envVar, i) => (
              <div
                key={envVar.key}
                className={cn(
                  "flex items-center gap-4 px-4 py-3 hover:bg-zinc-800/30 transition-colors",
                  i !== mockEnvVars.length - 1 && "border-b border-zinc-800/60",
                )}
              >
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-sm text-zinc-200">{envVar.key}</span>
                    {envVar.isSecret && (
                      <Badge variant="amber" className="text-[9px]">
                        <Shield className="w-2.5 h-2.5 mr-0.5" />
                        secret
                      </Badge>
                    )}
                  </div>
                  <div className="flex items-center gap-2 mt-1">
                    <span className="font-mono text-xs text-zinc-500">
                      {envVar.isSecret && !showSecrets.has(envVar.key)
                        ? "••••••••••••"
                        : envVar.value}
                    </span>
                  </div>
                </div>
                <span className="text-[10px] text-zinc-600 flex-shrink-0">
                  {formatRelativeTime(envVar.updatedAt)}
                </span>
                <div className="flex items-center gap-1 flex-shrink-0">
                  {envVar.isSecret && (
                    <button
                      onClick={() => toggleSecret(envVar.key)}
                      className="btn-ghost p-1.5"
                    >
                      {showSecrets.has(envVar.key) ? (
                        <EyeOff className="w-3.5 h-3.5" />
                      ) : (
                        <Eye className="w-3.5 h-3.5" />
                      )}
                    </button>
                  )}
                  <button
                    onClick={() => copyValue(envVar.key, envVar.value)}
                    className="btn-ghost p-1.5"
                  >
                    {copied === envVar.key ? (
                      <Check className="w-3.5 h-3.5 text-emerald-400" />
                    ) : (
                      <Copy className="w-3.5 h-3.5" />
                    )}
                  </button>
                  <button className="btn-ghost p-1.5 text-red-400 hover:text-red-300">
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Backup & Restore */}
      {activeTab === "backup" && (
        <div className="space-y-6">
          <div className="glass-panel p-6">
            <h3 className="text-sm font-semibold text-zinc-100 mb-1">Create Backup</h3>
            <p className="text-xs text-zinc-500 mb-4">
              Export a snapshot of your database, schema, and configuration.
            </p>
            <div className="flex gap-3">
              <button className="btn-primary text-sm">
                <Download className="w-4 h-4" />
                Full Backup
              </button>
              <button className="btn-secondary text-sm">
                <Download className="w-4 h-4" />
                Schema Only
              </button>
            </div>
          </div>

          <div className="glass-panel p-6">
            <h3 className="text-sm font-semibold text-zinc-100 mb-1">Restore</h3>
            <p className="text-xs text-zinc-500 mb-4">
              Restore from a previous backup. This will overwrite current data.
            </p>
            <button className="btn-secondary text-sm">
              <Upload className="w-4 h-4" />
              Upload Backup File
            </button>
          </div>

          <div className="glass-panel p-6">
            <h3 className="text-sm font-semibold text-zinc-100 mb-2">Recent Backups</h3>
            <div className="space-y-2">
              {[
                { name: "backup-20240401-full.darshan", size: "45.2 MB", date: "Apr 1, 2024 03:00 AM", type: "Automatic" },
                { name: "backup-20240315-manual.darshan", size: "44.8 MB", date: "Mar 15, 2024 02:30 PM", type: "Manual" },
                { name: "backup-20240301-full.darshan", size: "42.1 MB", date: "Mar 1, 2024 03:00 AM", type: "Automatic" },
              ].map((backup) => (
                <div
                  key={backup.name}
                  className="flex items-center justify-between py-2 px-3 rounded-lg hover:bg-zinc-800/30"
                >
                  <div className="flex items-center gap-3">
                    <Download className="w-4 h-4 text-zinc-600" />
                    <div>
                      <p className="text-xs font-medium text-zinc-200">{backup.name}</p>
                      <p className="text-[10px] text-zinc-500">{backup.size} -- {backup.date}</p>
                    </div>
                  </div>
                  <div className="flex items-center gap-2">
                    <Badge variant={backup.type === "Automatic" ? "sky" : "amber"} className="text-[9px]">
                      {backup.type}
                    </Badge>
                    <button className="btn-ghost text-xs">
                      <RefreshCw className="w-3 h-3" />
                      Restore
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}

      {/* Rate Limits */}
      {activeTab === "rate-limits" && (
        <div className="space-y-4">
          <p className="text-sm text-zinc-400">
            Configure rate limiting for your API endpoints.
          </p>
          <div className="glass-panel p-0 overflow-hidden">
            {[
              { endpoint: "Queries", limit: "1,000/min", current: 342, max: 1000 },
              { endpoint: "Mutations", limit: "500/min", current: 128, max: 500 },
              { endpoint: "Actions", limit: "100/min", current: 45, max: 100 },
              { endpoint: "File uploads", limit: "50/min", current: 3, max: 50 },
              { endpoint: "Auth attempts", limit: "10/min", current: 1, max: 10 },
            ].map((item, i, arr) => (
              <div
                key={item.endpoint}
                className={cn(
                  "flex items-center gap-4 px-4 py-3",
                  i !== arr.length - 1 && "border-b border-zinc-800/60",
                )}
              >
                <span className="text-sm text-zinc-200 w-36">{item.endpoint}</span>
                <div className="flex-1">
                  <div className="flex items-center justify-between mb-1">
                    <span className="text-xs text-zinc-500">{item.current} / {item.max}</span>
                    <span className="text-xs text-zinc-600">{item.limit}</span>
                  </div>
                  <div className="w-full h-1.5 bg-zinc-800 rounded-full overflow-hidden">
                    <div
                      className={cn(
                        "h-full rounded-full transition-all",
                        item.current / item.max > 0.8
                          ? "bg-red-500"
                          : item.current / item.max > 0.5
                            ? "bg-amber-500"
                            : "bg-emerald-500",
                      )}
                      style={{ width: `${(item.current / item.max) * 100}%` }}
                    />
                  </div>
                </div>
                <button className="btn-ghost text-xs">Edit</button>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Webhooks */}
      {activeTab === "webhooks" && (
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <p className="text-sm text-zinc-400">
              Send real-time notifications to external services.
            </p>
            <button className="btn-primary text-xs">
              <Plus className="w-3.5 h-3.5" />
              Add Webhook
            </button>
          </div>

          <div className="space-y-3">
            {[
              {
                url: "https://hooks.slack.com/services/T00/B00/xxx",
                events: ["mutation:*", "error:*"],
                status: "active",
                lastDelivery: Date.now() - 300_000,
              },
              {
                url: "https://api.example.com/webhooks/darshan",
                events: ["user:created", "user:deleted"],
                status: "active",
                lastDelivery: Date.now() - 3600_000,
              },
              {
                url: "https://old-service.example.com/hook",
                events: ["document:created"],
                status: "failing",
                lastDelivery: Date.now() - 86400_000,
              },
            ].map((webhook, i) => (
              <div key={i} className="glass-panel p-4">
                <div className="flex items-center justify-between mb-2">
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-xs text-zinc-300 truncate max-w-md">
                      {webhook.url}
                    </span>
                    <Badge
                      variant={webhook.status === "active" ? "emerald" : "red"}
                      className="text-[9px]"
                    >
                      {webhook.status}
                    </Badge>
                  </div>
                  <div className="flex items-center gap-1">
                    <button className="btn-ghost text-xs">Edit</button>
                    <button className="btn-ghost text-xs text-red-400">Delete</button>
                  </div>
                </div>
                <div className="flex items-center gap-2 flex-wrap">
                  {webhook.events.map((event) => (
                    <Badge key={event} variant="zinc" className="text-[10px] font-mono">
                      {event}
                    </Badge>
                  ))}
                  <span className="text-[10px] text-zinc-600 ml-auto flex items-center gap-1">
                    <Clock className="w-2.5 h-2.5" />
                    Last: {formatRelativeTime(webhook.lastDelivery)}
                  </span>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
