/**
 * DarshDB — the main TypeScript client for DarshJDB.
 *
 * Follows the SurrealDB SDK pattern: signin, use namespace/database,
 * then CRUD + query + live queries + graph relations.
 *
 * @example
 * ```typescript
 * import { DarshDB } from 'darshjdb';
 *
 * const db = new DarshDB('http://localhost:8080');
 * await db.signin({ user: 'root', pass: 'root' });
 * await db.use('test', 'test');
 *
 * const user = await db.create('users', { name: 'Darsh' });
 * const users = await db.select('users');
 * const results = await db.query('SELECT * FROM users WHERE age > 18');
 *
 * const stream = await db.live('SELECT * FROM users');
 * stream.on('change', (data) => console.log(data));
 * ```
 */

import { LiveQueryStream } from "./live.js";
import {
  ConnectionState,
  DarshDBAPIError,
  DarshDBAuthError,
  DarshDBConnectionError,
  DarshDBError,
  DarshDBQueryError,
  type AuthResponse,
  type BatchOperation,
  type Credentials,
  type DarshDBOptions,
  type LiveStream,
  type QueryResult,
} from "./types.js";

/**
 * Parse a SurrealDB-style record ID like 'users:darsh' into [table, id].
 * If no colon, returns [thing, undefined].
 */
function parseThing(thing: string): [string, string | undefined] {
  const idx = thing.indexOf(":");
  if (idx !== -1) {
    return [thing.slice(0, idx), thing.slice(idx + 1)];
  }
  return [thing, undefined];
}

export class DarshDB {
  private readonly url: string;
  private readonly timeout: number;
  private readonly fetchFn: typeof globalThis.fetch;
  private token: string | null = null;
  private namespace: string | null = null;
  private database: string | null = null;
  private _state: ConnectionState = ConnectionState.Connected;

  /**
   * Create a new DarshDB client.
   *
   * @param url - Base URL of the DarshJDB server.
   * @param options - Optional configuration.
   */
  constructor(url: string, options: DarshDBOptions = {}) {
    if (!url) {
      throw new DarshDBError("url is required");
    }
    this.url = url.replace(/\/+$/, "");
    this.timeout = options.timeout ?? 30_000;
    this.fetchFn = options.fetch ?? globalThis.fetch.bind(globalThis);
  }

  /** Current connection state. */
  get state(): ConnectionState {
    return this._state;
  }

  // -----------------------------------------------------------------------
  //  Connection lifecycle
  // -----------------------------------------------------------------------

  /** Close the client and release resources. */
  async close(): Promise<void> {
    this._state = ConnectionState.Disconnected;
  }

  // -----------------------------------------------------------------------
  //  Authentication
  // -----------------------------------------------------------------------

  /**
   * Sign in to the DarshJDB server.
   *
   * @param credentials - Auth credentials (user/pass or email/password).
   * @returns AuthResponse with JWT token and user data.
   */
  async signin(credentials: Credentials): Promise<AuthResponse> {
    const body: Record<string, unknown> = {};

    if (credentials.user && credentials.pass) {
      body.email = credentials.user;
      body.password = credentials.pass;
    } else if (credentials.email && credentials.password) {
      body.email = credentials.email;
      body.password = credentials.password;
    } else {
      Object.assign(body, credentials);
    }

    if (credentials.namespace) this.namespace = credentials.namespace;
    if (credentials.database) this.database = credentials.database;

    let result: Record<string, unknown>;
    try {
      result = await this.post("/api/auth/signin", body);
    } catch (err) {
      if (
        err instanceof DarshDBAPIError &&
        (err.statusCode === 401 || err.statusCode === 403)
      ) {
        throw new DarshDBAuthError(err.message);
      }
      throw err;
    }

    const token =
      (result.accessToken as string) ?? (result.token as string) ?? "";
    if (token) this.token = token;

    return {
      token,
      user: (result.user as Record<string, unknown>) ?? {},
      refreshToken: (result.refreshToken as string) ?? "",
    };
  }

