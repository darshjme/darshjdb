/**
 * @module @darshjdb/react
 * @description React bindings for DarshJDB.
 *
 * Provides a context provider and a suite of hooks for real-time queries,
 * mutations, presence, authentication, and file storage -- all backed by
 * the framework-agnostic `@darshjdb/client` core.
 *
 * @example
 * ```tsx
 * import {
 *   DarshanProvider,
 *   useQuery,
 *   useMutation,
 *   usePresence,
 *   useAuth,
 *   useStorage,
 * } from '@darshjdb/react';
 * ```
 *
 * @packageDocumentation
 */

// Provider & context -----------------------------------------------------------
export { DarshanProvider, useDarshanClient, DarshanContext } from './provider';
export type { DarshanProviderProps } from './provider';

// Hooks ------------------------------------------------------------------------
export { useQuery } from './use-query';
export type { UseQueryOptions, UseQueryResult } from './use-query';

export { useMutation } from './use-mutation';
export type { UseMutationResult } from './use-mutation';

export { usePresence } from './use-presence';
export type { UsePresenceResult } from './use-presence';

export { useAuth } from './use-auth';
export type { UseAuthResult, AuthCredentials } from './use-auth';

export { useStorage } from './use-storage';
export type { UseStorageResult } from './use-storage';

// Types (re-exported for consumers who need to type-annotate) ------------------
export type {
  Query,
  WhereClause,
  OrderClause,
  QuerySnapshot,
  MutationOperation,
  AuthUser,
  AuthState,
  PresencePeer,
  UploadProgress,
  UploadResult,
  DarshanClientInterface,
  DarshanClientOptions,
  Unsubscribe,
} from './types';
