# React SDK

The `@darshjdb/react` package provides idiomatic React hooks for DarshJDB, built on `useSyncExternalStore` for safe concurrent rendering with React 18+.

## Installation

```bash
npm install @darshjdb/react @darshjdb/client
```

## Provider Setup

Wrap your application with `DarshanProvider`. It creates and manages a DarshJDB client instance, connecting on mount and disconnecting on unmount.

```tsx
import { DarshanProvider } from '@darshjdb/react';

function App() {
  return (
    <DarshanProvider serverUrl="https://db.example.com" appId="my-app">
      <MyComponent />
    </DarshanProvider>
  );
}
```

### Provider Props

| Prop        | Type                       | Description                                        |
|-------------|----------------------------|----------------------------------------------------|
| `serverUrl` | `string`                   | DarshJDB server URL                                |
| `appId`     | `string`                   | Application identifier                             |
| `client`    | `DarshanClientInterface?`  | Pre-constructed client (bypasses internal creation) |
| `children`  | `ReactNode`                | Child components                                   |

When `serverUrl` or `appId` change, the previous client is torn down and a new one is created. This is intentional for HMR scenarios.

### External Client

If you need full control over the client lifecycle, pass a pre-constructed client:

```tsx
import { DarshJDB } from '@darshjdb/client';
import { DarshanProvider } from '@darshjdb/react';

const db = new DarshJDB({ serverUrl: '...', appId: '...' });
await db.connect();

<DarshanProvider client={db}>
  <App />
</DarshanProvider>
```

### Accessing the Client Directly

```ts
import { useDarshanClient } from '@darshjdb/react';

function MyComponent() {
  const client = useDarshanClient();
  // Use client.send(), client.state, etc.
}
```

Throws if called outside a `<DarshanProvider>`.

## useQuery

Subscribe to a live DarshJDB query. The data reference is stable across re-renders when the contents have not changed (shallow array comparison).

```tsx
import { useQuery } from '@darshjdb/react';

function TodoList() {
  const { data, isLoading, error } = useQuery({
    collection: 'todos',
    where: [{ field: 'done', op: '==', value: false }],
    orderBy: [{ field: 'createdAt', direction: 'desc' }],
    limit: 50,
  });

  if (isLoading) return <p>Loading...</p>;
  if (error) return <p>Error: {error.message}</p>;

  return (
    <ul>
      {data.map(todo => <li key={todo.id}>{todo.title}</li>)}
    </ul>
  );
}
```

### Options

| Option     | Type      | Default | Description                              |
|------------|-----------|---------|------------------------------------------|
| `suspense` | `boolean` | `false` | Throw pending promise for `<Suspense>`   |
| `enabled`  | `boolean` | `true`  | When `false`, subscription is paused     |

### Return Value

| Field       | Type              | Description                              |
|-------------|-------------------|------------------------------------------|
| `data`      | `ReadonlyArray<T>` | Current result set (empty while loading) |
| `isLoading` | `boolean`         | `true` until first snapshot arrives      |
| `error`     | `Error \| null`    | Non-null on subscription error           |

### Suspense Mode

```tsx
import { Suspense } from 'react';
import { useQuery } from '@darshjdb/react';

function Todos() {
  const { data } = useQuery(
    { collection: 'todos' },
    { suspense: true },
  );
  // data is always populated here -- Suspense handles the loading state
  return <ul>{data.map(t => <li key={t.id}>{t.title}</li>)}</ul>;
}

function App() {
  return (
    <Suspense fallback={<p>Loading...</p>}>
      <Todos />
    </Suspense>
  );
}
```

### Conditional Queries

```tsx
function UserProfile({ userId }: { userId: string | null }) {
  const { data } = useQuery(
    { collection: 'users', where: [{ field: 'id', op: '==', value: userId }] },
    { enabled: !!userId },
  );
  // When userId is null, the subscription is paused and last data is retained
}
```

## useMutation

Execute insert, update, and delete operations. Optimistic updates are handled at the client-core layer -- mutations appear instantly in active `useQuery` subscriptions and roll back automatically if the server rejects the write.

```tsx
import { useMutation } from '@darshjdb/react';

function AddTodo() {
  const { mutate, isLoading, error } = useMutation();

  const handleAdd = async () => {
    await mutate({
      type: 'insert',
      collection: 'todos',
      data: { title: 'New task', done: false },
    });
  };

  return (
    <>
      <button onClick={handleAdd} disabled={isLoading}>Add</button>
      {error && <p>Error: {error.message}</p>}
    </>
  );
}
```

