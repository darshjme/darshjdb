# DarshJDB Todo App

A minimal React + TypeScript todo app powered by DarshJDB real-time queries and mutations.

## What it demonstrates

- **Live queries** that update automatically when data changes
- **Mutations** for creating, toggling, and deleting records
- **DarshanProvider** setup with client initialization
- Optimistic UI updates via the `@darshjdb/react` SDK

## Prerequisites

- Node.js 18+
- A running DarshJDB server (default: `http://localhost:7700`)

## Setup

```bash
# From the repository root
npm install

# Start the dev server
cd examples/todo-app
npm run dev
```

The app runs at [http://localhost:3001](http://localhost:3001).

## Project structure

```
todo-app/
  index.html          Entry HTML shell
  package.json        Dependencies and scripts
  tsconfig.json       TypeScript config
  vite.config.ts      Vite dev server config
  src/
    main.tsx          App bootstrap and DarshJDB client init
    App.tsx           Todo list UI with live query and mutations
```

## Key code

### Initializing the client

```tsx
import { DarshanProvider } from "@darshjdb/react";

<DarshanProvider serverUrl="http://localhost:7700" appId="todo-example">
  <App />
</DarshanProvider>
```

### Live query

```tsx
const { data: todos, isLoading } = useQuery({
  collection: "todos",
  orderBy: [{ field: "createdAt", direction: "desc" }],
});
```

### Mutations

```tsx
const { mutate } = useMutation();

await mutate({
  type: "insert",
  collection: "todos",
  data: { title: "Learn DarshJDB", done: false, createdAt: Date.now() },
});
```
