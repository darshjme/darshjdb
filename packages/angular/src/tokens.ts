/**
 * @module tokens
 * @description Angular injection tokens for DarshanDB configuration and services.
 *
 * These tokens are the dependency injection backbone of the SDK. They are
 * provided either via `DarshanModule.forRoot()` or the standalone
 * `provideDarshan()` helper.
 */

import { InjectionToken } from '@angular/core';

import type { DarshanConfig } from './types';

/**
 * Injection token for the DarshanDB configuration object.
 *
 * @example
 * ```typescript
 * // Reading the config in a service:
 * const config = inject(DARSHAN_CONFIG);
 * console.log(config.serverUrl);
 * ```
 */
export const DARSHAN_CONFIG = new InjectionToken<DarshanConfig>(
  'DARSHAN_CONFIG',
);

/**
 * Injection token for the low-level DarshanDB client instance.
 *
 * The client manages the WebSocket connection, authentication state,
 * and query subscriptions. Framework wrappers (signals, observables)
 * delegate to this client internally.
 *
 * @remarks
 * Prefer the higher-level `injectDarshan()`, `injectDarshanAuth()`, or
 * query helpers over using the raw client directly.
 */
export const DARSHAN_CLIENT = new InjectionToken<DarshanClient>(
  'DARSHAN_CLIENT',
);

/**
 * Minimal interface for the `@darshan/client` core client.
 *
 * This decouples the Angular SDK from the concrete client implementation,
 * enabling tree-shaking and testability via mock providers.
 */
export interface DarshanClient {
  /** Connect to the DarshanDB server. */
  connect(): Promise<void>;

  /** Disconnect and clean up all subscriptions. */
  disconnect(): void;

  /** Whether the client is currently connected. */
  readonly connected: boolean;

  /**
   * Subscribe to a live query. Returns an unsubscribe function.
   *
   * @param collection - The collection/table name.
   * @param query - Query filter/projection object.
   * @param callback - Invoked on each result update.
   * @returns A function that cancels the subscription when called.
   */
  subscribe<T>(
    collection: string,
    query: Record<string, unknown>,
    callback: (result: { data: T; error: DarshanClientError | null }) => void,
  ): () => void;

  /**
   * Execute a one-shot query.
   *
   * @param collection - The collection/table name.
   * @param query - Query filter/projection object.
   */
  query<T>(
    collection: string,
    query: Record<string, unknown>,
  ): Promise<T>;

  /**
   * Execute a mutation (insert, update, delete).
   *
   * @param collection - The collection/table name.
   * @param mutation - The mutation descriptor.
   */
  mutate<T>(
    collection: string,
    mutation: Record<string, unknown>,
  ): Promise<T>;

  // ── Auth ──────────────────────────────────────────────────────────

  /** Sign in with the given credentials. */
  signIn(credentials: Record<string, unknown>): Promise<AuthResult>;

  /** Register a new user. */
  signUp(credentials: Record<string, unknown>): Promise<AuthResult>;

  /** Sign out the current user and revoke the session. */
  signOut(): Promise<void>;

  /** Get the current authenticated user, or `null`. */
  getUser(): AuthUser | null;

  /** Get the current access token, or `null`. */
  getToken(): string | null;

  /**
   * Register a listener for auth state changes.
   *
   * @returns An unsubscribe function.
   */
  onAuthStateChange(
    callback: (user: AuthUser | null) => void,
  ): () => void;

  // ── Presence ──────────────────────────────────────────────────────

  /**
   * Join a presence room.
   *
   * @param roomId - Unique room identifier.
   * @param data - Initial presence data to broadcast.
   * @param callback - Invoked when the room's presence state changes.
   * @returns An object with `update` (change own data) and `leave` methods.
   */
  joinPresence<TData = Record<string, unknown>>(
    roomId: string,
    data: TData,
    callback: (state: PresenceStateRaw<TData>) => void,
  ): PresenceHandle<TData>;
}

/** Raw auth result from the client. */
export interface AuthResult {
  readonly user: AuthUser;
  readonly accessToken: string;
  readonly refreshToken: string;
  readonly expiresAt: string;
}

/** Raw user from the client. */
export interface AuthUser {
  readonly id: string;
  readonly email?: string;
  readonly displayName?: string;
  readonly roles: string[];
}

/** Error shape from the client. */
export interface DarshanClientError {
  readonly code: string;
  readonly message: string;
  readonly status?: number;
}

/** Raw presence state from the client. */
export interface PresenceStateRaw<TData = Record<string, unknown>> {
  readonly users: Array<{
    readonly userId: string;
    readonly data: TData;
    readonly lastSeen: string;
  }>;
  readonly count: number;
}

/** Handle returned from joining a presence room. */
export interface PresenceHandle<TData = Record<string, unknown>> {
  /** Update the current user's presence data. */
  update(data: Partial<TData>): void;
  /** Leave the presence room. */
  leave(): void;
}
