# Permissions

DarshanDB uses a **zero-trust** model: everything is denied unless explicitly allowed.

## Permission DSL

Define permissions in `darshan/permissions.ts`:

```typescript
export default {
  todos: {
    // Only the owner can read their todos
    read: (ctx) => ({ userId: ctx.auth.userId }),

    // Any authenticated user can create
    create: (ctx) => !!ctx.auth,

    // Only the owner can update
    update: (ctx) => ({ userId: ctx.auth.userId }),

    // Only admins can delete
    delete: (ctx) => ctx.auth.role === 'admin',
  },

  users: {
    // Anyone can read users, but email is restricted
    read: {
      allow: true,
      fields: {
        email: (ctx, entity) => entity.id === ctx.auth.userId,
        passwordHash: false, // never exposed
      },
    },
  },
};
```

## How It Works

### Row-Level Security

For `read` operations, the permission function returns a **filter object** that becomes a SQL `WHERE` clause:

```typescript
read: (ctx) => ({ userId: ctx.auth.userId })
// Becomes: WHERE user_id = 'current-user-id'
```

Unauthorized data never leaves the database. It's not fetched and then filtered — it's invisible at the query level.

### Field-Level Permissions

```typescript
fields: {
  email: (ctx, entity) => entity.id === ctx.auth.userId,  // only own email visible
  passwordHash: false,                                      // never returned
  publicName: true,                                         // always returned
}
```

### Role Hierarchy

```typescript
// Built-in roles: admin > editor > viewer
delete: (ctx) => ctx.auth.role === 'admin'
update: (ctx) => ['admin', 'editor'].includes(ctx.auth.role)
read: (ctx) => !!ctx.auth  // any authenticated user
```

## Permission Rules

| Rule Type | Example | Behavior |
|-----------|---------|----------|
| Boolean | `true` / `false` | Allow or deny all |
| Auth check | `(ctx) => !!ctx.auth` | Require authentication |
| Filter object | `(ctx) => ({ userId: ctx.auth.userId })` | Row-level filtering |
| Role check | `(ctx) => ctx.auth.role === 'admin'` | Role-based access |
| Field restriction | `fields: { email: false }` | Hide specific fields |

## Testing Permissions

```typescript
// In the admin dashboard, use "Impersonate" to test as any user
// Or via the admin SDK:
const adminDb = DarshanDB.admin({ adminToken: '...' });
const asUser = adminDb.asUser('user@example.com');
const data = await asUser.query({ todos: {} });
// Returns only what that user would see
```
