/**
 * @module providers
 * @description Standalone provider function for Angular 16+ applications.
 *
 * For applications using the standalone component API (no NgModules),
 * `provideDarshan()` is the recommended way to configure DarshJDB.
 *
 * @example
 * ```typescript
 * // main.ts
 * import { bootstrapApplication } from '@angular/platform-browser';
 * import { provideDarshan } from '@darshjdb/angular';
 * import { AppComponent } from './app.component';
 *
 * bootstrapApplication(AppComponent, {
 *   providers: [
 *     provideDarshan({
 *       serverUrl: 'https://db.example.com',
 *       appId: 'my-app',
 *     }),
 *   ],
 * });
 * ```
 */

import {
  type EnvironmentProviders,
  makeEnvironmentProviders,
  APP_INITIALIZER,
  ENVIRONMENT_INITIALIZER,
  inject,
} from '@angular/core';

import type { DarshanConfig } from './types';
import { DDB_CLIENT, DDB_CONFIG, type DarshanClient } from './tokens';
import { createDarshanClient } from './client.factory';

/**
 * Provide DarshJDB services at the environment (root) injector level.
 *
 * This is the standalone-component equivalent of `DarshanModule.forRoot()`.
 * It registers the configuration, client factory, connection initializer,
 * and a teardown hook that disconnects on app destroy.
 *
 * @param config - Connection configuration for the DarshJDB server.
 * @returns An `EnvironmentProviders` token set for use in `bootstrapApplication`
 *          or a route's `providers` array.
 *
 * @example
 * ```typescript
 * // With debug and custom reconnect:
 * provideDarshan({
 *   serverUrl: 'https://db.example.com',
 *   appId: 'my-app',
 *   debug: true,
 *   reconnectInterval: 5_000,
 *   maxReconnectAttempts: 10,
 * })
 * ```
 */
export function provideDarshan(
  config: DarshanConfig,
): EnvironmentProviders {
  return makeEnvironmentProviders([
    { provide: DDB_CONFIG, useValue: config },
    {
      provide: DDB_CLIENT,
      useFactory: () => createDarshanClient(config),
    },
    {
      provide: APP_INITIALIZER,
      useFactory: (client: DarshanClient) => () => client.connect(),
      deps: [DDB_CLIENT],
      multi: true,
    },
    {
      provide: ENVIRONMENT_INITIALIZER,
      useFactory: () => {
        // Register disconnect on injector destroy via DestroyRef
        // (available Angular 16+). The ENVIRONMENT_INITIALIZER runs
        // once at injector creation, giving us a hook to schedule cleanup.
        const client = inject(DDB_CLIENT);
        return () => {
          // The return value of ENVIRONMENT_INITIALIZER factories is
          // not used, but we capture the client reference for the
          // DestroyRef teardown registered below.
          if (typeof globalThis !== 'undefined') {
            // Register a beforeunload listener as a safety net for
            // graceful disconnect in browser environments.
            globalThis.addEventListener?.('beforeunload', () => {
              client.disconnect();
            });
          }
        };
      },
      multi: true,
    },
  ]);
}
