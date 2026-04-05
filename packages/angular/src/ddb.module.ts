/**
 * @module ddb.module
 * @description NgModule-based configuration for DarshanDB.
 *
 * Use `DarshJDBModule.forRoot()` in traditional NgModule-based Angular
 * applications (Angular 14+). For standalone components (Angular 16+),
 * prefer the `provideDarshan()` function from `./providers`.
 *
 * @example
 * ```typescript
 * // app.module.ts
 * import { DarshJDBModule } from '@darshan/angular';
 *
 * @NgModule({
 *   imports: [
 *     DarshJDBModule.forRoot({
 *       serverUrl: 'https://db.example.com',
 *       appId: 'my-app',
 *     }),
 *   ],
 * })
 * export class AppModule {}
 * ```
 */

import {
  type ModuleWithProviders,
  NgModule,
  APP_INITIALIZER,
  type OnDestroy,
} from '@angular/core';

import type { DarshanConfig } from './types';
import { DARSHAN_CLIENT, DARSHAN_CONFIG, type DarshanClient } from './tokens';
import { createDarshanClient } from './client.factory';

/**
 * Initialize the DarshanDB WebSocket connection during app bootstrap.
 *
 * Returned as an `APP_INITIALIZER` factory so Angular waits for the
 * connection before rendering the root component.
 *
 * @internal
 */
function initDarshanFactory(client: DarshanClient): () => Promise<void> {
  return () => client.connect();
}

/**
 * Angular module that configures and provides the DarshanDB client.
 *
 * Call `DarshJDBModule.forRoot(config)` **once** in your root module.
 * Child modules that need DarshanDB services should simply inject
 * the tokens — no additional imports required.
 */
@NgModule()
export class DarshJDBModule implements OnDestroy {
  /**
   * Configure the DarshanDB client for the root injector.
   *
   * @param config - Connection configuration for the DarshanDB server.
   * @returns A `ModuleWithProviders` that registers all necessary providers.
   *
   * @example
   * ```typescript
   * DarshJDBModule.forRoot({
   *   serverUrl: 'https://db.example.com',
   *   appId: 'my-app',
   *   debug: environment.production === false,
   * })
   * ```
   */
  static forRoot(config: DarshanConfig): ModuleWithProviders<DarshJDBModule> {
    return {
      ngModule: DarshJDBModule,
      providers: [
        { provide: DARSHAN_CONFIG, useValue: config },
        {
          provide: DARSHAN_CLIENT,
          useFactory: () => createDarshanClient(config),
        },
        {
          provide: APP_INITIALIZER,
          useFactory: initDarshanFactory,
          deps: [DARSHAN_CLIENT],
          multi: true,
        },
      ],
    };
  }

  /** @internal Reference for cleanup. */
  private readonly _client: DarshanClient | null;

  constructor() {
    // The client is optional here since DarshJDBModule may be imported
    // without forRoot() in lazy-loaded child modules.
    this._client = null;
  }

  /** Disconnect the client when the root module is destroyed. */
  ngOnDestroy(): void {
    this._client?.disconnect();
  }
}
