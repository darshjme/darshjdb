# Next.js SDK

The `@darshjdb/nextjs` package integrates DarshJDB with the Next.js App Router, providing server-side query functions for Server Components, mutation helpers for Server Actions, a client-side provider with SSR hydration, and Edge Middleware for route protection.

## Installation

```bash
npm install @darshjdb/nextjs @darshjdb/react @darshjdb/client
```

## Environment Variables

| Variable               | Context          | Description                          |
|------------------------|------------------|--------------------------------------|
| `DDB_URL`              | Server-side only | DarshJDB server URL                  |
| `DDB_ADMIN_TOKEN`      | Server-side only | Admin token for server queries       |
| `NEXT_PUBLIC_DDB_URL`  | Client-side      | Public server URL for the browser    |
| `NEXT_PUBLIC_DDB_TOKEN`| Client-side      | Client authentication token          |

## Server Components

Use `queryServer` to fetch data directly in Server Components via the REST API. No WebSocket connection is opened.

```tsx
// app/page.tsx
import { queryServer } from '@darshjdb/nextjs/server';

export default async function Page() {
  const data = await queryServer({
    posts: { $where: { published: true }, $limit: 20 },
  });

  return <PostList items={data.posts} />;
}
```

### Caching and Revalidation

`queryServer` integrates with the Next.js `fetch` cache. Control behavior with the options parameter:

```tsx
// ISR: revalidate every 60 seconds
const data = await queryServer(
  { posts: { $where: { published: true } } },
  { revalidate: 60 },
);

// On-demand revalidation with tags
const data = await queryServer(
  { posts: { $where: { published: true } } },
  { revalidate: 60, tags: ['posts'] },
);

// No caching (default)
const data = await queryServer(
  { users: {} },
  { revalidate: false },
);
```

## Server Actions

Use `mutateServer` for writes from Server Actions. Supports `set`, `merge`, and `delete` operations.

```tsx
// app/actions.ts
'use server';
import { mutateServer } from '@darshjdb/nextjs/server';

export async function createTodo(title: string) {
  return mutateServer([
    { entity: 'todos', op: 'set', data: { title, done: false } },
  ]);
}

export async function deleteTodo(id: string) {
  return mutateServer([
    { entity: 'todos', id, op: 'delete' },
  ]);
}
```

### Server Functions

Call registered server-side functions from Server Actions:

```tsx
'use server';
import { callFunction } from '@darshjdb/nextjs/server';

export async function generateReport(params: { month: number; year: number }) {
  return callFunction('generateMonthlyReport', params);
}
```

### Admin Client

For advanced use cases, get a raw DarshJDB client configured from environment variables:

```ts
import { getAdminDb } from '@darshjdb/nextjs/server';

const db = getAdminDb();
```

## Client Provider (App Router)

The Next.js provider wraps `@darshjdb/react` with automatic environment variable configuration and SSR hydration support. Place it in your root layout.

```tsx
// app/layout.tsx
import { DarshanProvider } from '@darshjdb/nextjs/provider';

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html>
      <body>
        <DarshanProvider>
          {children}
        </DarshanProvider>
      </body>
    </html>
  );
}
```

### Provider Props

| Prop             | Type              | Default                       | Description                          |
|------------------|-------------------|-------------------------------|--------------------------------------|
| `url`            | `string?`         | `NEXT_PUBLIC_DDB_URL`         | Server URL                           |
| `token`          | `string?`         | `NEXT_PUBLIC_DDB_TOKEN`       | Client auth token                    |
| `clientConfig`   | `object?`         | `{}`                          | Additional client options            |
| `dehydratedState`| `DehydratedState?`| --                            | Server-fetched data for hydration    |
| `realtime`       | `boolean`         | `true`                        | Enable live subscriptions            |
| `offline`        | `boolean`         | `false`                       | Enable offline persistence           |

### SSR Hydration

Fetch data on the server and hydrate the client cache so the first render is instant with no loading spinners.

```tsx
// app/layout.tsx
import { DarshanProvider, dehydrate } from '@darshjdb/nextjs/provider';
import { queryServer } from '@darshjdb/nextjs/server';

export default async function Layout({ children }: { children: React.ReactNode }) {
  const users = await queryServer({ collection: 'users' });
  const config = await queryServer({ collection: 'config' });

  const dehydratedState = dehydrate({
    users: { data: users },
    config: { data: config },
  });

  return (
    <DarshanProvider dehydratedState={dehydratedState}>
      {children}
    </DarshanProvider>
  );
}
```

The `dehydrate()` function accepts a map of cache keys to `{ data, fetchedAt? }` objects. Each entry is timestamped for freshness tracking.

## Route Protection (Middleware)

Use `darshanMiddleware` to protect routes using cookie-based session authentication at the Edge.

```ts
// middleware.ts (project root)
import { darshanMiddleware } from '@darshjdb/nextjs/middleware';

export default darshanMiddleware({
  protectedRoutes: ['/dashboard', '/api/private', '/admin'],
  loginRoute: '/auth/login',
  publicRoutes: ['/dashboard/public-preview'],
});

export const config = {
  matcher: ['/((?!_next/static|_next/image|favicon.ico).*)'],
};
```

### Middleware Config

| Option             | Type                                          | Default             | Description                            |
|--------------------|-----------------------------------------------|---------------------|----------------------------------------|
| `protectedRoutes`  | `string[]`                                    | (required)          | Route prefixes requiring auth          |
| `loginRoute`       | `string`                                      | `'/login'`          | Redirect target for unauthenticated    |
| `publicRoutes`     | `string[]`                                    | `[]`                | Routes exempt from protection          |
| `validateSession`  | `(token: string) => Promise<boolean>`         | presence check only | Custom server-side token validation    |
| `onAuthenticated`  | `(req, res, token) => NextResponse \| void`    | --                  | Post-auth hook (inject headers, etc.)  |
| `cookieName`       | `string`                                      | `'darshan_session'` | Session cookie name                    |

### Session Validation

By default, the middleware only checks for cookie presence. For server-side validation:

```ts
export default darshanMiddleware({
  protectedRoutes: ['/dashboard'],
  validateSession: async (token) => {
    const res = await fetch(`${process.env.DDB_URL}/auth/validate`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    return res.ok;
  },
});
```

Invalid sessions have their cookie cleared before redirect.

### Cookie Utilities

Set and clear session cookies in API routes or Server Actions:

```ts
import { setSessionCookie, clearSessionCookie } from '@darshjdb/nextjs/middleware';
import { NextResponse } from 'next/server';

// After sign-in:
const response = NextResponse.json({ ok: true });
setSessionCookie(response, sessionToken, {
  maxAge: 86400,    // 1 day (default: 7 days)
  sameSite: 'lax',  // 'strict' | 'lax' | 'none'
});

// After sign-out:
clearSessionCookie(response);
```

The `x-ddb-session` header is injected into authenticated requests so that Server Components and API routes can access the session token without re-reading the cookie.

## Pages Router

The Next.js SDK targets the App Router. For Pages Router projects, use `@darshjdb/react` directly:

```tsx
// pages/_app.tsx
import { DarshanProvider } from '@darshjdb/react';

export default function App({ Component, pageProps }) {
  return (
    <DarshanProvider serverUrl={process.env.NEXT_PUBLIC_DDB_URL} appId="my-app">
      <Component {...pageProps} />
    </DarshanProvider>
  );
}
```

For `getServerSideProps`, use direct REST calls with the admin token -- `queryServer` relies on App Router internals.
