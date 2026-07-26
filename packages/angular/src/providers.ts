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
  DestroyRef,
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
      // Runs once in the environment injection context, where DestroyRef
      // (Angular 16+) lets us disconnect when the injector is destroyed.
      useValue: () => {
        const client = inject(DDB_CLIENT);
        const destroyRef = inject(DestroyRef);

        destroyRef.onDestroy(() => {
          client.disconnect();
        });
      },
      multi: true,
    },
  ]);
}
