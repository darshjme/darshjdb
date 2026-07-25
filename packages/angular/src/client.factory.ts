/**
 * @module client.factory
 * @description Factory function to create a DarshJDB client from configuration.
 *
 * Isolates the `@darshjdb/client` import to a single location for tree-shaking
 * and simplifies testing by providing a seam for mock injection.
 */

import { DarshJDB } from '@darshjdb/client';

import type { DarshanConfig } from './types';
import type { DarshanClient } from './tokens';

/**
 * Create a new `DarshanClient` instance bound to the given configuration.
 *
 * This factory is invoked by both `DarshanModule.forRoot()` and the
 * standalone `provideDarshan()` helper to wire the client into Angular DI.
 *
 * @param config - Server connection configuration.
 * @returns A configured, not-yet-connected `DarshanClient`.
 *
 * @remarks
 * `@darshjdb/client` exports its core client as `DarshJDB`; the Angular SDK
 * consumes it through the narrower {@link DarshanClient} facade so that tests
 * can provide mocks against `DDB_CLIENT` without importing the real client.
 */
export function createDarshanClient(config: DarshanConfig): DarshanClient {
  const client = new DarshJDB({
    serverUrl: config.serverUrl,
    appId: config.appId,
  });

  return client as unknown as DarshanClient;
}
