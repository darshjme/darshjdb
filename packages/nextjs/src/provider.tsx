'use client';

/**
 * @module @darshan/nextjs/provider
 *
 * Client-side DarshanDB provider for Next.js App Router.
 * Wraps `@darshan/react` with automatic environment configuration
 * and SSR hydration support.
 *
 * @example
 * ```tsx
 * // app/layout.tsx
 * import { DarshanProvider } from '@darshan/nextjs/provider';
 *
 * export default function RootLayout({ children }: { children: React.ReactNode }) {
 *   return (
 *     <html>
 *       <body>
 *         <DarshanProvider>
 *           {children}
 *         </DarshanProvider>
 *       </body>
 *     </html>
 *   );
 * }
 * ```
 *
 * @example
 * ```tsx
 * // With SSR hydration from a Server Component
 * import { DarshanProvider } from '@darshan/nextjs/provider';
 * import { dehydrate } from '@darshan/nextjs/provider';
 * import { queryServer } from '@darshan/nextjs/server';
 *
 * export default async function Layout({ children }: { children: React.ReactNode }) {
 *   const users = await queryServer({ collection: 'users' });
 *   const dehydratedState = dehydrate({ users: { data: users } });
 *
 *   return (
 *     <DarshanProvider dehydratedState={dehydratedState}>
 *       {children}
 *     </DarshanProvider>
 *   );
 * }
 * ```
 */

import React, { type ReactNode } from 'react';
import { DarshanProvider as DarshanReactProvider } from '@darshan/react';

// ---------------------------------------------------------------------------
// Dehydration / Hydration types
// ---------------------------------------------------------------------------

/** A single cache entry keyed by query identifier. */
export interface DehydratedCacheEntry {
  /** The query result data. */
  data: unknown;
  /** Timestamp when the data was fetched (ms since epoch). */
  fetchedAt?: number;
}

/**
 * Serializable snapshot of server-side query results.
 * Passed from Server Components to the client for hydration.
 */
export interface DehydratedState {
  /** Map of cache key to entry. */
  queries: Record<string, DehydratedCacheEntry>;
  /** ISO timestamp of when dehydration occurred. */
  timestamp: string;
}

// ---------------------------------------------------------------------------
// Dehydrate helper (runs on server, result is serialized to client)
// ---------------------------------------------------------------------------

/**
 * Dehydrate server-fetched data into a serializable snapshot that the
 * `DarshanProvider` can hydrate on the client. Call this in a Server
 * Component and pass the result as `dehydratedState` prop.
 *
 * @param queries - Map of cache keys to their data. Each value should
 *                  contain at minimum a `data` property.
 * @returns A serializable `DehydratedState` object.
 *
 * @example
 * ```ts
 * const state = dehydrate({
 *   users: { data: await queryServer({ collection: 'users' }) },
 *   config: { data: await queryServer({ collection: 'config' }) },
 * });
 * ```
 */
export function dehydrate(
  queries: Record<string, { data: unknown; fetchedAt?: number }>,
): DehydratedState {
  const entries: Record<string, DehydratedCacheEntry> = {};
  const now = Date.now();

  for (const [key, value] of Object.entries(queries)) {
    entries[key] = {
      data: value.data,
      fetchedAt: value.fetchedAt ?? now,
    };
  }

  return {
    queries: entries,
    timestamp: new Date(now).toISOString(),
  };
}

// ---------------------------------------------------------------------------
// Provider Props
// ---------------------------------------------------------------------------

/** Configuration for the DarshanDB Next.js provider. */
export interface DarshanProviderProps {
  /** Child components to render within the provider tree. */
  children: ReactNode;

  /**
   * DarshanDB server URL. Defaults to `NEXT_PUBLIC_DARSHAN_URL` env var.
   */
  url?: string;

  /**
   * Client authentication token. Defaults to `NEXT_PUBLIC_DARSHAN_TOKEN` env var.
   * For user-specific tokens, pass dynamically after authentication.
   */
  token?: string;

  /**
   * Additional client configuration passed to `createClient()`.
   */
  clientConfig?: Record<string, unknown>;

  /**
   * Dehydrated server state for SSR hydration.
   * Generate with the `dehydrate()` function in a Server Component.
   */
  dehydratedState?: DehydratedState;

  /**
   * Enable real-time subscriptions. Defaults to `true`.
   */
  realtime?: boolean;

  /**
   * Enable offline persistence. Defaults to `false`.
   */
  offline?: boolean;
}

// ---------------------------------------------------------------------------
// Provider Component
// ---------------------------------------------------------------------------

/**
 * Root provider for DarshanDB in Next.js applications.
 *
 * Wraps `@darshan/react`'s provider with:
 * - Automatic environment variable configuration
 * - SSR hydration of server-fetched data
 * - Singleton client management across re-renders
 *
 * Place this in your root layout (`app/layout.tsx`) or wrap individual
 * route segments as needed.
 *
 * @param props - Provider configuration. See {@link DarshanProviderProps}.
 */
export function DarshanProvider({
  children,
  url,
  token,
  clientConfig: _clientConfig,
  dehydratedState: _dehydratedState,
  realtime: _realtime = true,
  offline: _offline = false,
}: DarshanProviderProps): React.JSX.Element {
  // Resolve configuration from props or environment variables
  const resolvedUrl =
    url ?? process.env.NEXT_PUBLIC_DARSHAN_URL ?? '';
  const resolvedToken =
    token ?? process.env.NEXT_PUBLIC_DARSHAN_TOKEN ?? undefined;

  // Maintain a stable client reference across renders.
  if (!resolvedUrl) {
    throw new Error(
      '[DarshanDB] No URL provided. Set the `url` prop or the ' +
        'NEXT_PUBLIC_DARSHAN_URL environment variable.',
    );
  }

  return (
    <DarshanReactProvider serverUrl={resolvedUrl} appId={resolvedToken ?? 'nextjs-app'}>
      {children}
    </DarshanReactProvider>
  );
}

// ---------------------------------------------------------------------------
// Re-exports for convenience
// ---------------------------------------------------------------------------

// React provider is wrapped by this module's DarshanProvider above
