/**
 * @module use-auth
 * @description Reactive authentication hook that tracks the current user
 * and exposes sign-in / sign-up / sign-out actions.
 *
 * @example
 * ```tsx
 * import { useAuth } from '@darshjdb/react';
 *
 * function AuthGate({ children }: { children: React.ReactNode }) {
 *   const { user, isLoading, signIn, signOut } = useAuth();
 *
 *   if (isLoading) return <p>Authenticating...</p>;
 *
 *   if (!user) {
 *     return (
 *       <button onClick={() => signIn({ email: 'a@b.com', password: 'pw' })}>
 *         Sign In
 *       </button>
 *     );
 *   }
 *
 *   return (
 *     <>
 *       <p>Hello, {user.displayName ?? user.email}</p>
 *       <button onClick={signOut}>Sign Out</button>
 *       {children}
 *     </>
 *   );
 * }
 * ```
 */

import { useCallback, useEffect, useRef, useSyncExternalStore } from 'react';

import { useDarshanClient } from './provider';
import type { AuthState, AuthUnsubscribe, AuthUser } from './types';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Credentials accepted by {@link UseAuthResult.signIn} and {@link UseAuthResult.signUp}. */
export interface AuthCredentials {
  readonly email: string;
  readonly password: string;
  readonly displayName?: string;
}

/** Return value of {@link useAuth}. */
export interface UseAuthResult {
  /** The currently authenticated user, or `null` if signed out. */
  readonly user: AuthUser | null;
  /** `true` while the initial auth state is being resolved. */
  readonly isLoading: boolean;
  /** Non-null when the last auth action failed. */
  readonly error: Error | null;
  /**
   * Authenticate with email and password.
   * @returns The authenticated user on success.
   */
  readonly signIn: (credentials: AuthCredentials) => Promise<AuthUser>;
  /**
   * Create a new account.
   * @returns The newly created user on success.
   */
  readonly signUp: (credentials: AuthCredentials) => Promise<AuthUser>;
  /** Sign out the current user. */
  readonly signOut: () => Promise<void>;
}

// ---------------------------------------------------------------------------
// Internal store
// ---------------------------------------------------------------------------

interface AuthStore {
  snapshot: AuthState;
  error: Error | null;
  listeners: Set<() => void>;
  version: number;
}

function createAuthStore(initial: AuthState): AuthStore {
  return {
    snapshot: initial,
    error: null,
    listeners: new Set(),
    version: 0,
  };
}

interface AuthView {
  readonly user: AuthUser | null;
  readonly isLoading: boolean;
  readonly error: Error | null;
}

function getView(store: AuthStore): AuthView {
  return {
    user: store.snapshot.user,
    isLoading: store.snapshot.isLoading,
    error: store.error,
  };
}

function emit(store: AuthStore): void {
  store.version++;
  for (const l of store.listeners) l();
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

/**
 * Observe and control authentication state.
 *
 * Subscribes to `onAuthStateChange` from the client and exposes action
 * methods that are safe to call from event handlers.  All returned
 * references are stable across re-renders.
 *
 * @returns A {@link UseAuthResult} object.
 */
export function useAuth(): UseAuthResult {
  const client = useDarshanClient();
  const clientRef = useRef(client);
  clientRef.current = client;

  const storeRef = useRef<AuthStore | null>(null);
  if (!storeRef.current) {
    storeRef.current = createAuthStore(client.getAuthState());
  }
  const store = storeRef.current;

  // Memoised view -- only changes when store.version bumps.
  const viewRef = useRef<{ view: AuthView; version: number }>({
    view: getView(store),
    version: store.version,
  });

  // -----------------------------------------------------------------------
  // Subscribe to client auth state changes
  // -----------------------------------------------------------------------
  useEffect(() => {
    let unsub: AuthUnsubscribe | null = null;

    unsub = client.onAuthStateChange((state) => {
      store.snapshot = state;
      store.error = null;
      emit(store);
    });

    return () => {
      unsub?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client]);

  // -----------------------------------------------------------------------
  // useSyncExternalStore wiring
  // -----------------------------------------------------------------------
  const subscribe = useCallback(
    (onStoreChange: () => void) => {
      store.listeners.add(onStoreChange);
      return () => {
        store.listeners.delete(onStoreChange);
      };
    },
    [store],
  );

  const getSnapshot = useCallback(() => {
    if (viewRef.current.version !== store.version) {
      viewRef.current = { view: getView(store), version: store.version };
    }
    return viewRef.current.view;
  }, [store]);

  const getServerSnapshot = useCallback(
    (): AuthView => ({ user: null, isLoading: true, error: null }),
    [],
  );

  const { user, isLoading, error } = useSyncExternalStore(
    subscribe,
    getSnapshot,
    getServerSnapshot,
  );

  // -----------------------------------------------------------------------
  // Stable action callbacks
  // -----------------------------------------------------------------------
  const signIn = useCallback(async (creds: AuthCredentials): Promise<AuthUser> => {
    try {
      const authedUser = await clientRef.current.signIn({
        email: creds.email,
        password: creds.password,
      });
      return authedUser;
    } catch (err) {
      const authError = err instanceof Error ? err : new Error(String(err));
      store.error = authError;
      emit(store);
      throw authError;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const signUp = useCallback(async (creds: AuthCredentials): Promise<AuthUser> => {
    try {
      const newUser = await clientRef.current.signUp({
        email: creds.email,
        password: creds.password,
        displayName: creds.displayName,
      });
      return newUser;
    } catch (err) {
      const authError = err instanceof Error ? err : new Error(String(err));
      store.error = authError;
      emit(store);
      throw authError;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const signOut = useCallback(async (): Promise<void> => {
    try {
      await clientRef.current.signOut();
    } catch (err) {
      const authError = err instanceof Error ? err : new Error(String(err));
      store.error = authError;
      emit(store);
      throw authError;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return { user, isLoading, error, signIn, signUp, signOut };
}
