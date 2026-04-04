# Getting Started with DarshanDB

Get a complete backend running in under five minutes.

## Prerequisites

- Docker (for Postgres) or an existing PostgreSQL 16+ instance
- Node.js 18+ (for client SDKs)

## Install

```bash
# macOS / Linux
curl -fsSL https://darshandb.dev/install | sh

# Or with Docker
docker compose up -d
```

## Start the Dev Server

```bash
darshan dev
```

This will:
1. Start a PostgreSQL instance (via Docker if needed)
2. Create the database and tables
3. Start the DarshanDB server on `http://localhost:7700`
4. Open the admin dashboard at `http://localhost:7700/admin`

## Connect Your App

### React

```bash
npm install @darshan/react
```

```tsx
import { DarshanProvider, DarshanDB } from '@darshan/react';

const db = DarshanDB.init({ appId: 'my-app' });

function App() {
  return (
    <DarshanProvider db={db}>
      <TodoApp />
    </DarshanProvider>
  );
}

function TodoApp() {
  const { data, isLoading } = db.useQuery({
    todos: {
      $where: { done: false },
      $order: { createdAt: 'desc' }
    }
  });

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

### Next.js

```bash
npm install @darshan/nextjs
```

```tsx
// app/page.tsx (Server Component)
import { queryServer } from '@darshan/nextjs';

export default async function Page() {
  const data = await queryServer({ todos: { $order: { createdAt: 'desc' } } });
  return <TodoList items={data.todos} />;
}
```

### PHP

```bash
composer require darshan/darshan-php
```

```php
$db = new DarshanDB\Client(['serverUrl' => 'http://localhost:7700', 'apiKey' => 'your-key']);
$todos = $db->query(['todos' => ['$where' => ['done' => false]]]);
```

### Python

```bash
pip install darshandb
```

```python
from darshandb import DarshanDB

db = DarshanDB("http://localhost:7700", api_key="your-key")
todos = db.query({"todos": {"$where": {"done": False}}})
```

### cURL

```bash
curl http://localhost:7700/api/query \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"todos": {"$where": {"done": false}}}'
```

## What's Next

- [Query Language Reference](query-language.md)
- [Server Functions](server-functions.md)
- [Authentication](authentication.md)
- [Permissions](permissions.md)
- [Self-Hosting Guide](self-hosting.md)
