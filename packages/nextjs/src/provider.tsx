'use client';

/**
 * @module @darshjdb/nextjs/provider
 *
 * Client-side DarshJDB provider for Next.js App Router.
 * Wraps `@darshjdb/react` with automatic environment configuration.
 *
 * @example
 * ```tsx
 * // app/layout.tsx
 * import { DarshanProvider } from '@darshjdb/nextjs/provider';
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
 * // With an app id and a per-user token
 * import { DarshanProvider } from '@darshjdb/nextjs/provider';
 *
 * <DarshanProvider appId="my-app" token={sessionToken}>
 *   {children}
 * </DarshanProvider>
 * ```
 */

import React, { useEffect, type ReactNode } from 'react';
import {
  DarshanProvider as DarshanReactProvider,
  useDarshanClient,
} from '@darshjdb/react';

// ---------------------------------------------------------------------------
// Provider Props
// ---------------------------------------------------------------------------

/** Configuration for the DarshJDB Next.js provider. */
export interface DarshanProviderProps {
  /** Child components to render within the provider tree. */
  children: ReactNode;

  /**
   * DarshJDB server URL. Defaults to `NEXT_PUBLIC_DDB_URL` env var.
   */
  url?: string;

  /**
   * Public application identifier. Defaults to `NEXT_PUBLIC_DDB_APP_ID` env var.
   * This value is part of every request URL, so it must never hold a secret.
   */
  appId?: string;

  /**
   * Client authentication token. Defaults to `NEXT_PUBLIC_DDB_TOKEN` env var.
   * For user-specific tokens, pass dynamically after authentication.
   *
   * Sent as an `Authorization: Bearer` credential — never placed in a URL.
   */
  token?: string;
}

// ---------------------------------------------------------------------------
// Provider Component
// ---------------------------------------------------------------------------

/**
 * Bind the auth token to the client created by `@darshjdb/react`.
 *
 * Runs as a child of the provider so its effect fires before the provider's
 * own connect effect — the token is in place before the first request.
 * @internal
 */
function TokenBinder({
  token,
  children,
}: {
  token: string | undefined;
  children: ReactNode;
}): React.JSX.Element {
  const client = useDarshanClient() as {
    setAuthToken?: (token: string | null) => void;
  };

  useEffect(() => {
    client.setAuthToken?.(token ?? null);
  }, [client, token]);

  return <>{children}</>;
}

/**
 * Root provider for DarshJDB in Next.js applications.
 *
 * Wraps `@darshjdb/react`'s provider with:
 * - Automatic environment variable configuration
 * - Bearer-token authentication
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
  appId,
  token,
}: DarshanProviderProps): React.JSX.Element {
  // Resolve configuration from props or environment variables
  const resolvedUrl =
    url ?? process.env.NEXT_PUBLIC_DDB_URL ?? '';
  const resolvedAppId =
    appId ?? process.env.NEXT_PUBLIC_DDB_APP_ID ?? 'nextjs-app';
  const resolvedToken =
    token ?? process.env.NEXT_PUBLIC_DDB_TOKEN ?? undefined;

  // Maintain a stable client reference across renders.
  if (!resolvedUrl) {
    throw new Error(
      '[DarshJDB] No URL provided. Set the `url` prop or the ' +
        'NEXT_PUBLIC_DDB_URL environment variable.',
    );
  }

  return (
    <DarshanReactProvider serverUrl={resolvedUrl} appId={resolvedAppId}>
      <TokenBinder token={resolvedToken}>{children}</TokenBinder>
    </DarshanReactProvider>
  );
}

// ---------------------------------------------------------------------------
// Re-exports for convenience
// ---------------------------------------------------------------------------

// React provider is wrapped by this module's DarshanProvider above
