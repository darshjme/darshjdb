# Migrating from Convex to DarshJDB

This guide walks you through migrating an existing Convex application to DarshJDB, covering data model mapping, code translation, and the automated migration tooling.

## Why Migrate?

DarshJDB gives you the same real-time, reactive developer experience as Convex with:

- **Self-hosted** -- run on your own infrastructure with no vendor lock-in
- **Open source** -- inspect, modify, and contribute to the database engine
- **Triple store** -- flexible entity-attribute-value model with graph capabilities
- **Offline-first** -- built-in sync engine with optimistic updates

## Data Model Mapping

### Convex Documents to DarshJDB Entities

Convex stores data as documents in tables. DarshJDB stores data as entities with typed fields in collections. The mapping is straightforward:

| Convex Concept     | DarshJDB Equivalent         |
|--------------------|------------------------------|
| Table              | Collection (entity type)     |
| Document           | Entity                       |
| `_id`              | Entity `id`                  |
| `_creationTime`    | `:db/createdAt` attribute    |
| Document field     | Entity field                 |
| Index              | Index (declared in schema)   |
| `db.query()`       | `QueryBuilder` / DarshanQL   |
| `db.insert()`      | `transact()` with `.set()`   |
| `db.patch()`       | `transact()` with `.merge()` |
| `db.delete()`      | `transact()` with `.delete()`|
| `useQuery()`       | `useQuery()` (React SDK)     |
| `useMutation()`    | `useMutation()` (React SDK)  |

### Schema Definition

**Convex:**

```typescript
// convex/schema.ts
import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

export default defineSchema({
  todos: defineTable({
    title: v.string(),
    done: v.boolean(),
    userId: v.id("users"),
  }).index("by_user", ["userId"]),

  users: defineTable({
    name: v.string(),
    email: v.string(),
  }).index("by_email", ["email"]),
});
```

**DarshJDB:**

```typescript
// darshan/schema.ts
import { defineSchema, defineTable, v } from "@darshjdb/server";

export default defineSchema({
  todos: defineTable({
    title: v.string(),
    done: v.boolean(),
    userId: v.id(),
  }).index("by_user", ["userId"]),

  users: defineTable({
    name: v.string(),
    email: v.string(),
  }).index("by_email", ["email"]),
});
```

The schemas are nearly identical. The main difference is the import path and that `v.id()` in DarshJDB does not take a table name argument.

## Code Comparisons

### Querying Data

**Convex:**

```typescript
import { useQuery } from "convex/react";
import { api } from "../convex/_generated/api";

function TodoList() {
  const todos = useQuery(api.todos.list, { userId: "user-123" });
  return todos?.map(t => <li key={t._id}>{t.title}</li>);
}
```

**DarshJDB:**

```typescript
import { useQuery } from "@darshjdb/react";

function TodoList() {
  const { data } = useQuery("todos", {
    where: { userId: "user-123" },
    order: { createdAt: "desc" },
  });
  return data?.map(t => <li key={t.id}>{t.title}</li>);
}
```

Key differences:
- No codegen step -- query the collection name directly
- Filter and sort declared inline instead of in a separate server function
- Document `_id` becomes `id`

### Mutations

**Convex:**

```typescript
import { useMutation } from "convex/react";
import { api } from "../convex/_generated/api";

function AddTodo() {
  const createTodo = useMutation(api.todos.create);

  const handleAdd = async () => {
    await createTodo({ title: "Buy groceries", done: false });
  };
}
```

**DarshJDB:**

```typescript
import { useMutation } from "@darshjdb/react";

function AddTodo() {
  const createTodo = useMutation("todos");

  const handleAdd = async () => {
    await createTodo.set({ title: "Buy groceries", done: false });
  };
}
```

### Server Functions

**Convex:**

```typescript
// convex/todos.ts
import { query, mutation } from "./_generated/server";
import { v } from "convex/values";

export const list = query({
  args: { userId: v.id("users") },
  handler: async (ctx, args) => {
    return await ctx.db
      .query("todos")
      .withIndex("by_user", q => q.eq("userId", args.userId))
      .collect();
  },
});

export const create = mutation({
  args: { title: v.string(), done: v.boolean() },
  handler: async (ctx, args) => {
    return await ctx.db.insert("todos", args);
  },
});
```

**DarshJDB:**

```typescript
// darshan/functions/todos.ts
import { query, mutation } from "@darshjdb/server";
import { v } from "@darshjdb/server";

export const list = query({
  args: { userId: v.id() },
  handler: async (ctx, args) => {
    return await ctx.db.query("todos")
      .where("userId", "=", args.userId)
      .exec();
  },
});

export const create = mutation({
  args: { title: v.string(), done: v.boolean() },
  handler: async (ctx, args) => {
    const id = ctx.db.id();
    ctx.db.todos[id].set(args);
    return id;
  },
});
```

