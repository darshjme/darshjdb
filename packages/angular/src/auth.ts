/**
 * @module auth
 * @description Authentication utilities for Angular applications.
 *
 * Provides:
 * - `darshanAuthGuard` — A `CanActivateFn` route guard that redirects
 *   unauthenticated users.
 * - `darshanAuthInterceptor` — An `HttpInterceptorFn` that attaches
 *   the JWT access token to outgoing requests.
 *
 * The reactive auth state itself is provided by {@link injectDarshanAuth}
 * from the `inject` module. This module focuses on Angular Router and
 * HttpClient integration.
 *
 * @example
 * ```typescript
 * // app.routes.ts
 * import { darshanAuthGuard } from '@darshjdb/angular';
 *
 * export const routes: Routes = [
 *   {
 *     path: 'dashboard',
 *     canActivate: [darshanAuthGuard],
 *     loadComponent: () => import('./dashboard.component'),
 *   },
 * ];
 * ```
 */

import { inject } from '@angular/core';
import { type CanActivateFn, Router } from '@angular/router';
import {
  type HttpInterceptorFn,
  type HttpHandlerFn,
  type HttpRequest,
} from '@angular/common/http';

import { DDB_CLIENT, DDB_CONFIG } from './tokens';

/**
 * Angular Router guard that blocks navigation for unauthenticated users.
 *
 * When the user is not signed in, the guard redirects to `/auth/sign-in`
 * (configurable via the route's `data.authRedirect` property) and encodes
 * the original URL as a `returnUrl` query parameter for post-login redirect.
 *
 * @example
 * ```typescript
 * // Protect a route:
 * {
 *   path: 'settings',
 *   canActivate: [darshanAuthGuard],
 *   component: SettingsComponent,
 * }
 *
 * // Custom redirect:
 * {
 *   path: 'admin',
 *   canActivate: [darshanAuthGuard],
 *   data: { authRedirect: '/login' },
 *   component: AdminComponent,
 * }
 * ```
 */
export const darshanAuthGuard: CanActivateFn = (route, state) => {
  const client = inject(DDB_CLIENT);
  const router = inject(Router);

  const user = client.getUser();

  if (user) {
    return true;
  }

  const redirectPath =
    (route.data?.['authRedirect'] as string | undefined) ?? '/auth/sign-in';

  return router.createUrlTree([redirectPath], {
    queryParams: { returnUrl: state.url },
  });
};

/**
 * Angular Router guard factory that checks for specific roles.
 *
 * Returns a `CanActivateFn` that verifies the authenticated user
 * holds **all** of the specified roles. If not, the user is redirected
 * to `/auth/unauthorized` (configurable via route data).
 *
 * @param requiredRoles - One or more roles the user must possess.
 * @returns A `CanActivateFn` for use in route configuration.
 *
 * @example
 * ```typescript
 * {
 *   path: 'admin',
 *   canActivate: [darshanRoleGuard('admin')],
 *   component: AdminComponent,
 * }
 *
 * {
 *   path: 'reports',
 *   canActivate: [darshanRoleGuard('admin', 'analyst')],
 *   component: ReportsComponent,
 * }
 * ```
 */
export function darshanRoleGuard(
  ...requiredRoles: string[]
): CanActivateFn {
  return (route, state) => {
    const client = inject(DDB_CLIENT);
    const router = inject(Router);

    const user = client.getUser();

    if (!user) {
      const redirectPath =
        (route.data?.['authRedirect'] as string | undefined) ?? '/auth/sign-in';
      return router.createUrlTree([redirectPath], {
        queryParams: { returnUrl: state.url },
      });
    }

    const hasAllRoles = requiredRoles.every((role) =>
      user.roles.includes(role),
    );

    if (hasAllRoles) {
      return true;
    }

    const unauthorizedPath =
      (route.data?.['unauthorizedRedirect'] as string | undefined) ??
      '/auth/unauthorized';

    return router.createUrlTree([unauthorizedPath]);
  };
}

/**
 * HTTP interceptor that attaches the DarshJDB JWT to outgoing requests.
 *
 * Only attaches the token to requests whose URL starts with the
 * configured `serverUrl`, preventing token leakage to third-party APIs.
 *
 * Register via `provideHttpClient(withInterceptors([darshanAuthInterceptor]))`.
 *
 * @example
 * ```typescript
 * // main.ts
 * import { provideHttpClient, withInterceptors } from '@angular/common/http';
 * import { darshanAuthInterceptor } from '@darshjdb/angular';
 *
 * bootstrapApplication(AppComponent, {
 *   providers: [
 *     provideHttpClient(withInterceptors([darshanAuthInterceptor])),
 *   ],
 * });
 * ```
 */
export const darshanAuthInterceptor: HttpInterceptorFn = (
  req: HttpRequest<unknown>,
  next: HttpHandlerFn,
) => {
  const client = inject(DDB_CLIENT);
  const config = inject(DDB_CONFIG);

  const token = client.getToken();

  // Only attach the token to requests targeting the DarshJDB server.
  if (token && req.url.startsWith(config.serverUrl)) {
    const authedReq = req.clone({
      setHeaders: {
        Authorization: `Bearer ${token}`,
      },
    });
    return next(authedReq);
  }

  return next(req);
};
