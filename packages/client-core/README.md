# @darshjdb/client

Framework-agnostic TypeScript client SDK for DarshJDB. This is the core library that all framework-specific SDKs (`@darshjdb/react`, `@darshjdb/nextjs`, `@darshjdb/angular`) build on top of.

## Install

```bash
npm install @darshjdb/client
```

Use this package directly if you are building for a framework without a dedicated SDK, or if you need a plain TypeScript/JavaScript client.

## Usage

```typescript
import { DarshJDB } from '@darshjdb/client';

const db = DarshJDB.init({
  appId: 'my-app',
  serverUrl: 'http://localhost:7700', // optional, defaults to localhost
});

// One-shot query
const data = await db.query({ todos: { $where: { done: false } } });

// Subscribe to live updates
const unsub = db.subscribe(
  { todos: { $where: { done: false }, $order: { createdAt: 'desc' } } },
  (data) => {
    console.log('Todos:', data.todos);
  }
);

// Mutations (optimistic -- updates local cache immediately)
db.transact(db.tx.todos[db.id()].set({ title: 'Buy milk', done: false }));

// Batch mutations (atomic)
db.transact([
  db.tx.todos[id1].merge({ done: true }),
  db.tx.todos[id2].delete(),
]);

// Auth
await db.auth.signUp({ email: 'user@example.com', password: 'SecurePass123!' });
await db.auth.signIn({ email: 'user@example.com', password: 'SecurePass123!' });
const user = db.auth.getUser();
await db.auth.signOut();

db.auth.onAuthStateChange((user) => {
  console.log(user ? `Signed in as ${user.email}` : 'Signed out');
});

// Storage
const result = await db.storage.upload(file, { path: 'avatars/me.jpg' });
const url = await db.storage.getUrl('avatars/me.jpg');
await db.storage.delete('avatars/old.jpg');

// Presence
db.presence.enter('room-1', { name: 'Darsh', cursor: null });
db.presence.update('room-1', { cursor: { x: 100, y: 200 } });

const room = db.presence.join('room-1', { name: 'Darsh' });
room.on('change', (peers) => console.log(peers));
room.on('join', (peer) => console.log(`${peer.data.name} joined`));
room.on('leave', (peer) => console.log(`${peer.data.name} left`));

// Server functions
const result = await db.fn('createTodo', { title: 'Buy milk', listId: 'list-1' });

// Clean up
unsub();
db.presence.leave('room-1');
```

## Features

- **WebSocket + MsgPack** -- Persistent connection with binary encoding (28% smaller than JSON)
- **Offline-first** -- IndexedDB persistence, operation queue, sync on reconnect
- **Optimistic mutations** -- Instant UI updates with automatic server reconciliation and rollback
- **Type-safe** -- Full TypeScript types for queries, mutations, auth, and storage
- **Tree-shakeable** -- Only import what you use
- **REST fallback** -- Automatic fallback to HTTP when WebSocket is unavailable

## Architecture

| Module | Description |
|--------|-------------|
| `client.ts` | Core client class -- WebSocket management, MsgPack encoding, auto-reconnect |
| `query.ts` | DarshanQL query builder and live subscription manager |
| `transaction.ts` | Optimistic mutation engine with rollback on server rejection |
| `sync.ts` | Sync protocol -- initial load, delta diffs, catch-up after offline |
| `auth.ts` | Authentication -- sign in/up, OAuth, MFA, token refresh, session management |
| `presence.ts` | Presence rooms -- enter, update, leave, peer tracking |
| `storage.ts` | File storage -- upload, download, signed URLs, resumable uploads |
| `rest.ts` | REST fallback for environments without WebSocket |
| `types.ts` | Shared TypeScript type definitions |

## Building

```bash
npm run build      # Produces dist/ with ESM, CJS, and type declarations
npm run dev        # Watch mode
npm test           # Run tests
npm run typecheck  # Type check
npm run clean      # Remove dist/
```

## Documentation

- [Getting Started](../../docs/getting-started.md)
- [Query Language](../../docs/query-language.md)
- [Authentication](../../docs/authentication.md)
- [Presence](../../docs/presence.md)
- [Storage](../../docs/storage.md)
- [API Reference](../../docs/api-reference.md)

### Credential changes and subscriptions

Subscriptions are shared only within one client instance and identical query. Changing
the token with `setAuthToken` (including sign-out or token refresh) closes the previous
connection and retires its subscriptions and cached rows. Callbacks receive empty data
and `result.error` explaining that resubscription is required. Call `connect()` and subscribe
again after changing credentials. The auth module reconnects after automatic token refresh,
but subscriptions still require resubscription after their retirement error. Explicit `disconnect()` also retires subscriptions;
automatic network reconnects preserve and re-register them.

### Query document fields

REST and WebSocket query results expose the entity UUID as `id` (with `_id` retained
for wire compatibility). Namespaced attributes such as `todos/title` also get a
`title` alias when that suffix is unambiguous. Original attribute keys remain available;
an existing unqualified field or two namespaces with the same suffix are never overwritten.
REST nested query payloads remain under `nested` without flattening their fields.

Run the authenticated integration suite against a disposable server with
`DDB_URL=http://127.0.0.1:7700 npm test`. It creates unique users and data and exercises
real HTTP, SSE, and WebSocket sessions. With `DDB_URL` set, endpoint errors fail tests.
