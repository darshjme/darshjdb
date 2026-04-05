/**
 * @module use-mutation
 * @description Hook that returns a stable `mutate` function for performing
 * insert, update, and delete operations against DarshJDB.
 *
 * Optimistic updates are handled at the client-core layer -- mutations
 * applied locally appear instantly in any active `useQuery` subscription
 * and are rolled back automatically if the server rejects the write.
 *
 * @example
 * ```tsx
 * import { useMutation } from '@darshjdb/react';
 *
 * function AddTodo() {
 *   const { mutate, isLoading, error } = useMutation();
 *
 *   const handleAdd = async () => {
 *     await mutate({
 *       type: 'insert',
 *       collection: 'todos',
 *       data: { title: 'New task', done: false },
 *     });
 *   };
 *
 *   return (
 *     <>
 *       <button onClick={handleAdd} disabled={isLoading}>Add</button>
 *       {error && <p>Error: {error.message}</p>}
 *     </>
 *   );
 * }
 * ```
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import { useDarshanClient } from './provider';
import type { MutationOperation } from './types';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Return value of {@link useMutation}. */
export interface UseMutationResult {
  /**
   * Execute one or more mutation operations.
   *
   * @param ops - A single operation or an array of operations to execute
   *   atomically (all-or-nothing).
   * @returns Resolves when the server acknowledges the write.
   * @throws Rejects with an `Error` when the mutation fails.  The error is
   *   also surfaced reactively via the `error` field.
   */
  readonly mutate: (ops: MutationOperation | ReadonlyArray<MutationOperation>) => Promise<void>;
  /** `true` while a mutation is in-flight. */
  readonly isLoading: boolean;
  /** Non-null when the most recent mutation failed. */
  readonly error: Error | null;
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

/**
 * Provides a stable mutation function backed by the nearest
 * `DarshanProvider`.
 *
 * The `mutate` reference is guaranteed stable across re-renders (referential
 * identity never changes) so it is safe to pass as a prop or include in
 * dependency arrays without causing unnecessary effects.
 *
 * @returns A {@link UseMutationResult} object.
 */
export function useMutation(): UseMutationResult {
  const client = useDarshanClient();

  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);

  // Keep the latest client in a ref so the stable callback never goes stale.
  const clientRef = useRef(client);
  clientRef.current = client;

  // Track mount status to avoid state updates after unmount.
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Stable mutate callback.
  const mutate = useCallback(
    async (ops: MutationOperation | ReadonlyArray<MutationOperation>): Promise<void> => {
      setError(null);
      setIsLoading(true);
      try {
        await clientRef.current.mutate(ops);
      } catch (err) {
        const mutationError =
          err instanceof Error ? err : new Error(String(err));
        if (mountedRef.current) {
          setError(mutationError);
        }
        throw mutationError;
      } finally {
        if (mountedRef.current) {
          setIsLoading(false);
        }
      }
    },
    [], // Stable -- clientRef indirection means no deps needed.
  );

  return { mutate, isLoading, error };
}
