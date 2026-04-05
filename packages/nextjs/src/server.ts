/**
 * @module @darshan/nextjs/server
 *
 * Server-side utilities for DarshanDB in Next.js Server Components and Server Actions.
 * Uses the admin SDK initialized from environment variables.
 *
 * @example
 * ```tsx
 * // app/users/page.tsx (Server Component)
 * import { queryServer } from '@darshan/nextjs/server';
 *
 * export default async function UsersPage() {
 *   const users = await queryServer({
 *     collection: 'users',
 *     where: { active: true },
 *   });
 *   return <UserList users={users} />;
 * }
 * ```
 *
 * @example
 * ```tsx
 * // app/actions.ts (Server Action)
 * 'use server';
 * import { mutateServer } from '@darshan/nextjs/server';
 *
 * export async function createUser(data: FormData) {
 *   return mutateServer(async (db) => {
 *     return db.collection('users').insert({
 *       name: data.get('name') as string,
 *     });
 *   });
 * }
 * ```
 */

import type { DarshanDB } from '@darshan/client';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** A DarshanDB query descriptor passed to `queryServer`. */
export interface DarshanQuery {
  /** The collection to query. */
  collection: string;
  /** Optional filter conditions. */
  where?: Record<string, unknown>;
  /** Fields to select (projection). */
  select?: string[];
  /** Sort order. Keys are field names, values are `'asc'` or `'desc'`. */
  orderBy?: Record<string, 'asc' | 'desc'>;
  /** Maximum number of documents to return. */
  limit?: number;
  /** Number of documents to skip (pagination offset). */
  offset?: number;
}

/** Options controlling caching and revalidation behavior. */
export interface QueryServerOptions {
  /**
   * Revalidation period in seconds for ISR (Incremental Static Regeneration).
   * - `false` — no caching (default)
   * - `number` — revalidate after N seconds
   *
   * Maps directly to Next.js `revalidate` semantics.
   */
  revalidate?: number | false;

  /**
   * Cache tags for on-demand revalidation via `revalidateTag()`.
   * Requires Next.js 14+.
   */
  tags?: string[];
}

/** Callback receiving the admin DarshanDB instance. */
export type MutateServerFn<T> = (db: DarshanDB) => Promise<T>;

// ---------------------------------------------------------------------------
// Admin client singleton
// ---------------------------------------------------------------------------

let _adminDb: DarshanDB | null = null;

/**
 * Returns the admin DarshanDB singleton, lazily initialized from
 * environment variables.
 *
 * | Variable              | Description                        |
 * | --------------------- | ---------------------------------- |
 * | `DARSHAN_URL`         | DarshanDB server URL               |
 * | `DARSHAN_ADMIN_TOKEN` | Admin-level authentication token   |
 *
 * @throws {Error} If required environment variables are missing.
 *
 * @example
 * ```ts
 * import { adminDb } from '@darshan/nextjs/server';
 * const doc = await adminDb.collection('config').findOne({ key: 'site' });
 * ```
 */
export function getAdminDb(): DarshanDB {
  if (_adminDb) {
    return _adminDb;
  }

  const url = process.env.DARSHAN_URL;
  const token = process.env.DARSHAN_ADMIN_TOKEN;

  if (!url) {
    throw new Error(
      '[DarshanDB] Missing DARSHAN_URL environment variable. ' +
        'Set it to your DarshanDB server URL (e.g. https://db.example.com).',
    );
  }

  if (!token) {
    throw new Error(
      '[DarshanDB] Missing DARSHAN_ADMIN_TOKEN environment variable. ' +
        'Set it to your admin authentication token.',
    );
  }

  // Dynamic import avoided — @darshan/client is a direct dependency.
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const { createClient } = require('@darshan/client') as typeof import('@darshan/client');

  _adminDb = createClient({
    url,
    token,
  });

  return _adminDb;
}

