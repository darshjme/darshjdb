/**
 * @module @darshan/angular
 * @description Angular SDK for DarshanDB.
 *
 * Provides reactive database queries, authentication, presence tracking,
 * and SSR support for Angular 16+ applications.
 *
 * ## Quick Start
 *
 * ### Standalone (Angular 16+, recommended)
 *
 * ```typescript
 * import { bootstrapApplication } from '@angular/platform-browser';
 * import { provideDarshan } from '@darshan/angular';
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
 *
 * ### NgModule
 *
 * ```typescript
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
 *
 * ## Querying Data
 *
 * ```typescript
 * // Signal-based (Angular 17+):
 * readonly todos = darshanQuery<Todo[]>('todos', { where: { done: false } });
 *
 * // Observable-based:
 * readonly todos$ = darshanQuery$<Todo[]>('todos', { where: { done: false } });
 *
 * // SSR-aware:
 * readonly todos = darshanTransferQuery<Todo[]>('todos', { where: { done: false } });
 * ```
 *
 * @packageDocumentation
 */

// ── Configuration ──────────────────────────────────────────────────

export { DarshJDBModule } from './ddb.module';
export { provideDarshan } from './providers';

// ── Injection Tokens ───────────────────────────────────────────────

export {
  DARSHAN_CONFIG,
  DARSHAN_CLIENT,
  type DarshanClient,
  type AuthResult,
  type AuthUser,
  type DarshanClientError,
  type PresenceStateRaw,
  type PresenceHandle,
} from './tokens';

// ── Injection Functions ────────────────────────────────────────────

export {
  injectDarshan,
  injectDarshanAuth,
  injectDarshanPresence,
  type DarshanHandle,
  type DarshanAuthHandle,
  type DarshanPresenceHandle,
} from './inject';

// ── Signal Queries (Angular 17+) ───────────────────────────────────

export {
  darshanQuery,
  type SignalQueryResult,
} from './query.signal';

// ── Observable Queries ─────────────────────────────────────────────

export {
  darshanQuery$,
  darshanQueryOnce$,
  darshanMutate$,
  type ObservableQueryResult,
} from './query.observable';

// ── Auth Utilities ─────────────────────────────────────────────────

export {
  darshanAuthGuard,
  darshanRoleGuard,
  darshanAuthInterceptor,
} from './auth';

// ── Presence Utilities ─────────────────────────────────────────────

export {
  presenceUserCount,
  DarshanPresenceDirective,
  type DarshanPresenceContext,
} from './presence';

// ── SSR ────────────────────────────────────────────────────────────

export {
  darshanTransferQuery,
  type TransferQueryResult,
} from './ssr';

// ── Types ──────────────────────────────────────────────────────────

export type {
  DarshanConfig,
  EmailPasswordCredentials,
  MagicLinkCredentials,
  OAuthCredentials,
  SignInCredentials,
  SignUpCredentials,
  DarshanUser,
  TokenPair,
  QueryResult,
  DarshanError,
  PresenceUser,
  PresenceState,
  QueryOptions,
} from './types';

// ── Client Factory (advanced) ──────────────────────────────────────

export { createDarshanClient } from './client.factory';
