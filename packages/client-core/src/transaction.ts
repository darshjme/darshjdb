/**
 * Transaction builder for DarshJDB mutations.
 *
 * Supports set, merge, delete, link, and unlink operations with a fluent
 * proxy-based API.
 *
 * @module transaction
 */

import { v7 as uuidv7 } from 'uuid';
import type { DarshJDB } from './client.js';
import type {
  ServerMutation,
  ServerMutationOp,
  TxId,
  TxOp,
  TxOpKind,
} from './types.js';

/* -------------------------------------------------------------------------- */
/*  Entity Proxy                                                              */
/* -------------------------------------------------------------------------- */

/**
 * Proxy for a single entity instance, providing mutation methods.
 */
export interface EntityProxy {
  /**
   * Replace the entity data entirely.
   *
   * @param data - New document data.
   */
  set(data: Record<string, unknown>): void;

  /**
   * Merge fields into the existing entity data.
   *
   * @param data - Partial document data to merge.
   */
  merge(data: Record<string, unknown>): void;

  /**
   * Delete the entity.
   */
  delete(): void;

  /**
   * Create a link to another entity.
   *
   * @param targetEntity - Target entity collection name.
   * @param targetId     - Target entity id.
   */
  link(targetEntity: string, targetId: string): void;

  /**
   * Remove a link to another entity.
   *
   * @param targetEntity - Target entity collection name.
   * @param targetId     - Target entity id.
   */
  unlink(targetEntity: string, targetId: string): void;
}

/**
 * Proxy for an entity collection, where property access yields an
 * {@link EntityProxy} keyed by document id.
 *
 * @example
 * ```ts
 * db.tx.users['user-123'].set({ name: 'Alice' });
 * ```
 */
export type EntityCollectionProxy = {
  [id: string]: EntityProxy;
};

/* -------------------------------------------------------------------------- */
/*  Transaction Builder                                                      */
/* -------------------------------------------------------------------------- */

/**
 * Collects mutation operations and submits them as an atomic transaction.
 *
 * Access entity collections via `tx.<collection>[<id>].<method>(...)`.
 *
 * @example
 * ```ts
 * const txId = await db.transact((tx) => {
 *   tx.users[db.id()].set({ name: 'Alice', age: 30 });
 *   tx.posts['post-1'].merge({ title: 'Updated' });
 *   tx.comments['c-1'].delete();
 * });
 * ```
 */
export class TransactionBuilder {
  /** Accumulated operations. */
  readonly ops: TxOp[] = [];

  /**
   * Proxy that intercepts property access to create entity collection
   * and document proxies.
   */
  readonly proxy: Record<string, EntityCollectionProxy>;

  constructor() {
    this.proxy = new Proxy({} as Record<string, EntityCollectionProxy>, {
      get: (_target, entity: string) => {
        return this._privateMakeCollectionProxy(entity);
      },
    });
  }

  private _privateMakeCollectionProxy(entity: string): EntityCollectionProxy {
    return new Proxy({} as EntityCollectionProxy, {
      get: (_target, id: string) => {
        return this._privateMakeEntityProxy(entity, id);
      },
    });
  }

  private _privateMakeEntityProxy(entity: string, id: string): EntityProxy {
    const push = (kind: TxOpKind, data?: Record<string, unknown>, target?: { entity: string; id: string }) => {
      this.ops.push({ kind, entity, id, ...(data && { data }), ...(target && { target }) });
    };

    return {
      set(data: Record<string, unknown>) {
        push('set', data);
      },
      merge(data: Record<string, unknown>) {
        push('merge', data);
      },
      delete() {
        push('delete');
      },
      link(targetEntity: string, targetId: string) {
        push('link', undefined, { entity: targetEntity, id: targetId });
      },
      unlink(targetEntity: string, targetId: string) {
        push('unlink', undefined, { entity: targetEntity, id: targetId });
      },
    };
  }
}

/* -------------------------------------------------------------------------- */
/*  Public API                                                                */
/* -------------------------------------------------------------------------- */

/** Builder verb -> the mutation verb the server implements. */
const OP_MAP: Partial<Record<TxOpKind, ServerMutationOp>> = {
  set: 'insert',
  merge: 'update',
  delete: 'delete',
};

/**
 * Translate builder operations into the wire form accepted by
 * `POST /api/mutate` and the WebSocket `mut` frame.
 *
 * @throws If an operation has no server-side equivalent.
 */
export function toServerMutations(ops: TxOp[]): ServerMutation[] {
  return ops.map((op) => {
    const serverOp = OP_MAP[op.kind];
    if (!serverOp) {
      throw new Error(
        `Operation "${op.kind}" is not supported by the DarshJDB server`,
      );
    }
    return {
      op: serverOp,
      entity: op.entity,
      id: op.id,
      ...(op.data && { data: op.data }),
    };
  });
}

/**
 * Submit already-built operations as an atomic transaction.
 *
 * @param client - The DarshJDB client instance.
 * @param ops    - Operations to submit.
 * @returns The transaction id assigned by the server.
 */
export async function submitOps(
  client: DarshJDB,
  ops: TxOp[],
): Promise<TxId> {
  if (ops.length === 0) {
    throw new Error('Transaction has no operations');
  }

  if (client.usesRest) {
    return client.rest.transact(ops);
  }

  const resp = await client.send({
    type: 'mut',
    ops: toServerMutations(ops),
  });

  if (resp.type === 'mut-err') {
    throw new Error(`Transaction failed: ${resp.error}`);
  }
  if (resp.type !== 'mut-ok') {
    throw new Error(`Unexpected response to transaction: ${resp.type}`);
  }

  return String(resp.tx);
}

/**
 * Execute an atomic transaction against the DarshJDB server.
 *
 * @param client - The DarshJDB client instance.
 * @param fn     - Builder callback that accumulates operations via the proxy.
 * @returns The transaction id assigned by the server.
 *
 * @example
 * ```ts
 * const txId = await transact(db, (tx) => {
 *   tx.users['u1'].set({ name: 'Bob' });
 * });
 * ```
 */
export async function transact(
  client: DarshJDB,
  fn: (tx: Record<string, EntityCollectionProxy>) => void,
): Promise<TxId> {
  const builder = new TransactionBuilder();
  fn(builder.proxy);

  return submitOps(client, builder.ops);
}

/**
 * Generate a UUID v7 suitable for use as a DarshJDB entity id.
 *
 * UUID v7 is time-ordered, which makes it ideal for database primary keys
 * as it preserves insertion order and provides good index locality.
 *
 * @returns A new UUID v7 string.
 */
export function generateId(): string {
  return uuidv7();
}
