/**
 * @module use-query
 * @description Reactive data-fetching hook that subscribes to a DarshanDB
 * query and keeps the component in sync using `useSyncExternalStore`.
 *
 * @example
 * ```tsx
 * import { useQuery } from '@darshan/react';
 *
 * function TodoList() {
 *   const { data, isLoading, error } = useQuery({
 *     collection: 'todos',
 *     where: [{ field: 'done', op: '==', value: false }],
 *     orderBy: [{ field: 'createdAt', direction: 'desc' }],
 *     limit: 50,
 *   });
 *
 *   if (isLoading) return <p>Loading...</p>;
 *   if (error) return <p>Error: {error.message}</p>;
 *
 *   return (
 *     <ul>
 *       {data.map(todo => <li key={todo.id}>{todo.title}</li>)}
 *     </ul>
 *   );
 * }
 * ```
 */

import { useCallback, useEffect, useMemo, useRef, useSyncExternalStore } from 'react';

import { useDarshanClient } from './provider';
import type { Query, QuerySnapshot, Unsubscribe } from './types';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Options for {@link useQuery}. */
export interface UseQueryOptions {
  /**
   * When `true`, the hook throws the pending promise during initial load
   * so a React `<Suspense>` boundary can catch it.
   *
   * @default false
   */
  readonly suspense?: boolean;
  /**
   * When `false`, the subscription is paused and the last snapshot is
   * retained.  Useful for conditionally disabling queries.
   *
   * @default true
   */
  readonly enabled?: boolean;
}

/** Return value of {@link useQuery}. */
export interface UseQueryResult<T> {
  /** The current result set.  Empty array while loading. */
  readonly data: ReadonlyArray<T>;
  /** `true` until the first snapshot arrives. */
  readonly isLoading: boolean;
  /** Non-null when the subscription encountered an error. */
  readonly error: Error | null;
}

// ---------------------------------------------------------------------------
// Shallow comparison for snapshot stability
// ---------------------------------------------------------------------------

function shallowArrayEqual<T>(a: ReadonlyArray<T>, b: ReadonlyArray<T>): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

// ---------------------------------------------------------------------------
// Internal store (one per hook instance)
// ---------------------------------------------------------------------------

interface Store<T> {
  snapshot: UseQueryResult<T>;
  listeners: Set<() => void>;
  unsub: Unsubscribe | null;
  suspensePromise: Promise<void> | null;
}

const EMPTY_DATA: ReadonlyArray<never> = Object.freeze([]);

function createStore<T>(): Store<T> {
  return {
    snapshot: { data: EMPTY_DATA as ReadonlyArray<T>, isLoading: true, error: null },
    listeners: new Set(),
    unsub: null,
    suspensePromise: null,
  };
}

function emitChange<T>(store: Store<T>): void {
  for (const l of store.listeners) l();
}

// ---------------------------------------------------------------------------
// Stable query serialisation (for memoisation key)
// ---------------------------------------------------------------------------

function serialiseQuery<T>(q: Query<T>): string {
  return JSON.stringify(q);
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

/**
 * Subscribe to a DarshanDB query reactively.
 *
 * Uses `useSyncExternalStore` under the hood so it is safe for concurrent
 * rendering (React 18+).  The returned data reference is stable across
 * re-renders when the contents have not changed (shallow array comparison).
 *
 * @typeParam T - The document shape returned by the query.
 * @param query - A {@link Query} descriptor.
 * @param options - Optional {@link UseQueryOptions}.
 * @returns A {@link UseQueryResult} object.
 */
export function useQuery<T = Record<string, unknown>>(
  query: Query<T>,
  options: UseQueryOptions = {},
): UseQueryResult<T> {
  const { suspense = false, enabled = true } = options;
  const client = useDarshanClient();

  // Stable identity for the query object across renders.
  const queryKey = serialiseQuery(query);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const stableQuery = useMemo(() => query, [queryKey]);

  // Persistent store ref (survives re-renders, not re-mounts).
  const storeRef = useRef<Store<T> | null>(null);
  if (!storeRef.current) {
    storeRef.current = createStore<T>();
  }
  const store = storeRef.current;

  // Resolve suspense promise when first data arrives.
  const suspenseResolveRef = useRef<(() => void) | null>(null);

  // -----------------------------------------------------------------------
  // Subscribe / unsubscribe effect
  // -----------------------------------------------------------------------
  useEffect(() => {
    if (!enabled) {
      // Tear down any existing subscription when disabled.
      store.unsub?.();
      store.unsub = null;
      return;
    }

    // Reset loading state on new subscription.
    store.snapshot = { data: EMPTY_DATA as ReadonlyArray<T>, isLoading: true, error: null };
    emitChange(store);

    // Wire up suspense promise if needed.
    if (suspense && !store.suspensePromise) {
      store.suspensePromise = new Promise<void>((resolve) => {
        suspenseResolveRef.current = resolve;
      });
    }

    const previousData = { current: EMPTY_DATA as ReadonlyArray<T> };

    store.unsub = client.subscribe<T>(stableQuery, (snap: QuerySnapshot<T>) => {
      const nextData = snap.error
        ? previousData.current
        : shallowArrayEqual(previousData.current, snap.data)
          ? previousData.current
          : snap.data;

      previousData.current = nextData;

      store.snapshot = {
        data: nextData,
        isLoading: false,
        error: snap.error,
      };

      // Resolve suspense barrier on first successful snapshot.
      if (suspenseResolveRef.current) {
        suspenseResolveRef.current();
        suspenseResolveRef.current = null;
        store.suspensePromise = null;
      }

      emitChange(store);
    });

    return () => {
      store.unsub?.();
      store.unsub = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, stableQuery, enabled, suspense]);

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

  const getSnapshot = useCallback(() => store.snapshot, [store]);

  // Server snapshot returns the loading state (SSR will show loading).
  const getServerSnapshot = useCallback(
    (): UseQueryResult<T> => ({
      data: EMPTY_DATA as ReadonlyArray<T>,
      isLoading: true,
      error: null,
    }),
    [],
  );

  const result = useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);

  // -----------------------------------------------------------------------
  // Suspense integration
  // -----------------------------------------------------------------------
  if (suspense && result.isLoading && store.suspensePromise) {
    throw store.suspensePromise;
  }

  return result;
}
