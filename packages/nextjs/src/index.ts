/**
 * @module @darshan/nextjs
 *
 * Next.js SDK for DarshanDB. Provides Server Component queries, Server Actions,
 * client-side providers with SSR hydration, Pages Router helpers, Edge Middleware,
 * and API route wrappers.
 *
 * **Subpath imports (recommended):**
 * ```ts
 * import { queryServer, mutateServer, adminDb } from '@darshan/nextjs/server';
 * import { DarshanProvider, dehydrate } from '@darshan/nextjs/provider';
 * import { queryServerSide, queryStaticProps } from '@darshan/nextjs/pages';
 * import { darshanMiddleware } from '@darshan/nextjs/middleware';
 * import { withDarshan, withDarshanRoute } from '@darshan/nextjs/api';
 * ```
 *
 * **Barrel import (convenience):**
 * ```ts
 * import { queryServer, DarshanProvider, darshanMiddleware } from '@darshan/nextjs';
 * ```
 *
 * @packageDocumentation
 */

// ---------------------------------------------------------------------------
// Server utilities
// ---------------------------------------------------------------------------

export {
  queryServer,
  mutateServer,
  callFunction,
  type DarshanQuery,
  type QueryServerOptions,
  type MutationOp,
} from './server';

// ---------------------------------------------------------------------------
// Client provider
// ---------------------------------------------------------------------------

export {
  DarshanProvider,
  dehydrate,
  type DarshanProviderProps,
  type DehydratedState,
  type DehydratedCacheEntry,
} from './provider';

// ---------------------------------------------------------------------------
// Pages Router helpers
// ---------------------------------------------------------------------------

export {
  queryServerSide,
  queryStaticProps,
  type ServerSideQueryFn,
  type StaticQueryFn,
  type QueryStaticOptions,
} from './pages';

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

export {
  darshanMiddleware,
  setSessionCookie,
  clearSessionCookie,
  DARSHAN_SESSION_COOKIE,
  type DarshanMiddlewareConfig,
} from './middleware';

// ---------------------------------------------------------------------------
// API route helpers
// ---------------------------------------------------------------------------

export {
  withDarshan,
  withDarshanRoute,
  type DarshanApiContext,
  type DarshanRouteContext,
  type DarshanApiHandler,
  type DarshanRouteHandler,
  type DarshanSession,
  type WithDarshanOptions,
} from './api';
