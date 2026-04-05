# @darshjdb/nextjs

Next.js SDK for DarshJDB -- Server Components, App Router, Pages Router, Server Actions, and Middleware.

## Install

```bash
npm install @darshjdb/nextjs
```

Requires Next.js 13+ and React 18+ as peer dependencies.

## Exports

This package provides multiple entry points for different Next.js contexts:

| Import | Context | Description |
|--------|---------|-------------|
| `@darshjdb/nextjs` | Client Components | Client-side hooks and DarshJDB instance |
| `@darshjdb/nextjs/server` | Server Components | Server-side query functions |
| `@darshjdb/nextjs/provider` | Client layout | `DarshanProvider` for client-side state |
| `@darshjdb/nextjs/pages` | Pages Router | Provider and hooks for Pages Router |
| `@darshjdb/nextjs/middleware` | Edge Middleware | Auth middleware for route protection |
| `@darshjdb/nextjs/api` | API Routes | Server action helpers |

## App Router

### Server Components

```tsx
// app/page.tsx (Server Component -- no "use client")
import { queryServer } from '@darshjdb/nextjs/server';

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
// app/components/TodoList.tsx
'use client';
import { DarshJDB } from '@darshjdb/nextjs';

const db = DarshJDB.init({ appId: 'my-app' });

export function TodoList() {
  const { data, isLoading } = db.useQuery({
    todos: { $where: { done: false } }
  });

  if (isLoading) return <p>Loading...</p>;
  return <ul>{data.todos.map(t => <li key={t.id}>{t.title}</li>)}</ul>;
}
```

### Provider Setup

```tsx
// app/providers.tsx
'use client';
import { DarshanProvider, DarshJDB } from '@darshjdb/nextjs/provider';

const db = DarshJDB.init({ appId: 'my-app' });

export function Providers({ children }: { children: React.ReactNode }) {
  return <DarshanProvider db={db}>{children}</DarshanProvider>;
}
```

```tsx
// app/layout.tsx
import { Providers } from './providers';

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html>
      <body>
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
```

### Server Actions

```tsx
// app/actions.ts
'use server';
import { mutateServer } from '@darshjdb/nextjs/server';

export async function createTodo(title: string) {
  return mutateServer('createTodo', { title, listId: 'default' });
}

export async function toggleTodo(id: string, done: boolean) {
  return mutateServer('updateTodo', { id, done });
}
```

```tsx
// app/components/AddTodo.tsx
'use client';
import { createTodo } from '../actions';

export function AddTodo() {
  return (
    <form action={async (formData) => {
      await createTodo(formData.get('title') as string);
    }}>
      <input name="title" required />
      <button type="submit">Add</button>
    </form>
  );
}
```

### Auth Middleware

```typescript
// middleware.ts
import { ddbMiddleware } from '@darshjdb/nextjs/middleware';

export default ddbMiddleware({
  publicRoutes: ['/', '/about', '/sign-in', '/sign-up'],
  signInUrl: '/sign-in',
});

export const config = {
  matcher: ['/((?!_next/static|_next/image|favicon.ico).*)'],
};
```

### Server-Side Auth

```tsx
// app/dashboard/page.tsx
import { getAuth } from '@darshjdb/nextjs/server';
import { redirect } from 'next/navigation';

export default async function Dashboard() {
  const auth = await getAuth();
  if (!auth.userId) redirect('/sign-in');

  const data = await queryServer({ todos: { $where: { userId: auth.userId } } });
  return <TodoList items={data.todos} />;
}
```

## Pages Router

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

```tsx
// pages/index.tsx
import { DarshJDB } from '@darshjdb/nextjs/pages';

const db = DarshJDB.init({ appId: 'my-app' });

export default function Home() {
  const { data, isLoading } = db.useQuery({ todos: {} });
  if (isLoading) return <p>Loading...</p>;
  return <ul>{data.todos.map(t => <li key={t.id}>{t.title}</li>)}</ul>;
}
```

## API Routes

```typescript
// app/api/todos/route.ts
import { createServerAction } from '@darshjdb/nextjs/api';

export const POST = createServerAction(async (ctx, body) => {
  const id = ctx.db.id();
  await ctx.db.transact(
    ctx.db.tx.todos[id].set({ title: body.title, done: false, userId: ctx.auth.userId })
  );
  return { id };
});
```

## Features

- **Server Components** -- Query data on the server with zero client JavaScript
- **App Router + Pages Router** -- Works with both Next.js routing models
- **Server Actions** -- Call server functions from client components
- **Auth Middleware** -- Protect routes at the edge
- **Streaming** -- Compatible with React Suspense and streaming SSR
- **ISR/SSG compatible** -- Use `queryServer` in `generateStaticParams`

## Building

```bash
npm run build      # Produces dist/ with ESM, CJS, and type declarations
npm run dev        # Watch mode
npm test           # Run tests
npm run typecheck  # Type check
```

## Documentation

- [Getting Started](../../docs/getting-started.md)
- [Server Functions](../../docs/server-functions.md)
- [Authentication](../../docs/authentication.md)
- [Query Language](../../docs/query-language.md)