### Using the Compatibility Layer

If you want to minimize code changes during migration, use the Convex compatibility wrapper:

```typescript
import { DarshJDB } from "@darshjdb/client";
import { ConvexCompat } from "@darshjdb/client/convex-compat";

const db = new DarshJDB({
  serverUrl: "http://localhost:7700",
  appId: "my-app",
});
const compat = new ConvexCompat(db);

// These look almost identical to Convex calls:
const todos = await compat.query("todos", { done: false });
const id = await compat.mutation("todos", { title: "New", done: false });
await compat.patch("todos", id, { done: true });
await compat.remove("todos", id);

// Live queries
const unsub = compat.watch("todos", { done: false }, (results) => {
  console.log("Updated:", results);
});
```

## Step-by-Step Migration Process

### 1. Export Your Convex Data

Use the Convex dashboard or CLI to export your data:

```bash
# From the Convex dashboard: Settings > Export Data > Download
# Or use the Convex CLI:
npx convex export --path ./convex-export
```

This produces a directory with one JSON file per table.

### 2. Start a DarshJDB Instance

```bash
# Docker (quickest)
docker compose up -d

# Or from source
cargo run --release
```

DarshJDB runs on `http://localhost:7700` by default.

### 3. Run the Migration Script

```bash
npx tsx scripts/migrate-from-convex.ts \
  --input  ./convex-export          \
  --url    http://localhost:7700    \
  --token  YOUR_ACCESS_TOKEN
```

The script:
1. Reads each JSON file from the export directory
2. Converts each document to a DarshJDB entity (preserving `_id` values)
3. Writes to DarshJDB in batches of 100 via `POST /api/mutate`
4. Reports progress per table

**Dry run first** to verify the mapping:

```bash
npx tsx scripts/migrate-from-convex.ts \
  --input  ./convex-export          \
  --dry-run
```

### 4. Update Your Schema

Copy your Convex schema and adjust imports:

```diff
- import { defineSchema, defineTable } from "convex/server";
- import { v } from "convex/values";
+ import { defineSchema, defineTable, v } from "@darshjdb/server";

  export default defineSchema({
    todos: defineTable({
      title: v.string(),
      done: v.boolean(),
-     userId: v.id("users"),
+     userId: v.id(),
    }).index("by_user", ["userId"]),
  });
```

### 5. Update Client Code

Install the DarshJDB SDKs:

```bash
npm install @darshjdb/client @darshjdb/react
npm uninstall convex
```

Replace imports and update queries:

```diff
- import { ConvexProvider, useQuery, useMutation } from "convex/react";
- import { api } from "../convex/_generated/api";
+ import { DarshanProvider, useQuery, useMutation } from "@darshjdb/react";
```

### 6. Update Server Functions

Move your Convex functions from `convex/` to `darshan/functions/` and update the API:

```diff
- import { query, mutation } from "./_generated/server";
+ import { query, mutation } from "@darshjdb/server";
```

### 7. Verify the Migration

Use the DarshJDB export script to verify data integrity:

```bash
npx tsx scripts/export-darshjdb.ts \
  --url    http://localhost:7700    \
  --output ./ddb-verify         \
  --tables todos,users
```

Compare the exported data against your original Convex export to confirm all documents migrated correctly.

### 8. Switch Your Application

Update your app's provider:

```diff
- <ConvexProvider client={convex}>
+ <DarshanProvider url="http://localhost:7700" appId="my-app">
    <App />
- </ConvexProvider>
+ </DarshanProvider>
```

## Common Gotchas

### 1. Document IDs

Convex uses opaque `Id<"tableName">` types. DarshJDB uses UUID v7 strings. The migration script preserves your original Convex `_id` values, so foreign key references remain valid.

### 2. System Fields

Convex's `_creationTime` is mapped to `:db/createdAt`. If your code reads `doc._creationTime`, update it to read the standard DarshJDB timestamp field.

### 3. Validators

Convex's `v.id("tableName")` becomes `v.id()` in DarshJDB. Table-scoped ID validation is handled by the schema layer automatically.

### 4. File Storage

Convex's built-in file storage maps to DarshJDB's Storage module. Upload URLs and patterns are similar but not identical. See the [Storage docs](storage.md) for the DarshJDB API.

### 5. Scheduled Functions

Convex's `ctx.scheduler` maps to DarshJDB's cron/scheduled function support. See the [Server Functions docs](server-functions.md) for configuration.

---

[Previous: Migration Guide](migration.md) | [All Docs](README.md)
