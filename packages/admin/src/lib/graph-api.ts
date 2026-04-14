/**
 * Graph API client for the DarshJDB admin dashboard.
 *
 * Thin wrapper over the server's `/api/graph/*` endpoints that mirrors
 * the shapes in `packages/server/src/graph/`. Re-uses the bearer-token
 * pattern from `./api.ts` so authentication is identical to every other
 * admin page.
 */

const API_URL = import.meta.env.VITE_DDB_URL || "http://localhost:7700";
const AUTH_TOKEN = import.meta.env.VITE_DDB_TOKEN || "ddb-admin-dev-token";

async function graphFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${API_URL}${path}`, {
    ...init,
    headers: {
      Accept: "application/json",
      ...(init?.body ? { "Content-Type": "application/json" } : {}),
      Authorization: `Bearer ${AUTH_TOKEN}`,
      ...(init?.headers as Record<string, string> | undefined),
    },
  });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`Graph API ${res.status} on ${path}: ${body || res.statusText}`);
  }
  return res.json() as Promise<T>;
}

// ---------------------------------------------------------------------------
// Server shapes (must stay in sync with `packages/server/src/graph/traverse.rs`)
// ---------------------------------------------------------------------------

export type Direction = "out" | "in" | "both";
export type TraversalAlgorithm = "bfs" | "dfs" | "shortestpath";

/** Mirrors `graph::traverse::TraversalConfig` on the server. */
export interface TraversalRequest {
  /** Starting node in `table:id` format. */
  start: string;
  direction?: Direction;
  edge_type?: string;
  max_depth?: number;
  max_nodes?: number;
  algorithm?: TraversalAlgorithm;
  target?: string;
}

export interface ServerRecordId {
  table: string;
  id: string;
}

export interface ServerEdgeSummary {
  id: string;
  edge_type: string;
  from: string;
  to: string;
}

export interface ServerTraversalNode {
  record: ServerRecordId;
  depth: number;
  via_edge?: ServerEdgeSummary;
}

export interface ServerTraversalResult {
  nodes: ServerTraversalNode[];
  edges_examined: number;
  truncated: boolean;
}

// ---------------------------------------------------------------------------
// Client graph shapes (what `react-force-graph-2d` consumes)
// ---------------------------------------------------------------------------

export interface GraphNode {
  /** `table:id` composite key, used as node id. */
  id: string;
  table: string;
  entityId: string;
  depth: number;
  label: string;
}

export interface GraphEdge {
  id: string;
  source: string;
  target: string;
  relation: string;
}

export interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
  edgesExamined: number;
  truncated: boolean;
  fetchMs: number;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * `POST /api/graph/traverse` — BFS/DFS traversal from a start node.
 *
 * Transforms the server's `TraversalResult` into a flat
 * `{ nodes, edges }` shape suitable for force-directed rendering.
 */
export async function traverse(opts: TraversalRequest): Promise<GraphData> {
  const start = performance.now();

  const body: TraversalRequest = {
    start: opts.start,
    direction: opts.direction ?? "out",
    max_depth: opts.max_depth ?? 2,
    max_nodes: opts.max_nodes ?? 250,
    algorithm: opts.algorithm ?? "bfs",
  };
  if (opts.edge_type && opts.edge_type.trim().length > 0) {
    body.edge_type = opts.edge_type.trim();
  }
  if (opts.target) body.target = opts.target;

  const raw = await graphFetch<ServerTraversalResult>("/api/graph/traverse", {
    method: "POST",
    body: JSON.stringify(body),
  });

  const fetchMs = Math.round(performance.now() - start);

  const nodes: GraphNode[] = raw.nodes.map((n) => {
    const id = `${n.record.table}:${n.record.id}`;
    return {
      id,
      table: n.record.table,
      entityId: n.record.id,
      depth: n.depth,
      label: id,
    };
  });

  // Dedupe edges by id — `via_edge` can repeat on convergent paths.
  const edgeMap = new Map<string, GraphEdge>();
  for (const n of raw.nodes) {
    if (!n.via_edge) continue;
    if (edgeMap.has(n.via_edge.id)) continue;
    edgeMap.set(n.via_edge.id, {
      id: n.via_edge.id,
      source: n.via_edge.from,
      target: n.via_edge.to,
      relation: n.via_edge.edge_type,
    });
  }

  return {
    nodes,
    edges: Array.from(edgeMap.values()),
    edgesExamined: raw.edges_examined,
    truncated: raw.truncated,
    fetchMs,
  };
}

// ---------------------------------------------------------------------------
// Neighbour helpers — used by the "Expand" button in the node inspector.
// ---------------------------------------------------------------------------

interface ServerNeighboursResponse {
  record: string;
  direction?: string;
  edges: Array<{
    id: string;
    from: string;
    edge_type: string;
    to: string;
    data?: unknown;
    created_at?: string;
  }>;
  count: number;
}

/**
 * `GET /api/graph/neighbors/:table/:id` — one-hop neighbours (both directions).
 *
 * Used by the node inspector to expand a node without re-running the
 * full traversal from the root.
 */
export async function neighbours(
  table: string,
  id: string,
  edgeType?: string,
): Promise<GraphEdge[]> {
  const qs = edgeType ? `?edge_type=${encodeURIComponent(edgeType)}` : "";
  const raw = await graphFetch<ServerNeighboursResponse>(
    `/api/graph/neighbors/${encodeURIComponent(table)}/${encodeURIComponent(id)}${qs}`,
  );
  return raw.edges.map((e) => ({
    id: e.id,
    source: e.from,
    target: e.to,
    relation: e.edge_type,
  }));
}

/** Parse a `table:id` composite back into its parts. */
export function parseRecordId(id: string): { table: string; entityId: string } | null {
  const idx = id.indexOf(":");
  if (idx <= 0 || idx === id.length - 1) return null;
  return { table: id.slice(0, idx), entityId: id.slice(idx + 1) };
}
