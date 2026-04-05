/**
 * @module query.signal
 * @description Signal-based reactive queries for Angular 17+.
 *
 * Provides `darshanQuery()`, a function that subscribes to a live
 * DarshJDB query and exposes the result as Angular Signals. The
 * subscription is automatically cleaned up via `DestroyRef`.
 *
 * Designed for zoneless / `OnPush` components where fine-grained
 * reactivity eliminates the need for `markForCheck()` or `async` pipes.
 *
 * @example
 * ```typescript
 * import { darshanQuery } from '@darshjdb/angular';
 *
 * @Component({
 *   template: `
 *     @if (todos.isLoading()) {
 *       <spinner />
 *     } @else if (todos.error()) {
 *       <error [message]="todos.error()!.message" />
 *     } @else {
 *       @for (todo of todos.data(); track todo.id) {
 *         <todo-item [todo]="todo" />
 *       }
 *     }
 *   `,
 * })
 * export class TodoListComponent {
 *   readonly todos = darshanQuery<Todo[]>('todos', { where: { done: false } });
 * }
 * ```
 */

import {
  inject,
  signal,
  DestroyRef,
  type Signal,
  type WritableSignal,
} from '@angular/core';

import { DDB_CLIENT } from './tokens';
import type { DarshJError, QueryOptions } from './types';

/**
 * Signal-based query result.
 *
 * Each property is an independent `Signal` for optimal change detection:
 * reading `data` does not trigger re-render when only `isLoading` changes.
 */
export interface SignalQueryResult<T> {
  /** The current query data, or `undefined` while loading. */
  readonly data: Signal<T | undefined>;
  /** Whether the query is actively loading or reconnecting. */
  readonly isLoading: Signal<boolean>;
  /** The query error, or `null` when healthy. */
  readonly error: Signal<DarshJError | null>;
  /**
   * Manually re-execute the query, discarding cached state.
   * Useful after an optimistic mutation to force a server round-trip.
   */
  readonly refetch: () => void;
}

/**
 * Subscribe to a live DarshJDB query with Angular Signal output.
 *
 * The subscription opens a WebSocket channel to the DarshJDB server
 * and receives real-time diff patches. On each update, the `data`
 * signal is set to the latest result.
 *
 * **Lifecycle:** The subscription is automatically cancelled when the
 * calling component/service is destroyed (via `DestroyRef`).
 *
 * @typeParam T - Expected shape of the query result.
 * @param collection - The collection/table to query.
 * @param query - Filter, projection, and sort parameters.
 * @param options - Optional query configuration.
 * @returns A {@link SignalQueryResult} with reactive state.
 *
 * @example
 * ```typescript
 * // Parameterized query that reacts to input changes:
 * @Component({ ... })
 * export class UserProfile {
 *   readonly userId = input.required<string>();
 *   readonly profile = darshanQuery<User>('users', { where: { id: this.userId } });
 * }
 * ```
 */
export function darshanQuery<T>(
  collection: string,
  query: Record<string, unknown>,
  options?: QueryOptions,
): SignalQueryResult<T> {
  const client = inject(DDB_CLIENT);
  const destroyRef = inject(DestroyRef);

  const _data: WritableSignal<T | undefined> = signal<T | undefined>(undefined);
  const _isLoading = signal(true);
  const _error = signal<DarshJError | null>(null);

  let _unsubscribe: (() => void) | null = null;

  /**
   * Start (or restart) the subscription.
   * @internal
   */
  function subscribe(): void {
    // Clean up any existing subscription before re-subscribing.
    _unsubscribe?.();

    if (!options?.skipInitialFetch) {
      _isLoading.set(true);
    }
    _error.set(null);

    let debounceTimer: ReturnType<typeof setTimeout> | null = null;

    _unsubscribe = client.subscribe<T>(
      collection,
      query,
      (result) => {
        const applyUpdate = () => {
          if (result.error) {
            _error.set({
              code: result.error.code,
              message: result.error.message,
              status: result.error.status,
            });
          } else {
            _data.set(result.data);
            _error.set(null);
          }
          _isLoading.set(false);
        };

        if (options?.debounceMs && options.debounceMs > 0) {
          if (debounceTimer !== null) {
            clearTimeout(debounceTimer);
          }
          debounceTimer = setTimeout(applyUpdate, options.debounceMs);
        } else {
          applyUpdate();
        }
      },
    );
  }

  // Initial subscription.
  subscribe();

  // Auto-cleanup on destroy.
  destroyRef.onDestroy(() => {
    _unsubscribe?.();
    _unsubscribe = null;
  });

  return {
    data: _data.asReadonly(),
    isLoading: _isLoading.asReadonly(),
    error: _error.asReadonly(),
    refetch: () => subscribe(),
  };
}
