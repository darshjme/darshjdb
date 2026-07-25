'use client';

/**
 * @module use-query
 * @description Reactive data-fetching hook that subscribes to a DarshJDB
 * query and keeps the component in sync using `useSyncExternalStore`.
 *
 * @example
 * ```tsx
 * import { useQuery } from '@darshjdb/react';
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
import type {
  DarshanClientInterface,
  Query,
  QuerySnapshot,
  Unsubscribe,
} from './types';

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
  /** Serialised query this store belongs to, `null` before the first subscribe. */
  key: string | null;
  /** Number of mounted hooks using this store. */
  refs: number;
  previousData: ReadonlyArray<T>;
  suspensePromise: Promise<void> | null;
  resolveSuspense: (() => void) | null;
}

const EMPTY_DATA: ReadonlyArray<never> = Object.freeze([]);

function createStore<T>(isLoading: boolean): Store<T> {
  return {
    snapshot: { data: EMPTY_DATA as ReadonlyArray<T>, isLoading, error: null },
    listeners: new Set(),
    unsub: null,
    key: null,
    refs: 0,
    previousData: EMPTY_DATA as ReadonlyArray<T>,
    suspensePromise: null,
    resolveSuspense: null,
  };
}

function emitChange<T>(store: Store<T>): void {
  for (const l of store.listeners) l();
}

/** Tear down the live subscription, leaving the last snapshot intact. */
function stopSubscription<T>(store: Store<T>): void {
  store.unsub?.();
  store.unsub = null;
}

/**
 * Open a live subscription for `query`, resetting the store to its loading
 * state first.  Safe to call from render (suspense) or from an effect.
 */
function startSubscription<T>(
  store: Store<T>,
  client: DarshanClientInterface,
  query: Query<T>,
  key: string,
): void {
  stopSubscription(store);

  store.key = key;
  store.previousData = EMPTY_DATA as ReadonlyArray<T>;
  store.snapshot = { data: EMPTY_DATA as ReadonlyArray<T>, isLoading: true, error: null };

  store.unsub = client.subscribe<T>(query, (snap: QuerySnapshot<T>) => {
    const nextData = snap.error
      ? store.previousData
      : shallowArrayEqual(store.previousData, snap.data)
        ? store.previousData
        : snap.data;

    store.previousData = nextData;

    store.snapshot = {
      data: nextData,
      isLoading: false,
      error: snap.error,
    };

    // Release the suspense barrier on the first snapshot.
    if (store.resolveSuspense) {
      store.resolveSuspense();
      store.resolveSuspense = null;
      store.suspensePromise = null;
    }

    emitChange(store);
  });
}

// ---------------------------------------------------------------------------
// Suspense store cache
//
// A component that suspends never commits, so every hook ref it created is
// discarded before the retry render.  Suspense stores therefore live in a
// module-level cache keyed by client + query, so the retry re-attaches to the
// in-flight subscription instead of restarting (and re-suspending) forever.
// ---------------------------------------------------------------------------

const SUSPENSE_STORES = new WeakMap<DarshanClientInterface, Map<string, Store<unknown>>>();

function acquireSuspenseStore<T>(
  client: DarshanClientInterface,
  key: string,
  query: Query<T>,
): Store<T> {
  let byKey = SUSPENSE_STORES.get(client);
  if (!byKey) {
    byKey = new Map<string, Store<unknown>>();
    SUSPENSE_STORES.set(client, byKey);
  }

  const cached = byKey.get(key) as Store<T> | undefined;
  if (cached) return cached;

  const store = createStore<T>(true);
  byKey.set(key, store as unknown as Store<unknown>);

  // Created before subscribing so a synchronous first snapshot can resolve it.
  store.suspensePromise = new Promise<void>((resolve) => {
    store.resolveSuspense = resolve;
  });
  startSubscription(store, client, query, key);

  return store;
}

function releaseSuspenseStore<T>(
  client: DarshanClientInterface,
  key: string,
  store: Store<T>,
): void {
  const byKey = SUSPENSE_STORES.get(client);
  if (byKey?.get(key) === (store as unknown as Store<unknown>)) {
    byKey.delete(key);
  }
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
 * Subscribe to a DarshJDB query reactively.
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

  if (suspense && enabled) {
    // In suspense mode the subscription must start during render -- a
    // suspended component never commits, so an effect would never run to
    // resolve the barrier.
    if (!storeRef.current || storeRef.current.key !== queryKey) {
      storeRef.current = acquireSuspenseStore<T>(client, queryKey, stableQuery);
    }
  } else if (!storeRef.current) {
    // A disabled query is not loading -- it never starts.
    storeRef.current = createStore<T>(enabled);
  }

  const store = storeRef.current;

  // -----------------------------------------------------------------------
  // Subscribe / unsubscribe effect
  // -----------------------------------------------------------------------
  useEffect(() => {
    if (!enabled) {
      // Tear down any existing subscription when disabled and settle the
      // loading flag -- a disabled query never receives a snapshot.
      stopSubscription(store);
      if (store.snapshot.isLoading) {
        store.snapshot = { ...store.snapshot, isLoading: false };
        emitChange(store);
      }
      return;
    }

    store.refs += 1;

    // A render-phase (suspense) subscription for this exact query is adopted
    // as-is; anything else starts a fresh one.
    if (store.key !== queryKey || !store.unsub) {
      startSubscription(store, client, stableQuery, queryKey);
      emitChange(store);
    }

    return () => {
      store.refs -= 1;
      if (store.refs <= 0) {
        stopSubscription(store);
        releaseSuspenseStore(client, queryKey, store);
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, stableQuery, enabled, suspense, store]);

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

  // Server snapshot is a cached object (SSR shows loading for active queries).
  const serverSnapshot = useMemo<UseQueryResult<T>>(
    () => ({
      data: EMPTY_DATA as ReadonlyArray<T>,
      isLoading: enabled,
      error: null,
    }),
    [enabled],
  );
  const getServerSnapshot = useCallback(() => serverSnapshot, [serverSnapshot]);

  const result = useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);

  // -----------------------------------------------------------------------
  // Suspense integration
  // -----------------------------------------------------------------------
  if (suspense && enabled && result.isLoading && store.suspensePromise) {
    throw store.suspensePromise;
  }

  return result;
}
