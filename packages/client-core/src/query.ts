/**
 * Type-safe DarshJQL query builder with deduplication and subscriptions.
 *
 * @module query
 */

import type { DarshJDB } from './client.js';
import type {
  DarshJQL,
  DarshJQLOp,
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

/** Client operator -> DarshJQL operator. */
const OP_MAP: Partial<Record<WhereOp, DarshJQLOp>> = {
  '=': 'Eq',
  '!=': 'Neq',
  '>': 'Gt',
  '>=': 'Gte',
  '<': 'Lt',
  '<=': 'Lte',
  contains: 'Contains',
  'starts-with': 'Like',
};

/**
 * Translate a {@link QueryDescriptor} into the DarshJQL object the server's
 * parser expects (`{ type, $where, $order, $limit, $offset }`).
 *
 * @throws If the descriptor uses an operator the server does not implement.
 */
export function toDarshJQL(desc: QueryDescriptor): DarshJQL {
  const query: DarshJQL = { type: desc.collection };

  if (desc.where && desc.where.length > 0) {
    query.$where = desc.where.map((clause: WhereClause) => {
      const op = OP_MAP[clause.op];
      if (!op) {
        throw new Error(
          `Operator "${clause.op}" is not supported by the DarshJDB server`,
        );
      }
      return {
        attribute: clause.field,
        op,
        value:
          clause.op === 'starts-with' ? `${String(clause.value)}%` : clause.value,
      };
    });
  }

  if (desc.order && desc.order.length > 0) {
    query.$order = desc.order.map((clause: OrderClause) => ({
      attribute: clause.field,
      direction: clause.direction === 'desc' ? 'Desc' : 'Asc',
    }));
  }

  if (desc.limit !== undefined) query.$limit = desc.limit;
  if (desc.offset !== undefined) query.$offset = desc.offset;

  return query;
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
  /** Server-assigned subscription id; null until `sub-ok` arrives. */
  subId: string | null;
  refCount: number;
  /** Latest result set, replayed to late joiners and used as the diff base. */
  rows: Record<string, unknown>[];
  /** Removes the reconnect hook when the subscription is torn down. */
  disposeReconnect: (() => void) | null;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- type-safe at call-sites
const _privateActiveSubs = new Map<string, ActiveSubscription<any>>();

/* -------------------------------------------------------------------------- */
/*  Public API                                                                */
/* -------------------------------------------------------------------------- */

/**
 * Execute a one-shot query against the server.
 *
 * Uses `POST /api/query` when the client is in REST mode; otherwise the query
 * is registered as a WebSocket subscription and its initial result set is
 * returned before the subscription is released.
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
  if (client.usesRest) {
    return client.rest.query<T>(descriptor);
  }

  const resp = await client.send({
    type: 'sub',
    query: toDarshJQL(descriptor),
  });

  if (resp.type === 'sub-err') {
    throw new Error(`Query failed: ${resp.error}`);
  }
  if (resp.type !== 'sub-ok') {
    throw new Error(`Unexpected response to query: ${resp.type}`);
  }

  // One-shot: release the server-side subscription straight away.
  client
    .send({ type: 'unsub', sub_id: resp.sub_id })
    .catch(() => {
      /* best effort */
    });

  return { data: resp.initial as T[], txId: '' };
}

/**
 * Subscribe to live query results.
 *
 * Queries are deduplicated by their hash: if two callers subscribe to an
 * identical query, only one server subscription is created. Subscriptions are
 * automatically re-established after a WebSocket reconnect.
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
  if (client.usesRest) {
    return client.rest.subscribe<T>(descriptor, callback);
  }

  const hash = hashQuery(descriptor);

  let sub = _privateActiveSubs.get(hash) as ActiveSubscription<T> | undefined;

  if (sub) {
    // Dedup: reuse existing server subscription.
    sub.callbacks.add(callback);
    sub.refCount++;
    if (sub.subId) {
      deliver(sub, { data: sub.rows as T[], txId: '' });
    }
  } else {
    sub = {
      descriptor,
      callbacks: new Set([callback]),
      subId: null,
      refCount: 1,
      rows: [],
      disposeReconnect: null,
    };

    _privateActiveSubs.set(hash, sub);

    const active = sub;
    void openSubscription(client, hash, active);
    active.disposeReconnect = client.onReconnected(() => {
      // The socket dropped, so the server-side registration is gone.
      if (active.subId) client.unregisterSubscriptionHandler(active.subId);
      active.subId = null;
      return openSubscription(client, hash, active);
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
      active.disposeReconnect?.();
      const { subId } = active;
      if (subId) {
        client.unregisterSubscriptionHandler(subId);
        client.send({ type: 'unsub', sub_id: subId }).catch(() => {
          /* best effort */
        });
      }
    }
  };
}

/* -------------------------------------------------------------------------- */
/*  Internals                                                                 */
/* -------------------------------------------------------------------------- */

/** Register (or re-register) a live subscription on the server. */
async function openSubscription<T>(
  client: DarshJDB,
  hash: string,
  active: ActiveSubscription<T>,
): Promise<void> {
  try {
    const resp = await client.send({
      type: 'sub',
      query: toDarshJQL(active.descriptor),
    });

    if (resp.type === 'sub-err') {
      throw new Error(resp.error);
    }
    if (resp.type !== 'sub-ok') {
      throw new Error(`unexpected response: ${resp.type}`);
    }

    // The caller may have unsubscribed while the request was in flight.
    if (_privateActiveSubs.get(hash) !== active) {
      client.send({ type: 'unsub', sub_id: resp.sub_id }).catch(() => {
        /* best effort */
      });
      return;
    }

    active.subId = resp.sub_id;
    active.rows = resp.initial;

    client.registerSubscriptionHandler(resp.sub_id, (msg: ServerMessage) => {
      applyPush(active, msg);
    });

    deliver(active, { data: active.rows as T[], txId: '' });
  } catch (err) {
    console.error('[DarshJDB] Subscription error:', err);
  }
}

/** Fold a server-pushed diff into the cached row set and notify subscribers. */
function applyPush<T>(active: ActiveSubscription<T>, msg: ServerMessage): void {
  if (msg.type !== 'sub') return;

  const removed = new Set(
    (msg.removed ?? []).map((row) => rowKey(row)).filter((k): k is string => !!k),
  );
  const updated = new Map(
    (msg.updated ?? [])
      .map((row) => [rowKey(row), row] as const)
      .filter((entry): entry is readonly [string, Record<string, unknown>] =>
        Boolean(entry[0]),
      ),
  );

  const next = active.rows
    .filter((row) => {
      const key = rowKey(row);
      return !key || !removed.has(key);
    })
    .map((row) => {
      const key = rowKey(row);
      return key ? (updated.get(key) ?? row) : row;
    });

  active.rows = [...next, ...(msg.added ?? [])];
  deliver(active, { data: active.rows as T[], txId: '' });
}

/** Identity of a row in a result set, as emitted by the server. */
function rowKey(row: Record<string, unknown>): string | null {
  const id = row['_id'] ?? row['id'] ?? row['entity_id'];
  return typeof id === 'string' ? id : null;
}

function deliver<T>(
  active: ActiveSubscription<T>,
  result: QueryResult<T>,
): void {
  for (const cb of active.callbacks) {
    try {
      cb(result);
    } catch {
      /* subscriber errors must not break the notification loop */
    }
  }
}