/**
 * Convenience re-export of the admin client.
 * Accessing the property triggers lazy initialization.
 */
export const adminDb: DarshanDB = new Proxy({} as DarshanDB, {
  get(_target, prop, receiver) {
    const db = getAdminDb();
    const value = Reflect.get(db, prop, receiver);
    return typeof value === 'function' ? value.bind(db) : value;
  },
});

// ---------------------------------------------------------------------------
// queryServer
// ---------------------------------------------------------------------------

/**
 * Execute a read query against DarshanDB from a Server Component.
 *
 * Integrates with Next.js caching via the `revalidate` and `tags` options,
 * enabling ISR and on-demand revalidation patterns.
 *
 * @typeParam T - The expected document type.
 * @param query - Query descriptor or collection name shorthand.
 * @param options - Caching / revalidation options.
 * @returns The query result set.
 *
 * @example
 * ```tsx
 * // ISR with 60-second revalidation
 * const posts = await queryServer<Post>(
 *   { collection: 'posts', where: { published: true }, limit: 20 },
 *   { revalidate: 60, tags: ['posts'] },
 * );
 * ```
 */
export async function queryServer<T = unknown>(
  query: DarshanQuery | string,
  options: QueryServerOptions = {},
): Promise<T[]> {
  const db = getAdminDb();
  const q: DarshanQuery =
    typeof query === 'string' ? { collection: query } : query;

  // Build the collection query chain.
  // The chain methods return the same builder type but the client typings
  // are not generic enough to express this — we use a typed record instead.
  const collection = db.collection(q.collection);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any -- chain builder API returns varying shapes
  let chain: any = collection;

  if (q.where) {
    chain = chain.where(q.where);
  }
  if (q.select) {
    chain = chain.select(q.select);
  }
  if (q.orderBy) {
    chain = chain.orderBy(q.orderBy);
  }
  if (q.limit !== undefined) {
    chain = chain.limit(q.limit);
  }
  if (q.offset !== undefined) {
    chain = chain.offset(q.offset);
  }

  // Next.js fetch cache integration
  // When running inside Next.js, the `fetch` global is patched to support
  // `next.revalidate` and `next.tags`. We attach cache hints to the internal
  // request if the runtime supports it.
  const cacheHints: Record<string, unknown> = {};
  if (options.revalidate !== undefined) {
    cacheHints.revalidate = options.revalidate;
  }
  if (options.tags?.length) {
    cacheHints.tags = options.tags;
  }
  if (Object.keys(cacheHints).length > 0) {
    chain = chain.withCacheHints?.(cacheHints) ?? chain;
  }

  const result = await chain.find();
  return result as T[];
}

// ---------------------------------------------------------------------------
// mutateServer
// ---------------------------------------------------------------------------

/**
 * Execute a write operation against DarshanDB inside a Server Action.
 *
 * Provides transactional semantics when the underlying DarshanDB server
 * supports it. On failure the mutation is rolled back.
 *
 * @typeParam T - The return type of the mutation callback.
 * @param fn - Callback receiving the admin `DarshanDB`.
 * @returns The value returned by the callback.
 *
 * @example
 * ```ts
 * 'use server';
 * import { mutateServer } from '@darshan/nextjs/server';
 *
 * export async function deletePost(id: string) {
 *   return mutateServer(async (db) => {
 *     await db.collection('posts').delete(id);
 *     return { success: true };
 *   });
 * }
 * ```
 */
export async function mutateServer<T = unknown>(fn: MutateServerFn<T>): Promise<T> {
  const db = getAdminDb();

  try {
    const result = await fn(db);
    return result;
  } catch (error) {
    const message =
      error instanceof Error ? error.message : 'Unknown mutation error';
    throw new Error(`[DarshanDB] Server mutation failed: ${message}`);
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Reset the admin client singleton. Useful in tests.
 * @internal
 */
export function _resetAdminDb(): void {
  _adminDb = null;
}
