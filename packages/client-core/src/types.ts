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
/*  DarshJQL (server query dialect)                                           */
/* -------------------------------------------------------------------------- */

/** Comparison operators understood by the server's DarshJQL parser. */
export type DarshJQLOp =
  | 'Eq'
  | 'Neq'
  | 'Gt'
  | 'Gte'
  | 'Lt'
  | 'Lte'
  | 'Contains'
  | 'Like';

/** A single DarshJQL `$where` predicate. */
export interface DarshJQLWhere {
  attribute: string;
  op: DarshJQLOp;
  value: unknown;
}

/** A single DarshJQL `$order` clause. */
export interface DarshJQLOrder {
  attribute: string;
  direction: 'Asc' | 'Desc';
}

/** Wire form of a query as accepted by `POST /api/query`, the WS `sub`
 *  frame, and the `q` parameter of `GET /api/subscribe`. */
export interface DarshJQL {
  type: string;
  $where?: DarshJQLWhere[];
  $order?: DarshJQLOrder[];
  $limit?: number;
  $offset?: number;
}

/** Mutation verbs accepted by `POST /api/mutate` and the WS `mut` frame. */
export type ServerMutationOp = 'insert' | 'update' | 'delete';

/** Wire form of a single mutation. */
export interface ServerMutation {
  op: ServerMutationOp;
  entity: string;
  id?: string;
  data?: Record<string, unknown>;
}

/* -------------------------------------------------------------------------- */
/*  Protocol messages (wire format)                                           */
/* -------------------------------------------------------------------------- */

/**
 * Client-to-server frame.
 *
 * Mirrors the server's `ClientMessage` enum (`packages/server/src/api/ws.rs`),
 * which is internally tagged on `type` with kebab-case variant names and
 * flat, per-variant fields — there is no `payload` envelope.
 */
export type ClientMessage =
  | { type: 'auth'; token: string }
  | { type: 'sub'; id: string; query: DarshJQL }
  | { type: 'unsub'; id: string; sub_id: string }
  | { type: 'mut'; id: string; ops: ServerMutation[] }
  | { type: 'pres-join'; room: string; state?: unknown }
  | { type: 'pres-state'; room: string; state: unknown }
  | { type: 'pres-leave'; room: string }
  | { type: 'live-select'; id: string; query: string }
  | { type: 'kill'; id: string; live_id: string }
  | { type: 'pub-sub'; id: string; channel: string }
  | { type: 'pub-unsub'; id: string }
  | { type: 'ping' };

/** Distributes `Omit` across the members of a union. */
type DistributiveOmit<T, K extends PropertyKey> = T extends unknown
  ? Omit<T, K>
  : never;

/** Client frames that the server answers with a correlated reply, minus the
 *  correlation id (which {@link DarshJDB.send} assigns). */
export type ClientRequest = DistributiveOmit<
  Extract<ClientMessage, { id: string }>,
  'id'
>;

/** Client frames the server never replies to directly. */
export type ClientNotification = Exclude<ClientMessage, { id: string }>;

/**
 * Server-to-client frame.
 *
 * Mirrors the server's `ServerMessage` enum (`packages/server/src/api/ws.rs`).
 */
export type ServerMessage =
  | { type: 'auth-ok'; session_id: string }
  | { type: 'auth-err'; error: string }
  | {
      type: 'sub-ok';
      id: string;
      sub_id: string;
      initial: Record<string, unknown>[];
    }
  | { type: 'sub-err'; id: string; error: string }
  | {
      type: 'sub';
      sub_id: string;
      added?: Record<string, unknown>[];
      removed?: Record<string, unknown>[];
      updated?: Record<string, unknown>[];
    }
  | { type: 'diff'; sub_id: string; tx: number; changes: unknown }
  | { type: 'unsub-ok'; id: string }
  | { type: 'mut-ok'; id: string; tx: number }
  | { type: 'mut-err'; id: string; error: string }
  | { type: 'pres-snap'; room: string; members: PresenceMember[] }
  | {
      type: 'pres-diff';
      room: string;
      joined?: PresenceMember[];
      left?: string[];
      updated?: PresenceMember[];
    }
  | { type: 'live-select-ok'; id: string; live_id: string }
  | { type: 'live-select-err'; id: string; error: string }
  | {
      type: 'live-event';
      live_id: string;
      action: string;
      result: unknown;
      tx_id: number;
    }
  | { type: 'kill-ok'; id: string; live_id: string }
  | { type: 'kill-err'; id: string; error: string }
  | { type: 'pub-sub-ok'; id: string; channel: string }
  | { type: 'pub-unsub-ok'; id: string }
  | {
      type: 'pub-event';
      id: string;
      event: string;
      entity_type?: string;
      entity_id?: string;
      changed?: string[];
      tx_id: number;
      payload?: unknown;
    }
  | { type: 'batch-result'; id: string; results: unknown[]; duration_ms: number }
  | { type: 'pong' }
  | { type: 'error'; error: string };

/** A room member as reported by the server's presence frames. */
export interface PresenceMember {
  user_id: string;
  state: unknown;
}
