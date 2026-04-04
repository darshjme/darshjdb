/**
 * @module client.factory
 * @description Factory function to create a DarshanDB client from configuration.
 *
 * Isolates the `@darshan/client` import to a single location for tree-shaking
 * and simplifies testing by providing a seam for mock injection.
 */

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
 * The factory imports `@darshan/client` dynamically so that bundlers can
 * tree-shake the client when the Angular SDK is imported but the factory
 * is never invoked (e.g., in test suites that provide mocks).
 */
export function createDarshanClient(config: DarshanConfig): DarshanClient {
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const { DarshanClient: ClientImpl } = require('@darshan/client');

  const wsUrl =
    config.wsUrl ??
    config.serverUrl
      .replace(/^http:/, 'ws:')
      .replace(/^https:/, 'wss:');

  return new ClientImpl({
    serverUrl: config.serverUrl,
    wsUrl,
    appId: config.appId,
    debug: config.debug ?? false,
    connectTimeout: config.connectTimeout ?? 10_000,
    reconnectInterval: config.reconnectInterval ?? 3_000,
    maxReconnectAttempts: config.maxReconnectAttempts ?? Infinity,
  });
}
