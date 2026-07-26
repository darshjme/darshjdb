/**
 * @module @darshjdb/nextjs/api
 *
 * API route helpers for DarshJDB. Wraps Next.js API route handlers
 * with authenticated DarshJDB context.
 *
 * @example
 * ```ts
 * // pages/api/users.ts (Pages Router)
 * import { withDarshan } from '@darshjdb/nextjs/api';
 *
 * export default withDarshan(async (req, res, { db, session }) => {
 *   if (req.method === 'GET') {
 *     const users = await db.collection('users').find();
 *     return res.json(users);
 *   }
 *   res.status(405).json({ error: 'Method not allowed' });
 * });
 * ```
 *
 * @example
 * ```ts
 * // app/api/users/route.ts (App Router)
 * import { withDarshanRoute } from '@darshjdb/nextjs/api';
 *
 * export const GET = withDarshanRoute(async ({ db, session, request }) => {
 *   const users = await db.collection('users').find();
 *   return Response.json(users);
 * });
 * ```
 */

import type { NextApiRequest, NextApiResponse } from 'next';
import type { NextRequest } from 'next/server';
import { queryServer, mutateServer } from './server';
import { DDB_SESSION_COOKIE } from './middleware';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** The identity behind a validated session. */
export interface DarshanSessionUser {
  /** Server-side user id. */
  id: string;
  /** User email address. */
  email: string;
  /** Roles granted to the user. */
  roles: string[];
  /** Server-side session id, when reported. */
  sessionId?: string;
}

/** Session information extracted from the request. */
export interface DarshanSession {
  /** The raw session token, or `null` if unauthenticated. */
  token: string | null;
  /** Whether the token was validated against the DarshJDB server. */
  authenticated: boolean;
  /** The authenticated user, or `null` if unauthenticated. */
  user: DarshanSessionUser | null;
}

/** Context injected into Pages Router API handlers by `withDarshan`. */
export interface DarshanApiContext {
  /** Query server data using DarshJQL. */
  query: typeof queryServer;
  /** Mutate server data. */
  mutate: typeof mutateServer;
  /** Session information from the request cookie. */
  session: DarshanSession;
}

/** Context injected into App Router route handlers by `withDarshanRoute`. */
export interface DarshanRouteContext {
  /** Query server data using DarshJQL. */
  query: typeof queryServer;
  /** Mutate server data. */
  mutate: typeof mutateServer;
  /** Session information from the request cookie. */
  session: DarshanSession;
  /** The original Next.js request object. */
  request: NextRequest;
  /** Route params (dynamic segments). */
  params: Record<string, string | string[]>;
}

/**
 * A Pages Router API handler augmented with DarshJDB context.
 * @see {@link withDarshan}
 */
export type DarshanApiHandler = (
  req: NextApiRequest,
  res: NextApiResponse,
  context: DarshanApiContext,
) => Promise<void> | void;

/**
 * An App Router route handler augmented with DarshJDB context.
 * @see {@link withDarshanRoute}
 */
export type DarshanRouteHandler = (
  context: DarshanRouteContext,
) => Promise<Response> | Response;

/** Configuration for `withDarshan` / `withDarshanRoute`. */
export interface WithDarshanOptions {
  /**
   * Require authentication. If `true`, unauthenticated requests
   * receive a 401 response automatically.
   *
   * @default false
   */
  requireAuth?: boolean;

  /**
   * Custom cookie name for session token extraction.
   * @default 'darshan_session'
   */
  cookieName?: string;

  /**
   * Allowed HTTP methods. Requests with non-matching methods
   * receive a 405 response.
   *
   * @example ['GET', 'POST']
   */
  methods?: string[];

  /**
   * Validate the session token. Return the authenticated user, or `null`
   * if the token is invalid.
   *
   * Defaults to `GET {DDB_URL}/api/auth/me` with the token as a bearer
   * credential. A token is never trusted on presence alone.
   *
   * @example
   * ```ts
   * validateSession: async (token) => {
   *   const res = await fetch(`${process.env.DDB_URL}/api/auth/me`, {
   *     headers: { Authorization: `Bearer ${token}` },
   *   });
   *   if (!res.ok) return null;
   *   const body = await res.json();
   *   return { id: body.user_id, email: body.email, roles: body.roles };
   * }
   * ```
   */
  validateSession?: (token: string) => Promise<DarshanSessionUser | null>;
}

// ---------------------------------------------------------------------------
// Session extraction
// ---------------------------------------------------------------------------

/** Build an unauthenticated session. @internal */
function anonymousSession(): DarshanSession {
  return { token: null, authenticated: false, user: null };
}

/**
 * Validate a session token against the DarshJDB server.
 * @internal
 */
async function validateSessionToken(
  token: string,
): Promise<DarshanSessionUser | null> {
  const url = process.env.DDB_URL;

  if (!url) {
    throw new Error(
      '[DarshJDB] Missing DDB_URL environment variable. ' +
        'It is required to validate session cookies.',
    );
  }

  const response = await fetch(`${url.replace(/\/+$/, '')}/api/auth/me`, {
    method: 'GET',
    headers: { Authorization: `Bearer ${token}` },
    cache: 'no-store',
  });

  if (!response.ok) {
    return null;
  }

  const body = (await response.json()) as {
    user_id?: string;
    email?: string;
    roles?: unknown;
    session_id?: string;
  };

  if (!body.user_id) {
    return null;
  }

  return {
    id: body.user_id,
    email: body.email ?? '',
    roles: Array.isArray(body.roles) ? (body.roles as string[]) : [],
    sessionId: body.session_id,
  };
}

