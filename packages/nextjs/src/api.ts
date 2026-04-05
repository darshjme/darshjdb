/**
 * @module @darshan/nextjs/api
 *
 * API route helpers for DarshanDB. Wraps Next.js API route handlers
 * with authenticated DarshanDB context.
 *
 * @example
 * ```ts
 * // pages/api/users.ts (Pages Router)
 * import { withDarshan } from '@darshan/nextjs/api';
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
 * import { withDarshanRoute } from '@darshan/nextjs/api';
 *
 * export const GET = withDarshanRoute(async ({ db, session, request }) => {
 *   const users = await db.collection('users').find();
 *   return Response.json(users);
 * });
 * ```
 */

import type { NextApiRequest, NextApiResponse } from 'next';
import type { NextRequest } from 'next/server';
import type { DarshanDB } from '@darshan/client';
import { getAdminDb } from './server';
import { DARSHAN_SESSION_COOKIE } from './middleware';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Session information extracted from the request. */
export interface DarshanSession {
  /** The raw session token, or `null` if unauthenticated. */
  token: string | null;
  /** Whether a valid session token was found. */
  authenticated: boolean;
}

/** Context injected into Pages Router API handlers by `withDarshan`. */
export interface DarshanApiContext {
  /** The admin DarshanDB instance. */
  db: DarshanDB;
  /** Session information from the request cookie. */
  session: DarshanSession;
}

/** Context injected into App Router route handlers by `withDarshanRoute`. */
export interface DarshanRouteContext {
  /** The admin DarshanDB instance. */
  db: DarshanDB;
  /** Session information from the request cookie. */
  session: DarshanSession;
  /** The original Next.js request object. */
  request: NextRequest;
  /** Route params (dynamic segments). */
  params: Record<string, string | string[]>;
}

/**
 * A Pages Router API handler augmented with DarshanDB context.
 * @see {@link withDarshan}
 */
export type DarshanApiHandler = (
  req: NextApiRequest,
  res: NextApiResponse,
  context: DarshanApiContext,
) => Promise<void> | void;

/**
 * An App Router route handler augmented with DarshanDB context.
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
}

// ---------------------------------------------------------------------------
// Session extraction
// ---------------------------------------------------------------------------

/**
 * Extract session information from a Pages Router request.
 * @internal
 */
function extractSessionFromApiRequest(
  req: NextApiRequest,
  cookieName: string,
): DarshanSession {
  const token = req.cookies[cookieName] ?? null;
  return {
    token,
    authenticated: token !== null && token.length > 0,
  };
}

/**
 * Extract session information from an App Router request.
 * @internal
 */
function extractSessionFromRequest(
  request: NextRequest,
  cookieName: string,
): DarshanSession {
  const token = request.cookies.get(cookieName)?.value ?? null;
  return {
    token,
    authenticated: token !== null && token.length > 0,
  };
}

// ---------------------------------------------------------------------------
// withDarshan (Pages Router)
// ---------------------------------------------------------------------------

/**
 * Wrap a Pages Router API route handler with DarshanDB context.
 *
 * Injects the admin `DarshanDB` and session information into the
 * handler. Optionally enforces authentication and method restrictions.
 *
 * @param handler - The API route handler.
 * @param options - Authentication and method options.
 * @returns A standard Next.js API route handler.
 *
 * @example
 * ```ts
 * // pages/api/posts.ts
 * import { withDarshan } from '@darshan/nextjs/api';
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
    cookieName = DARSHAN_SESSION_COOKIE,
    methods,
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

    // Extract session
    const session = extractSessionFromApiRequest(req, cookieName);

    // Auth check
    if (requireAuth && !session.authenticated) {
      res.status(401).json({
        error: 'Unauthorized',
        message: 'A valid session is required to access this endpoint.',
      });
      return;
    }

    // Build context
    const db = getAdminDb();
    const context: DarshanApiContext = { db, session };

    try {
      await handler(req, res, context);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : 'Internal server error';
      console.error(`[DarshanDB] API route error: ${message}`);

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
 * Wrap an App Router route handler with DarshanDB context.
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
 * import { withDarshanRoute } from '@darshan/nextjs/api';
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
    cookieName = DARSHAN_SESSION_COOKIE,
  } = options;

  return async (
    request: NextRequest,
    routeContext: { params: Record<string, string | string[]> },
  ): Promise<Response> => {
    // Extract session
    const session = extractSessionFromRequest(request, cookieName);

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
    const db = getAdminDb();
    const context: DarshanRouteContext = {
      db,
      session,
      request,
      params: routeContext.params ?? {},
    };

    try {
      return await handler(context);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : 'Internal server error';
      console.error(`[DarshanDB] Route handler error: ${message}`);

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