### Batch Mutations

Pass an array for atomic all-or-nothing writes:

```ts
await mutate([
  { type: 'insert', collection: 'todos', data: { title: 'First' } },
  { type: 'update', collection: 'todos', id: 'abc', data: { done: true } },
  { type: 'delete', collection: 'todos', id: 'xyz' },
]);
```

The `mutate` function reference is stable across re-renders -- safe to include in dependency arrays or pass as props.

## usePresence

Join a real-time presence room. Peers join on mount and leave on unmount. Uses `useSyncExternalStore` for concurrent-safe rendering.

```tsx
import { usePresence } from '@darshjdb/react';

interface CursorState {
  x: number;
  y: number;
  name: string;
}

function Cursors() {
  const { peers, publishState } = usePresence<CursorState>('canvas-room');

  const handleMouseMove = (e: React.MouseEvent) => {
    publishState({ x: e.clientX, y: e.clientY, name: 'Alice' });
  };

  return (
    <div onMouseMove={handleMouseMove}>
      {peers.map(p => (
        <div
          key={p.peerId}
          style={{ position: 'fixed', left: p.state.x, top: p.state.y }}
        >
          {p.state.name}
        </div>
      ))}
    </div>
  );
}
```

### Return Value

| Field          | Type                              | Description                           |
|----------------|-----------------------------------|---------------------------------------|
| `peers`        | `ReadonlyArray<PresencePeer<S>>`  | Current peers (excluding self)        |
| `publishState` | `(state: S) => void`              | Broadcast local state to all peers    |

Each `PresencePeer` contains `peerId`, optional `userId`, the typed `state` object, and a `lastSeen` timestamp.

## useAuth

Observe and control authentication state reactively. All returned references are stable across re-renders.

```tsx
import { useAuth } from '@darshjdb/react';

function AuthGate({ children }: { children: React.ReactNode }) {
  const { user, isLoading, error, signIn, signUp, signOut } = useAuth();

  if (isLoading) return <p>Authenticating...</p>;

  if (!user) {
    return (
      <button onClick={() => signIn({ email: 'a@b.com', password: 'pw' })}>
        Sign In
      </button>
    );
  }

  return (
    <>
      <p>Hello, {user.displayName ?? user.email}</p>
      <button onClick={signOut}>Sign Out</button>
      {children}
    </>
  );
}
```

### Return Value

| Field      | Type                                                      | Description                    |
|------------|-----------------------------------------------------------|--------------------------------|
| `user`     | `AuthUser \| null`                                         | Current user or null           |
| `isLoading`| `boolean`                                                 | Initial auth state resolving   |
| `error`    | `Error \| null`                                            | Last auth action error         |
| `signIn`   | `(credentials: AuthCredentials) => Promise<AuthUser>`     | Email/password sign-in         |
| `signUp`   | `(credentials: AuthCredentials) => Promise<AuthUser>`     | Create account                 |
| `signOut`  | `() => Promise<void>`                                      | Sign out                       |

## useStorage

Upload files with reactive progress tracking. Progress updates are throttled to at most once every 50ms to prevent render thrashing.

```tsx
import { useStorage } from '@darshjdb/react';

function AvatarUpload() {
  const { upload, isUploading, progress, error } = useStorage();

  const handleFileChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;

    const result = await upload(file, `avatars/${file.name}`);
    console.log('Uploaded to:', result.url);
  };

  return (
    <div>
      <input type="file" onChange={handleFileChange} disabled={isUploading} />
      {isUploading && (
        <progress value={progress.fraction} max={1} />
      )}
      {error && <p>Upload failed: {error.message}</p>}
    </div>
  );
}
```

### Return Value

| Field         | Type                                          | Description                  |
|---------------|-----------------------------------------------|------------------------------|
| `upload`      | `(file: File \| Blob, path: string) => Promise<UploadResult>` | Upload function |
| `isUploading` | `boolean`                                     | In-flight upload indicator   |
| `progress`    | `UploadProgress`                              | `{ bytesTransferred, totalBytes, fraction }` |
| `error`       | `Error \| null`                                | Last upload error            |

The `upload` reference is stable across re-renders.
