import { useState, useEffect, useCallback } from "react";
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
  Loader2,
  AlertCircle,
  UserPlus,
  RefreshCw,
} from "lucide-react";
import { Badge } from "../components/Badge";
import { mockUsers } from "../lib/mock-data";
import { fetchEntities, fetchSchema, fetchSessions, createUser, ApiError } from "../lib/api";
import type { AdminSessionsResponse } from "../lib/api";
import { cn, formatRelativeTime, formatTimestamp } from "../lib/utils";
import type { User } from "../types";

const roleBadge: Record<User["role"], { variant: "amber" | "emerald" | "sky"; icon: typeof Shield }> = {
  admin: { variant: "amber", icon: ShieldCheck },
  developer: { variant: "sky", icon: Shield },
  viewer: { variant: "emerald", icon: Eye },
};

/** Map a raw entity record from the API into the User shape the UI expects. */
function entityToUser(
  rec: Record<string, unknown>,
  index: number,
  sessionsData?: AdminSessionsResponse,
): User {
  const userId = (rec._id as string) ?? `api_${index}`;

  // Match sessions from admin sessions endpoint by user_id.
  const userSessions: User["sessions"] = [];
  if (sessionsData) {
    for (const s of sessionsData.sessions as Record<string, unknown>[]) {
      if (String(s.user_id) === userId) {
        userSessions.push({
          id: String(s.session_id ?? ""),
          device: String(s.user_agent ?? "Unknown"),
          ip: String(s.ip ?? ""),
          lastActive: s.created_at
            ? new Date(String(s.created_at)).getTime()
            : Date.now(),
          current: false,
        });
      }
    }
  }

  return {
    id: userId,
    email: (rec.email as string) ?? "",
    name: (rec.name as string) ?? (rec.email as string) ?? "Unknown",
    role: (["admin", "developer", "viewer"].includes(rec.role as string)
      ? (rec.role as User["role"])
      : "viewer"),
    createdAt: typeof rec.createdAt === "number"
      ? (rec.createdAt as number)
      : typeof rec.created_at === "string"
        ? new Date(rec.created_at as string).getTime()
        : (rec._creationTime as number) ?? Date.now(),
    lastLogin: typeof rec.lastLogin === "number"
      ? (rec.lastLogin as number)
      : userSessions.length > 0
        ? Math.max(...userSessions.map((s) => s.lastActive))
        : Date.now(),
    sessions: userSessions,
  };
}

