import { useState, useEffect, useCallback } from "react";
import {
  Search,
  Wifi,
  WifiOff,
  ChevronDown,
  Command,
  Bell,
  Loader2,
} from "lucide-react";
import type { ConnectionStatus } from "../types";
import { fetchHealth } from "../lib/api";
import { cn } from "../lib/utils";

interface TopBarProps {
  title: string;
  onOpenCommandPalette: () => void;
}

const statusConfig: Record<
  ConnectionStatus,
  { label: string; color: string; icon: typeof Wifi }
> = {
  connected: { label: "Connected", color: "text-emerald-400", icon: Wifi },
  connecting: { label: "Connecting", color: "text-amber-400", icon: Loader2 },
  disconnected: { label: "Disconnected", color: "text-red-400", icon: WifiOff },
};

export function TopBar({ title, onOpenCommandPalette }: TopBarProps) {
  const [status, setStatus] = useState<ConnectionStatus>("connecting");

  const checkHealth = useCallback(async () => {
    const healthy = await fetchHealth();
    setStatus(healthy ? "connected" : "disconnected");
  }, []);

  useEffect(() => {
    checkHealth();
    const interval = setInterval(checkHealth, 10_000);
    return () => clearInterval(interval);
  }, [checkHealth]);

  const statusInfo = statusConfig[status];

  return (
    <header className="flex items-center justify-between h-14 px-6 border-b border-zinc-800 bg-zinc-950/80 backdrop-blur-sm">
      <div className="flex items-center gap-4">
        <h1 className="text-sm font-semibold text-zinc-100">{title}</h1>
        <span className="badge bg-amber-500/10 text-amber-500 border border-amber-500/20">
          production
        </span>
      </div>

      <div className="flex items-center gap-3">
        <button
          onClick={onOpenCommandPalette}
          className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-zinc-900 border border-zinc-800 text-zinc-500 text-sm hover:border-zinc-700 transition-colors"
        >
          <Search className="w-3.5 h-3.5" />
          <span>Search</span>
          <kbd className="flex items-center gap-0.5 px-1.5 py-0.5 rounded bg-zinc-800 text-[10px] font-mono text-zinc-500">
            <Command className="w-2.5 h-2.5" />K
          </kbd>
        </button>

        <button className="btn-ghost relative" aria-label="Notifications">
          <Bell className="w-4 h-4" />
          <span className="absolute -top-0.5 -right-0.5 w-2 h-2 rounded-full bg-amber-500" aria-hidden="true" />
        </button>

        <div className={cn("flex items-center gap-1.5 text-xs", statusInfo.color)}>
          <statusInfo.icon className={cn("w-3.5 h-3.5", status === "connecting" && "animate-spin")} />
          <span>{statusInfo.label}</span>
        </div>

        <div className="w-px h-6 bg-zinc-800" />

        <button className="flex items-center gap-2 px-2 py-1.5 rounded-lg hover:bg-zinc-800/60 transition-colors" aria-label="User menu">
          <div className="w-7 h-7 rounded-full bg-gradient-to-br from-amber-400 to-orange-500 flex items-center justify-center">
            <span className="text-xs font-semibold text-zinc-950">A</span>
          </div>
          <ChevronDown className="w-3 h-3 text-zinc-500" />
        </button>
      </div>
    </header>
  );
}