  /**
   * Create a new account and sign in.
   *
   * @param credentials - Signup credentials with email, password, optional name.
   */
  async signup(credentials: Credentials): Promise<AuthResponse> {
    const body: Record<string, unknown> = {};
    if (credentials.email) body.email = credentials.email;
    if (credentials.password) body.password = credentials.password;
    if (credentials.name) body.name = credentials.name;

    let result: Record<string, unknown>;
    try {
      result = await this.post("/api/auth/signup", body);
    } catch (err) {
      if (
        err instanceof DarshDBAPIError &&
        [401, 403, 409].includes(err.statusCode)
      ) {
        throw new DarshDBAuthError(err.message);
      }
      throw err;
    }

    const token =
      (result.accessToken as string) ?? (result.token as string) ?? "";
    if (token) this.token = token;

    if (credentials.namespace) this.namespace = credentials.namespace;
    if (credentials.database) this.database = credentials.database;

    return {
      token,
      user: (result.user as Record<string, unknown>) ?? {},
      refreshToken: (result.refreshToken as string) ?? "",
    };
  }

  /** Sign out and invalidate the current session. */
  async invalidate(): Promise<void> {
    if (this.token) {
      try {
        await this.post("/api/auth/signout", {});
      } catch {
        // Best-effort signout
      }
    }
    this.token = null;
  }

  /** Set the auth token directly (e.g., from a stored session). */
  async authenticate(token: string): Promise<void> {
    this.token = token;
  }

  // -----------------------------------------------------------------------
  //  Namespace / Database
  // -----------------------------------------------------------------------

  /**
   * Set the active namespace and database.
   *
   * @param ns - Namespace name.
   * @param db - Database name.
   */
  async use(ns: string, db: string): Promise<void> {
    this.namespace = ns;
    this.database = db;
  }

  // -----------------------------------------------------------------------
  //  CRUD
  // -----------------------------------------------------------------------

  /**
   * Select all records from a table, or a specific record by ID.
   *
   * @param thing - Table name ("users") or record ID ("users:darsh").
   */
  async select<T = Record<string, unknown>>(thing: string): Promise<T[]> {
    const [table, id] = parseThing(thing);
    if (id) {
      const result = await this.get<T>(`/api/data/${table}/${id}`);
      return result ? [result as T] : [];
    }
    const result = await this.get<{ data: T[] } | T[]>(
      `/api/data/${table}`,
    );
    if (Array.isArray(result)) return result;
    return (result as { data: T[] }).data ?? [result as unknown as T];
  }

  /**
   * Create a new record in a table.
   *
   * @param thing - Table name or record ID ("users:darsh").
   * @param data - Record data.
   */
  async create<T = Record<string, unknown>>(
    thing: string,
    data?: Record<string, unknown>,
  ): Promise<T> {
    const [table, id] = parseThing(thing);
    const body = id ? { ...data, id } : (data ?? {});
    return this.post(`/api/data/${table}`, body) as Promise<T>;
  }

  /**
   * Insert one or more records into a table.
   *
   * @param table - Target table name.
   * @param data - A single record or array of records.
   */
  async insert<T = Record<string, unknown>>(
    table: string,
    data: Record<string, unknown> | Record<string, unknown>[],
  ): Promise<T[]> {
    const records = Array.isArray(data) ? data : [data];
    const result = await this.post("/api/mutate", {
      mutations: records.map((record) => ({
        op: "insert",
        entity: table,
        data: record,
      })),
    });
    return ((result as Record<string, unknown>).results as T[]) ?? (records as unknown as T[]);
  }

  /**
   * Update an existing record (merge semantics via PATCH).
   *
   * @param thing - Record ID ("users:darsh").
   * @param data - Fields to update.
   */
  async update<T = Record<string, unknown>>(
    thing: string,
    data?: Record<string, unknown>,
  ): Promise<T> {
    const [table, id] = parseThing(thing);
    if (!id) {
      throw new DarshDBError(
        `update() requires a record ID like '${table}:id', got '${thing}'`,
      );
    }
    return this.patch(`/api/data/${table}/${id}`, data ?? {}) as Promise<T>;
  }

  /**
   * Merge data into an existing record (partial update).
   * Alias for update() — both use PATCH semantics.
   *
   * @param thing - Record ID ("users:darsh").
   * @param data - Fields to merge.
   */
  async merge<T = Record<string, unknown>>(
    thing: string,
    data: Record<string, unknown>,
  ): Promise<T> {
    return this.update<T>(thing, data);
  }

  /**
   * Delete a record or all records in a table.
   *
   * @param thing - Record ID or table name.
   */
  async delete<T = Record<string, unknown>>(
    thing: string,
  ): Promise<T> {
    const [table, id] = parseThing(thing);
    const path = id ? `/api/data/${table}/${id}` : `/api/data/${table}`;
    return this.del(path) as Promise<T>;
  }

