/**
 * @darshan/client - Framework-agnostic TypeScript client SDK for DarshanDB.
 *
 * @packageDocumentation
 *
 * @example
 * ```ts
 * import { DarshanDB, QueryBuilder, AuthClient, SyncEngine } from '@darshan/client';
 *
 * const db = new DarshanDB({
 *   serverUrl: 'https://db.example.com',
 *   appId: 'my-app',
 * });
 *
 * await db.connect();
 *
 * // Query
 * const users = await new QueryBuilder(db, 'users')
 *   .where('age', '>=', 18)
 *   .orderBy('createdAt', 'desc')
 *   .limit(10)
 *   .exec();
 *
 * // Transact
 * const txId = await transact(db, (tx) => {
 *   tx.users[generateId()].set({ name: 'Alice', age: 30 });
 * });
 * ```
 *
 * @module @darshan/client
 */

/* -- Client --------------------------------------------------------------- */
export { DarshanDB, msgpackEncode, msgpackDecode } from './client.js';

/* -- Query ---------------------------------------------------------------- */
export { QueryBuilder, queryOnce, subscribe } from './query.js';

/* -- Transaction ---------------------------------------------------------- */
export {
  TransactionBuilder,
  transact,
  generateId,
  type EntityProxy,
  type EntityCollectionProxy,
} from './transaction.js';

/* -- Sync ----------------------------------------------------------------- */
export { SyncEngine } from './sync.js';

/* -- Presence ------------------------------------------------------------- */
export { PresenceRoom } from './presence.js';

/* -- Auth ----------------------------------------------------------------- */
export { AuthClient } from './auth.js';

/* -- Storage -------------------------------------------------------------- */
export { StorageClient } from './storage.js';

/* -- REST fallback -------------------------------------------------------- */
export { RestTransport } from './rest.js';

/* -- Types ---------------------------------------------------------------- */
export type {
  /* Config */
  DarshanConfig,
  TransportMode,
  TokenStorage,

  /* Connection */
  ConnectionState,
  ConnectionStateListener,

  /* Query */
  WhereOp,
  WhereClause,
  OrderDirection,
  OrderClause,
  QueryDescriptor,
  QueryResult,
  SubscriptionCallback,
  Unsubscribe,

  /* Transaction */
  TxId,
  TxOpKind,
  TxOp,

  /* Sync */
  OfflineQueueEntry,
  OptimisticUpdate,

  /* Presence */
  Peer,
  PresenceSnapshot,
  PresenceCallback,

  /* Auth */
  User,
  AuthTokens,
  OAuthProvider,
  AuthStateEvent,
  AuthStateCallback,

  /* Storage */
  UploadOptions,
  UploadResult,

  /* Protocol */
  ClientMessage,
  ServerMessage,
} from './types.js';
