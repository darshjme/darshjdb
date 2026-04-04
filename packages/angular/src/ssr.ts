/**
 * @module ssr
 * @description Server-Side Rendering (SSR) support via Angular's TransferState.
 *
 * Provides `darshanTransferQuery()` which executes a query on the server,
 * serializes the result into `TransferState`, and on the client hydrates
 * from the transfer cache before opening a live WebSocket subscription.
 *
 * This avoids a flash of empty content during SSR hydration and prevents
 * duplicate server requests.
 *
 * @example
 * ```typescript
 * import { darshanTransferQuery } from '@darshan/angular';
 *
 * @Component({
 *   template: `
 *     @if (products.isLoading()) {
 *       <skeleton-list />
 *     } @else {
 *       @for (p of products.data(); track p.id) {
 *         <product-card [product]="p" />
 *       }
 *     }
 *   `,
 * })
 * export class ProductListComponent {
 *   readonly products = darshanTransferQuery<Product[]>('products', {
 *     where: { active: true },
 *     limit: 20,
 *   });
 * }
 * ```
 */

import {
  inject,
  signal,
  makeStateKey,
  TransferState,
  PLATFORM_ID,
  DestroyRef,
  type Signal,
  type WritableSignal,
} from '@angular/core';
import { isPlatformServer, isPlatformBrowser } from '@angular/common';

import { DARSHAN_CLIENT } from './tokens';
import type { DarshanError, QueryOptions } from './types';
import { DARSHAN_TRANSFER_KEY_PREFIX } from './types';

/**
 * Result shape for SSR-aware queries. Identical to `SignalQueryResult`
 * but documented separately to emphasize the hydration behavior.
 */
export interface TransferQueryResult<T> {
  /** Current data. Populated from TransferState on the client before the WS connects. */
  readonly data: Signal<T | undefined>;
  /** Whether the query is loading. `false` immediately on client if transfer data exists. */
  readonly isLoading: Signal<boolean>;
  /** Query error, or `null`. */
  readonly error: Signal<DarshanError | null>;
  /** Whether the data was hydrated from TransferState (client-side only). */
  readonly hydrated: Signal<boolean>;
  /** Re-execute the query. */
  readonly refetch: () => void;
}

/**
 * Build a deterministic transfer state key from collection and query.
 *
 * @internal
 */
function buildTransferKey(
  collection: string,
  query: Record<string, unknown>,
): string {
  // JSON.stringify with sorted keys for deterministic output.
  const queryStr = JSON.stringify(query, Object.keys(query).sort());
  return `${DARSHAN_TRANSFER_KEY_PREFIX}${collection}_${simpleHash(queryStr)}`;
}

/**
 * Simple string hash for transfer state key uniqueness.
 * Not cryptographic — just needs to be deterministic and fast.
 *
 * @internal
 */
function simpleHash(str: string): string {
  let hash = 0;
  for (let i = 0; i < str.length; i++) {
    const char = str.charCodeAt(i);
    hash = ((hash << 5) - hash + char) | 0;
  }
  return Math.abs(hash).toString(36);
}

/**
 * Execute a DarshanDB query with TransferState integration for SSR.
 *
 * **Server behavior:**
 * 1. Executes a one-shot query via `client.query()`.
 * 2. Stores the result in Angular's `TransferState`.
 * 3. Returns the data synchronously in a signal.
 *
 * **Client behavior:**
 * 1. Checks `TransferState` for a cached result.
 * 2. If found: populates `data` immediately, sets `hydrated` to `true`,
 *    then opens a live subscription for real-time updates.
 * 3. If not found: behaves identically to `darshanQuery()`.
 *
 * @typeParam T - Expected shape of the query result.
 * @param collection - The collection/table to query.
 * @param query - Filter, projection, and sort parameters.
 * @param options - Optional query configuration.
 * @returns A {@link TransferQueryResult} with SSR-aware signals.
 */
export function darshanTransferQuery<T>(
  collection: string,
  query: Record<string, unknown>,
  options?: QueryOptions,
): TransferQueryResult<T> {
  const client = inject(DARSHAN_CLIENT);
  const transferState = inject(TransferState);
  const platformId = inject(PLATFORM_ID);
  const destroyRef = inject(DestroyRef);

  const stateKey = makeStateKey<T>(buildTransferKey(collection, query));

  const _data: WritableSignal<T | undefined> = signal<T | undefined>(undefined);
  const _isLoading = signal(true);
  const _error = signal<DarshanError | null>(null);
  const _hydrated = signal(false);

  let _unsubscribe: (() => void) | null = null;

  // ── Server path ──────────────────────────────────────────────

  if (isPlatformServer(platformId)) {
    // On the server we do a one-shot query and store the result.
    client
      .query<T>(collection, query)
      .then((data) => {
        _data.set(data);
        _isLoading.set(false);
        transferState.set(stateKey, data);
      })
      .catch((err: unknown) => {
        _error.set({
          code: 'SSR_QUERY_FAILED',
          message: err instanceof Error ? err.message : String(err),
          cause: err,
        });
        _isLoading.set(false);
      });

    return {
      data: _data.asReadonly(),
      isLoading: _isLoading.asReadonly(),
      error: _error.asReadonly(),
      hydrated: _hydrated.asReadonly(),
      refetch: () => {
        // No-op on server — SSR queries are one-shot.
      },
    };
  }

  // ── Client path ──────────────────────────────────────────────

  // Attempt hydration from transfer state.
  if (isPlatformBrowser(platformId) && transferState.hasKey(stateKey)) {
    const cached = transferState.get(stateKey, undefined as unknown as T);
    if (cached !== undefined) {
      _data.set(cached);
      _isLoading.set(false);
      _hydrated.set(true);
      // Remove from transfer state to prevent stale data on future navigations.
      transferState.remove(stateKey);
    }
  }

  /**
   * Open the live subscription.
   * @internal
   */
  function subscribe(): void {
    _unsubscribe?.();
    _error.set(null);

    // If we hydrated, don't show loading — we already have data.
    if (!_hydrated()) {
      _isLoading.set(true);
    }

    _unsubscribe = client.subscribe<T>(
      collection,
      query,
      (result) => {
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
      },
    );
  }

  // Start the live subscription (even if hydrated, for real-time updates).
  if (!options?.skipInitialFetch) {
    subscribe();
  }

  destroyRef.onDestroy(() => {
    _unsubscribe?.();
    _unsubscribe = null;
  });

  return {
    data: _data.asReadonly(),
    isLoading: _isLoading.asReadonly(),
    error: _error.asReadonly(),
    hydrated: _hydrated.asReadonly(),
    refetch: () => subscribe(),
  };
}
