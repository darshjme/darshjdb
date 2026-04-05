import type {
  EntityType,
  EntityRecord,
  FunctionDef,
  FunctionExecution,
  User,
  StorageFile,
  LogEntry,
  EnvVariable,
  DashboardStats,
} from "../types";

export const mockStats: DashboardStats = {
  totalEntities: 12,
  totalDocuments: 48_291,
  totalFunctions: 34,
  activeUsers: 156,
  storageUsed: 2.4 * 1024 * 1024 * 1024,
  requestsToday: 124_853,
};

export const mockEntityTypes: EntityType[] = [
  {
    name: "users",
    count: 1243,
    fields: [
      { name: "_id", type: "Id<users>", required: true, indexed: true, unique: true },
      { name: "email", type: "string", required: true, indexed: true, unique: true },
      { name: "name", type: "string", required: true, indexed: false, unique: false },
      { name: "role", type: "string", required: true, indexed: true, unique: false, default: '"viewer"' },
      { name: "avatarUrl", type: "string?", required: false, indexed: false, unique: false },
      { name: "createdAt", type: "number", required: true, indexed: true, unique: false },
    ],
  },
  {
    name: "documents",
    count: 8_412,
    fields: [
      { name: "_id", type: "Id<documents>", required: true, indexed: true, unique: true },
      { name: "title", type: "string", required: true, indexed: true, unique: false },
      { name: "content", type: "string", required: true, indexed: false, unique: false },
      { name: "authorId", type: "Id<users>", required: true, indexed: true, unique: false },
      { name: "status", type: "string", required: true, indexed: true, unique: false },
      { name: "updatedAt", type: "number", required: true, indexed: true, unique: false },
    ],
  },
  {
    name: "messages",
    count: 34_201,
    fields: [
      { name: "_id", type: "Id<messages>", required: true, indexed: true, unique: true },
      { name: "channelId", type: "Id<channels>", required: true, indexed: true, unique: false },
      { name: "authorId", type: "Id<users>", required: true, indexed: true, unique: false },
      { name: "body", type: "string", required: true, indexed: false, unique: false },
      { name: "timestamp", type: "number", required: true, indexed: true, unique: false },
    ],
  },
  {
    name: "channels",
    count: 87,
    fields: [
      { name: "_id", type: "Id<channels>", required: true, indexed: true, unique: true },
      { name: "name", type: "string", required: true, indexed: true, unique: true },
      { name: "description", type: "string?", required: false, indexed: false, unique: false },
      { name: "isPrivate", type: "boolean", required: true, indexed: true, unique: false },
    ],
  },
  {
    name: "files",
    count: 2_156,
    fields: [
      { name: "_id", type: "Id<files>", required: true, indexed: true, unique: true },
      { name: "name", type: "string", required: true, indexed: true, unique: false },
      { name: "storageId", type: "string", required: true, indexed: true, unique: true },
      { name: "size", type: "number", required: true, indexed: false, unique: false },
      { name: "mimeType", type: "string", required: true, indexed: true, unique: false },
    ],
  },
  {
    name: "sessions",
    count: 2_192,
    fields: [
      { name: "_id", type: "Id<sessions>", required: true, indexed: true, unique: true },
      { name: "userId", type: "Id<users>", required: true, indexed: true, unique: false },
      { name: "token", type: "string", required: true, indexed: true, unique: true },
      { name: "expiresAt", type: "number", required: true, indexed: true, unique: false },
    ],
  },
];

export const mockRecords: EntityRecord[] = [
  { _id: "jd72k3m4n5", _creationTime: 1711234567890, email: "alex@db.darshj.me", name: "Alex Rivera", role: "admin", avatarUrl: null },
  { _id: "kf83l4n5o6", _creationTime: 1711334567890, email: "sam@db.darshj.me", name: "Sam Chen", role: "developer", avatarUrl: null },
  { _id: "lg94m5o6p7", _creationTime: 1711434567890, email: "jordan@example.com", name: "Jordan Lee", role: "viewer", avatarUrl: null },
  { _id: "mh05n6p7q8", _creationTime: 1711534567890, email: "taylor@example.com", name: "Taylor Kim", role: "developer", avatarUrl: null },
  { _id: "ni16o7q8r9", _creationTime: 1711634567890, email: "morgan@example.com", name: "Morgan Patel", role: "viewer", avatarUrl: null },
  { _id: "oj27p8r9s0", _creationTime: 1711734567890, email: "casey@db.darshj.me", name: "Casey Wu", role: "admin", avatarUrl: null },
  { _id: "pk38q9s0t1", _creationTime: 1711834567890, email: "riley@example.com", name: "Riley Brooks", role: "viewer", avatarUrl: null },
  { _id: "ql49r0t1u2", _creationTime: 1711934567890, email: "avery@example.com", name: "Avery Singh", role: "developer", avatarUrl: null },
];

