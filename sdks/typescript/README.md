# DarshJDB TypeScript SDK

Official TypeScript/JavaScript client for [DarshJDB](https://db.darshj.me) — a real-time database with graph relations, live queries, auth, and storage.

## Installation

```bash
npm install darshjdb
```

## Quick Start

```typescript
import { DarshDB } from 'darshjdb';

const db = new DarshDB('http://localhost:8080');
await db.signin({ user: 'root', pass: 'root' });
await db.use('test', 'test');

// Create
const user = await db.create('users', { name: 'Darsh', age: 30 });

// Read
const users = await db.select('users');
const darsh = await db.select('users:darsh');

// Update
await db.update('users:darsh', { age: 31 });

// Delete
await db.delete('users:darsh');

// Query (DarshanQL)
const results = await db.query({
  type: 'users',
  $where: [{ attribute: 'age', op: 'Gt', value: 18 }],
});

// Graph relations
await db.relate('user:darsh', 'works_at', 'company:knowai');

// Live queries (WebSocket)
const stream = await db.live('users');
stream.on('change', (data) => {
  console.log(data.action, data.result);
});

// Server-side functions
const report = await db.run('generateReport', { month: '2026-04' });

await db.close();
```

## API Reference

### Connection

```typescript
const db = new DarshDB('http://localhost:8080', { timeout: 30000 });
await db.close();
```

### Authentication

```typescript
// Root/system auth
await db.signin({ user: 'root', pass: 'root' });

// Email/password auth
await db.signin({ email: 'alice@example.com', password: 'secret' });

// Signup
await db.signup({ email: 'bob@example.com', password: 'pass', name: 'Bob' });

// Set token directly
await db.authenticate('existing-jwt-token');

// Sign out
await db.invalidate();
```

### Namespace & Database

```typescript
await db.use('namespace', 'database');
```

### CRUD

```typescript
// Select all from table — every page the server offers is followed
const users = await db.select('users');

// One page at a time, with the server's pagination metadata
const page = await db.selectPage('users', { limit: 100 });
console.log(page.data, page.hasMore, page.cursor);

// Select specific record
const user = await db.select('users:darsh');

// Create
const newUser = await db.create('users', { name: 'Darsh' });
const withId = await db.create('users:darsh', { name: 'Darsh' });

// Insert (batch)
await db.insert('users', [{ name: 'A' }, { name: 'B' }]);

// Update / Merge
await db.update('users:darsh', { age: 31 });
await db.merge('users:darsh', { age: 31 });

// Delete
await db.delete('users:darsh');
await db.delete('users');
```

### Queries

`/api/query` takes a DarshanQL object; a bare table name is accepted as
shorthand. SQL strings are rejected client-side.

```typescript
const results = await db.query(
  {
    type: 'users',
    $where: [{ attribute: 'age', op: 'Gt', value: 18 }],
    $order: [{ attribute: 'created_at', direction: 'Desc' }],
    $limit: 50,
  },
  { min_age: 18 },
);

for (const qr of results) {
  console.log(qr.data);
  console.log(qr.meta.count);
}

// Shorthand for { type: 'users' }
const everyone = await db.query('users');
```

Operators: `Eq`, `Neq`, `Gt`, `Gte`, `Lt`, `Lte`, `Contains`, `Like`.

### Batch

```typescript
const results = await db.batch([
  { type: 'query', id: 'q1', body: { type: 'users' } },
  {
    type: 'mutate',
    id: 'm1',
    body: { mutations: [{ op: 'insert', entity: 'users', data: { name: 'A' } }] },
  },
  { type: 'fn', id: 'f1', name: 'ping', args: {} },
]);

for (const r of results) {
  console.log(r.id, r.status, r.data ?? r.error);
}
```

### Live Queries

```typescript
const stream = await db.live('users');
stream.on('change', (data) => {
  console.log(data.action); // LiveAction.Create / Update / Delete
  console.log(data.result);
});
stream.on('error', (err) => console.error(err));

// Later:
stream.close();
```

### Graph Relations

```typescript
await db.relate('user:darsh', 'works_at', 'company:knowai');
await db.relate('user:darsh', 'follows', 'user:alice', { since: '2026-01-01' });
```

### Server-Side Functions

```typescript
const result = await db.run('generateReport', { month: '2026-04' });
```

### Full Type Safety

```typescript
interface User {
  id: string;
  name: string;
  age: number;
}

const users = await db.select<User>('users');
// users is User[]

const results = await db.query<User>({ type: 'users' });
// results[0].data is User[]
```

## Development

```bash
npm install
npm run build
npm test
```

## License

MIT
