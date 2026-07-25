# @darshjdb/react

React bindings for DarshJDB -- hooks, provider, and real-time primitives.

## Install

```bash
npm install @darshjdb/react @darshjdb/client
```

Requires React 18 or later as a peer dependency.

## Quick Start

```tsx
import { DarshanProvider, useQuery } from '@darshjdb/react';

function App() {
  return (
    <DarshanProvider serverUrl="https://db.example.com" appId="my-app">
      <TodoList />
    </DarshanProvider>
  );
}
```

The provider creates the client, connects on mount, and disconnects on unmount.
To own the lifecycle yourself, build the client up-front and pass it in:

```tsx
import { DarshanProvider, createDarshanClient } from '@darshjdb/react';

const client = createDarshanClient({
  serverUrl: 'https://db.example.com',
  appId: 'my-app',
});

await client.connect();

<DarshanProvider serverUrl="" appId="" client={client}>
  <App />
</DarshanProvider>;
```

Every module in this package carries the `'use client'` directive, so it can be
imported directly from a Next.js App Router server component tree.

## Hooks

### useQuery -- Live data subscriptions

```tsx
import { useQuery } from '@darshjdb/react';

interface Todo {
  id: string;
  title: string;
  done: boolean;
}

function TodoList() {
  const { data, isLoading, error } = useQuery<Todo>({
    collection: 'todos',
    where: [{ field: 'done', op: '==', value: false }],
    orderBy: [{ field: 'createdAt', direction: 'desc' }],
    limit: 50,
  });

  if (error) return <p>Error: {error.message}</p>;
  if (isLoading) return <p>Loading...</p>;

  return (
    <ul>
      {data.map(todo => (
        <li key={todo.id}>{todo.title}</li>
      ))}
    </ul>
  );
}
```

Options:

```tsx
// Pause the subscription -- `isLoading` settles to false and no query is sent.
useQuery<Todo>({ collection: 'todos' }, { enabled: Boolean(listId) });

// Suspend the first render instead of returning a loading state.
useQuery<Todo>({ collection: 'todos' }, { suspense: true });
```

### useMutation -- Atomic writes

```tsx
import { useMutation } from '@darshjdb/react';

function CreateTodoForm() {
  const { mutate, isLoading, error } = useMutation();

  const handleSubmit = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const title = new FormData(e.currentTarget).get('title') as string;

    await mutate({ type: 'insert', collection: 'todos', data: { title, done: false } });
  };

  return (
    <form onSubmit={handleSubmit}>
      <input name="title" required />
      <button disabled={isLoading}>Add</button>
      {error && <p>{error.message}</p>}
    </form>
  );
}
```

An array of operations is applied atomically:

```tsx
await mutate([
  { type: 'update', collection: 'todos', id: 'todo-1', data: { done: true } },
  { type: 'delete', collection: 'drafts', id: 'draft-9' },
]);
```

### useAuth -- Authentication state

```tsx
import { useAuth } from '@darshjdb/react';

function AuthButton() {
  const { user, signIn, signUp, signOut, isLoading, error } = useAuth();

  if (isLoading) return <Spinner />;
  if (user) return <button onClick={() => signOut()}>Sign out ({user.email})</button>;

  return (
    <button onClick={() => signIn({ email, password })}>
      Sign in{error ? ` -- ${error.message}` : ''}
    </button>
  );
}
```

### usePresence -- Real-time presence

```tsx
import { usePresence } from '@darshjdb/react';

interface Cursor {
  x: number;
  y: number;
  name: string;
}

function CollaborativeEditor() {
  const { peers, publishState } = usePresence<Cursor>('doc-123');

  return (
    <div onMouseMove={e => publishState({ x: e.clientX, y: e.clientY, name: 'Alice' })}>
      {peers.map(peer => (
        <RemoteCursor key={peer.peerId} position={peer.state} name={peer.state.name} />
      ))}
    </div>
  );
}
```

The room is joined on mount and left when the last subscriber unmounts.

### useStorage -- File uploads with progress

```tsx
import { useStorage } from '@darshjdb/react';

function AvatarUpload({ userId }: { userId: string }) {
  const { upload, isUploading, progress, error } = useStorage();

  const handleFile = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;

    const result = await upload(file, `avatars/${userId}.png`);
    console.log('Uploaded:', result.url);
  };

  return (
    <div>
      <input type="file" onChange={handleFile} />
      {isUploading && <progress value={progress.fraction} max={1} />}
      {error && <p>{error.message}</p>}
    </div>
  );
}
```

## API Summary

| Export | Returns | Description |
|--------|---------|-------------|
| `<DarshanProvider serverUrl appId [client]>` | -- | Creates/holds the client and shares it with every hook |
| `useDarshanClient()` | `DarshanClientInterface` | The client from the nearest provider |
| `createDarshanClient(options)` | `DarshanClientInterface` | Build a client manually (wraps `DarshJDB`) |
| `useQuery(query, options?)` | `{ data, isLoading, error }` | Subscribe to a live query |
| `useMutation()` | `{ mutate, isLoading, error }` | Insert / update / delete, atomically |
| `useAuth()` | `{ user, isLoading, error, signIn, signUp, signOut }` | Auth state and actions |
| `usePresence(roomId)` | `{ peers, publishState }` | Real-time presence in a room |
| `useStorage()` | `{ upload, isUploading, progress, error }` | File upload with progress |

## Features

- **Live queries** -- Components re-render automatically when subscribed data changes
- **Suspense support** -- `{ suspense: true }` suspends the first render, not a later one
- **SSR compatible** -- `'use client'` entry, server snapshots, works with Next.js
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
