# @darshan/react

React bindings for DarshanDB -- hooks, provider, and real-time primitives.

## Install

```bash
npm install @darshan/react
```

## Usage

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
  const { data, isLoading, error } = db.useQuery({
    todos: { $where: { done: false }, $order: { createdAt: 'desc' } }
  });

  if (isLoading) return <p>Loading...</p>;
  if (error) return <p>Error: {error.message}</p>;

  return (
    <ul>
      {data.todos.map(todo => (
        <li key={todo.id}>{todo.title}</li>
      ))}
    </ul>
  );
}
```

## Hooks

| Hook | Description |
|------|-------------|
| `db.useQuery(query)` | Subscribe to a live query. Returns `{ data, isLoading, error }` |
| `db.useAuth()` | Auth state and methods. Returns `{ user, signIn, signOut, isLoading }` |
| `usePresence(room, initialData)` | Real-time presence. Returns `{ peers, myPresence, updatePresence }` |
| `db.useUpload()` | File upload with progress. Returns `{ upload, isUploading, progress }` |

## Features

- **Live queries** -- Components re-render when subscribed data changes
- **Suspense support** -- Use with React Suspense for loading states
- **Optimistic mutations** -- UI updates instantly, reconciles with server
- **SSR compatible** -- Works with Next.js and other SSR frameworks

## Documentation

- [Getting Started](../../docs/getting-started.md)
- [Query Language](../../docs/query-language.md)
- [Authentication](../../docs/authentication.md)
- [Presence](../../docs/presence.md)