/**
 * Resolve a cookie value into a verified session. A token that cannot be
 * validated yields an anonymous session — presence alone never authenticates.
 * @internal
 */
async function resolveSession(
  token: string | undefined,
  validate: (token: string) => Promise<DarshanSessionUser | null>,
): Promise<DarshanSession> {
  if (!token) {
    return anonymousSession();
  }

  try {
    const user = await validate(token);
    if (!user) {
      return anonymousSession();
    }
    return { token, authenticated: true, user };
  } catch (error) {
    console.error(
      '[DarshJDB] Session validation error:',
      error instanceof Error ? error.message : error,
    );
    return anonymousSession();
  }
}

// ---------------------------------------------------------------------------
// withDarshan (Pages Router)
// ---------------------------------------------------------------------------

/**
 * Wrap a Pages Router API route handler with DarshJDB context.
 *
 * Injects the admin `DarshJDB` and session information into the
 * handler. Optionally enforces authentication and method restrictions.
 *
 * @param handler - The API route handler.
 * @param options - Authentication and method options.
 * @returns A standard Next.js API route handler.
 *
 * @example
 * ```ts
 * // pages/api/posts.ts
 * import { withDarshan } from '@darshjdb/nextjs/api';
 *
 * export default withDarshan(
 *   async (req, res, { db, session }) => {
 *     if (req.method === 'GET') {
 *       const posts = await db.collection('posts').find();
 *       return res.json(posts);
 *     }
 *
 *     if (req.method === 'POST') {
 *       const post = await db.collection('posts').insert(req.body);
 *       return res.status(201).json(post);
 *     }
 *   },
 *   { requireAuth: true, methods: ['GET', 'POST'] },
 * );
 * ```
 */
export function withDarshan(
  handler: DarshanApiHandler,
  options: WithDarshanOptions = {},
): (req: NextApiRequest, res: NextApiResponse) => Promise<void> {
  const {
    requireAuth = false,
    cookieName = DDB_SESSION_COOKIE,
    methods,
    validateSession = validateSessionToken,
  } = options;

  return async (req: NextApiRequest, res: NextApiResponse): Promise<void> => {
    // Method check
    if (methods && !methods.includes(req.method ?? 'GET')) {
      res.setHeader('Allow', methods.join(', '));
      res.status(405).json({
        error: 'Method not allowed',
        allowed: methods,
      });
      return;
    }

    // Extract and validate session
    const session = await resolveSession(req.cookies[cookieName], validateSession);

    // Auth check
    if (requireAuth && !session.authenticated) {
      res.status(401).json({
        error: 'Unauthorized',
        message: 'A valid session is required to access this endpoint.',
      });
      return;
    }

    // Build context
    const context: DarshanApiContext = { query: queryServer, mutate: mutateServer, session };

    try {
      await handler(req, res, context);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : 'Internal server error';
      console.error(`[DarshJDB] API route error: ${message}`);

      if (!res.headersSent) {
        res.status(500).json({
          error: 'Internal server error',
          ...(process.env.NODE_ENV === 'development' ? { detail: message } : {}),
        });
      }
    }
  };
}

// ---------------------------------------------------------------------------
// withDarshanRoute (App Router)
// ---------------------------------------------------------------------------

/**
 * Wrap an App Router route handler with DarshJDB context.
 *
 * Returns a function compatible with Next.js App Router route exports
 * (`GET`, `POST`, etc.). Injects the admin client and session.
 *
 * @param handler - The route handler.
 * @param options - Authentication options.
 * @returns A Next.js App Router route handler.
 *
 * @example
 * ```ts
 * // app/api/users/route.ts
 * import { withDarshanRoute } from '@darshjdb/nextjs/api';
 *
 * export const GET = withDarshanRoute(async ({ db }) => {
 *   const users = await db.collection('users').find();
 *   return Response.json(users);
 * });
 *
 * export const POST = withDarshanRoute(
 *   async ({ db, request }) => {
 *     const body = await request.json();
 *     const user = await db.collection('users').insert(body);
 *     return Response.json(user, { status: 201 });
 *   },
 *   { requireAuth: true },
 * );
 * ```
 */
export function withDarshanRoute(
  handler: DarshanRouteHandler,
  options: WithDarshanOptions = {},
): (request: NextRequest, context: { params: Record<string, string | string[]> }) => Promise<Response> {
  const {
    requireAuth = false,
    cookieName = DDB_SESSION_COOKIE,
    validateSession = validateSessionToken,
  } = options;

  return async (
    request: NextRequest,
    routeContext: { params: Record<string, string | string[]> },
  ): Promise<Response> => {
    // Extract and validate session
    const session = await resolveSession(
      request.cookies.get(cookieName)?.value,
      validateSession,
    );

    // Auth check
    if (requireAuth && !session.authenticated) {
      return Response.json(
        {
          error: 'Unauthorized',
          message: 'A valid session is required to access this endpoint.',
        },
        { status: 401 },
      );
    }

    // Build context
    const context: DarshanRouteContext = {
      query: queryServer,
      mutate: mutateServer,
      session,
      request,
      params: routeContext.params ?? {},
    };

    try {
      return await handler(context);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : 'Internal server error';
      console.error(`[DarshJDB] Route handler error: ${message}`);

      return Response.json(
        {
          error: 'Internal server error',
          ...(process.env.NODE_ENV === 'development' ? { detail: message } : {}),
        },
        { status: 500 },
      );
    }
  };
}
