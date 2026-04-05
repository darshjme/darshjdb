/**
 * Convex compatibility layer for DarshanDB.
 *
 * Wraps the DarshanDB client with Convex-style API aliases so teams
 * migrating from Convex can adopt DarshanDB incrementally without
 * rewriting every call site at once.
 *
 * @module convex-compat
 *
 * @example
 * ```ts
 * import { DarshanDB } from '@darshan/client';
 * import { ConvexCompat } from '@darshan/client/convex-compat';
 *
 * const db = new DarshanDB({ serverUrl: 'http://localhost:7700', appId: 'app' });
 * const compat = new ConvexCompat(db);
 *
 * // Convex-style queries
 * const todos = await compat.query('todos', { done: false });
 *
 * // Convex-style mutations
 * await compat.mutation('todos', { title: 'New task', done: false });
 *
 * // Live subscription (Convex-style watch)
 * const unsub = compat.watch('todos', { done: false }, (results) => {
 *   console.log('Live update:', results);
 * });
 * ```
 */

import type { DarshanDB } from './client.js';
import { QueryBuilder } from './query.js';
import { transact, generateId } from './transaction.js';
import type {
  QueryResult,
  Unsubscribe,
  WhereOp,
} from './types.js';

/* -------------------------------------------------------------------------- */
/*  Types                                                                      */
/* -------------------------------------------------------------------------- */

/**
 * Convex-style filter object.
 * Keys are field names, values are either exact-match values or operator objects.
 *
 * @example
 * ```ts
 * { done: false }                          // equality
 * { priority: { $gt: 3 } }                // comparison
 * { status: { $in: ['active', 'pending'] } }  // set membership
 * ```
 */
export type ConvexFilter = Record<string, unknown>;

/** Options for Convex-style queries. */
export interface ConvexQueryOptions {
  order?: Record<string, 'asc' | 'desc'>;
  limit?: number;
  offset?: number;
}

/** Callback for Convex-style watch (live queries). */
export type ConvexWatchCallback<T = Record<string, unknown>> = (
  results: T[],
) => void;

/* -------------------------------------------------------------------------- */
/*  Operator mapping                                                           */
/* -------------------------------------------------------------------------- */

const OPERATOR_MAP: Record<string, WhereOp> = {
  $eq: '=',
  $ne: '!=',
  $gt: '>',
  $gte: '>=',
  $lt: '<',
  $lte: '<=',
  $in: 'in',
  $nin: 'not-in',
  $contains: 'contains',
  $startsWith: 'starts-with',
};

/* -------------------------------------------------------------------------- */
/*  ConvexCompat                                                               */
/* -------------------------------------------------------------------------- */

/**
 * Convex compatibility wrapper around a DarshanDB client.
 *
 * Provides `query()`, `mutation()`, `patch()`, `remove()`, and `watch()`
 * methods that mirror the Convex developer experience while delegating
 * to native DarshanDB operations.
 */
export class ConvexCompat {
  private _client: DarshanDB;

  constructor(client: DarshanDB) {
    this._client = client;
  }

  /* -- Query -------------------------------------------------------------- */

  /**
   * Query a table, similar to Convex's `db.query(tableName)`.
   *
   * @param tableName - The collection/table to query.
   * @param filter    - Optional Convex-style filter object.
   * @param options   - Optional ordering, limit, offset.
   * @returns Array of matching documents.
   */
  async query<T = Record<string, unknown>>(
    tableName: string,
    filter?: ConvexFilter,
    options?: ConvexQueryOptions,
  ): Promise<T[]> {
    const builder = this._buildQuery<T>(tableName, filter, options);
    const result: QueryResult<T> = await builder.exec();
    return result.data;
  }

  /* -- Mutation (insert) -------------------------------------------------- */

  /**
   * Insert a new document, similar to Convex's `db.insert(tableName, data)`.
   *
   * @param tableName - The collection/table to insert into.
   * @param data      - Document data (id auto-generated if not provided).
   * @returns The id of the inserted document.
   */
  async mutation(
    tableName: string,
    data: Record<string, unknown>,
  ): Promise<string> {
    const id = (data['_id'] as string) ?? generateId();
    const cleanData = { ...data };
    delete cleanData['_id'];

    await transact(this._client, (tx) => {
      tx[tableName]![id]!.set(cleanData);
    });

    return id;
  }

  /* -- Patch (partial update) --------------------------------------------- */

  /**
   * Partially update a document, similar to Convex's `db.patch(id, fields)`.
   *
   * @param tableName - The collection/table.
   * @param id        - Document id to update.
   * @param fields    - Fields to merge.
   */
  async patch(
    tableName: string,
    id: string,
    fields: Record<string, unknown>,
  ): Promise<void> {
    await transact(this._client, (tx) => {
      tx[tableName]![id]!.merge(fields);
    });
  }

  /* -- Delete ------------------------------------------------------------- */

  /**
   * Delete a document, similar to Convex's `db.delete(id)`.
   *
   * @param tableName - The collection/table.
   * @param id        - Document id to delete.
   */
  async remove(
    tableName: string,
    id: string,
  ): Promise<void> {
    await transact(this._client, (tx) => {
      tx[tableName]![id]!.delete();
    });
  }

  /* -- Watch (live subscription) ------------------------------------------ */

  /**
   * Subscribe to live query updates, similar to Convex's `useQuery` reactivity.
   *
   * @param tableName - The collection/table to watch.
   * @param filter    - Optional Convex-style filter object.
   * @param callback  - Called with the full result set on every change.
   * @param options   - Optional ordering, limit, offset.
   * @returns An unsubscribe function.
   */
  watch<T = Record<string, unknown>>(
    tableName: string,
    filter: ConvexFilter | undefined,
    callback: ConvexWatchCallback<T>,
    options?: ConvexQueryOptions,
  ): Unsubscribe {
    const builder = this._buildQuery<T>(tableName, filter, options);
    return builder.subscribe((result) => {
      callback(result.data);
    });
  }

  /* -- Generate ID -------------------------------------------------------- */

  /**
   * Generate a new document id, equivalent to Convex's auto-generated `_id`.
   */
  id(): string {
    return generateId();
  }

  /* -- Internal ----------------------------------------------------------- */

  private _buildQuery<T>(
    tableName: string,
    filter?: ConvexFilter,
    options?: ConvexQueryOptions,
  ): QueryBuilder<T> {
    const builder = new QueryBuilder<T>(this._client, tableName);

    // Apply filters
    if (filter) {
      for (const [field, value] of Object.entries(filter)) {
        if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
          // Operator object like { $gt: 3 }
          const opObj = value as Record<string, unknown>;
          for (const [opKey, opVal] of Object.entries(opObj)) {
            const mapped = OPERATOR_MAP[opKey];
            if (mapped) {
              builder.where(field, mapped, opVal);
            }
          }
        } else {
          // Exact equality
          builder.where(field, '=', value);
        }
      }
    }

    // Apply ordering
    if (options?.order) {
      for (const [field, dir] of Object.entries(options.order)) {
        builder.orderBy(field, dir);
      }
    }

    if (options?.limit !== undefined) {
      builder.limit(options.limit);
    }
    if (options?.offset !== undefined) {
      builder.offset(options.offset);
    }

    return builder;
  }
}
