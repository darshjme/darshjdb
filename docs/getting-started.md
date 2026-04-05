# Getting Started with DarshJDB

Get a complete backend running in under five minutes.

## Prerequisites

- Docker (for Postgres) or an existing PostgreSQL 16+ instance with [pgvector](https://github.com/pgvector/pgvector)
- Node.js 18+ (for client SDKs)
- Rust toolchain (only if building from source)

## Install

> **Status: Alpha** — DarshJDB is not yet published to package registries. Install from source.

```bash
# Clone and build from source
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb
cargo build --release

# Or with Docker (includes PostgreSQL 16 + pgvector)
docker compose up -d
```

Verify the build:

```bash
./target/release/ddb-server --version
# darshjdb 0.1.0
```

## Start the Dev Server

```bash
ddb dev
```

This will:
1. Start a PostgreSQL instance (via Docker if needed)
2. Create the database and apply the initial schema
3. Start the DarshJDB server on `http://localhost:7700`
4. Open the admin dashboard at `http://localhost:7700/admin`
5. Watch `darshan/functions/` for changes and hot-reload server functions

## Connect Your App

### React

```bash
npm install @darshjdb/react
```

```tsx
import { DarshanProvider, DarshJDB } from '@darshjdb/react';

const db = DarshJDB.init({ appId: 'my-app' });

function App() {
  return (
    <DarshanProvider db={db}>
      <TodoApp />
    </DarshanProvider>
  );
}

function TodoApp() {
  const { data, isLoading, error } = db.useQuery({
    todos: {
      $where: { done: false },
      $order: { createdAt: 'desc' }
    }
  });

  if (error) return <p>Error: {error.message}</p>;
  if (isLoading) return <p>Loading...</p>;

  return (
    <ul>
      {data.todos.map(todo => (
        <li key={todo.id}>{todo.title}</li>
      ))}
    </ul>
  );
}
```

### Next.js (App Router)

```bash
npm install @darshjdb/nextjs
```

```tsx
// app/providers.tsx ("use client")
import { DarshanProvider, DarshJDB } from '@darshjdb/nextjs';

const db = DarshJDB.init({ appId: 'my-app' });

export function Providers({ children }: { children: React.ReactNode }) {
  return <DarshanProvider db={db}>{children}</DarshanProvider>;
}
```

```tsx
// app/page.tsx (Server Component)
import { queryServer } from '@darshjdb/nextjs/server';

export default async function Page() {
  const data = await queryServer({ todos: { $order: { createdAt: 'desc' } } });
  return <TodoList items={data.todos} />;
}
```

```tsx
// app/api/todos/route.ts (Server Action)
import { createServerAction } from '@darshjdb/nextjs/api';

export const POST = createServerAction(async (ctx, body) => {
  return ctx.db.transact(
    ctx.db.tx.todos[ctx.db.id()].set({ title: body.title, done: false })
  );
});
```

### Next.js (Pages Router)

```tsx
// pages/_app.tsx
import { DarshanProvider, DarshJDB } from '@darshjdb/nextjs/pages';

const db = DarshJDB.init({ appId: 'my-app' });

export default function App({ Component, pageProps }) {
  return (
    <DarshanProvider db={db}>
      <Component {...pageProps} />
    </DarshanProvider>
  );
}
```

### Angular

```bash
npm install @darshjdb/angular
```

```typescript
// app.config.ts
import { provideDarshan } from '@darshjdb/angular';

export const appConfig = {
  providers: [
    provideDarshan({ appId: 'my-app' }),
  ],
};
```

```typescript
// todo-list.component.ts
import { Component, inject } from '@angular/core';
import { DarshanService, injectQuery } from '@darshjdb/angular';

@Component({
  selector: 'app-todo-list',
  template: `
    @if (todos.isLoading()) {
      <p>Loading...</p>
    } @else {
      <ul>
        @for (todo of todos.data()?.todos; track todo.id) {
          <li>{{ todo.title }}</li>
        }
      </ul>
    }
  `,
})
export class TodoListComponent {
  todos = injectQuery({ todos: { $where: { done: false } } });
}
```

### Vanilla JavaScript (CDN)

```html
<script src="https://cdn.db.darshj.me/client.min.js"></script>
<script>
  const db = DarshJDB.init({ appId: 'my-app' });

  db.query({ todos: { $where: { done: false } } }).then(data => {
    data.todos.forEach(todo => {
      document.body.innerHTML += `<p>${todo.title}</p>`;
    });
  });
</script>
```

### PHP

```bash
composer require darshan/darshan-php
```

```php
use DarshJDB\Client;

$db = new Client([
    'serverUrl' => 'http://localhost:7700',
    'apiKey' => 'your-key',
]);

// Query
$todos = $db->query(['todos' => ['$where' => ['done' => false]]]);

// Mutation
$db->transact([
    ['entity' => 'todos', 'id' => $db->id(), 'op' => 'set', 'data' => [
        'title' => 'Buy groceries',
        'done' => false,
    ]],
]);
```

#### Laravel Integration

```php
// config/ddb.php (published by the ServiceProvider)
return [
    'server_url' => env('DDB_URL', 'http://localhost:7700'),
    'api_key' => env('DDB_API_KEY'),
];
```

```php
// Usage via facade
use DarshJDB\Facades\Darshan;

$todos = Darshan::query(['todos' => ['$where' => ['done' => false]]]);
```

### Python

```bash
pip install darshjdb
```

```python
from darshjdb import DarshJDB

db = DarshJDB("http://localhost:7700", api_key="your-key")

# Query
todos = db.query({"todos": {"$where": {"done": False}}})

# Mutation
db.transact([
    {"entity": "todos", "id": db.id(), "op": "set", "data": {"title": "Buy groceries", "done": False}}
])
```

#### FastAPI Integration

```python
from fastapi import FastAPI, Depends
from darshjdb.fastapi import get_db, DarshJDB

app = FastAPI()

@app.get("/todos")
async def list_todos(db: DarshJDB = Depends(get_db)):
    return await db.query({"todos": {"$where": {"done": False}}})
```

### cURL

```bash
# Query
curl http://localhost:7700/api/query \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"todos": {"$where": {"done": false}}}'

# Create
curl -X POST http://localhost:7700/api/data/todos \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title": "Buy groceries", "done": false}'
```

## Project Structure

After running `ddb dev`, your project looks like this:

```
my-app/
  darshan/
    functions/       # Server functions (TypeScript)
    permissions.ts   # Permission rules
    schema.ts        # Schema definition (optional, for strict mode)
    env.ts           # Environment variable declarations
    migrations/      # Auto-generated migration files (production)
  src/               # Your app code
  package.json
```

## What's Next

- [Query Language Reference](query-language.md) -- Learn DarshanQL filtering, sorting, and mutations
- [Server Functions](server-functions.md) -- Write backend logic that runs on the server
- [Authentication](authentication.md) -- Set up email/password, OAuth, and MFA
- [Permissions](permissions.md) -- Control who can read and write data
- [Architecture](architecture.md) -- Understand how DarshJDB works under the hood
- [Presence](presence.md) -- Add real-time cursors and typing indicators
- [Storage](storage.md) -- Upload and serve files
- [Self-Hosting Guide](self-hosting.md) -- Deploy to your own infrastructure
- [REST API Reference](api-reference.md) -- Complete HTTP endpoint reference

## Troubleshooting

### `ddb dev` fails to start

**Port already in use:**

```bash
# Check what's using port 7700
lsof -i :7700
# Kill the process or use a different port
ddb dev --port 7701
```

**PostgreSQL connection failed:**

```bash
# If using Docker, make sure it's running
docker ps | grep postgres

# If using an external Postgres, verify the connection string
ddb dev --database-url "postgres://user:pass@localhost:5432/darshjdb"
```

**Permission denied when building:**

```bash
# Ensure you have write access to the project directory
sudo chown -R $(whoami) darshjdb/
cargo build --release
```

### Client SDK won't connect

**CORS errors in browser:**

DarshJDB has CORS disabled by default in production. In development (`ddb dev`), it allows `localhost` origins. For production, configure allowed origins:

```bash
DDB_CORS_ORIGINS=https://myapp.com,https://admin.myapp.com
```

**WebSocket connection drops:**

Check that your reverse proxy (nginx, Cloudflare, etc.) supports WebSocket upgrades:

```nginx
# nginx.conf
location / {
    proxy_pass http://localhost:7700;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_read_timeout 86400s;
}
```

### SDK type errors after server update

After updating the DarshJDB server, make sure your client SDKs are on a compatible version:

```bash
# Check server version
curl http://localhost:7700/api/admin/health

# Update all SDKs
npm update @darshjdb/react @darshjdb/nextjs @darshjdb/client
```

See the full [Troubleshooting Guide](troubleshooting.md) for more solutions.

---

[Next: Architecture](architecture.md) | [All Docs](README.md)
