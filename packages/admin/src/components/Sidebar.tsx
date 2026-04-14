import { NavLink } from "react-router-dom";
import {
  Database,
  GitBranch,
  Network,
  Zap,
  Users,
  HardDrive,
  ScrollText,
  Settings,
  ChevronLeft,
  ChevronRight,
} from "lucide-react";
import { useState } from "react";
import { cn } from "../lib/utils";

const navItems = [
  { to: "/", icon: Database, label: "Data Explorer" },
  { to: "/schema", icon: GitBranch, label: "Schema" },
  { to: "/graph", icon: Network, label: "Graph" },
  { to: "/functions", icon: Zap, label: "Functions" },
  { to: "/auth", icon: Users, label: "Auth & Users" },
  { to: "/storage", icon: HardDrive, label: "Storage" },
  { to: "/logs", icon: ScrollText, label: "Logs" },
  { to: "/settings", icon: Settings, label: "Settings" },
];

export function Sidebar() {
  const [collapsed, setCollapsed] = useState(false);

  return (
    <aside
      className={cn(
        "flex flex-col h-screen bg-zinc-950 border-r border-zinc-800 transition-all duration-200",
        collapsed ? "w-16" : "w-60",
      )}
      aria-label="Main navigation"
    >
      <div className="flex items-center gap-3 px-4 h-14 border-b border-zinc-800">
        <div className="w-8 h-8 rounded-lg bg-gradient-to-br from-amber-400 to-amber-600 flex items-center justify-center flex-shrink-0">
          <span className="text-zinc-950 font-bold text-sm">D</span>
        </div>
        {!collapsed && (
          <div className="flex flex-col min-w-0">
            <span className="text-sm font-semibold text-zinc-100 truncate">
              DarshJDB
            </span>
            <span className="text-[10px] text-zinc-500 truncate">
              Dashboard
            </span>
          </div>
        )}
      </div>

      <nav className="flex-1 px-2 py-3 space-y-0.5 overflow-y-auto">
        {navItems.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.to === "/"}
            className={({ isActive }) =>
              cn("sidebar-link", isActive && "active", collapsed && "justify-center px-0")
            }
            title={collapsed ? item.label : undefined}
          >
            <item.icon className="w-4 h-4 flex-shrink-0" />
            {!collapsed && <span>{item.label}</span>}
          </NavLink>
        ))}
      </nav>

      <div className="px-2 py-3 border-t border-zinc-800">
        <button
          onClick={() => setCollapsed(!collapsed)}
          className="sidebar-link w-full justify-center"
          aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
        >
          {collapsed ? (
            <ChevronRight className="w-4 h-4" />
          ) : (
            <ChevronLeft className="w-4 h-4" />
          )}
        </button>
      </div>
    </aside>
  );
}
