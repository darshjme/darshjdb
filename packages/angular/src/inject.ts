/**
 * @module inject
 * @description Convenience injection functions for DarshanDB services.
 *
 * These functions wrap Angular's `inject()` to provide typed, discoverable
 * access to DarshanDB capabilities from components, directives, pipes,
 * and other injection contexts.
 *
 * @example
 * ```typescript
 * import { injectDarshan, injectDarshanAuth } from '@darshan/angular';
 *
 * @Component({ ... })
 * export class DashboardComponent {
 *   private readonly db = injectDarshan();
 *   private readonly auth = injectDarshanAuth();
 * }
 * ```
 */

import { inject, signal, computed, DestroyRef, type Signal } from '@angular/core';

import { DARSHAN_CLIENT, DARSHAN_CONFIG, type DarshanClient } from './tokens';
import type {
  DarshanConfig,
  DarshanUser,
  DarshanError,
  PresenceUser,
} from './types';

// ── injectDarshan ──────────────────────────────────────────────────

/**
 * Facade returned by {@link injectDarshan}.
 *
 * Provides typed access to the raw client, configuration, and
 * commonly used query/mutation operations.
 */
export interface DarshanHandle {
  /** The underlying DarshanDB client instance. */
  readonly client: DarshanClient;
  /** The active configuration. */
  readonly config: DarshanConfig;
  /** Execute a one-shot query. */
  query<T>(collection: string, q: Record<string, unknown>): Promise<T>;
  /** Execute a mutation. */
  mutate<T>(collection: string, mutation: Record<string, unknown>): Promise<T>;
}

/**
 * Inject the core DarshanDB handle.
 *
 * Must be called in an injection context (constructor, field initializer,
 * or `runInInjectionContext`).
 *
 * @returns A {@link DarshanHandle} with typed client access and shortcuts.
 *
 * @example
 * ```typescript
 * @Component({ ... })
 * export class ItemListComponent {
 *   private readonly db = injectDarshan();
 *
 *   async addItem(name: string) {
 *     await this.db.mutate('items', { insert: { name } });
 *   }
 * }
 * ```
 */
export function injectDarshan(): DarshanHandle {
  const client = inject(DARSHAN_CLIENT);
  const config = inject(DARSHAN_CONFIG);

  return {
    client,
    config,
    query: <T>(collection: string, q: Record<string, unknown>) =>
      client.query<T>(collection, q),
    mutate: <T>(collection: string, mutation: Record<string, unknown>) =>
      client.mutate<T>(collection, mutation),
  };
}

// ── injectDarshanAuth ──────────────────────────────────────────────

/**
 * Reactive auth handle returned by {@link injectDarshanAuth}.
 *
 * All state is exposed via Angular Signals for fine-grained change
 * detection in zoneless or OnPush components.
 */
export interface DarshanAuthHandle {
  /** The currently authenticated user, or `null`. */
  readonly user: Signal<DarshanUser | null>;
  /** Whether an auth operation is in progress. */
  readonly isLoading: Signal<boolean>;
  /** Whether a user is currently signed in (computed). */
  readonly isAuthenticated: Signal<boolean>;
  /** The last auth error, or `null`. */
  readonly error: Signal<DarshanError | null>;
  /** Sign in with the given credentials. */
  signIn(credentials: Record<string, unknown>): Promise<void>;
  /** Register a new user. */
  signUp(credentials: Record<string, unknown>): Promise<void>;
  /** Sign out the current user. */
  signOut(): Promise<void>;
}

/**
 * Inject reactive authentication state and actions.
 *
 * Automatically subscribes to auth state changes from the DarshanDB
 * client and exposes them as Angular Signals. The subscription is
 * cleaned up when the injection context is destroyed.
 *
 * @returns A {@link DarshanAuthHandle} with signal-based state and action methods.
 *
 * @example
 * ```typescript
 * @Component({
 *   template: `
 *     @if (auth.isAuthenticated()) {
 *       <p>Welcome, {{ auth.user()?.displayName }}</p>
 *       <button (click)="auth.signOut()">Sign Out</button>
 *     } @else {
 *       <button (click)="auth.signIn({ email, password })">Sign In</button>
 *     }
 *   `,
 * })
 * export class NavComponent {
 *   readonly auth = injectDarshanAuth();
 * }
 * ```
 */
