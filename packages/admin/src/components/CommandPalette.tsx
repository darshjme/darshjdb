import { useEffect, useState, useRef } from "react";
import { useNavigate } from "react-router-dom";
import {
  Database,
  GitBranch,
  Network,
  Zap,
  Users,
  HardDrive,
  ScrollText,
  Settings,
  Search,
  ArrowRight,
} from "lucide-react";
import { cn } from "../lib/utils";

interface CommandItem {
  id: string;
  label: string;
  description?: string;
  icon: typeof Database;
  action: () => void;
  category: string;
}

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
}

export function CommandPalette({ open, onClose }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const navigate = useNavigate();

  const commands: CommandItem[] = [
    { id: "nav-data", label: "Data Explorer", description: "Browse and query data", icon: Database, action: () => navigate("/"), category: "Navigation" },
    { id: "nav-schema", label: "Schema", description: "View entity relationships", icon: GitBranch, action: () => navigate("/schema"), category: "Navigation" },
    { id: "nav-graph", label: "Graph Explorer", description: "Traverse record links visually", icon: Network, action: () => navigate("/graph"), category: "Navigation" },
    { id: "nav-functions", label: "Functions", description: "Manage queries and mutations", icon: Zap, action: () => navigate("/functions"), category: "Navigation" },
    { id: "nav-auth", label: "Auth & Users", description: "User management", icon: Users, action: () => navigate("/auth"), category: "Navigation" },
    { id: "nav-storage", label: "Storage", description: "File management", icon: HardDrive, action: () => navigate("/storage"), category: "Navigation" },
    { id: "nav-logs", label: "Logs", description: "View system logs", icon: ScrollText, action: () => navigate("/logs"), category: "Navigation" },
    { id: "nav-settings", label: "Settings", description: "Environment and configuration", icon: Settings, action: () => navigate("/settings"), category: "Navigation" },
  ];

  const filtered = query
    ? commands.filter(
        (c) =>
          c.label.toLowerCase().includes(query.toLowerCase()) ||
          c.description?.toLowerCase().includes(query.toLowerCase()),
      )
    : commands;

  useEffect(() => {
    if (open) {
      setQuery("");
      setSelectedIndex(0);
      setTimeout(() => inputRef.current?.focus(), 50);
    }
  }, [open]);

  useEffect(() => {
    setSelectedIndex(0);
  }, [query]);

  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelectedIndex((i) => Math.min(i + 1, filtered.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelectedIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter" && filtered[selectedIndex]) {
        filtered[selectedIndex].action();
        onClose();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open, filtered, selectedIndex, onClose]);

  if (!open) return null;

  const grouped = filtered.reduce(
    (acc, item) => {
      if (!acc[item.category]) acc[item.category] = [];
      acc[item.category].push(item);
      return acc;
    },
    {} as Record<string, CommandItem[]>,
  );

  let flatIndex = 0;

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[20vh]" role="dialog" aria-modal="true" aria-label="Command palette">
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={onClose} aria-hidden="true" />
      <div className="relative w-full max-w-lg bg-zinc-900 border border-zinc-800 rounded-xl shadow-2xl animate-fade-in overflow-hidden">
        <div className="flex items-center gap-3 px-4 border-b border-zinc-800">
          <Search className="w-4 h-4 text-zinc-500 flex-shrink-0" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Type a command or search..."
            className="flex-1 bg-transparent py-3.5 text-sm text-zinc-100 placeholder-zinc-500 focus:outline-none"
            aria-label="Search commands"
          />
        </div>

        <div className="max-h-80 overflow-y-auto py-2">
          {filtered.length === 0 ? (
            <div className="px-4 py-8 text-center text-sm text-zinc-500">
              No results found
            </div>
          ) : (
            Object.entries(grouped).map(([category, items]) => (
              <div key={category}>
                <div className="px-4 py-1.5 text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
                  {category}
                </div>
                {items.map((item) => {
                  const idx = flatIndex++;
                  return (
                    <button
                      key={item.id}
                      className={cn(
                        "flex items-center gap-3 w-full px-4 py-2.5 text-left transition-colors",
                        idx === selectedIndex
                          ? "bg-amber-500/10 text-amber-500"
                          : "text-zinc-300 hover:bg-zinc-800/60",
                      )}
                      onClick={() => {
                        item.action();
                        onClose();
                      }}
                      onMouseEnter={() => setSelectedIndex(idx)}
                    >
                      <item.icon className="w-4 h-4 flex-shrink-0 opacity-60" />
                      <div className="flex-1 min-w-0">
                        <div className="text-sm font-medium">{item.label}</div>
                        {item.description && (
                          <div className="text-xs text-zinc-500 truncate">
                            {item.description}
                          </div>
                        )}
                      </div>
                      {idx === selectedIndex && (
                        <ArrowRight className="w-3.5 h-3.5 opacity-60" />
                      )}
                    </button>
                  );
                })}
              </div>
            ))
          )}
        </div>

        <div className="flex items-center gap-4 px-4 py-2.5 border-t border-zinc-800 text-[10px] text-zinc-600">
          <span className="flex items-center gap-1">
            <kbd className="px-1 py-0.5 rounded bg-zinc-800">Up/Down</kbd> navigate
          </span>
          <span className="flex items-center gap-1">
            <kbd className="px-1 py-0.5 rounded bg-zinc-800">Enter</kbd> select
          </span>
          <span className="flex items-center gap-1">
            <kbd className="px-1 py-0.5 rounded bg-zinc-800">Esc</kbd> close
          </span>
        </div>
      </div>
    </div>
  );
}
