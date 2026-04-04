# Server Functions

Server functions are TypeScript files in `darshan/functions/` that run on the server with full database access.

## Function Types

### Query — Read-only, cacheable, reactive

```typescript
// darshan/functions/getTodos.ts
import { query, v } from '@darshan/server';

export const getTodos = query({
  args: { listId: v.id(), status: v.optional(v.string()) },
  handler: async (ctx, { listId, status }) => {
    const filter: any = { listId };
    if (status) filter.status = status;
    return ctx.db.query({ todos: { $where: filter } });
  },
});
```

### Mutation — Transactional ACID writes

```typescript
import { mutation, v } from '@darshan/server';

export const createTodo = mutation({
  args: {
    title: v.string().min(1).max(500),
    listId: v.id(),
  },
  handler: async (ctx, { title, listId }) => {
    const todoId = ctx.db.id();
    await ctx.db.transact([
      ctx.db.tx.todos[todoId].set({
        title,
        done: false,
        listId,
        userId: ctx.auth.userId,
        createdAt: Date.now(),
      }),
    ]);
    return todoId;
  },
});
```

### Action — Side effects (HTTP, email, webhooks)

```typescript
import { action, v } from '@darshan/server';

export const sendWelcomeEmail = action({
  args: { userId: v.id() },
  handler: async (ctx, { userId }) => {
    const user = await ctx.db.query({ users: { $where: { id: userId } } });
    await ctx.fetch('https://api.sendgrid.com/v3/mail/send', {
      method: 'POST',
      headers: { Authorization: `Bearer ${process.env.SENDGRID_KEY}` },
      body: JSON.stringify({ to: user.email, subject: 'Welcome!' }),
    });
  },
});
```

### Scheduled — Cron jobs

```typescript
import { scheduled } from '@darshan/server';

export const dailyCleanup = scheduled({
  cron: '0 3 * * *', // 3 AM daily
  handler: async (ctx) => {
    const stale = await ctx.db.query({
      sessions: { $where: { lastUsedAt: { $lt: Date.now() - 30 * 86400000 } } },
    });
    for (const session of stale.sessions) {
      await ctx.db.transact(ctx.db.tx.sessions[session.id].delete());
    }
  },
});
```

### Internal — Server-to-server only

```typescript
import { internal } from '@darshan/server';

export const computeAnalytics = internal({
  handler: async (ctx) => {
    // Only callable from other server functions, never from clients
  },
});
```

## Argument Validation

```typescript
v.string()              // string
v.string().min(1)       // non-empty string
v.string().max(500)     // max 500 chars
v.number()              // number
v.number().min(0)       // non-negative
v.boolean()             // boolean
v.id()                  // UUID entity reference
v.array(v.string())     // array of strings
v.object({ key: v.string() }) // object with known shape
v.optional(v.string())  // optional field
v.union(v.string(), v.number()) // string or number
```

## Calling Functions from Client

```typescript
// React
const result = await db.fn('createTodo', { title: 'Buy milk', listId: 'list-1' });

// PHP
$result = $db->fn('createTodo', ['title' => 'Buy milk', 'listId' => 'list-1']);

// cURL
curl -X POST http://localhost:7700/api/fn/createTodo \
  -H "Authorization: Bearer TOKEN" \
  -d '{"title": "Buy milk", "listId": "list-1"}'
```

## Resource Limits

| Resource | Default | Configurable |
|----------|---------|-------------|
| CPU time | 30 seconds | Per function |
| Memory | 128 MB | Per function |
| Network | Allowlist only | Global config |
