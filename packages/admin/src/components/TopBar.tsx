import { useState, useEffect, useCallback } from "react";
import {
  Search,
  Wifi,
  WifiOff,
  LogOut,
  Command,
  Loader2,
} from "lucide-react";
import type { ConnectionStatus } from "../types";
import { fetchHealth } from "../lib/api";
import { signOut } from "../lib/http";
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
    <header className="flex items-center justify-between min-h-14 px-3 sm:px-6 border-b border-zinc-800 bg-zinc-950/80 backdrop-blur-sm">
      <div className="flex items-center gap-4">
        <h1 className="text-sm font-semibold text-zinc-100">{title}</h1>
      </div>

      <div className="flex items-center gap-3">
        <button
          onClick={onOpenCommandPalette}
          className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-zinc-900 border border-zinc-800 text-zinc-500 text-sm hover:border-zinc-700 transition-colors"
        >
          <Search className="w-3.5 h-3.5" />
          <span className="hidden sm:inline">Search</span>
          <kbd className="hidden sm:flex items-center gap-0.5 px-1.5 py-0.5 rounded bg-zinc-800 text-[10px] font-mono text-zinc-500">
            <Command className="w-2.5 h-2.5" />K
          </kbd>
        </button>

        <div className={cn("flex items-center gap-1.5 text-xs", statusInfo.color)}>
          <statusInfo.icon className={cn("w-3.5 h-3.5", status === "connecting" && "animate-spin")} />
          <span>{statusInfo.label}</span>
        </div>

        <div className="w-px h-6 bg-zinc-800" />

        <button onClick={() => { void signOut().catch(() => {}); }} className="btn-ghost" aria-label="Sign out" title="Sign out"><LogOut className="w-4 h-4" /></button>
      </div>
    </header>
  );
}