export function AuthUsers() {
  const [search, setSearch] = useState("");
  const [roleFilter, setRoleFilter] = useState<string>("all");
  const [selectedUser, setSelectedUser] = useState<User | null>(null);

  // Live data state
  const [users, setUsers] = useState<User[]>(mockUsers);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [isLive, setIsLive] = useState(false);

  // Create user dialog
  const [showCreate, setShowCreate] = useState(false);
  const [createEmail, setCreateEmail] = useState("");
  const [createName, setCreateName] = useState("");
  const [createPassword, setCreatePassword] = useState("");
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const loadUsers = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // Fetch sessions in parallel with schema+entities.
      const sessionsPromise = fetchSessions().catch(() => ({
        sessions: [],
        count: 0,
      } as AdminSessionsResponse));

      // Try fetching "users" entity type from the data API.
      const schema = await fetchSchema();
      const userType = schema.find(
        (et) => et.name === "users" || et.name === "user",
      );

      if (!userType) {
        setUsers(mockUsers);
        setIsLive(false);
        setLoading(false);
        return;
      }

      const [res, sessionsData] = await Promise.all([
        fetchEntities(userType.name, 200),
        sessionsPromise,
      ]);

      if (res.data.length === 0) {
        setUsers(mockUsers);
        setIsLive(false);
      } else {
        setUsers(res.data.map((r, i) => entityToUser(r, i, sessionsData)));
        setIsLive(true);
      }
    } catch (err) {
      console.warn("[AuthUsers] API unavailable, using mock data:", err);
      setUsers(mockUsers);
      setIsLive(false);
      if (err instanceof ApiError) {
        setError(`API ${err.status}: ${err.body}`);
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadUsers();
  }, [loadUsers]);

  const handleCreateUser = async () => {
    if (!createEmail || !createPassword) return;
    setCreating(true);
    setCreateError(null);
    try {
      await createUser({
        email: createEmail,
        password: createPassword,
        name: createName || undefined,
      });
      // Refresh the list
      setShowCreate(false);
      setCreateEmail("");
      setCreateName("");
      setCreatePassword("");
      await loadUsers();
    } catch (err) {
      if (err instanceof ApiError) {
        setCreateError(err.body || err.message);
      } else {
        setCreateError("Failed to create user");
      }
    } finally {
      setCreating(false);
    }
  };

  const filtered = users.filter((user) => {
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
              {loading ? (
                <span className="flex items-center gap-1.5">
                  <Loader2 className="w-3 h-3 animate-spin" />
                  Loading users...
                </span>
              ) : (
                <>
                  {users.length} users
                  {!isLive && (
                    <Badge variant="zinc" className="ml-2 text-[9px]">mock data</Badge>
                  )}
                  {isLive && (
                    <Badge variant="emerald" className="ml-2 text-[9px]">live</Badge>
                  )}
                </>
              )}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={loadUsers}
              className="btn-ghost text-xs"
              title="Refresh"
            >
              <RefreshCw className={cn("w-3.5 h-3.5", loading && "animate-spin")} />
            </button>
            <button
              onClick={() => setShowCreate(true)}
              className="btn-primary text-sm"
            >
              <UserPlus className="w-4 h-4" />
              Create User
            </button>
          </div>
        </div>

        {error && (
          <div className="glass-panel p-3 mb-4 border-amber-500/30 flex items-center gap-2 text-xs text-amber-400">
            <AlertCircle className="w-3.5 h-3.5 flex-shrink-0" />
            <span>{error} -- showing mock data as fallback</span>
          </div>
        )}

        {/* Create user dialog */}
        {showCreate && (
          <div className="glass-panel p-4 mb-4 border-amber-500/20">
            <h3 className="text-sm font-semibold text-zinc-100 mb-3">Create New User</h3>
            <div className="space-y-3">
              <div>
                <label className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
                  Email *
                </label>
                <input
                  value={createEmail}
                  onChange={(e) => setCreateEmail(e.target.value)}
                  placeholder="user@example.com"
                  className="input-field text-xs mt-1"
                  type="email"
                />
              </div>
              <div>
                <label className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
                  Name
                </label>
                <input
                  value={createName}
                  onChange={(e) => setCreateName(e.target.value)}
                  placeholder="Full name"
                  className="input-field text-xs mt-1"
                />
              </div>
              <div>
                <label className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
                  Password * (min 8 chars)
                </label>
                <input
                  value={createPassword}
                  onChange={(e) => setCreatePassword(e.target.value)}
                  placeholder="Minimum 8 characters"
                  className="input-field text-xs mt-1"
                  type="password"
                />
              </div>
              {createError && (
                <p className="text-xs text-red-400 flex items-center gap-1.5">
                  <AlertCircle className="w-3 h-3" />
                  {createError}
                </p>
              )}
              <div className="flex items-center gap-2 pt-1">
                <button
                  onClick={handleCreateUser}
                  disabled={creating || !createEmail || createPassword.length < 8}
                  className="btn-primary text-xs disabled:opacity-50"
                >
                  {creating ? (
                    <Loader2 className="w-3.5 h-3.5 animate-spin" />
                  ) : (
                    <UserPlus className="w-3.5 h-3.5" />
                  )}
                  {creating ? "Creating..." : "Create"}
                </button>
                <button
                  onClick={() => {
                    setShowCreate(false);
                    setCreateError(null);
                  }}
                  className="btn-ghost text-xs"
                >
                  Cancel
                </button>
              </div>
            </div>
          </div>
        )}

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
                    <button className="btn-ghost p-1" aria-label={`More options for ${user.name}`}>
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
                  User ID
                </label>
                <p className="text-xs text-zinc-300 mt-0.5 font-mono break-all">
                  {selectedUser.id}
                </p>
              </div>
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
