/**
 * @module @darshan/nextjs/middleware
 *
 * Next.js Edge Middleware for DarshanDB session-based authentication.
 * Intercepts requests to protected routes and validates session cookies.
 *
 * @example
 * ```ts
 * // middleware.ts (project root)
 * import { darshanMiddleware } from '@darshan/nextjs/middleware';
 *
 * export default darshanMiddleware({
 *   protectedRoutes: ['/dashboard', '/api/private'],
 *   loginRoute: '/auth/login',
 * });
 *
 * export const config = {
 *   matcher: ['/((?!_next/static|_next/image|favicon.ico).*)'],
 * };
 * ```
 */

import { NextResponse, type NextRequest } from 'next/server';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Name of the cookie storing the DarshanDB session token. */
export const DARSHAN_SESSION_COOKIE = 'darshan_session';

/** Configuration for the DarshanDB middleware. */
export interface DarshanMiddlewareConfig {
  /**
   * Route prefixes that require authentication.
   * Matching is prefix-based: `'/dashboard'` protects `/dashboard`,
   * `/dashboard/settings`, etc.
   *
   * @example ['/dashboard', '/api/private', '/admin']
   */
  protectedRoutes: string[];

  /**
   * Route to redirect unauthenticated users to.
   * The original URL is appended as a `?callbackUrl=` parameter.
   *
   * @default '/login'
   */
  loginRoute?: string;

  /**
   * Optional async function to validate the session token.
   * If not provided, the middleware only checks for cookie presence.
   *
   * Return `true` if the session is valid, `false` otherwise.
   *
   * @example
   * ```ts
   * validateSession: async (token) => {
   *   const res = await fetch(`${process.env.DARSHAN_URL}/auth/validate`, {
   *     headers: { Authorization: `Bearer ${token}` },
   *   });
   *   return res.ok;
   * }
   * ```
   */
  validateSession?: (token: string) => Promise<boolean>;

  /**
   * Routes that are always accessible, even if they match a protected prefix.
   * Useful for public API endpoints nested under a protected prefix.
   *
   * @example ['/dashboard/public-preview']
   */
  publicRoutes?: string[];

  /**
   * Called when a request is authenticated. Allows modifying the response
   * (e.g. injecting headers).
   */
  onAuthenticated?: (
    request: NextRequest,
    response: NextResponse,
    sessionToken: string,
  ) => NextResponse | void;

  /**
   * Custom cookie name override.
   * @default 'darshan_session'
   */
  cookieName?: string;
}

// ---------------------------------------------------------------------------
// Route matching helpers
// ---------------------------------------------------------------------------

/**
 * Check if a pathname matches any of the given route prefixes.
 * @internal
 */
function matchesRoutePrefix(pathname: string, routes: string[]): boolean {
  return routes.some(
    (route) => pathname === route || pathname.startsWith(`${route}/`),
  );
}

// ---------------------------------------------------------------------------
// Middleware factory
// ---------------------------------------------------------------------------

/**
 * Create a Next.js middleware function that protects routes using
 * DarshanDB cookie-based sessions.
 *
 * Unauthenticated requests to protected routes are redirected to the
 * login page with a `callbackUrl` parameter for post-login redirect.
 *
 * @param config - Middleware configuration.
 * @returns A Next.js middleware function.
 *
 * @example
 * ```ts
 * // middleware.ts
 * import { darshanMiddleware } from '@darshan/nextjs/middleware';
 *
 * export default darshanMiddleware({
 *   protectedRoutes: ['/dashboard', '/settings'],
 *   loginRoute: '/auth/login',
 *   validateSession: async (token) => {
 *     const res = await fetch(`${process.env.DARSHAN_URL}/auth/validate`, {
 *       headers: { Authorization: `Bearer ${token}` },
 *     });
 *     return res.ok;
 *   },
 * });
 *
 * export const config = {
 *   matcher: ['/((?!_next/static|_next/image|favicon.ico).*)'],
 * };
 * ```
 */
