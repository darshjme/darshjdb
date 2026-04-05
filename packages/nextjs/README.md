# @darshan/nextjs

Next.js SDK for DarshanDB -- Server Components, App Router, Pages Router, and Middleware support.

## Install

```bash
npm install @darshan/nextjs
```

## Usage

### Server Components (App Router)

```tsx
// app/page.tsx
import { queryServer } from '@darshan/nextjs';

export default async function Page() {
  const data = await queryServer({
    todos: { $where: { done: false }, $order: { createdAt: 'desc' } }
  });

  return (
    <ul>
      {data.todos.map(todo => (
        <li key={todo.id}>{todo.title}</li>
      ))}
    </ul>
  );
}
```

### Client Components

```tsx
'use client';
import { DarshanDB } from '@darshan/nextjs';

const db = DarshanDB.init({ appId: 'my-app' });

export function TodoList() {
  const { data, isLoading } = db.useQuery({
    todos: { $where: { done: false } }
  });

  if (isLoading) return <p>Loading...</p>;
  return <ul>{data.todos.map(t => <li key={t.id}>{t.title}</li>)}</ul>;
}
```

### Server Actions

```tsx
// app/actions.ts
'use server';
import { mutateServer } from '@darshan/nextjs';

export async function createTodo(title: string) {
  return mutateServer('createTodo', { title, listId: 'default' });
}
```

### Middleware (Auth)

```typescript
// middleware.ts
import { darshanMiddleware } from '@darshan/nextjs';

export default darshanMiddleware({
  protectedRoutes: ['/dashboard', '/settings'],
  signInUrl: '/login',
});
```

## Features

- **Server Components** -- Query data on the server with zero client JavaScript
- **App Router + Pages Router** -- Works with both Next.js routing models
- **Server Actions** -- Call server functions from client components
- **Auth Middleware** -- Protect routes at the edge
- **Streaming** -- Compatible with React Suspense and streaming SSR

## Documentation

- [Getting Started](../../docs/getting-started.md)
- [Server Functions](../../docs/server-functions.md)
- [Authentication](../../docs/authentication.md)
