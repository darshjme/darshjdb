/**
 * Type-safe DarshJQL query builder with deduplication and subscriptions.
 *
 * @module query
 */

import type { DarshJDB } from './client.js';
import type {
  QueryDescriptor,
  QueryResult,
  WhereClause,
  WhereOp,
  OrderClause,
  OrderDirection,
  SubscriptionCallback,
  Unsubscribe,
  ServerMessage,
} from './types.js';

/* -------------------------------------------------------------------------- */
/*  Helpers                                                                   */
/* -------------------------------------------------------------------------- */

/**
 * Produce a stable hash string from a query descriptor for deduplication.
 * Uses a simple JSON serialisation since query descriptors are small.
 */
function hashQuery(desc: QueryDescriptor): string {
  return JSON.stringify(desc, Object.keys(desc).sort());
}

/* -------------------------------------------------------------------------- */
/*  Query Builder                                                             */
/* -------------------------------------------------------------------------- */

/**
 * Fluent, type-safe query builder for DarshJQL.
 *
 * @typeParam T - The expected shape of each document in the result set.
 *
 * @example
 * ```ts
 * const users = await db.query<User>('users')
 *   .where('age', '>=', 18)
 *   .orderBy('createdAt', 'desc')
 *   .limit(20)
 *   .exec();
 * ```
 */
export class QueryBuilder<T = Record<string, unknown>> {
  private _privateCollection: string;
  private _privateWheres: WhereClause[] = [];
  private _privateOrders: OrderClause[] = [];
  private _privateLimit?: number;
  private _privateOffset?: number;
  private _privateSelect?: string[];
  private _privateClient: DarshJDB;

  constructor(client: DarshJDB, collection: string) {
    this._privateClient = client;
    this._privateCollection = collection;
  }

  /**
   * Add a filter condition.
   *
   * @param field - Document field path (dot-notation supported).
   * @param op    - Comparison operator.
   * @param value - Value to compare against.
   */
  where(field: string, op: WhereOp, value: unknown): this {
    this._privateWheres.push({ field, op, value });
    return this;
  }

  /**
   * Add a sort clause.
   *
   * @param field     - Document field path.
   * @param direction - `'asc'` or `'desc'` (default `'asc'`).
   */
  orderBy(field: string, direction: OrderDirection = 'asc'): this {
    this._privateOrders.push({ field, direction });
    return this;
  }

  /**
   * Limit the number of results.
   *
   * @param n - Maximum number of documents to return.
   */
  limit(n: number): this {
    this._privateLimit = n;
    return this;
  }

  /**
   * Skip a number of results (for pagination).
   *
   * @param n - Number of documents to skip.
   */
  offset(n: number): this {
    this._privateOffset = n;
    return this;
  }

  /**
   * Select a subset of fields to return.
   *
   * @param fields - Field names to include.
   */
  select(...fields: (keyof T & string)[]): this {
    this._privateSelect = fields;
    return this;
  }

  /**
   * Build the query descriptor (for introspection or manual use).
   */
  toDescriptor(): QueryDescriptor {
    return {
      collection: this._privateCollection,
      ...(this._privateWheres.length > 0 && { where: this._privateWheres }),
      ...(this._privateOrders.length > 0 && { order: this._privateOrders }),
      ...(this._privateLimit !== undefined && { limit: this._privateLimit }),
      ...(this._privateOffset !== undefined && { offset: this._privateOffset }),
      ...(this._privateSelect && { select: this._privateSelect }),
    };
  }

  /**
   * Compute a stable hash of this query for deduplication.
   */
  hash(): string {
    return hashQuery(this.toDescriptor());
  }

  /**
   * Execute the query once and return the result set.
   */
  async exec(): Promise<QueryResult<T>> {
    return queryOnce<T>(this._privateClient, this.toDescriptor());
  }

  /**
   * Subscribe to live updates matching this query.
   *
   * @param callback - Invoked whenever the result set changes.
   * @returns An unsubscribe function.
   */
  subscribe(callback: SubscriptionCallback<T>): Unsubscribe {
    return subscribe<T>(this._privateClient, this.toDescriptor(), callback);
  }
}

/* -------------------------------------------------------------------------- */
/*  Active subscriptions (deduplication map)                                  */
/* -------------------------------------------------------------------------- */

/** Internal tracking for deduplication. */
interface ActiveSubscription<T> {
  descriptor: QueryDescriptor;
  callbacks: Set<SubscriptionCallback<T>>;
  subId: string;
  refCount: number;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- type-safe at call-sites
const _privateActiveSubs = new Map<string, ActiveSubscription<any>>();

let _privateSubCounter = 0;

/* -------------------------------------------------------------------------- */
/*  Public API                                                                */
/* -------------------------------------------------------------------------- */

/**
 * Execute a one-shot query against the server.
 *
 * @typeParam T - Expected document shape.
 * @param client     - DarshJDB client instance.
 * @param descriptor - Query descriptor.
 * @returns The query result set.
 */
export async function queryOnce<T = Record<string, unknown>>(
  client: DarshJDB,
  descriptor: QueryDescriptor,
): Promise<QueryResult<T>> {
  const resp = await client.send({
    type: 'query',
    payload: descriptor,
  });

  const payload = resp.payload as { data: T[]; txId: string };
  return { data: payload.data, txId: payload.txId };
}

/**
 * Subscribe to live query results.
 *
 * Queries are deduplicated by their hash: if two callers subscribe to an
 * identical query, only one server subscription is created.
 *
 * @typeParam T - Expected document shape.
 * @param client     - DarshJDB client instance.
 * @param descriptor - Query descriptor.
 * @param callback   - Invoked on every result update.
 * @returns An unsubscribe function.
 */
export function subscribe<T = Record<string, unknown>>(
  client: DarshJDB,
  descriptor: QueryDescriptor,
  callback: SubscriptionCallback<T>,
): Unsubscribe {
  const hash = hashQuery(descriptor);

  let sub = _privateActiveSubs.get(hash) as ActiveSubscription<T> | undefined;

  if (sub) {
    // Dedup: reuse existing server subscription.
    sub.callbacks.add(callback);
    sub.refCount++;
  } else {
    const subId = `sub_${(++_privateSubCounter).toString(36)}`;

    sub = {
      descriptor,
      callbacks: new Set([callback]),
      subId,
      refCount: 1,
    };

    _privateActiveSubs.set(hash, sub);

    // Wire up server subscription.
    client
      .send({ type: 'subscribe', payload: { subId, query: descriptor } })
      .catch((err) => {
        console.error('[DarshJDB] Subscription error:', err);
      });

    client.registerSubscriptionHandler(subId, (msg: ServerMessage) => {
      const active = _privateActiveSubs.get(hash);
      if (!active) return;
      const payload = msg.payload as { data: T[]; txId: string };
      const result: QueryResult<T> = { data: payload.data, txId: payload.txId };
      for (const cb of active.callbacks) {
        try {
          cb(result);
        } catch {
          /* subscriber errors must not break the notification loop */
        }
      }
    });
  }

  let unsubscribed = false;

  return () => {
    if (unsubscribed) return;
    unsubscribed = true;

    const active = _privateActiveSubs.get(hash);
    if (!active) return;

    active.callbacks.delete(callback);
    active.refCount--;

    if (active.refCount <= 0) {
      _privateActiveSubs.delete(hash);
      client.unregisterSubscriptionHandler(active.subId);
      client
        .send({
          type: 'unsubscribe',
          payload: { subId: active.subId },
        })
        .catch(() => {
          /* best effort */
        });
    }
  };
}
