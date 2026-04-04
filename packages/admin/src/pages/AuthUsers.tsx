import { useState } from "react";
import {
  Search,
  Shield,
  ShieldCheck,
  Eye,
  MoreVertical,
  Monitor,
  Smartphone,
  Clock,
  MapPin,
  X,
} from "lucide-react";
import { Badge } from "../components/Badge";
import { mockUsers } from "../lib/mock-data";
import { cn, formatRelativeTime, formatTimestamp } from "../lib/utils";
import type { User } from "../types";

const roleBadge: Record<User["role"], { variant: "amber" | "emerald" | "sky"; icon: typeof Shield }> = {
  admin: { variant: "amber", icon: ShieldCheck },
  developer: { variant: "sky", icon: Shield },
  viewer: { variant: "emerald", icon: Eye },
};

export function AuthUsers() {
  const [search, setSearch] = useState("");
  const [roleFilter, setRoleFilter] = useState<string>("all");
  const [selectedUser, setSelectedUser] = useState<User | null>(null);

  const filtered = mockUsers.filter((user) => {
    if (roleFilter !== "all" && user.role !== roleFilter) return false;
    if (search) {
      const q = search.toLowerCase();
      return user.name.toLowerCase().includes(q) || user.email.toLowerCase().includes(q);
    }
    return true;
  });

  return (
    <div className="flex h-full">
      {/* User list */}
      <div className="flex-1 overflow-auto p-6">
        <div className="flex items-center justify-between mb-6">
          <div>
            <h2 className="text-lg font-semibold text-zinc-100">Auth & Users</h2>
            <p className="text-sm text-zinc-500 mt-0.5">
              {mockUsers.length} users, {mockUsers.filter((u) => u.sessions.length > 0).length} active
            </p>
          </div>
          <button className="btn-primary text-sm">
            <Shield className="w-4 h-4" />
            Invite User
          </button>
        </div>

        {/* Filters */}
        <div className="flex items-center gap-3 mb-4">
          <div className="relative flex-1 max-w-sm">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-zinc-500" />
            <input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search users..."
              className="input-field pl-9 text-xs"
            />
          </div>
          <div className="flex items-center gap-1 bg-zinc-900 rounded-lg p-0.5 border border-zinc-800">
            {["all", "admin", "developer", "viewer"].map((r) => (
              <button
                key={r}
                onClick={() => setRoleFilter(r)}
                className={cn(
                  "px-2.5 py-1 rounded-md text-xs font-medium transition-colors capitalize",
                  roleFilter === r
                    ? "bg-zinc-800 text-zinc-100"
                    : "text-zinc-500 hover:text-zinc-300",
                )}
              >
                {r}
              </button>
            ))}
          </div>
        </div>

        {/* User cards */}
        <div className="space-y-2">
          {filtered.map((user) => {
            const role = roleBadge[user.role];
            return (
              <button
                key={user.id}
                onClick={() => setSelectedUser(user)}
                className={cn(
                  "w-full glass-panel p-4 text-left transition-all hover:border-zinc-700",
                  selectedUser?.id === user.id && "border-amber-500/40",
                )}
              >
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-3">
                    <div className="w-9 h-9 rounded-full bg-gradient-to-br from-zinc-700 to-zinc-800 flex items-center justify-center">
                      <span className="text-sm font-semibold text-zinc-300">
                        {user.name.charAt(0)}
                      </span>
                    </div>
                    <div>
                      <div className="flex items-center gap-2">
                        <span className="text-sm font-medium text-zinc-100">
                          {user.name}
                        </span>
                        <Badge variant={role.variant} className="text-[10px]">
                          <role.icon className="w-2.5 h-2.5 mr-1" />
                          {user.role}
                        </Badge>
                      </div>
                      <span className="text-xs text-zinc-500">{user.email}</span>
                    </div>
                  </div>
                  <div className="flex items-center gap-3 text-xs text-zinc-500">
                    <span className="flex items-center gap-1">
                      <Clock className="w-3 h-3" />
                      {formatRelativeTime(user.lastLogin)}
                    </span>
                    {user.sessions.length > 0 && (
                      <div className="w-2 h-2 rounded-full bg-emerald-400" title="Active" />
                    )}
                    <button className="btn-ghost p-1">
                      <MoreVertical className="w-3.5 h-3.5" />
                    </button>
                  </div>
                </div>
              </button>
            );
          })}
        </div>
      </div>

      {/* User detail panel */}
      {selectedUser && (
        <div className="w-80 flex-shrink-0 border-l border-zinc-800 bg-zinc-950/50 overflow-y-auto">
          <div className="px-4 py-3 border-b border-zinc-800 flex items-center justify-between">
            <h3 className="text-sm font-semibold text-zinc-100">User Details</h3>
            <button
              onClick={() => setSelectedUser(null)}
              className="btn-ghost p-1"
            >
              <X className="w-4 h-4" />
            </button>
          </div>

          <div className="p-4 space-y-6">
            {/* Profile */}
            <div className="flex flex-col items-center text-center">
              <div className="w-16 h-16 rounded-full bg-gradient-to-br from-amber-400 to-orange-500 flex items-center justify-center mb-3">
                <span className="text-xl font-bold text-zinc-950">
                  {selectedUser.name.charAt(0)}
                </span>
              </div>
              <h4 className="text-sm font-semibold text-zinc-100">{selectedUser.name}</h4>
              <p className="text-xs text-zinc-500">{selectedUser.email}</p>
              <Badge variant={roleBadge[selectedUser.role].variant} className="mt-2 text-[10px]">
                {selectedUser.role}
              </Badge>
            </div>

            {/* Info */}
            <div className="space-y-3">
              <div>
                <label className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
                  Created
                </label>
                <p className="text-xs text-zinc-300 mt-0.5">
                  {formatTimestamp(selectedUser.createdAt)}
                </p>
              </div>
              <div>
                <label className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
                  Last Login
                </label>
                <p className="text-xs text-zinc-300 mt-0.5">
                  {formatRelativeTime(selectedUser.lastLogin)}
                </p>
              </div>
            </div>

            {/* Sessions */}
            <div>
              <h4 className="text-xs font-semibold text-zinc-400 mb-2">
                Active Sessions ({selectedUser.sessions.length})
              </h4>
              {selectedUser.sessions.length === 0 ? (
                <p className="text-xs text-zinc-600 italic">No active sessions</p>
              ) : (
                <div className="space-y-2">
                  {selectedUser.sessions.map((session) => (
                    <div
                      key={session.id}
                      className="glass-panel p-3"
                    >
                      <div className="flex items-center gap-2 mb-1">
                        {session.device.includes("iOS") || session.device.includes("Android") ? (
                          <Smartphone className="w-3.5 h-3.5 text-zinc-500" />
                        ) : (
                          <Monitor className="w-3.5 h-3.5 text-zinc-500" />
                        )}
                        <span className="text-xs text-zinc-200">{session.device}</span>
                        {session.current && (
                          <Badge variant="emerald" className="text-[9px] ml-auto">
                            Current
                          </Badge>
                        )}
                      </div>
                      <div className="flex items-center gap-3 text-[10px] text-zinc-500 ml-5.5">
                        <span className="flex items-center gap-1">
                          <MapPin className="w-2.5 h-2.5" />
                          {session.ip}
                        </span>
                        <span>{formatRelativeTime(session.lastActive)}</span>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>

            {/* Permissions */}
            <div>
              <h4 className="text-xs font-semibold text-zinc-400 mb-2">Permissions</h4>
              <div className="space-y-1.5">
                {[
                  { label: "Read data", allowed: true },
                  { label: "Write data", allowed: selectedUser.role !== "viewer" },
                  { label: "Deploy functions", allowed: selectedUser.role !== "viewer" },
                  { label: "Manage users", allowed: selectedUser.role === "admin" },
                  { label: "Access settings", allowed: selectedUser.role === "admin" },
                ].map((perm) => (
                  <div key={perm.label} className="flex items-center justify-between text-xs">
                    <span className="text-zinc-400">{perm.label}</span>
                    <span className={perm.allowed ? "text-emerald-400" : "text-zinc-600"}>
                      {perm.allowed ? "Allowed" : "Denied"}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
