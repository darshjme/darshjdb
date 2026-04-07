/**
 * DarshJDB TypeScript SDK.
 *
 * Official client library for DarshJDB — a real-time database
 * with graph relations, live queries, auth, and storage.
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
 *
 * @packageDocumentation
 */

export { DarshDB } from "./client.js";
export { LiveQueryStream } from "./live.js";
export {
  // Enums
  ConnectionState,
  LiveAction,
  // Error classes
  DarshDBError,
  DarshDBConnectionError,
  DarshDBAuthError,
  DarshDBQueryError,
  DarshDBAPIError,
  // Types
  type DarshDBOptions,
  type Credentials,
  type AuthResponse,
  type QueryResult,
  type LiveNotification,
  type LiveStream,
  type Mutation,
  type MutationOp,
  type BatchOperation,
} from "./types.js";
