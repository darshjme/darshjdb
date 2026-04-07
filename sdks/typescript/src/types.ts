/**
 * Core type definitions for the DarshJDB TypeScript SDK.
 */

// ---------------------------------------------------------------------------
//  Connection
// ---------------------------------------------------------------------------

/** Options for creating a DarshDB client. */
export interface DarshDBOptions {
  /** HTTP request timeout in milliseconds. Default: 30000. */
  timeout?: number;
  /** Custom fetch implementation (for Node 18+ or polyfills). */
  fetch?: typeof globalThis.fetch;
}

/** Connection lifecycle states. */
export enum ConnectionState {
  Disconnected = "disconnected",
  Connecting = "connecting",
  Connected = "connected",
  Closing = "closing",
}

// ---------------------------------------------------------------------------
//  Authentication
// ---------------------------------------------------------------------------

/** Credentials for signin/signup. */
export interface Credentials {
  /** Root/system username. */
  user?: string;
  /** Root/system password. */
  pass?: string;
  /** Email for user-level auth. */
  email?: string;
  /** Password for user-level auth. */
  password?: string;
  /** Optional namespace scope. */
  namespace?: string;
  /** Optional database scope. */
  database?: string;
  /** Optional display name (for signup). */
  name?: string;
}

/** Response from signin/signup operations. */
export interface AuthResponse {
  /** JWT access token. */
  token: string;
  /** User profile data. */
  user: Record<string, unknown>;
  /** Refresh token for token renewal. */
  refreshToken: string;
}

// ---------------------------------------------------------------------------
//  Query
// ---------------------------------------------------------------------------

/** Result from a query or mutation. */
export interface QueryResult<T = Record<string, unknown>> {
  /** List of result records. */
  data: T[];
  /** Server metadata. */
  meta: {
    count?: number;
    duration_ms?: number;
    cached?: boolean;
    filtered?: boolean;
  };
}

// ---------------------------------------------------------------------------
//  Live queries
// ---------------------------------------------------------------------------

/** Actions emitted by live query subscriptions. */
export enum LiveAction {
  Create = "CREATE",
  Update = "UPDATE",
  Delete = "DELETE",
}

/** A single change event from a live query subscription. */
export interface LiveNotification<T = Record<string, unknown>> {
  /** The type of change. */
  action: LiveAction;
  /** The affected record data. */
  result: T;
}

/** Event emitter interface for live query streams. */
export interface LiveStream<T = Record<string, unknown>> {
  /** Register a callback for change events. */
  on(event: "change", callback: (data: LiveNotification<T>) => void): void;
  /** Register a callback for error events. */
  on(event: "error", callback: (error: Error) => void): void;
  /** Register a one-time callback. */
  once(event: "change", callback: (data: LiveNotification<T>) => void): void;
  /** Remove a callback. */
  off(event: string, callback: (...args: unknown[]) => void): void;
  /** Close the live query subscription. */
  close(): void;
}

// ---------------------------------------------------------------------------
//  Mutations
// ---------------------------------------------------------------------------

/** Supported mutation operations. */
export type MutationOp = "insert" | "update" | "delete" | "upsert";

/** A single mutation within a batch. */
export interface Mutation {
  op: MutationOp;
  entity: string;
  id?: string;
  data?: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
//  Batch
// ---------------------------------------------------------------------------

/** A single operation in a batch request. */
export interface BatchOperation {
  method: "GET" | "POST" | "PATCH" | "DELETE";
  path: string;
  body?: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
//  Errors
// ---------------------------------------------------------------------------

/** Base error class for all DarshJDB SDK errors. */
export class DarshDBError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DarshDBError";
  }
}

/** Raised when the SDK cannot connect to the server. */
export class DarshDBConnectionError extends DarshDBError {
  constructor(message = "Failed to connect to DarshJDB server") {
    super(message);
    this.name = "DarshDBConnectionError";
  }
}

/** Raised when authentication fails. */
export class DarshDBAuthError extends DarshDBError {
  constructor(message = "Authentication failed") {
    super(message);
    this.name = "DarshDBAuthError";
  }
}

/** Raised when a query fails. */
export class DarshDBQueryError extends DarshDBError {
  public readonly query?: string;

  constructor(message = "Query execution failed", query?: string) {
    super(message);
    this.name = "DarshDBQueryError";
    this.query = query;
  }
}

/** Raised when the server returns an HTTP error response. */
export class DarshDBAPIError extends DarshDBError {
  public readonly statusCode: number;
  public readonly errorCode?: string;
  public readonly errorBody: Record<string, unknown>;

  constructor(
    message: string,
    statusCode: number,
    errorCode?: string,
    errorBody?: Record<string, unknown>,
  ) {
    super(message);
    this.name = "DarshDBAPIError";
    this.statusCode = statusCode;
    this.errorCode = errorCode;
    this.errorBody = errorBody ?? {};
  }
}
