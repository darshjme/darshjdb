# DarshJDB + Next.js App Router

A minimal Next.js 14 application demonstrating DarshJDB with the App Router. Server Components fetch initial data for fast page loads and SEO, while Client Components subscribe to real-time updates.

## What it demonstrates

- **Server Component data fetching** via `queryServer` with ISR caching
- **Client-side real-time queries** via `useQuery` that pick up where the server left off
- **Server Actions** for secure server-side mutations with `mutateServer`
- **SSR hydration** -- no loading spinner on first paint
- **Cache revalidation** via Next.js `revalidateTag`

## Prerequisites

- Node.js 18+
- A running DarshJDB server (default: `http://localhost:7700`)

## Setup

```bash
# From the repository root
npm install

# Configure environment
cd examples/nextjs-app
cp .env.local.example .env.local
# Edit .env.local with your DarshJDB URL and admin token

# Start the dev server
npm run dev
```

The app runs at [http://localhost:3003](http://localhost:3003).

## Project structure

```
nextjs-app/
  next.config.mjs        Next.js config with workspace transpilation
  .env.local.example      Environment variable template
  app/
    layout.tsx            Root layout with DarshanProvider
    page.tsx              Server Component -- fetches posts at request time
    actions/
      posts.ts            Server Actions for creating and deleting posts
    components/
      providers.tsx       Client-side DarshanProvider wrapper
      post-list.tsx       Client Component with real-time query + mutation form
```

## Key patterns

### Server Component data fetching

```tsx
// app/page.tsx (runs on the server)
import { queryServer } from "@darshjdb/nextjs/server";

const posts = await queryServer({
  collection: "posts",
  orderBy: { createdAt: "desc" },
  limit: 20,
}, { revalidate: 10, tags: ["posts"] });
```

### Client-side real-time

```tsx
// app/components/post-list.tsx ("use client")
import { useQuery, useMutation } from "@darshjdb/react";

const { data } = useQuery({
  collection: "posts",
  orderBy: [{ field: "createdAt", direction: "desc" }],
  limit: 20,
});
```

### Server Action

```tsx
// app/actions/posts.ts ("use server")
import { mutateServer } from "@darshjdb/nextjs/server";
import { revalidateTag } from "next/cache";

export async function createPost(title: string, body: string) {
  await mutateServer(async (db) => {
    return db.collection("posts").insert({ title, body, createdAt: Date.now() });
  });
  revalidateTag("posts");
}
```

## Environment variables

| Variable | Where | Description |
|----------|-------|-------------|
| `DDB_URL` | Server only | DarshJDB server URL |
| `DDB_ADMIN_TOKEN` | Server only | Admin auth token (keep secret) |
| `NEXT_PUBLIC_DDB_URL` | Client + Server | Public URL for browser WebSocket |