export const mockFunctions: FunctionDef[] = [
  { name: "users:list", type: "query", module: "users", args: { limit: "number?", cursor: "string?" }, returns: "PaginatedResult<User>", avgDuration: 12, errorRate: 0.1 },
  { name: "users:getById", type: "query", module: "users", args: { id: "Id<users>" }, returns: "User | null", avgDuration: 3, errorRate: 0 },
  { name: "users:create", type: "mutation", module: "users", args: { email: "string", name: "string", role: "string" }, returns: "Id<users>", avgDuration: 18, errorRate: 0.4 },
  { name: "users:update", type: "mutation", module: "users", args: { id: "Id<users>", data: "Partial<User>" }, returns: "void", avgDuration: 15, errorRate: 0.2 },
  { name: "documents:search", type: "query", module: "documents", args: { query: "string", limit: "number?" }, returns: "Document[]", avgDuration: 45, errorRate: 0.8 },
  { name: "documents:create", type: "mutation", module: "documents", args: { title: "string", content: "string" }, returns: "Id<documents>", avgDuration: 22, errorRate: 0.3 },
  { name: "messages:send", type: "mutation", module: "messages", args: { channelId: "Id<channels>", body: "string" }, returns: "Id<messages>", avgDuration: 8, errorRate: 0.1 },
  { name: "messages:list", type: "query", module: "messages", args: { channelId: "Id<channels>", limit: "number" }, returns: "Message[]", avgDuration: 15, errorRate: 0 },
  { name: "files:generateUploadUrl", type: "mutation", module: "files", args: {}, returns: "string", avgDuration: 120, errorRate: 1.2 },
  { name: "analytics:aggregate", type: "action", module: "analytics", args: { metric: "string", range: "string" }, returns: "AggregateResult", avgDuration: 340, errorRate: 2.1 },
  { name: "cleanup:expiredSessions", type: "cron", module: "cleanup", args: {}, returns: "void", avgDuration: 85, errorRate: 0 },
  { name: "sync:externalData", type: "cron", module: "sync", args: {}, returns: "void", avgDuration: 2400, errorRate: 3.5 },
];

export const mockExecutions: FunctionExecution[] = Array.from({ length: 20 }, (_, i) => ({
  id: `exec_${i}`,
  functionName: mockFunctions[i % mockFunctions.length].name,
  status: i === 3 ? "error" : i === 7 ? "running" : "success",
  duration: Math.floor(Math.random() * 200) + 5,
  timestamp: Date.now() - i * 60_000 * Math.random() * 10,
  error: i === 3 ? "Document not found: jd72k3m4n5" : undefined,
}));

export const mockUsers: User[] = [
  {
    id: "usr_001", email: "alex@db.darshj.me", name: "Alex Rivera", role: "admin",
    createdAt: 1711234567890, lastLogin: Date.now() - 3600_000,
    sessions: [
      { id: "sess_1", device: "Chrome / macOS", ip: "192.168.1.100", lastActive: Date.now() - 300_000, current: true },
      { id: "sess_2", device: "Safari / iOS", ip: "10.0.0.5", lastActive: Date.now() - 86400_000, current: false },
    ],
  },
  {
    id: "usr_002", email: "sam@db.darshj.me", name: "Sam Chen", role: "developer",
    createdAt: 1711334567890, lastLogin: Date.now() - 7200_000,
    sessions: [
      { id: "sess_3", device: "Firefox / Linux", ip: "172.16.0.50", lastActive: Date.now() - 1800_000, current: true },
    ],
  },
  {
    id: "usr_003", email: "jordan@example.com", name: "Jordan Lee", role: "viewer",
    createdAt: 1711434567890, lastLogin: Date.now() - 86400_000,
    sessions: [],
  },
  {
    id: "usr_004", email: "taylor@example.com", name: "Taylor Kim", role: "developer",
    createdAt: 1711534567890, lastLogin: Date.now() - 14400_000,
    sessions: [
      { id: "sess_4", device: "Chrome / Windows", ip: "192.168.2.30", lastActive: Date.now() - 600_000, current: true },
    ],
  },
  {
    id: "usr_005", email: "morgan@example.com", name: "Morgan Patel", role: "viewer",
    createdAt: 1711634567890, lastLogin: Date.now() - 172800_000,
    sessions: [],
  },
];

