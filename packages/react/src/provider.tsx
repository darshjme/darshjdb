/**
 * @module provider
 * @description React context provider that initialises and distributes a
 * `DarshanClient` instance to all descendant hooks.
 *
 * @example
 * ```tsx
 * import { DarshanProvider } from '@darshan/react';
 *
 * function App() {
 *   return (
 *     <DarshanProvider serverUrl="https://db.example.com" appId="my-app">
 *       <MyComponent />
 *     </DarshanProvider>
 *   );
 * }
 * ```
 */

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  type ReactNode,
} from 'react';

import type { DarshanClientInterface, DarshanClientOptions } from './types';

// ---------------------------------------------------------------------------
// Lazy import helper -- allows tree-shaking when the React SDK is loaded
// without `@darshan/client` being bundled at definition time.
// ---------------------------------------------------------------------------

let _createClient: ((opts: DarshanClientOptions) => DarshanClientInterface) | null = null;

/**
 * Resolve the `createClient` factory from `@darshan/client`.
 * Throws a clear error if the dependency is missing at runtime.
 */
function getCreateClient(): (opts: DarshanClientOptions) => DarshanClientInterface {
  if (_createClient) return _createClient;

  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const mod = require('@darshan/client') as {
      createClient?: (opts: DarshanClientOptions) => DarshanClientInterface;
      DarshanClient?: new (opts: DarshanClientOptions) => DarshanClientInterface;
    };

    if (typeof mod.createClient === 'function') {
      _createClient = mod.createClient;
    } else if (typeof mod.DarshanClient === 'function') {
      const Ctor = mod.DarshanClient;
      _createClient = (opts) => new Ctor(opts);
    } else {
      throw new Error(
        '@darshan/client must export either `createClient` or `DarshanClient`.',
      );
    }

    return _createClient;
  } catch (err) {
    throw new Error(
      `@darshan/react requires @darshan/client as a dependency. ` +
        `Install it with: npm install @darshan/client\n` +
        `Original error: ${err instanceof Error ? err.message : String(err)}`,
    );
  }
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

const DarshanContext = createContext<DarshanClientInterface | null>(null);
DarshanContext.displayName = 'DarshanContext';

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/** Props accepted by {@link DarshanProvider}. */
export interface DarshanProviderProps {
  /** WebSocket / HTTP base URL of the DarshanDB server. */
  readonly serverUrl: string;
  /** Application identifier registered on the server. */
  readonly appId: string;
  /**
   * Optionally pass a pre-constructed client instance.
   * When provided, `serverUrl` and `appId` are ignored and the provider
   * will **not** manage the client lifecycle (you own connect/disconnect).
   */
  readonly client?: DarshanClientInterface;
  readonly children: ReactNode;
}

/**
 * Top-level provider that creates (or accepts) a DarshanDB client and
 * exposes it to every `useQuery`, `useMutation`, `usePresence`, `useAuth`,
 * and `useStorage` hook in the tree.
 *
 * The client connects on mount and disconnects on unmount.  If `serverUrl`
 * or `appId` change, the previous client is torn down and a fresh one is
 * created -- this is intentional for dev-server HMR scenarios.
 *
 * @param props - {@link DarshanProviderProps}
 */
export function DarshanProvider({
  serverUrl,
  appId,
  client: externalClient,
  children,
}: DarshanProviderProps): ReactNode {
  // Track whether we own the client (created internally) or received it.
  const isExternalRef = useRef(!!externalClient);
  isExternalRef.current = !!externalClient;

  const client = useMemo<DarshanClientInterface>(() => {
    if (externalClient) return externalClient;
    return getCreateClient()({ serverUrl, appId });
    // Re-create when connection params change (external client bypasses this).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [externalClient, serverUrl, appId]);

  useEffect(() => {
    // Only manage lifecycle for internally-created clients.
    if (isExternalRef.current) return;

    let cancelled = false;

    void client.connect().catch((err) => {
      if (!cancelled) {
        console.error('[DarshanProvider] connection failed:', err);
      }
    });

    return () => {
      cancelled = true;
      client.disconnect();
    };
  }, [client]);

  return (
    <DarshanContext.Provider value={client}>{children}</DarshanContext.Provider>
  );
}

// ---------------------------------------------------------------------------
// Hook to consume context
// ---------------------------------------------------------------------------

/**
 * Retrieve the `DarshanClient` instance from the nearest
 * {@link DarshanProvider}.  Throws if called outside the provider tree.
 *
 * @returns The active client instance.
 */
export function useDarshanClient(): DarshanClientInterface {
  const client = useContext(DarshanContext);
  if (!client) {
    throw new Error(
      'useDarshanClient must be used within a <DarshanProvider>. ' +
        'Wrap your component tree with <DarshanProvider serverUrl="..." appId="...">.',
    );
  }
  return client;
}

export { DarshanContext };
