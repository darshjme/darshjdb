import { NavLink } from "react-router";
import {
  Database,
  Home,
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
  { to: "/", icon: Home, label: "Overview" },
  { to: "/data", icon: Database, label: "Data Explorer" },
  { to: "/schema", icon: GitBranch, label: "Schema" },
  { to: "/graph", icon: Network, label: "Graph" },
  { to: "/functions", icon: Zap, label: "Functions" },
  { to: "/auth", icon: Users, label: "Auth & Users" },
  { to: "/storage", icon: HardDrive, label: "Storage" },
  { to: "/logs", icon: ScrollText, label: "Logs" },
  { to: "/settings", icon: Settings, label: "Settings" },
];

export function Sidebar() {
  const [collapsed, setCollapsed] = useState(() => window.innerWidth < 768);

  return (
    <aside
      className={cn(
        "flex shrink-0 flex-col h-screen bg-surface-subtle border-r border-line transition-all duration-200",
        collapsed ? "w-16" : "w-52",
      )}
      aria-label="Main navigation"
    >
      <div className="flex items-center gap-3 px-4 h-14 border-b border-line">
        <div className="w-7 h-7 rounded-md bg-[#DFEDF0] flex items-center justify-center flex-shrink-0">
          <span className="text-[#597683] font-semibold text-sm">D</span>
        </div>
        {!collapsed && (
          <div className="flex flex-col min-w-0">
            <span className="text-sm font-semibold text-ink truncate">
              DarshJDB
            </span>
            <span className="text-[10px] text-ink-muted truncate">
              Workspace
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

      <div className="px-2 py-3 border-t border-line">
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
