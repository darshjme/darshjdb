# @darshan/client

Framework-agnostic TypeScript client SDK for DarshanDB. This is the core library that all framework-specific SDKs (`@darshan/react`, `@darshan/nextjs`, `@darshan/angular`) build on top of.

## Install

```bash
npm install @darshan/client
```

Use this package directly if you are building for a framework without a dedicated SDK, or if you need a plain TypeScript/JavaScript client.

## Usage

```typescript
import { DarshanDB } from '@darshan/client';

const db = DarshanDB.init({
  appId: 'my-app',
  serverUrl: 'http://localhost:7700', // optional, defaults to localhost
});

// Query (returns a live-updating observable)
const unsub = db.subscribe(
  { todos: { $where: { done: false }, $order: { createdAt: 'desc' } } },
  (data) => {
    console.log('Todos:', data.todos);
  }
);

// One-shot query
const data = await db.query({ todos: {} });

// Mutations
db.transact(db.tx.todos[db.id()].set({ title: 'Buy milk', done: false }));

// Auth
await db.auth.signIn({ email: 'user@example.com', password: 'password' });
const user = db.auth.getUser();

// Storage
const result = await db.storage.upload(file, { path: 'avatars/me.jpg' });

// Presence
db.presence.enter('room-1', { name: 'Darsh', cursor: null });
```

## Features

- **WebSocket + MsgPack** -- Persistent connection with binary encoding
- **Offline-first** -- IndexedDB persistence, operation queue, sync on reconnect
- **Optimistic mutations** -- Instant UI updates with server reconciliation
- **Type-safe** -- Full TypeScript types for queries, mutations, and auth
- **Tree-shakeable** -- Only import what you use

## Building

```bash
npm run build
```

Output is in `dist/` with ESM, CJS, and TypeScript declaration files.

## Documentation

- [Query Language](../../docs/query-language.md)
- [Authentication](../../docs/authentication.md)
- [Presence](../../docs/presence.md)
- [Storage](../../docs/storage.md)
