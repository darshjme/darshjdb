/**
 * Core type definitions for the DarshJDB client SDK.
 * @module types
 */

/* -------------------------------------------------------------------------- */
/*  Configuration                                                             */
/* -------------------------------------------------------------------------- */

/** Transport layer protocol selection. */
export type TransportMode = 'ws' | 'rest' | 'auto';

/** Configuration for constructing a {@link DarshJDB} instance. */
export interface DarshanConfig {
  /** Base URL of the DarshJDB server (e.g. `https://db.example.com`). */
  serverUrl: string;
  /** Application identifier issued by the DarshJDB dashboard. */
  appId: string;
  /**
   * Transport protocol to use.
   * - `'ws'`   - WebSocket only
   * - `'rest'` - HTTP/SSE only
   * - `'auto'` - WebSocket with REST fallback (default)
   */
  transport?: TransportMode;
  /** Override the default token storage strategy. */
  tokenStorage?: TokenStorage;
}

/* -------------------------------------------------------------------------- */
/*  Connection                                                                */
/* -------------------------------------------------------------------------- */

/** States for the client connection state machine. */
export type ConnectionState =
  | 'disconnected'
  | 'connecting'
  | 'authenticating'
  | 'connected'
  | 'reconnecting';

/** Callback invoked when connection state changes. */
export type ConnectionStateListener = (
  state: ConnectionState,
  prev: ConnectionState,
) => void;

/* -------------------------------------------------------------------------- */
/*  Query                                                                     */
/* -------------------------------------------------------------------------- */

/** Supported comparison operators in a where-clause. */
export type WhereOp =
  | '='
  | '!='
  | '>'
  | '>='
  | '<'
  | '<='
  | 'in'
  | 'not-in'
  | 'contains'
  | 'starts-with';

/** A single where-clause filter. */
export interface WhereClause {
  field: string;
  op: WhereOp;
  value: unknown;
}

/** Sort direction. */
export type OrderDirection = 'asc' | 'desc';

/** Order-by specification. */
export interface OrderClause {
  field: string;
  direction: OrderDirection;
}

/** Serialisable query descriptor used for hashing and deduplication. */
export interface QueryDescriptor {
  collection: string;
  where?: WhereClause[];
  order?: OrderClause[];
  limit?: number;
  offset?: number;
  select?: string[];
}

/** Result set returned from a query. */
export interface QueryResult<T = Record<string, unknown>> {
  data: T[];
  /** Server-reported transaction id at the time the result was generated. */
  txId: string;
}

/** Subscription callback. */
export type SubscriptionCallback<T = Record<string, unknown>> = (
  result: QueryResult<T>,
) => void;

/** Unsubscribe handle. */
export type Unsubscribe = () => void;

/* -------------------------------------------------------------------------- */
/*  Transactions                                                              */
/* -------------------------------------------------------------------------- */

/** Unique transaction identifier. */
export type TxId = string;

/** Operation types supported by the transaction builder. */
export type TxOpKind = 'set' | 'merge' | 'delete' | 'link' | 'unlink';

/** A single transaction operation. */
export interface TxOp {
  kind: TxOpKind;
  entity: string;
  id: string;
  /** Payload for set/merge. */
  data?: Record<string, unknown>;
  /** Link/unlink target. */
  target?: { entity: string; id: string };
}

/* -------------------------------------------------------------------------- */
/*  Sync                                                                      */
/* -------------------------------------------------------------------------- */

/** Entry queued for offline replay. */
export interface OfflineQueueEntry {
  id: string;
  ops: TxOp[];
  createdAt: number;
  /** Number of attempts made so far. */
  attempts: number;
}

/** Optimistic update metadata. */
export interface OptimisticUpdate {
  tempTxId: string;
  ops: TxOp[];
  appliedAt: number;
}

/* -------------------------------------------------------------------------- */
/*  Presence                                                                  */
/* -------------------------------------------------------------------------- */

/** A peer in a presence room. */
export interface Peer<T = Record<string, unknown>> {
  peerId: string;
  userId?: string;
  state: T;
  lastSeen: number;
}

/** Presence room snapshot. */
export interface PresenceSnapshot<T = Record<string, unknown>> {
  roomId: string;
  peers: Peer<T>[];
  self: Peer<T> | null;
}

/** Callback for presence updates. */
export type PresenceCallback<T = Record<string, unknown>> = (
  snapshot: PresenceSnapshot<T>,
) => void;

/* -------------------------------------------------------------------------- */
/*  Auth                                                                      */
/* -------------------------------------------------------------------------- */

/** Authenticated user object. */
export interface User {
  id: string;
  email?: string;
  displayName?: string;
  avatarUrl?: string;
  metadata?: Record<string, unknown>;
}

/** Token pair returned after authentication. */
export interface AuthTokens {
  accessToken: string;
  refreshToken: string;
  /** Epoch milliseconds when the access token expires. */
  expiresAt: number;
}

/** Supported OAuth providers. */
export type OAuthProvider = 'google' | 'github' | 'apple' | 'discord' | string;

/** Auth state change event. */
export interface AuthStateEvent {
  user: User | null;
  tokens: AuthTokens | null;
}

/** Callback for auth state changes. */
export type AuthStateCallback = (event: AuthStateEvent) => void;

/** Pluggable token storage interface. */
export interface TokenStorage {
  get(key: string): string | null | Promise<string | null>;
  set(key: string, value: string): void | Promise<void>;
  remove(key: string): void | Promise<void>;
}

/* -------------------------------------------------------------------------- */
/*  Storage                                                                   */
/* -------------------------------------------------------------------------- */

/** Options for file upload. */
export interface UploadOptions {
  /** Content-Type override. */
  contentType?: string;
  /** Progress callback receiving 0-1 fraction. */
  onProgress?: (progress: number) => void;
  /** Custom metadata to attach to the file. */
  metadata?: Record<string, string>;
}

/** Result of a successful upload. */
export interface UploadResult {
  path: string;
  url: string;
  size: number;
  contentType: string;
}

/* -------------------------------------------------------------------------- */
/*  Protocol messages (wire format)                                           */
/* -------------------------------------------------------------------------- */

/** Client-to-server message envelope. */
export interface ClientMessage {
  type:
    | 'auth'
    | 'query'
    | 'subscribe'
    | 'unsubscribe'
    | 'transact'
    | 'presence-join'
    | 'presence-publish'
    | 'presence-leave'
    | 'ping';
  id: string;
  payload: unknown;
}

/** Server-to-client message envelope. */
export interface ServerMessage {
  type:
    | 'auth-ok'
    | 'auth-error'
    | 'query-result'
    | 'subscription-update'
    | 'tx-ok'
    | 'tx-error'
    | 'presence-update'
    | 'pong'
    | 'error';
  id?: string;
  payload: unknown;
}
