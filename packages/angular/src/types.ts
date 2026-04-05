/**
 * @module types
 * @description Core type definitions for the DarshJDB Angular SDK.
 */

/**
 * Configuration for connecting to a DarshJDB server instance.
 *
 * @example
 * ```typescript
 * const config: DarshanConfig = {
 *   serverUrl: 'https://db.example.com',
 *   appId: 'my-app',
 *   debug: true,
 * };
 * ```
 */
export interface DarshanConfig {
  /** Base URL of the DarshJDB server (e.g., `https://db.example.com`). */
  readonly serverUrl: string;

  /** Application identifier registered with the DarshJDB instance. */
  readonly appId: string;

  /**
   * WebSocket URL override. Defaults to deriving from `serverUrl`
   * by replacing `http(s)` with `ws(s)`.
   */
  readonly wsUrl?: string;

  /** Enable verbose debug logging to the console. Defaults to `false`. */
  readonly debug?: boolean;

  /**
   * Connection timeout in milliseconds for the initial WebSocket handshake.
   * Defaults to `10_000`.
   */
  readonly connectTimeout?: number;

  /**
   * Interval in milliseconds between automatic reconnect attempts.
   * Defaults to `3_000`. Set to `0` to disable auto-reconnect.
   */
  readonly reconnectInterval?: number;

  /**
   * Maximum number of reconnect attempts before giving up.
   * Defaults to `Infinity`.
   */
  readonly maxReconnectAttempts?: number;
}

/** Credentials for email/password authentication. */
export interface EmailPasswordCredentials {
  readonly email: string;
  readonly password: string;
}

/** Credentials for magic link authentication. */
export interface MagicLinkCredentials {
  readonly email: string;
}

/** Credentials for OAuth2 provider authentication. */
export interface OAuthCredentials {
  readonly provider: 'google' | 'github' | 'apple' | (string & {});
  readonly token: string;
}

/** Union of all supported sign-in credential types. */
export type SignInCredentials =
  | EmailPasswordCredentials
  | MagicLinkCredentials
  | OAuthCredentials;

/** Union of all supported sign-up credential types. */
export type SignUpCredentials =
  | (EmailPasswordCredentials & { readonly displayName?: string })
  | OAuthCredentials;

/**
 * Authenticated user object returned after successful sign-in.
 */
export interface DarshanUser {
  /** Unique user identifier (UUID). */
  readonly id: string;
  /** User email address, if available. */
  readonly email?: string;
  /** Display name, if set. */
  readonly displayName?: string;
  /** Roles assigned to this user. */
  readonly roles: readonly string[];
  /** Raw JWT access token for the current session. */
  readonly token: string;
  /** ISO-8601 token expiration timestamp. */
  readonly tokenExpiresAt: string;
}

/**
 * JWT token pair issued by the DarshJDB auth subsystem.
 */
export interface TokenPair {
  readonly accessToken: string;
  readonly refreshToken: string;
  readonly expiresAt: string;
}

/**
 * Reactive query result container shared by both Signal and Observable APIs.
 */
export interface QueryResult<T> {
  /** The current query data, or `undefined` if still loading. */
  readonly data: T | undefined;
  /** Whether the query is currently loading or reconnecting. */
  readonly isLoading: boolean;
  /** The error, if the query failed. `null` when healthy. */
  readonly error: DarshJError | null;
}

/**
 * Structured error from the DarshJDB client layer.
 */
export interface DarshJError {
  /** Machine-readable error code. */
  readonly code: string;
  /** Human-readable description. */
  readonly message: string;
  /** HTTP status code, if the error originated from a REST call. */
  readonly status?: number;
  /** Original error, if wrapping a lower-level exception. */
  readonly cause?: unknown;
}

/**
 * A single user presence entry within a room.
 */
export interface PresenceUser<TData = Record<string, unknown>> {
  /** User identifier. */
  readonly userId: string;
  /** Custom presence data broadcast by this user. */
  readonly data: TData;
  /** ISO-8601 timestamp of the last heartbeat. */
  readonly lastSeen: string;
}

/**
 * Aggregate presence state for a room.
 */
export interface PresenceState<TData = Record<string, unknown>> {
  /** All users currently present in the room. */
  readonly users: readonly PresenceUser<TData>[];
  /** Number of connected users. */
  readonly count: number;
  /** The current user's own presence data, if joined. */
  readonly self: PresenceUser<TData> | null;
}

/**
 * Options for a query subscription.
 */
export interface QueryOptions {
  /** Skip the initial data fetch and wait for the first push. */
  readonly skipInitialFetch?: boolean;
  /** Debounce incoming updates by this many milliseconds. */
  readonly debounceMs?: number;
}

/**
 * SSR transfer state key prefix used to namespace DarshJDB cache entries.
 */
export const DDB_TRANSFER_KEY_PREFIX = 'darshan_';
