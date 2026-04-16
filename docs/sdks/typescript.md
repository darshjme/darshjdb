# TypeScript SDK

The `@darshjdb/client` package is the core TypeScript SDK for DarshJDB. It provides a real-time WebSocket connection (with REST fallback), type-safe queries with live subscriptions, atomic transactions, authentication, file storage, and offline-first sync -- all communicating via MessagePack over the wire.

## Installation

```bash
npm install @darshjdb/client
```

## Connection

```ts
import { DarshJDB } from '@darshjdb/client';

const db = new DarshJDB({
  serverUrl: 'https://db.example.com',
  appId: 'my-app',
  transport: 'auto', // 'ws' | 'rest' | 'auto' (default)
});

await db.connect();
```

The client manages the full WebSocket lifecycle internally: automatic reconnection with exponential backoff (500ms initial, 30s cap, 30% jitter), a 25-second keepalive ping, and MessagePack binary encoding for all wire messages.

### Connection States

The connection state machine transitions through five states:

| State            | Meaning                                         |
|------------------|------------------------------------------------|
| `disconnected`   | No active connection                            |
| `connecting`     | WebSocket handshake in progress                 |
| `authenticating` | Connected, exchanging auth credentials          |
| `connected`      | Fully operational                               |
| `reconnecting`   | Connection lost, attempting to re-establish      |

Listen for transitions:

```ts
const unsubscribe = db.onConnectionStateChange((state, prev) => {
  console.log(`${prev} -> ${state}`);
});
```

### Transport Modes

- **`auto`** (default) -- WebSocket with automatic REST fallback.
- **`ws`** -- WebSocket only.
- **`rest`** -- HTTP-only mode. No persistent connection; marks as `connected` immediately.

## Queries

The `QueryBuilder` provides a fluent, type-safe API for constructing DarshJQL queries.

```ts
interface User {
  id: string;
  name: string;
  age: number;
  createdAt: string;
}

const result = await db.query<User>('users')
  .where('age', '>=', 18)
  .where('name', 'starts-with', 'A')
  .orderBy('createdAt', 'desc')
  .limit(20)
  .offset(40)
  .select('id', 'name', 'age')
  .exec();

console.log(result.data);  // User[]
console.log(result.txId);  // server transaction ID
```

### Supported Where Operators

`=`, `!=`, `>`, `>=`, `<`, `<=`, `in`, `not-in`, `contains`, `starts-with`

Dot notation is supported for nested fields: `.where('address.city', '=', 'Berlin')`.

### Live Subscriptions

Queries are automatically deduplicated. If two components subscribe to the same query, only one server subscription is created.

```ts
const unsubscribe = db.query<User>('users')
  .where('active', '=', true)
  .subscribe((result) => {
    console.log('Live data:', result.data);
    console.log('At tx:', result.txId);
  });

// Later:
unsubscribe();
```

When the last subscriber for a given query unsubscribes, the server subscription is torn down automatically.

## Mutations (Transactions)

All writes go through atomic transactions. The transaction builder uses a Proxy-based API where collection access and document access produce mutation proxies on the fly.

```ts
import { transact, generateId } from '@darshjdb/client';

const txId = await transact(db, (tx) => {
  // Create a new user
  const userId = generateId(); // UUID v7 (time-ordered)
  tx.users[userId].set({ name: 'Alice', age: 30 });

  // Partial update
  tx.posts['post-1'].merge({ title: 'Updated Title' });

  // Delete
  tx.comments['c-1'].delete();

  // Graph relations
  tx.users[userId].link('teams', 'team-42');
  tx.users['old-user'].unlink('teams', 'team-42');
});
```

### Transaction Operations

| Method     | Description                                    |
|------------|------------------------------------------------|
| `set(data)` | Replace entity data entirely                   |
| `merge(data)` | Merge fields into existing entity              |
| `delete()`  | Remove the entity                               |
| `link(entity, id)` | Create a graph edge to another entity     |
| `unlink(entity, id)` | Remove a graph edge                      |

All operations within a single `transact()` call are atomic -- they all succeed or all fail. Empty transactions throw an error.

### ID Generation

`generateId()` produces UUID v7 identifiers, which are time-ordered for optimal database index locality.

## Authentication

```ts
import { AuthClient } from '@darshjdb/client';

const auth = new AuthClient(db);
await auth.init(); // Restore persisted session

// Email/password
const user = await auth.signUp({
  email: 'alice@example.com',
  password: 's3cret',
  displayName: 'Alice',
});

await auth.signIn({ email: 'alice@example.com', password: 's3cret' });

// OAuth popup
const user = await auth.signInWithOAuth('google');
// Supported providers: google, github, apple, discord, or any custom string

// Session management
auth.getUser();    // User | null
auth.getTokens();  // { accessToken, refreshToken, expiresAt } | null
await auth.signOut();
```

### Auth State Listener

The callback fires immediately with the current state, then on every subsequent change.

```ts
const unsubscribe = auth.onAuthStateChange(({ user, tokens }) => {
  if (user) {
    console.log('Signed in:', user.displayName);
  } else {
    console.log('Signed out');
  }
});
```

### Token Management

Tokens are persisted to `localStorage` (with an in-memory fallback for non-browser environments). Refresh happens automatically 60 seconds before expiry. You can supply a custom `TokenStorage` adapter:

```ts
const auth = new AuthClient(db, {
  get: (key) => myStore.get(key),
  set: (key, value) => myStore.set(key, value),
  remove: (key) => myStore.remove(key),
});
```

## File Storage

```ts
import { StorageClient } from '@darshjdb/client';

const storage = new StorageClient(db);

// Upload with progress tracking
const result = await storage.upload('avatars/profile.png', file, {
  contentType: 'image/png',
  metadata: { userId: '123' },
  onProgress: (fraction) => console.log(`${(fraction * 100).toFixed(0)}%`),
});
// result: { path, url, size, contentType }

// Get a signed URL
const url = await storage.getUrl('avatars/profile.png');

// Delete
await storage.delete('avatars/profile.png');
```

Files under 5 MB are uploaded in a single request. Larger files use a resumable chunked upload protocol (2 MB chunks) with three phases: initiate, upload chunks, complete.

## Offline Sync

The `SyncEngine` provides IndexedDB-backed caching, optimistic updates, and an offline mutation queue.

```ts
import { SyncEngine } from '@darshjdb/client';

const sync = new SyncEngine(db);
await sync.init();

// Cache query results locally
await sync.setCache(queryHash, result);
const cached = await sync.getCached<User>(queryHash);

// Optimistic updates
const tempId = sync.applyOptimistic(ops);
// On server confirm:
sync.confirmOptimistic(tempId);
// On server reject:
const rolledBack = sync.rollbackOptimistic(tempId);

// Offline queue
await sync.enqueue(ops);
const replayed = await sync.replayQueue(); // Returns count of successful replays

// Track server position for catch-up
await sync.setLastTxId(txId);
const lastTx = await sync.getLastTxId();
```

Failed queue entries are retried up to 5 times before being discarded. If a network error occurs during replay, the engine stops to avoid sending duplicate mutations while still offline.

## Error Handling

All async methods throw standard `Error` objects with descriptive messages:

```ts
try {
  await transact(db, (tx) => {
    tx.users['u1'].set({ name: 'Alice' });
  });
} catch (err) {
  if (err.message.includes('Transaction failed')) {
    // Server rejected the write
  }
}
```

Connection errors, authentication failures, upload errors, and transaction rejections all follow this pattern. The `message` field contains the HTTP status code and server response body where applicable.
