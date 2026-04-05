/**
 * @module @darshan/nextjs/server
 *
 * Server-side utilities for DarshanDB in Next.js Server Components and Server Actions.
 * Uses the REST API with admin token, initialized from environment variables.
 *
 * @example
 * ```tsx
 * // app/page.tsx (Server Component)
 * import { queryServer } from '@darshan/nextjs/server';
 *
 * export default async function Page() {
 *   const data = await queryServer({ todos: { $where: { done: false } } });
 *   return <TodoList items={data.todos} />;
 * }
 * ```
 *
 * @example
 * ```tsx
 * // app/actions.ts (Server Action)
 * 'use server';
 * import { mutateServer } from '@darshan/nextjs/server';
 *
 * export async function createTodo(title: string) {
 *   return mutateServer([{ entity: 'todos', op: 'set', data: { title, done: false } }]);
 * }
 * ```
 */

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** DarshanQL query object — the same format used by client SDKs. */
export type DarshanQuery = Record<string, unknown>;

/** Options controlling caching and revalidation behavior. */
export interface QueryServerOptions {
  /**
   * Revalidation period in seconds for ISR.
   * - `false` — no caching (default)
   * - `number` — revalidate after N seconds
   */
  revalidate?: number | false;

  /** Cache tags for on-demand revalidation via `revalidateTag()`. */
  tags?: string[];
}

/** A mutation operation to send to the server. */
export interface MutationOp {
  entity: string;
  id?: string;
  op: 'set' | 'merge' | 'delete';
  data?: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Admin configuration
// ---------------------------------------------------------------------------

function getConfig(): { url: string; token: string } {
  const url = process.env.DARSHAN_URL;
  const token = process.env.DARSHAN_ADMIN_TOKEN;

  if (!url) {
    throw new Error(
      '[DarshanDB] Missing DARSHAN_URL environment variable. ' +
        'Set it to your DarshanDB server URL (e.g. http://localhost:7700).',
    );
  }

  if (!token) {
    throw new Error(
      '[DarshanDB] Missing DARSHAN_ADMIN_TOKEN environment variable.',
    );
  }

  return { url, token };
}

// ---------------------------------------------------------------------------
// queryServer
// ---------------------------------------------------------------------------

/**
 * Execute a DarshanQL query from a Server Component via the REST API.
 *
 * @param query - DarshanQL query object.
 * @param options - Caching / revalidation options.
 * @returns The query result.
 *
 * @example
 * ```tsx
 * const data = await queryServer(
 *   { posts: { $where: { published: true }, $limit: 20 } },
 *   { revalidate: 60, tags: ['posts'] },
 * );
 * ```
 */
export async function queryServer<T = Record<string, unknown>>(
  query: DarshanQuery,
  options: QueryServerOptions = {},
): Promise<T> {
  const { url, token } = getConfig();

  const fetchOptions: RequestInit & { next?: Record<string, unknown> } = {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify(query),
  };

  // Next.js fetch cache integration
  const next: Record<string, unknown> = {};
  if (options.revalidate !== undefined) {
    next.revalidate = options.revalidate;
  }
  if (options.tags?.length) {
    next.tags = options.tags;
  }
  if (Object.keys(next).length > 0) {
    fetchOptions.next = next;
  }

  const response = await fetch(`${url}/api/query`, fetchOptions);

  if (!response.ok) {
    const body = await response.text();
    throw new Error(`[DarshanDB] Query failed (${response.status}): ${body}`);
  }

  return response.json() as Promise<T>;
}

// ---------------------------------------------------------------------------
// mutateServer
// ---------------------------------------------------------------------------

/**
 * Execute mutations against DarshanDB from a Server Action.
 *
 * @param ops - Array of mutation operations.
 * @returns The mutation result from the server.
 *
 * @example
 * ```ts
 * 'use server';
 * await mutateServer([
 *   { entity: 'posts', op: 'set', data: { title: 'Hello', published: true } },
 * ]);
 * ```
 */
export async function mutateServer<T = unknown>(ops: MutationOp[]): Promise<T> {
  const { url, token } = getConfig();

  const response = await fetch(`${url}/api/mutate`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({ ops }),
  });

  if (!response.ok) {
    const body = await response.text();
    throw new Error(`[DarshanDB] Mutation failed (${response.status}): ${body}`);
  }

  return response.json() as Promise<T>;
}

// ---------------------------------------------------------------------------
// callFunction
// ---------------------------------------------------------------------------

/**
 * Call a DarshanDB server function from a Server Action.
 *
 * @param name - Function name.
 * @param args - Arguments to pass.
 * @returns The function result.
 */
export async function callFunction<T = unknown>(
  name: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  const { url, token } = getConfig();

  const response = await fetch(`${url}/api/fn/${encodeURIComponent(name)}`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify(args),
  });

  if (!response.ok) {
    const body = await response.text();
    throw new Error(`[DarshanDB] Function "${name}" failed (${response.status}): ${body}`);
  }

  return response.json() as Promise<T>;
}