  // -----------------------------------------------------------------------
  //  Query
  // -----------------------------------------------------------------------

  /**
   * Execute a DarshJQL query string.
   *
   * @param sql - The query (e.g., "SELECT * FROM users WHERE age > 18").
   * @param vars - Optional bind variables.
   */
  async query<T = Record<string, unknown>>(
    sql: string,
    vars?: Record<string, unknown>,
  ): Promise<QueryResult<T>[]> {
    const body: Record<string, unknown> = { query: sql };
    if (vars) body.vars = vars;

    let result: unknown;
    try {
      result = await this.post("/api/query", body);
    } catch (err) {
      if (err instanceof DarshDBAPIError) {
        throw new DarshDBQueryError(err.message, sql);
      }
      throw err;
    }

    if (Array.isArray(result)) {
      return result.map((r: Record<string, unknown>) => ({
        data: (r.data as T[]) ?? [r as unknown as T],
        meta: (r.meta as QueryResult["meta"]) ?? {},
      }));
    }

    const res = result as Record<string, unknown>;
    return [
      {
        data: (res.data as T[]) ?? [],
        meta: (res.meta as QueryResult["meta"]) ?? {},
      },
    ];
  }

  /**
   * Execute a query and return the raw server response.
   */
  async queryRaw(
    sql: string,
    vars?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const body: Record<string, unknown> = { query: sql };
    if (vars) body.vars = vars;
    return this.post("/api/query", body);
  }

  // -----------------------------------------------------------------------
  //  Live queries
  // -----------------------------------------------------------------------

  /**
   * Subscribe to a live query via WebSocket.
   *
   * @param queryOrTable - A SQL query or table name.
   * @returns A LiveStream that emits change/error events.
   *
   * @example
   * ```typescript
   * const stream = await db.live('SELECT * FROM users');
   * stream.on('change', (data) => console.log(data.action, data.result));
   * stream.on('error', (err) => console.error(err));
   * // Later: stream.close();
   * ```
   */
  async live<T = Record<string, unknown>>(
    queryOrTable: string,
  ): Promise<LiveStream<T>> {
    const parsed = new URL(this.url);
    const wsScheme = parsed.protocol === "https:" ? "wss:" : "ws:";
    const wsUrl = `${wsScheme}//${parsed.host}/ws`;

    return new LiveQueryStream<T>(wsUrl, this.token, queryOrTable);
  }

  // -----------------------------------------------------------------------
  //  Graph relations
  // -----------------------------------------------------------------------

  /**
   * Create a graph relation between two records.
   *
   * @param from - Source record ID ("user:darsh").
   * @param relation - Relation type ("works_at").
   * @param to - Target record ID ("company:knowai").
   * @param data - Optional data for the relation edge.
   */
  async relate(
    from: string,
    relation: string,
    to: string,
    data?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const [fromTable, fromId] = parseThing(from);
    const [toTable, toId] = parseThing(to);

    const result = await this.post("/api/mutate", {
      mutations: [
        {
          op: "insert",
          entity: relation,
          data: {
            from_entity: fromTable,
            from_id: fromId ?? from,
            to_entity: toTable,
            to_id: toId ?? to,
            ...(data ?? {}),
          },
        },
      ],
    });

    const results = (result as Record<string, unknown>).results;
    return Array.isArray(results)
      ? ((results[0] as Record<string, unknown>) ?? {})
      : (result as Record<string, unknown>);
  }

  // -----------------------------------------------------------------------
  //  Server-side functions
  // -----------------------------------------------------------------------

  /**
   * Invoke a server-side function.
   *
   * @param name - Function name.
   * @param args - Arguments to pass.
   */
  async run<T = unknown>(
    name: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    const result = await this.post(`/api/fn/${name}`, args ?? {});
    const res = result as Record<string, unknown>;
    return (res.result !== undefined ? res.result : result) as T;
  }

  // -----------------------------------------------------------------------
  //  Batch
  // -----------------------------------------------------------------------

  /**
   * Execute multiple operations in a single batch request.
   *
   * @param operations - List of operations.
   */
  async batch(
    operations: BatchOperation[],
  ): Promise<Record<string, unknown>[]> {
    const result = await this.post("/api/batch", { operations });
    return (
      ((result as Record<string, unknown>).results as Record<
        string,
        unknown
      >[]) ?? []
    );
  }

  // -----------------------------------------------------------------------
  //  Storage
  // -----------------------------------------------------------------------

