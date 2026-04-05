/**
 * DarshanDB REST API client.
 *
 * Talks to the live server when available, with every function returning
 * a typed result. Callers handle fallback to mock data themselves.
 *
 * The base URL is configurable via `VITE_DARSHAN_URL` (defaults to
 * `http://localhost:7700` for local development).
 */

import type { EntityType, EntityField, EntityRecord } from "../types";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const API_URL = import.meta.env.VITE_DARSHAN_URL || "http://localhost:7700";

/**
 * Admin bearer token. In production this would come from a real auth flow;
 * for now the server's `require_admin_role` is a stub that accepts any
 * non-empty token, so a dev placeholder is fine.
 */
const AUTH_TOKEN =
  import.meta.env.VITE_DARSHAN_TOKEN || "darshan-admin-dev-token";

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

async function apiFetch<T>(
  path: string,
  init?: RequestInit,
): Promise<T> {
  const url = `${API_URL}${path}`;
  const headers: Record<string, string> = {
    Accept: "application/json",
    ...(init?.body ? { "Content-Type": "application/json" } : {}),
    Authorization: `Bearer ${AUTH_TOKEN}`,
  };

  const res = await fetch(url, {
    ...init,
    headers: { ...headers, ...(init?.headers as Record<string, string>) },
  });

  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new ApiError(res.status, body || res.statusText, path);
  }

  return res.json() as Promise<T>;
}

/** Lightweight fetch without auth — used for the /health endpoint. */
async function publicFetch<T>(path: string): Promise<T> {
  const res = await fetch(`${API_URL}${path}`, {
    headers: { Accept: "application/json" },
  });
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  return res.json() as Promise<T>;
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

export class ApiError extends Error {
  constructor(
    public status: number,
    public body: string,
    public path: string,
  ) {
    super(`API ${status} on ${path}: ${body}`);
    this.name = "ApiError";
  }
}

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
      ...Object.values(et.attributes).map((attr) => ({
        name: attr.name,
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
      const shortKey = key.includes("/") ? key.split("/").pop()! : key;
      if (shortKey === ":db/type") continue; // internal, skip
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
 * `POST /api/query` -- run a DarshanQL query object.
 */
export async function queryDarshanQL(
  query: Record<string, unknown>,
): Promise<EntityRecord[]> {
  const res = await apiFetch<{ data: ServerQueryRow[] }>("/api/query", {
    method: "POST",
    body: JSON.stringify(query),
  });

  return res.data.map((row) => {
    const record: EntityRecord = {
      _id: row.entity_id,
      _creationTime: Date.now(),
    };
    for (const [key, value] of Object.entries(row.attributes)) {
      const shortKey = key.includes("/") ? key.split("/").pop()! : key;
      if (shortKey === ":db/type") continue;
      record[shortKey] = value;
    }
    return record;
  });
}
