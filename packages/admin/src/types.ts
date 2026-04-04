export interface EntityType {
  name: string;
  count: number;
  fields: EntityField[];
}

export interface EntityField {
  name: string;
  type: string;
  required: boolean;
  indexed: boolean;
  unique: boolean;
  default?: string;
}

export interface EntityRecord {
  _id: string;
  _creationTime: number;
  [key: string]: unknown;
}

export interface FunctionDef {
  name: string;
  type: "query" | "mutation" | "action" | "cron";
  module: string;
  args: Record<string, string>;
  returns: string;
  lastExecuted?: number;
  avgDuration?: number;
  errorRate?: number;
}

export interface FunctionExecution {
  id: string;
  functionName: string;
  status: "success" | "error" | "running";
  duration: number;
  timestamp: number;
  error?: string;
}

export interface User {
  id: string;
  email: string;
  name: string;
  role: "admin" | "developer" | "viewer";
  createdAt: number;
  lastLogin: number;
  sessions: UserSession[];
}

export interface UserSession {
  id: string;
  device: string;
  ip: string;
  lastActive: number;
  current: boolean;
}

export interface StorageFile {
  id: string;
  name: string;
  size: number;
  mimeType: string;
  url: string;
  uploadedAt: number;
  uploadedBy: string;
}

export interface LogEntry {
  id: string;
  level: "debug" | "info" | "warn" | "error";
  message: string;
  function?: string;
  userId?: string;
  timestamp: number;
  data?: Record<string, unknown>;
}

export interface EnvVariable {
  key: string;
  value: string;
  isSecret: boolean;
  updatedAt: number;
}

export type ConnectionStatus = "connected" | "connecting" | "disconnected";

export interface DashboardStats {
  totalEntities: number;
  totalDocuments: number;
  totalFunctions: number;
  activeUsers: number;
  storageUsed: number;
  requestsToday: number;
}