  /**
   * Upload a file to DarshJDB storage.
   *
   * @param path - Storage path.
   * @param content - File content as Blob, Buffer, or ArrayBuffer.
   * @param filename - The filename.
   */
  async upload(
    path: string,
    content: Blob | ArrayBuffer,
    filename: string,
  ): Promise<Record<string, unknown>> {
    const formData = new FormData();
    formData.append("path", path);
    formData.append(
      "file",
      content instanceof Blob ? content : new Blob([content]),
      filename,
    );

    const headers = this.buildHeaders();
    delete headers["Content-Type"]; // Let browser set multipart boundary

    const response = await this.fetchFn(`${this.url}/api/storage/upload`, {
      method: "POST",
      headers,
      body: formData,
      signal: AbortSignal.timeout(this.timeout),
    });

    return this.handleResponse(response);
  }

  /**
   * Download a file from DarshJDB storage.
   *
   * @param path - Storage path.
   */
  async download(path: string): Promise<ArrayBuffer> {
    const response = await this.fetchFn(
      `${this.url}/api/storage/${path.replace(/^\/+/, "")}`,
      {
        method: "GET",
        headers: this.buildHeaders(),
        signal: AbortSignal.timeout(this.timeout),
      },
    );

    if (!response.ok) {
      throw new DarshDBAPIError(
        response.statusText,
        response.status,
      );
    }

    return response.arrayBuffer();
  }

  // -----------------------------------------------------------------------
  //  Health
  // -----------------------------------------------------------------------

  /** Check if the server is reachable. */
  async health(): Promise<boolean> {
    try {
      const response = await this.fetchFn(`${this.url}/api/health`, {
        signal: AbortSignal.timeout(5000),
      });
      return response.ok;
    } catch {
      return false;
    }
  }

  /** Get the server version string. */
  async version(): Promise<string> {
    const result = await this.get<{ version: string }>("/api/health");
    return (result as { version: string }).version ?? "unknown";
  }

  // -----------------------------------------------------------------------
  //  Internal HTTP helpers
  // -----------------------------------------------------------------------

  private buildHeaders(): Record<string, string> {
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      Accept: "application/json",
    };
    if (this.token) {
      headers["Authorization"] = `Bearer ${this.token}`;
    }
    if (this.namespace) {
      headers["X-DarshDB-NS"] = this.namespace;
    }
    if (this.database) {
      headers["X-DarshDB-DB"] = this.database;
    }
    return headers;
  }

  private async post(
    path: string,
    body: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("POST", path, body);
  }

  private async get<T>(path: string): Promise<T> {
    return this.request("GET", path) as Promise<T>;
  }

  private async patch(
    path: string,
    body: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("PATCH", path, body);
  }

  private async del(path: string): Promise<Record<string, unknown>> {
    return this.request("DELETE", path);
  }

  private async request(
    method: string,
    path: string,
    body?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    let response: Response;

    try {
      response = await this.fetchFn(`${this.url}${path}`, {
        method,
        headers: this.buildHeaders(),
        body: body ? JSON.stringify(body) : undefined,
        signal: AbortSignal.timeout(this.timeout),
      });
    } catch (err) {
      if (err instanceof TypeError) {
        throw new DarshDBConnectionError(
          `Cannot connect to ${this.url}: ${err.message}`,
        );
      }
      throw new DarshDBConnectionError(`Network error: ${err}`);
    }

    return this.handleResponse(response);
  }

  private async handleResponse(
    response: Response,
  ): Promise<Record<string, unknown>> {
    if (response.status === 204) {
      return {};
    }

    if (!response.ok) {
      let body: Record<string, unknown>;
      try {
        body = (await response.json()) as Record<string, unknown>;
      } catch {
        body = { raw: await response.text() };
      }

      const errorObj = body.error as Record<string, unknown> | undefined;
      const message =
        (typeof errorObj === "object"
          ? (errorObj?.message as string)
          : undefined) ??
        (body.message as string) ??
        (body.error as string) ??
        response.statusText;

      if (response.status === 401 || response.status === 403) {
        throw new DarshDBAuthError(message);
      }

      throw new DarshDBAPIError(
        message,
        response.status,
        typeof errorObj === "object"
          ? (errorObj?.code as string)
          : undefined,
        body,
      );
    }

    try {
      return (await response.json()) as Record<string, unknown>;
    } catch (err) {
      throw new DarshDBError(`Invalid JSON response: ${err}`);
    }
  }
}
