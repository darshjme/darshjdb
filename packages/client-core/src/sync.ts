/**
 * Client-side sync engine with IndexedDB cache, optimistic updates,
 * offline queue, and server reconciliation.
 *
 * @module sync
 */

import { openDB, type IDBPDatabase } from 'idb';
import type { DarshanDB } from './client.js';
import type {
  TxOp,
  TxId,
  OfflineQueueEntry,
  OptimisticUpdate,
  QueryResult,
} from './types.js';
import { generateId } from './transaction.js';

/* -------------------------------------------------------------------------- */
/*  Constants                                                                 */
/* -------------------------------------------------------------------------- */

const DB_NAME_PREFIX = 'darshan_sync_';
const DB_VERSION = 1;
const STORE_CACHE = 'queryCache';
const STORE_QUEUE = 'offlineQueue';
const STORE_META = 'meta';
const MAX_REPLAY_ATTEMPTS = 5;

/* -------------------------------------------------------------------------- */
/*  SyncEngine                                                                */
/* -------------------------------------------------------------------------- */

/**
 * Manages client-side caching, optimistic updates, and offline queue replay.
 *
 * @example
 * ```ts
 * const sync = new SyncEngine(db);
 * await sync.init();
 *
 * // Cached query
 * const result = await sync.getCached('hash123');
 *
 * // Queue offline mutation
 * await sync.enqueue([{ kind: 'set', entity: 'users', id: 'u1', data: { name: 'Alice' } }]);
 *
 * // Replay queue when back online
 * await sync.replayQueue();
 * ```
 */
export class SyncEngine {
  private _privateClient: DarshanDB;
  private _privateDb: IDBPDatabase | null = null;
  private _privateAppId: string;
  private _privateOptimistic = new Map<string, OptimisticUpdate>();
  private _privateIsReplaying = false;

  constructor(client: DarshanDB) {
    this._privateClient = client;
    this._privateAppId = client.appId;
  }

  /**
   * Initialise the IndexedDB database for this application.
   * Must be called before any other sync operations.
   */
  async init(): Promise<void> {
    this._privateDb = await openDB(
      `${DB_NAME_PREFIX}${this._privateAppId}`,
      DB_VERSION,
      {
        upgrade(db) {
          if (!db.objectStoreNames.contains(STORE_CACHE)) {
            db.createObjectStore(STORE_CACHE, { keyPath: 'queryHash' });
          }
          if (!db.objectStoreNames.contains(STORE_QUEUE)) {
            const store = db.createObjectStore(STORE_QUEUE, { keyPath: 'id' });
            store.createIndex('byCreatedAt', 'createdAt');
          }
          if (!db.objectStoreNames.contains(STORE_META)) {
            db.createObjectStore(STORE_META, { keyPath: 'key' });
          }
        },
      },
    );
  }

  /* -- Query cache -------------------------------------------------------- */

  /**
   * Retrieve a cached query result by its hash.
   *
   * @param queryHash - Stable hash of the query descriptor.
   * @returns The cached result, or `null` if not found.
   */
  async getCached<T = Record<string, unknown>>(
    queryHash: string,
  ): Promise<QueryResult<T> | null> {
    const db = this._privateRequireDb();
    const entry = await db.get(STORE_CACHE, queryHash);
    if (!entry) return null;
    return entry.result as QueryResult<T>;
  }

  /**
   * Store a query result in the local cache.
   *
   * @param queryHash - Stable hash of the query descriptor.
   * @param result    - The result to cache.
   */
  async setCache(queryHash: string, result: QueryResult): Promise<void> {
    const db = this._privateRequireDb();
    await db.put(STORE_CACHE, {
      queryHash,
      result,
      updatedAt: Date.now(),
    });
  }

  /**
   * Clear a single cache entry.
   *
   * @param queryHash - Hash of the query to invalidate.
   */
  async invalidateCache(queryHash: string): Promise<void> {
    const db = this._privateRequireDb();
    await db.delete(STORE_CACHE, queryHash);
  }

  /**
   * Clear all cached query results.
   */
  async clearCache(): Promise<void> {
    const db = this._privateRequireDb();
    await db.clear(STORE_CACHE);
  }

  /* -- Optimistic updates ------------------------------------------------- */

  /**
   * Apply an optimistic update locally before the server confirms.
   *
   * @param ops - The operations being optimistically applied.
   * @returns A temporary transaction id for tracking.
   */
  applyOptimistic(ops: TxOp[]): string {
    const tempTxId = `optimistic_${generateId()}`;
    this._privateOptimistic.set(tempTxId, {
      tempTxId,
      ops,
      appliedAt: Date.now(),
    });
    return tempTxId;
  }

