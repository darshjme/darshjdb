/**
 * @module query.observable
 * @description Observable-based reactive queries for DarshanDB.
 *
 * Provides `darshanQuery$()`, an RxJS-native query function that wraps
 * the DarshanDB WebSocket subscription as an `Observable`. Best suited
 * for Angular applications that rely heavily on RxJS pipelines.
 *
 * The Observable variant uses `shareReplay(1)` by default so late
 * subscribers immediately receive the most recent value without
 * triggering a duplicate server subscription.
 *
 * @example
 * ```typescript
 * import { darshanQuery$ } from '@darshan/angular';
 *
 * @Component({
 *   template: `
 *     <ul>
 *       <li *ngFor="let todo of todos$ | async">{{ todo.title }}</li>
 *     </ul>
 *   `,
 * })
 * export class TodoListComponent {
 *   readonly todos$ = darshanQuery$<Todo[]>('todos', { where: { done: false } });
 * }
 * ```
 */

import { inject } from '@angular/core';
import { Observable, shareReplay, debounceTime, type OperatorFunction } from 'rxjs';

import { DARSHAN_CLIENT } from './tokens';
import type { DarshanError, QueryOptions } from './types';

/**
 * Observable query result payload.
 *
 * Emitted on each update from the live query subscription.
 * Unlike the signal API, error states are delivered as emissions
 * (not thrown), allowing `switchMap`/`catchError` composition.
 */
export interface ObservableQueryResult<T> {
  /** The current query data. */
  readonly data: T;
  /** The query error, or `null` when healthy. */
  readonly error: DarshanError | null;
}

/**
 * Subscribe to a live DarshanDB query as an RxJS Observable.
 *
 * Each emission contains the latest query result. The underlying
 * WebSocket subscription is reference-counted: it opens when the
 * first subscriber appears and closes when the last unsubscribes.
 *
 * @typeParam T - Expected shape of the query result.
 * @param collection - The collection/table to query.
 * @param query - Filter, projection, and sort parameters.
 * @param options - Optional query configuration.
 * @returns An `Observable` that emits {@link ObservableQueryResult} on each update.
 *
 * @remarks
 * The returned Observable has `shareReplay({ bufferSize: 1, refCount: true })`
 * applied by default. This means:
 * - Late subscribers get the last emitted value immediately.
 * - The WebSocket subscription is torn down when all subscribers unsubscribe.
 * - No memory leak from unbounded replay buffers.
 *
 * @example
 * ```typescript
 * // Compose with RxJS operators:
 * readonly activeTodos$ = darshanQuery$<Todo[]>('todos', { where: { done: false } }).pipe(
 *   map(result => result.data.filter(t => t.priority > 3)),
 * );
 * ```
 */
export function darshanQuery$<T>(
  collection: string,
  query: Record<string, unknown>,
  options?: QueryOptions,
): Observable<ObservableQueryResult<T>> {
  const client = inject(DARSHAN_CLIENT);

  const source$ = new Observable<ObservableQueryResult<T>>((subscriber) => {
    const unsubscribe = client.subscribe<T>(
      collection,
      query,
      (result) => {
        if (result.error) {
          subscriber.next({
            data: result.data,
            error: {
              code: result.error.code,
              message: result.error.message,
              status: result.error.status,
            },
          });
        } else {
          subscriber.next({ data: result.data, error: null });
        }
      },
    );

    // Teardown: unsubscribe from the DarshanDB query when the
    // Observable subscription is cancelled.
    return () => {
      unsubscribe();
    };
  });

  // Build the operator pipeline.
  const operators: OperatorFunction<ObservableQueryResult<T>, ObservableQueryResult<T>>[] = [];

  if (options?.debounceMs && options.debounceMs > 0) {
    operators.push(debounceTime(options.debounceMs));
  }

  // Always apply shareReplay for multicast + late-subscriber support.
  operators.push(
    shareReplay({ bufferSize: 1, refCount: true }),
  );

  // Apply operators in sequence.
  let piped$: Observable<ObservableQueryResult<T>> = source$;
  for (const op of operators) {
    piped$ = piped$.pipe(op);
  }

  return piped$;
}

/**
 * Execute a one-shot DarshanDB query as an Observable.
 *
 * Unlike `darshanQuery$`, this does **not** open a live subscription.
 * It performs a single request and completes.
 *
 * @typeParam T - Expected shape of the query result.
 * @param collection - The collection/table to query.
 * @param query - Filter, projection, and sort parameters.
 * @returns An `Observable<T>` that emits once and completes.
 *
 * @example
 * ```typescript
 * readonly userCount$ = darshanQueryOnce$<number>('users', { count: true });
 * ```
 */
export function darshanQueryOnce$<T>(
  collection: string,
  query: Record<string, unknown>,
): Observable<T> {
  const client = inject(DARSHAN_CLIENT);

  return new Observable<T>((subscriber) => {
    client
      .query<T>(collection, query)
      .then((data) => {
        subscriber.next(data);
        subscriber.complete();
      })
      .catch((err: unknown) => {
        subscriber.error(err);
      });
  });
}

/**
 * Execute a DarshanDB mutation as an Observable.
 *
 * Wraps `client.mutate()` in an Observable for seamless integration
 * with RxJS pipelines (`switchMap`, `mergeMap`, etc.).
 *
 * @typeParam T - Expected shape of the mutation result.
 * @param collection - The collection/table to mutate.
 * @param mutation - The mutation descriptor.
 * @returns An `Observable<T>` that emits the mutation result and completes.
 *
 * @example
 * ```typescript
 * this.addTodo$.pipe(
 *   switchMap(title => darshanMutate$<Todo>('todos', { insert: { title, done: false } })),
 * ).subscribe(todo => console.log('Created:', todo.id));
 * ```
 */
export function darshanMutate$<T>(
  collection: string,
  mutation: Record<string, unknown>,
): Observable<T> {
  const client = inject(DARSHAN_CLIENT);

  return new Observable<T>((subscriber) => {
    client
      .mutate<T>(collection, mutation)
      .then((data) => {
        subscriber.next(data);
        subscriber.complete();
      })
      .catch((err: unknown) => {
        subscriber.error(err);
      });
  });
}