export function darshanMiddleware(config: DarshanMiddlewareConfig) {
  const {
    protectedRoutes,
    loginRoute = '/login',
    validateSession,
    publicRoutes = [],
    onAuthenticated,
    cookieName = DARSHAN_SESSION_COOKIE,
  } = config;

  return async function middleware(request: NextRequest): Promise<NextResponse> {
    const { pathname } = request.nextUrl;

    // Allow public routes to pass through unconditionally
    if (matchesRoutePrefix(pathname, publicRoutes)) {
      return NextResponse.next();
    }

    // Check if the current path is protected
    const isProtected = matchesRoutePrefix(pathname, protectedRoutes);

    if (!isProtected) {
      return NextResponse.next();
    }

    // Read session cookie
    const sessionToken = request.cookies.get(cookieName)?.value;

    if (!sessionToken) {
      return redirectToLogin(request, loginRoute);
    }

    // Validate the session if a validator is provided
    if (validateSession) {
      try {
        const isValid = await validateSession(sessionToken);
        if (!isValid) {
          // Clear the invalid cookie and redirect
          const response = redirectToLogin(request, loginRoute);
          response.cookies.delete(cookieName);
          return response;
        }
      } catch (error) {
        console.error(
          '[DarshanDB] Session validation error:',
          error instanceof Error ? error.message : error,
        );
        // On validation failure, deny access
        return redirectToLogin(request, loginRoute);
      }
    }

    // Session is valid — proceed
    const response = NextResponse.next();

    // Inject session token into request headers so Server Components
    // and API routes can access it without re-reading the cookie.
    response.headers.set('x-ddb-session', sessionToken);

    // Allow custom post-authentication logic
    if (onAuthenticated) {
      const customResponse = onAuthenticated(request, response, sessionToken);
      if (customResponse) return customResponse;
    }

    return response;
  };
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Build a redirect response to the login route with a callback URL.
 * @internal
 */
function redirectToLogin(
  request: NextRequest,
  loginRoute: string,
): NextResponse {
  const loginUrl = new URL(loginRoute, request.url);

  // Preserve the original URL as a callback parameter
  loginUrl.searchParams.set('callbackUrl', request.nextUrl.pathname);

  // Preserve search params from the original request
  const originalSearch = request.nextUrl.search;
  if (originalSearch) {
    loginUrl.searchParams.set('callbackSearch', originalSearch);
  }

  return NextResponse.redirect(loginUrl);
}

// ---------------------------------------------------------------------------
// Cookie utilities
// ---------------------------------------------------------------------------

/**
 * Set a DarshanDB session cookie in a response.
 *
 * @param response - The NextResponse to modify.
 * @param token - The session token value.
 * @param options - Cookie options.
 * @returns The modified response.
 *
 * @example
 * ```ts
 * import { setSessionCookie } from '@darshan/nextjs/middleware';
 *
 * const response = NextResponse.json({ ok: true });
 * setSessionCookie(response, 'session_token_here', { maxAge: 86400 });
 * ```
 */
export function setSessionCookie(
  response: NextResponse,
  token: string,
  options: {
    /** Cookie max age in seconds. Default: 7 days. */
    maxAge?: number;
    /** Cookie path. Default: '/'. */
    path?: string;
    /** Cookie name override. Default: 'darshan_session'. */
    cookieName?: string;
    /** SameSite attribute. Default: 'lax'. */
    sameSite?: 'strict' | 'lax' | 'none';
    /** Secure flag. Default: auto-detect from URL. */
    secure?: boolean;
  } = {},
): NextResponse {
  const {
    maxAge = 7 * 24 * 60 * 60, // 7 days
    path = '/',
    cookieName = DARSHAN_SESSION_COOKIE,
    sameSite = 'lax',
    secure,
  } = options;

  response.cookies.set(cookieName, token, {
    httpOnly: true,
    secure: secure ?? process.env.NODE_ENV === 'production',
    sameSite,
    path,
    maxAge,
  });

  return response;
}

/**
 * Clear the DarshanDB session cookie from a response.
 *
 * @param response - The NextResponse to modify.
 * @param cookieName - Cookie name. Default: 'darshan_session'.
 * @returns The modified response.
 */
export function clearSessionCookie(
  response: NextResponse,
  cookieName: string = DARSHAN_SESSION_COOKIE,
): NextResponse {
  response.cookies.delete(cookieName);
  return response;
}