export function injectDarshanAuth(): DarshanAuthHandle {
  const client = inject(DARSHAN_CLIENT);
  const destroyRef = inject(DestroyRef);

  const _user = signal<DarshanUser | null>(null);
  const _isLoading = signal(false);
  const _error = signal<DarshanError | null>(null);

  // Seed with current auth state.
  const currentUser = client.getUser();
  if (currentUser) {
    _user.set({
      id: currentUser.id,
      email: currentUser.email,
      displayName: currentUser.displayName,
      roles: currentUser.roles,
      token: client.getToken() ?? '',
      tokenExpiresAt: '',
    });
  }

  // Subscribe to auth state changes.
  const unsub = client.onAuthStateChange((raw) => {
    if (raw) {
      _user.set({
        id: raw.id,
        email: raw.email,
        displayName: raw.displayName,
        roles: raw.roles,
        token: client.getToken() ?? '',
        tokenExpiresAt: '',
      });
    } else {
      _user.set(null);
    }
    _error.set(null);
  });

  destroyRef.onDestroy(() => unsub());

  const isAuthenticated = computed(() => _user() !== null);

  return {
    user: _user.asReadonly(),
    isLoading: _isLoading.asReadonly(),
    isAuthenticated,
    error: _error.asReadonly(),

    async signIn(credentials) {
      _isLoading.set(true);
      _error.set(null);
      try {
        const result = await client.signIn(credentials);
        _user.set({
          id: result.user.id,
          email: result.user.email,
          displayName: result.user.displayName,
          roles: result.user.roles,
          token: result.accessToken,
          tokenExpiresAt: result.expiresAt,
        });
      } catch (err) {
        _error.set(toDarshanError(err));
      } finally {
        _isLoading.set(false);
      }
    },

    async signUp(credentials) {
      _isLoading.set(true);
      _error.set(null);
      try {
        const result = await client.signUp(credentials);
        _user.set({
          id: result.user.id,
          email: result.user.email,
          displayName: result.user.displayName,
          roles: result.user.roles,
          token: result.accessToken,
          tokenExpiresAt: result.expiresAt,
        });
      } catch (err) {
        _error.set(toDarshanError(err));
      } finally {
        _isLoading.set(false);
      }
    },

    async signOut() {
      _isLoading.set(true);
      _error.set(null);
      try {
        await client.signOut();
        _user.set(null);
      } catch (err) {
        _error.set(toDarshanError(err));
      } finally {
        _isLoading.set(false);
      }
    },
  };
}

// ── injectDarshanPresence ──────────────────────────────────────────

/**
 * Reactive presence handle returned by {@link injectDarshanPresence}.
 */
export interface DarshanPresenceHandle<TData = Record<string, unknown>> {
  /** All users currently present in the room. */
  readonly users: Signal<readonly PresenceUser<TData>[]>;
  /** Number of connected users. */
  readonly count: Signal<number>;
  /** The current user's own presence entry, if joined. */
  readonly self: Signal<PresenceUser<TData> | null>;
  /** Whether the room is currently connecting. */
  readonly isLoading: Signal<boolean>;
  /** Update the current user's presence data. */
  update(data: Partial<TData>): void;
  /** Leave the room (also called automatically on destroy). */
  leave(): void;
}

/**
 * Inject presence tracking for a specific room.
 *
 * Automatically joins the room with optional initial data and
 * leaves when the injection context is destroyed.
 *
 * @typeParam TData - Shape of custom presence data per user.
 * @param roomId - Unique identifier of the presence room.
 * @param initialData - Initial presence data to broadcast for this user.
 * @returns A {@link DarshanPresenceHandle} with reactive state and controls.
 *
 * @example
 * ```typescript
 * @Component({
 *   template: `
 *     <div>{{ presence.count() }} users online</div>
 *     @for (user of presence.users(); track user.userId) {
 *       <span>{{ user.data.name }}</span>
 *     }
 *   `,
 * })
 * export class CollaborativeEditor {
 *   readonly presence = injectDarshanPresence<{ name: string; cursor: number }>(
 *     'doc-123',
 *     { name: 'Alice', cursor: 0 },
 *   );
 *
 *   onCursorMove(pos: number) {
 *     this.presence.update({ cursor: pos });
 *   }
 * }
 * ```
 */
export function injectDarshanPresence<TData = Record<string, unknown>>(
  roomId: string,
  initialData: TData = {} as TData,
): DarshanPresenceHandle<TData> {
  const client = inject(DARSHAN_CLIENT);
  const destroyRef = inject(DestroyRef);

  const _users = signal<readonly PresenceUser<TData>[]>([]);
  const _count = signal(0);
  const _self = signal<PresenceUser<TData> | null>(null);
  const _isLoading = signal(true);

  const handle = client.joinPresence<TData>(
    roomId,
    initialData,
    (state) => {
      _users.set(state.users);
      _count.set(state.count);
      _isLoading.set(false);

      // Find self in the user list.
      const currentUser = client.getUser();
      if (currentUser) {
        const selfEntry = state.users.find(
          (u) => u.userId === currentUser.id,
        ) ?? null;
        _self.set(selfEntry);
      }
    },
  );

  destroyRef.onDestroy(() => handle.leave());

  return {
    users: _users.asReadonly(),
    count: _count.asReadonly(),
    self: _self.asReadonly(),
    isLoading: _isLoading.asReadonly(),
    update: (data) => handle.update(data),
    leave: () => handle.leave(),
  };
}

// ── Helpers ────────────────────────────────────────────────────────

/**
 * Normalize an unknown thrown value into a `DarshanError`.
 *
 * @internal
 */
function toDarshanError(err: unknown): DarshanError {
  if (
    err !== null &&
    typeof err === 'object' &&
    'code' in err &&
    'message' in err
  ) {
    return err as DarshanError;
  }

  if (err instanceof Error) {
    return {
      code: 'UNKNOWN',
      message: err.message,
      cause: err,
    };
  }

  return {
    code: 'UNKNOWN',
    message: String(err),
    cause: err,
  };
}