export const mockStorageFiles: StorageFile[] = [
  { id: "file_001", name: "hero-banner.png", size: 2_456_000, mimeType: "image/png", url: "#", uploadedAt: Date.now() - 86400_000, uploadedBy: "Alex Rivera" },
  { id: "file_002", name: "api-docs.pdf", size: 1_200_000, mimeType: "application/pdf", url: "#", uploadedAt: Date.now() - 172800_000, uploadedBy: "Sam Chen" },
  { id: "file_003", name: "backup-20240315.zip", size: 45_000_000, mimeType: "application/zip", url: "#", uploadedAt: Date.now() - 259200_000, uploadedBy: "System" },
  { id: "file_004", name: "logo.svg", size: 12_000, mimeType: "image/svg+xml", url: "#", uploadedAt: Date.now() - 345600_000, uploadedBy: "Jordan Lee" },
  { id: "file_005", name: "user-export.csv", size: 890_000, mimeType: "text/csv", url: "#", uploadedAt: Date.now() - 432000_000, uploadedBy: "Taylor Kim" },
  { id: "file_006", name: "screenshot.jpg", size: 3_400_000, mimeType: "image/jpeg", url: "#", uploadedAt: Date.now() - 518400_000, uploadedBy: "Alex Rivera" },
  { id: "file_007", name: "schema-v2.json", size: 45_000, mimeType: "application/json", url: "#", uploadedAt: Date.now() - 604800_000, uploadedBy: "Sam Chen" },
  { id: "file_008", name: "training-data.jsonl", size: 120_000_000, mimeType: "application/jsonl", url: "#", uploadedAt: Date.now() - 691200_000, uploadedBy: "System" },
];

export const mockLogs: LogEntry[] = Array.from({ length: 50 }, (_, i) => {
  const levels: LogEntry["level"][] = ["debug", "info", "warn", "error"];
  const fns = ["users:list", "documents:search", "messages:send", "files:generateUploadUrl", "analytics:aggregate"];
  const messages = [
    "Query executed successfully",
    "Cache miss, fetching from database",
    "Rate limit approaching threshold",
    "Failed to process request",
    "New user session created",
    "Document indexed for search",
    "Webhook delivery failed, retrying",
    "Storage quota at 85%",
    "Authentication token refreshed",
    "Cron job completed",
  ];
  const level = levels[i < 5 ? 3 : i < 15 ? 2 : i < 30 ? 1 : 0];
  return {
    id: `log_${String(i).padStart(3, "0")}`,
    level,
    message: messages[i % messages.length],
    function: fns[i % fns.length],
    userId: i % 3 === 0 ? `usr_${String((i % 5) + 1).padStart(3, "0")}` : undefined,
    timestamp: Date.now() - i * 30_000,
    data: i % 4 === 0 ? { duration: Math.floor(Math.random() * 500), cached: i % 2 === 0 } : undefined,
  };
});

export const mockEnvVars: EnvVariable[] = [
  { key: "DARSHAN_DEPLOY_KEY", value: "dk_prod_a1b2c3d4e5f6", isSecret: true, updatedAt: Date.now() - 604800_000 },
  { key: "DDB_URL", value: "https://api.db.darshj.me", isSecret: false, updatedAt: Date.now() - 2592000_000 },
  { key: "SMTP_HOST", value: "smtp.resend.com", isSecret: false, updatedAt: Date.now() - 1296000_000 },
  { key: "SMTP_API_KEY", value: "re_abc123def456", isSecret: true, updatedAt: Date.now() - 1296000_000 },
  { key: "WEBHOOK_SECRET", value: "whsec_xyz789", isSecret: true, updatedAt: Date.now() - 864000_000 },
  { key: "STORAGE_BUCKET", value: "ddb-prod-files", isSecret: false, updatedAt: Date.now() - 2592000_000 },
  { key: "CDN_URL", value: "https://cdn.db.darshj.me", isSecret: false, updatedAt: Date.now() - 2592000_000 },
];

export const mockExecutionHistory = Array.from({ length: 24 }, (_, i) => ({
  hour: `${String(i).padStart(2, "0")}:00`,
  queries: Math.floor(Math.random() * 5000) + 1000,
  mutations: Math.floor(Math.random() * 2000) + 500,
  errors: Math.floor(Math.random() * 50),
}));
