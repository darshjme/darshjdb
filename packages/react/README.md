# @darshan/react

React bindings for DarshanDB -- hooks, provider, and real-time primitives.

## Install

```bash
npm install @darshan/react
```

Requires React 18 or later as a peer dependency.

## Quick Start

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
```

## Hooks

### useQuery -- Live data subscriptions

```tsx
function TodoList() {
  const { data, isLoading, error } = db.useQuery({
    todos: {
      $where: { done: false },
      $order: { createdAt: 'desc' },
      owner: {}  // load related user
    }
  });

  if (error) return <p>Error: {error.message}</p>;
  if (isLoading) return <p>Loading...</p>;

  return (
    <ul>
      {data.todos.map(todo => (
        <li key={todo.id}>
          {todo.title} - by {todo.owner?.name}
        </li>
      ))}
    </ul>
  );
}
```

### useAuth -- Authentication state

```tsx
function AuthButton() {
  const { user, signIn, signUp, signOut, isLoading } = db.useAuth();

  if (isLoading) return <Spinner />;

  if (user) {
    return <button onClick={signOut}>Sign Out ({user.email})</button>;
  }

  return <button onClick={() => signIn({ email, password })}>Sign In</button>;
}
```

### usePresence -- Real-time presence

```tsx
import { usePresence } from '@darshan/react';

function CollaborativeEditor() {
  const { peers, myPresence, updatePresence } = usePresence('doc-123', {
    name: currentUser.name,
    cursor: null,
  });

  return (
    <div onMouseMove={(e) => updatePresence({ cursor: { x: e.clientX, y: e.clientY } })}>
      {peers.map(peer => (
        <RemoteCursor key={peer.id} position={peer.data.cursor} name={peer.data.name} />
      ))}
    </div>
  );
}
```

### useUpload -- File uploads with progress

```tsx
function AvatarUpload() {
  const { upload, isUploading, progress } = db.useUpload();

  const handleFile = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (file) {
      const result = await upload(file, { path: `avatars/${user.id}` });
      console.log('Uploaded:', result.url);
    }
  };

  return (
    <div>
      <input type="file" onChange={handleFile} />
      {isUploading && <progress value={progress} max={100} />}
    </div>
  );
}
```

### useMutation -- Server function calls

```tsx
function CreateTodoForm() {
  const { mutate, isLoading } = db.useMutation('createTodo');

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const data = new FormData(e.currentTarget);
    await mutate({ title: data.get('title'), listId: 'default' });
  };

  return (
    <form onSubmit={handleSubmit}>
      <input name="title" required />
      <button disabled={isLoading}>Add</button>
    </form>
  );
}
```

## Hooks Summary

| Hook | Returns | Description |
|------|---------|-------------|
| `db.useQuery(query)` | `{ data, isLoading, error }` | Subscribe to a live DarshanQL query |
| `db.useAuth()` | `{ user, signIn, signUp, signOut, isLoading }` | Auth state and methods |
| `usePresence(room, data)` | `{ peers, myPresence, updatePresence }` | Real-time presence in a room |
| `db.useUpload()` | `{ upload, isUploading, progress }` | File upload with progress tracking |
| `db.useMutation(name)` | `{ mutate, isLoading, error }` | Call a server function |
| `db.useFn(name, args)` | `{ data, isLoading, error }` | Call a server query function (reactive) |

## Features

- **Live queries** -- Components re-render automatically when subscribed data changes
- **Suspense support** -- Works with React Suspense for loading states
- **Optimistic mutations** -- UI updates instantly, reconciles with server
- **SSR compatible** -- Works with Next.js and other SSR frameworks
- **Concurrent mode safe** -- Uses `useSyncExternalStore` under the hood

## Building

```bash
npm run build      # Produces dist/ with ESM, CJS, and type declarations
npm run dev        # Watch mode
npm test           # Run tests
npm run typecheck  # Type check
```

## Documentation

- [Getting Started](../../docs/getting-started.md)
- [Query Language](../../docs/query-language.md)
- [Authentication](../../docs/authentication.md)
- [Presence](../../docs/presence.md)
- [Storage](../../docs/storage.md)
