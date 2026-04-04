/**
 * @module types
 * @description Internal type definitions for the DarshanDB React SDK.
 * These mirror the public API surface of `@darshan/client` and ensure
 * the React layer stays decoupled from client-core internals.
 */

// ---------------------------------------------------------------------------
// Query Engine
// ---------------------------------------------------------------------------

/** Serialisable query descriptor passed to the client query engine. */
export interface Query<T = unknown> {
  readonly collection: string;
  readonly where?: ReadonlyArray<WhereClause>;
  readonly orderBy?: ReadonlyArray<OrderClause>;
  readonly limit?: number;
  readonly offset?: number;
  readonly select?: ReadonlyArray<keyof T & string>;
}

export interface WhereClause {
  readonly field: string;
  readonly op: '==' | '!=' | '<' | '<=' | '>' | '>=' | 'in' | 'not-in' | 'array-contains';
  readonly value: unknown;
}

export interface OrderClause {
  readonly field: string;
  readonly direction: 'asc' | 'desc';
}

// ---------------------------------------------------------------------------
// Subscription
// ---------------------------------------------------------------------------

/** Returned by `client.subscribe()` -- call to tear down the subscription. */
export type Unsubscribe = () => void;

export interface SubscriptionCallback<T> {
  (snapshot: QuerySnapshot<T>): void;
}

export interface QuerySnapshot<T> {
  readonly data: ReadonlyArray<T>;
  readonly error: Error | null;
}

// ---------------------------------------------------------------------------
// Mutation
// ---------------------------------------------------------------------------

export type MutationOperation =
  | { readonly type: 'insert'; readonly collection: string; readonly data: Record<string, unknown> }
  | { readonly type: 'update'; readonly collection: string; readonly id: string; readonly data: Record<string, unknown> }
  | { readonly type: 'delete'; readonly collection: string; readonly id: string };

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

export interface AuthUser {
  readonly id: string;
  readonly email: string | null;
  readonly displayName: string | null;
  readonly photoUrl: string | null;
  readonly metadata: Record<string, unknown>;
}

export interface AuthState {
  readonly user: AuthUser | null;
  readonly isLoading: boolean;
}

export type AuthUnsubscribe = () => void;

// ---------------------------------------------------------------------------
// Presence
// ---------------------------------------------------------------------------

export interface PresencePeer<S = unknown> {
  readonly peerId: string;
  readonly state: S;
  readonly lastSeen: number;
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

export interface UploadProgress {
  readonly bytesTransferred: number;
  readonly totalBytes: number;
  /** 0..1 */
  readonly fraction: number;
}

export interface UploadResult {
  readonly url: string;
  readonly path: string;
  readonly size: number;
  readonly contentType: string;
}

// ---------------------------------------------------------------------------
// Client Interface (consumed by React hooks)
// ---------------------------------------------------------------------------

/**
 * The contract that `@darshan/client` must satisfy for the React bindings
 * to function.  The concrete class (`DarshanClient`) lives in client-core;
 * this interface is the **only** coupling point.
 */
export interface DarshanClientInterface {
  // Lifecycle
  connect(): Promise<void>;
  disconnect(): void;

  // Queries
  subscribe<T>(query: Query<T>, callback: SubscriptionCallback<T>): Unsubscribe;
  query<T>(query: Query<T>): Promise<ReadonlyArray<T>>;

  // Mutations
  mutate(operations: MutationOperation | ReadonlyArray<MutationOperation>): Promise<void>;

  // Auth
  signIn(credentials: { email: string; password: string }): Promise<AuthUser>;
  signUp(credentials: { email: string; password: string; displayName?: string }): Promise<AuthUser>;
  signOut(): Promise<void>;
  getAuthState(): AuthState;
  onAuthStateChange(callback: (state: AuthState) => void): AuthUnsubscribe;

  // Presence
  joinRoom(roomId: string): Promise<void>;
  leaveRoom(roomId: string): Promise<void>;
  publishPresence<S>(roomId: string, state: S): void;
  onPresenceChange<S>(roomId: string, callback: (peers: ReadonlyArray<PresencePeer<S>>) => void): Unsubscribe;

  // Storage
  upload(
    file: File | Blob,
    path: string,
    options?: { onProgress?: (progress: UploadProgress) => void },
  ): Promise<UploadResult>;
}

// ---------------------------------------------------------------------------
// Client constructor options (used by DarshanProvider)
// ---------------------------------------------------------------------------

export interface DarshanClientOptions {
  readonly serverUrl: string;
  readonly appId: string;
}
