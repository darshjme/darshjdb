/**
 * DarshJDB REST API client.
 *
 * Talks to the live server when available, with every function returning
 * a typed result. Failures remain visible to callers.
 *
 * The base URL is configurable via `VITE_DDB_URL` (defaults to
 * the current origin).
 */

import type { EntityType, EntityField, EntityRecord } from "../types";

import { apiFetch } from "./http";
export { ApiError } from "./http";
async function publicFetch<T>(path: string): Promise<T> { return apiFetch<T>(path, undefined, ""); }

// ---------------------------------------------------------------------------
// Server-side schema shapes (what the Rust API actually returns)
// ---------------------------------------------------------------------------

interface ServerAttributeInfo {
  name: string;
  value_types: string[];
  required: boolean;
  cardinality: number;
}

interface ServerEntityType {
  name: string;
  attributes: Record<string, ServerAttributeInfo>;
  references: { attribute: string; target_type: string; cardinality: number }[];
  entity_count: number;
}

interface ServerSchema {
  entity_types: Record<string, ServerEntityType>;
  as_of_tx: number;
}

interface ServerDataListResponse {
  data: ServerQueryRow[];
  cursor: string | null;
  has_more: boolean;
}

interface ServerQueryRow {
  entity_id: string;
  attributes: Record<string, unknown>;
  nested: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * `GET /health` -- unauthenticated health check.
 * Returns `true` when the server is reachable and healthy.
 */
export async function fetchHealth(): Promise<boolean> {
  try {
    await publicFetch("/health");
    return true;
  } catch {
    return false;
  }
}

/**
 * `GET /api/admin/schema` -- fetch the full inferred schema.
 * Transforms the Rust `Schema` shape into the admin dashboard's
 * `EntityType[]` format so the UI code stays unchanged.
 */
export async function fetchSchema(): Promise<EntityType[]> {
  const schema = await apiFetch<ServerSchema>("/api/admin/schema");

  return Object.values(schema.entity_types).map((et) => {
    const fields: EntityField[] = [
      // Synthetic _id field (always present)
      {
        name: "_id",
        type: `Id<${et.name}>`,
        required: true,
        indexed: true,
        unique: true,
      },
      ...Object.values(et.attributes).filter(attr => attr.name !== ":db/type").map((attr) => ({
        name: attr.name.startsWith(`${et.name}/`) ? attr.name.slice(et.name.length + 1) : attr.name,
        type: attr.value_types.join(" | ") || "unknown",
        required: attr.required,
        indexed: false,
        unique: false,
      })),
    ];

    return {
      name: et.name,
      count: et.entity_count,
      fields,
      references: et.references,
    };
  });
}

/**
 * `GET /api/data/:entity` -- list entities of a given type.
 * Transforms the server's `QueryResultRow[]` into flat `EntityRecord[]`
 * that the DataTable component expects.
 */
export async function fetchEntities(
  type: string,
  limit = 100,
): Promise<{ data: EntityRecord[]; hasMore: boolean }> {
  const res = await apiFetch<ServerDataListResponse>(
    `/api/data/${encodeURIComponent(type)}?limit=${limit}`,
  );

  const data: EntityRecord[] = res.data.map((row) => {
    // Flatten: strip the `type/` prefix from attribute keys and merge
    const record: EntityRecord = {
      _id: row.entity_id,
      _creationTime: Date.now(), // server doesn't expose this yet
    };
    for (const [key, value] of Object.entries(row.attributes)) {
      // Attributes come as "users/email" — strip the entity prefix
      if (key === ":db/type") continue;
      const shortKey = key.includes("/") ? key.split("/").pop()! : key; // internal, skip
      record[shortKey] = value;
    }
    return record;
  });

  return { data, hasMore: res.has_more };
}

/**
 * `POST /api/data/:entity` -- create a new entity.
 */
export async function createEntity(
  type: string,
  data: Record<string, unknown>,
): Promise<{ id: string }> {
  const res = await apiFetch<{ id: string }>(
    `/api/data/${encodeURIComponent(type)}`,
    {
      method: "POST",
      body: JSON.stringify(data),
    },
  );
  return res;
}

/**
 * `DELETE /api/data/:entity/:id` -- delete an entity by UUID.
 */
export async function deleteEntity(type: string, id: string): Promise<void> {
  await apiFetch(
    `/api/data/${encodeURIComponent(type)}/${encodeURIComponent(id)}`,
    { method: "DELETE" },
  );
}

/**
 * `POST /api/query` -- run a DarshJQL query object.
 */
export async function queryDarshJQL(
  query: Record<string, unknown>,
): Promise<EntityRecord[]> {
  const res = await apiFetch<{ data: ServerQueryRow[] }>("/api/query", {
    method: "POST",
    body: JSON.stringify({ query }),
  });

  return res.data.map((row) => {
    const record: EntityRecord = {
      _id: row.entity_id,
      _creationTime: Date.now(),
    };
    for (const [key, value] of Object.entries(row.attributes)) {
      if (key === ":db/type") continue;
      const shortKey = key.includes("/") ? key.split("/").pop()! : key;
      record[shortKey] = value;
    }
    return record;
  });
}

// ---------------------------------------------------------------------------
// Auth Users
// ---------------------------------------------------------------------------

export interface SignupRequest {
  email: string;
  password: string;
  name?: string;
}

export interface SignupResponse {
  user_id: string;
  email: string;
  access_token: string;
  refresh_token: string;
  expires_in: number;
  token_type: string;
}

/**
 * `POST /api/auth/signup` -- create a new user account.
 */
export async function createUser(body: SignupRequest): Promise<SignupResponse> {
  return apiFetch<SignupResponse>("/api/auth/signup", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

// ---------------------------------------------------------------------------
// Admin: Functions
// ---------------------------------------------------------------------------

export interface ServerFunctionInfo {
  name: string;
  export_name: string;
  file_path: string;
  kind: string;
  description: string | null;
  args_schema: Record<string, unknown> | null;
}

export interface AdminFunctionsResponse {
  functions: ServerFunctionInfo[];
  count: number;
}

/**
 * `GET /api/admin/functions` -- list registered server-side functions.
 * Wired to the function registry; returns actual registered functions.
 */
export async function fetchFunctions(): Promise<AdminFunctionsResponse> {
  return apiFetch<AdminFunctionsResponse>("/api/admin/functions");
}

// ---------------------------------------------------------------------------
// Admin: Cache stats
// ---------------------------------------------------------------------------

export interface CacheStats {
  cache: Record<string, unknown>;
}

/**
 * `GET /api/admin/cache` -- hot-cache statistics (size, hit/miss, evictions).
 */
export async function fetchCacheStats(): Promise<CacheStats> {
  return apiFetch<CacheStats>("/api/admin/cache");
}

// ---------------------------------------------------------------------------
// Admin: Audit chain
// ---------------------------------------------------------------------------

export interface AuditChainResult {
  valid: boolean;
  total_transactions: number;
  first_broken_tx: number | null;
  detail: string | null;
}

/**
 * `GET /api/admin/audit/chain` -- verify the full audit hash chain.
 */
export async function fetchAuditChain(): Promise<AuditChainResult> {
  return apiFetch<AuditChainResult>("/api/admin/audit/chain");
}

// ---------------------------------------------------------------------------
// Admin: Sessions
// ---------------------------------------------------------------------------

export interface AdminSessionsResponse {
  sessions: unknown[];
  count: number;
}

/**
 * `GET /api/admin/sessions` -- list active sync sessions.
 */
export async function fetchSessions(): Promise<AdminSessionsResponse> {
  return apiFetch<AdminSessionsResponse>("/api/admin/sessions");
}

// ---------------------------------------------------------------------------
// Health (detailed)
// ---------------------------------------------------------------------------

export interface HealthResponse {
  status: string;
  service: string;
  version: string;
  uptime_secs: number;
  pool: {
    size: number;
    idle: number;
    active: number;
    max: number;
  };
  pool_stats: Record<string, unknown>;
  websockets: {
    active_connections: number;
  };
  triples: number;
  database: string;
}

/**
 * `GET /health` -- detailed health check (unauthenticated).
 * Returns the full health response object instead of just a boolean.
 */
export async function fetchHealthDetailed(): Promise<HealthResponse> {
  const data = await publicFetch<HealthResponse>("/health/full");
  if (typeof data?.triples !== "number" || typeof data?.uptime_secs !== "number" || !data?.pool || !data?.websockets) {
    throw new Error("Server returned an incomplete health response.");
  }
  return data;
}

// ---------------------------------------------------------------------------
// Admin: Storage
// ---------------------------------------------------------------------------

export interface StorageFileInfo {
  id: string;
  name: string;
  path: string;
  size: number;
  mimeType: string;
  uploadedAt: number;
  modifiedAt: number;
  metadata: Record<string, string>;
}

export interface AdminStorageResponse {
  files: StorageFileInfo[];
  count: number;
}

/**
 * `GET /api/admin/storage` -- list files in storage.
 */
export async function fetchStorageFiles(
  limit = 200,
): Promise<AdminStorageResponse> {
  return apiFetch<AdminStorageResponse>(
    `/api/admin/storage?limit=${limit}`,
  );
}

// ---------------------------------------------------------------------------
// Admin: Auth Users (via data API)
// ---------------------------------------------------------------------------

export interface AuthUserRecord {
  id: string;
  email: string;
  name: string;
  role: string;
  created_at: string;
}

/**
 * Fetch auth users by querying the `users` entity type via the data API,
 * then enrich with session data from the sessions admin endpoint.
 */
export async function fetchAuthUsers(): Promise<{
  users: AuthUserRecord[];
  sessions: AdminSessionsResponse;
}> {
  const [entityResult, sessions] = await Promise.all([
    fetchEntities("users", 200).catch(() => ({ data: [], hasMore: false })),
    fetchSessions().catch(() => ({ sessions: [], count: 0 })),
  ]);

  const users: AuthUserRecord[] = entityResult.data.map((rec) => ({
    id: rec._id,
    email: (rec.email as string) ?? "",
    name: (rec.name as string) ?? "",
    role: (rec.role as string) ?? "viewer",
    created_at: (rec.created_at as string) ?? new Date(rec._creationTime).toISOString(),
  }));

  return { users, sessions };
}