  /**
   * Confirm that the server accepted the optimistic update.
   * Removes it from the pending optimistic set.
   *
   * @param tempTxId - The temporary transaction id.
   */
  confirmOptimistic(tempTxId: string): void {
    this._privateOptimistic.delete(tempTxId);
  }

  /**
   * Roll back an optimistic update that the server rejected.
   * Returns the operations that were rolled back so the caller can
   * undo their local effects.
   *
   * @param tempTxId - The temporary transaction id.
   * @returns The rolled-back operations, or `null` if not found.
   */
  rollbackOptimistic(tempTxId: string): TxOp[] | null {
    const update = this._privateOptimistic.get(tempTxId);
    if (!update) return null;
    this._privateOptimistic.delete(tempTxId);
    return update.ops;
  }

  /**
   * Get all pending optimistic updates (for UI indicators).
   */
  getPendingOptimistic(): OptimisticUpdate[] {
    return Array.from(this._privateOptimistic.values());
  }

  /* -- Offline queue ------------------------------------------------------ */

  /**
   * Enqueue operations for later replay (when the client is offline).
   *
   * @param ops - The transaction operations to enqueue.
   * @returns The queue entry id.
   */
  async enqueue(ops: TxOp[]): Promise<string> {
    const db = this._privateRequireDb();
    const entry: OfflineQueueEntry = {
      id: generateId(),
      ops,
      createdAt: Date.now(),
      attempts: 0,
    };
    await db.put(STORE_QUEUE, entry);
    return entry.id;
  }

  /**
   * Get all entries in the offline queue, ordered by creation time.
   */
  async getQueue(): Promise<OfflineQueueEntry[]> {
    const db = this._privateRequireDb();
    return db.getAllFromIndex(STORE_QUEUE, 'byCreatedAt');
  }

  /**
   * Remove an entry from the offline queue after successful replay.
   *
   * @param id - Queue entry id.
   */
  async dequeue(id: string): Promise<void> {
    const db = this._privateRequireDb();
    await db.delete(STORE_QUEUE, id);
  }

  /**
   * Replay all queued offline operations.
   *
   * Sends each entry to the server in order. Entries that fail after
   * {@link MAX_REPLAY_ATTEMPTS} are discarded with a warning.
   *
   * @returns The number of successfully replayed entries.
   */
  async replayQueue(): Promise<number> {
    if (this._privateIsReplaying) return 0;
    this._privateIsReplaying = true;

    let replayed = 0;

    try {
      const queue = await this.getQueue();

      for (const entry of queue) {
        try {
          const resp = await this._privateClient.send({
            type: 'transact',
            payload: { ops: entry.ops },
          });

          if (resp.type === 'tx-error') {
            entry.attempts++;
            if (entry.attempts >= MAX_REPLAY_ATTEMPTS) {
              console.warn(
                `[DarshanDB Sync] Discarding queue entry ${entry.id} after ${MAX_REPLAY_ATTEMPTS} attempts`,
              );
              await this.dequeue(entry.id);
            } else {
              const db = this._privateRequireDb();
              await db.put(STORE_QUEUE, entry);
            }
            continue;
          }

          await this.dequeue(entry.id);
          replayed++;
        } catch {
          // Network error; stop replaying — we're likely offline again.
          break;
        }
      }
    } finally {
      this._privateIsReplaying = false;
    }

    return replayed;
  }

  /**
   * Clear the entire offline queue.
   */
  async clearQueue(): Promise<void> {
    const db = this._privateRequireDb();
    await db.clear(STORE_QUEUE);
  }

  /* -- Last transaction tracking ------------------------------------------ */

  /**
   * Store the last known server transaction id for catch-up on reconnect.
   *
   * @param txId - The latest transaction id from the server.
   */
  async setLastTxId(txId: TxId): Promise<void> {
    const db = this._privateRequireDb();
    await db.put(STORE_META, { key: 'lastTxId', value: txId });
  }

  /**
   * Retrieve the last known server transaction id.
   *
   * @returns The transaction id, or `null` if none stored.
   */
  async getLastTxId(): Promise<TxId | null> {
    const db = this._privateRequireDb();
    const entry = await db.get(STORE_META, 'lastTxId');
    if (!entry) return null;
    return entry.value as TxId;
  }

  /* -- Cleanup ------------------------------------------------------------ */

  /**
   * Close the IndexedDB connection and release resources.
   */
  close(): void {
    if (this._privateDb) {
      this._privateDb.close();
      this._privateDb = null;
    }
    this._privateOptimistic.clear();
  }

  /* -- Internals ---------------------------------------------------------- */

  private _privateRequireDb(): IDBPDatabase {
    if (!this._privateDb) {
      throw new Error(
        'SyncEngine not initialised. Call init() before using sync features.',
      );
    }
    // Cast needed because openDB returns generic IDBPDatabase
    return this._privateDb as IDBPDatabase;
  }
}
