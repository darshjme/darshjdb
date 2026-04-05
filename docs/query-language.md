# DarshanQL Query Language Reference

DarshanQL is a declarative, relational query language designed for client-side use. Every query is automatically a live subscription -- when data changes, your app updates instantly.

## Query Execution Pipeline

```mermaid
flowchart LR
    A[DarshanQL Object] --> B[Parse & Validate]
    B --> C[Permission Injection]
    C --> D[SQL Generation]
    D --> E[Complexity Check]
    E --> F[PostgreSQL Execution]
    F --> G[Field Filtering]
    G --> H[MsgPack Serialization]
    H --> I[Client Response]
```

## Basic Query

```typescript
const { data } = db.useQuery({
  todos: {}
});
// data.todos -> all todos (subject to permission rules)
```

## Filtering with $where

### Equality

```typescript
{ todos: { $where: { done: false } } }
{ todos: { $where: { status: 'active', priority: 3 } } }  // AND logic
```

### Comparison Operators

```typescript
{ todos: { $where: { priority: { $gt: 3 } } } }     // greater than
{ todos: { $where: { priority: { $gte: 3 } } } }    // greater than or equal
{ todos: { $where: { priority: { $lt: 3 } } } }     // less than
{ todos: { $where: { priority: { $lte: 3 } } } }    // less than or equal
{ todos: { $where: { priority: { $ne: 0 } } } }     // not equal
```

### Set Operators

```typescript
{ todos: { $where: { status: { $in: ['active', 'pending'] } } } }
{ todos: { $where: { status: { $nin: ['archived', 'deleted'] } } } }
```

### String Operators

```typescript
{ todos: { $where: { title: { $contains: 'buy' } } } }         // case-insensitive
{ todos: { $where: { title: { $startsWith: 'Important' } } } }  // prefix match
{ todos: { $where: { title: { $endsWith: '.md' } } } }          // suffix match
```

### Logical Operators

```typescript
// OR: match either condition
{
  todos: {
    $where: {
      $or: [
        { priority: { $gte: 5 } },
        { status: 'urgent' }
      ]
    }
  }
}

// AND (explicit): all conditions must match
{
  todos: {
    $where: {
      $and: [
        { priority: { $gte: 3 } },
        { done: false },
        { status: { $ne: 'archived' } }
      ]
    }
  }
}

// NOT: negate a condition
{
  todos: {
    $where: {
      $not: { status: 'archived' }
    }
  }
}
```

### Null Checks

```typescript
// Find entities where a field exists
{ todos: { $where: { assignedTo: { $ne: null } } } }

// Find entities where a field is null/missing
{ todos: { $where: { assignedTo: null } } }
```

## Sorting with $order

```typescript
// Single field
{ todos: { $order: { createdAt: 'desc' } } }

// Multiple fields (applied in order)
{ todos: { $order: { priority: 'desc', createdAt: 'asc' } } }

// Sort by nested relation attribute
{ todos: { $order: { 'owner.name': 'asc' } } }
```

## Pagination

### Offset-based Pagination

```typescript
// Page 1 (first 20 items)
{ todos: { $limit: 20, $offset: 0 } }

// Page 2
{ todos: { $limit: 20, $offset: 20 } }

// Page 3
{ todos: { $limit: 20, $offset: 40 } }
```

### Cursor-based Pagination

For large datasets, cursor-based pagination is more efficient and avoids skipping rows when data changes:

```typescript
// First page
const { data, cursor } = db.useQuery({
  todos: { $limit: 20, $order: { createdAt: 'desc' } }
});

// Next page (using the cursor from the previous response)
const { data: page2 } = db.useQuery({
  todos: { $limit: 20, $after: cursor, $order: { createdAt: 'desc' } }
});
```

### Infinite Scroll Pattern

```typescript
function InfiniteList() {
  const [cursor, setCursor] = useState(null);
  const { data, hasMore } = db.useQuery({
    todos: {
      $limit: 20,
      $after: cursor,
      $order: { createdAt: 'desc' },
    },
  });

  const loadMore = () => {
    if (data?.todos.length > 0) {
      setCursor(data.todos[data.todos.length - 1].id);
    }
  };

  return (
    <div>
      {data?.todos.map(todo => <TodoItem key={todo.id} todo={todo} />)}
      {hasMore && <button onClick={loadMore}>Load More</button>}
    </div>
  );
}
```

## Full-Text Search

Uses PostgreSQL's built-in tsvector full-text search:

```typescript
{ articles: { $search: 'machine learning tutorial' } }

// Combined with filters
{
  articles: {
    $search: 'machine learning',
    $where: { published: true },
    $order: { $relevance: 'desc' },
    $limit: 20
  }
}
```

## Semantic / Vector Search

Uses pgvector for embedding-based similarity search:

```typescript
// Find semantically similar content
{ articles: { $semantic: { field: 'embedding', query: 'things about cats', limit: 10 } } }

// Hybrid search: combine vector similarity with filters
{
  articles: {
    $semantic: { field: 'embedding', query: 'machine learning basics', limit: 50 },
    $where: { published: true, category: 'tutorials' },
    $limit: 10
  }
}
```

## Nested Relations

Follow references to load related entities in a single query:

```typescript
// Load todos with their owners
{ todos: { owner: {} } }

// Load users with their todos and each todo's tags
{ users: { todos: { tags: {} } } }

// Filter nested relations
{ users: { todos: { $where: { done: false }, $limit: 5 } } }

// Deep nesting (up to DDB_MAX_QUERY_DEPTH levels)
{
  organizations: {
    teams: {
      members: {
        profile: {},
        todos: {
          $where: { done: false },
          tags: {}
        }
      }
    }
  }
}
```

### Reverse Relations

Query entities that reference the current entity:

```typescript
// Find all todos that reference this user (reverse link)
{
  users: {
    $where: { id: userId },
    _todos: {}   // underscore prefix = reverse relation
  }
}
```

## Mutations

### Create

```typescript
db.transact(db.tx.todos[db.id()].set({
  title: 'Buy groceries',
  done: false,
  createdAt: Date.now()
}));
```

### Update (Merge)

```typescript
// Partial update -- only changes specified fields
db.transact(db.tx.todos[existingId].merge({ done: true }));
```

### Delete

```typescript
db.transact(db.tx.todos[existingId].delete());
```

### Link Relations

```typescript
db.transact(db.tx.users[userId].link({ todos: todoId }));
```

### Unlink Relations

```typescript
db.transact(db.tx.users[userId].unlink({ todos: todoId }));
```

### Batch Transactions

Multiple operations execute atomically -- all succeed or all fail:

```typescript
db.transact([
  // Create a new todo
  db.tx.todos[newId].set({ title: 'New task', done: false }),

  // Mark another as done
  db.tx.todos[id1].merge({ done: true }),

  // Delete a third
  db.tx.todos[id2].delete(),

  // Update the user's link
  db.tx.users[userId].link({ todos: newId }),
  db.tx.users[userId].unlink({ todos: id2 }),
]);
```

### Optimistic Updates

Mutations are applied to the local cache immediately and synced to the server in the background:

```typescript
// This updates the UI instantly, then reconciles with the server
db.transact(db.tx.todos[id].merge({ done: true }));

// If the server rejects (e.g., permission denied), the local change is rolled back
```

## Aggregations

```typescript
// Count all todos
{ todos: { $aggregate: 'count' } }
// Returns: { todos: 42 }

// Count with filter
{ todos: { $where: { done: true }, $aggregate: 'count' } }
// Returns: { todos: 18 }

// Sum
{ orders: { $aggregate: { $sum: 'amount' } } }
// Returns: { orders: 15420.50 }

// Average
{ products: { $aggregate: { $avg: 'price' } } }
// Returns: { products: 29.99 }

// Min / Max
{ products: { $aggregate: { $min: 'price' } } }
{ products: { $aggregate: { $max: 'price' } } }

// Multiple aggregations
{
  orders: {
    $where: { status: 'completed' },
    $aggregate: {
      $count: true,
      $sum: 'amount',
      $avg: 'amount',
      $min: 'amount',
      $max: 'amount'
    }
  }
}
// Returns: { orders: { count: 100, sum: 15420.50, avg: 154.20, min: 5.00, max: 999.99 } }

// Group by
{
  orders: {
    $groupBy: 'status',
    $aggregate: { $count: true, $sum: 'amount' }
  }
}
// Returns: { orders: { pending: { count: 10, sum: 1500 }, completed: { count: 90, sum: 13920.50 } } }
```

## Combining Clauses

All query clauses can be combined in a single query:

```typescript
{
  todos: {
    $where: { done: false, priority: { $gte: 3 } },
    $order: { priority: 'desc', createdAt: 'asc' },
    $limit: 10,
    $offset: 0,
    owner: {}  // include related user
  }
}
```

## Multi-Entity Queries

Fetch multiple entity types in a single round trip:

```typescript
const { data } = db.useQuery({
  todos: {
    $where: { done: false },
    $limit: 10,
    owner: {}
  },
  users: {
    $where: { role: 'admin' }
  },
  settings: {}
});

// data.todos, data.users, data.settings -- all populated from one query
```

## Query Complexity Limits

To prevent expensive queries from degrading performance, DarshJDB enforces limits:

| Limit | Default | Config Variable |
|-------|---------|-----------------|
| Max query depth | 12 | `DDB_MAX_QUERY_DEPTH` |
| Max results per query | 10,000 | `DDB_MAX_QUERY_RESULTS` |
| Max entities per transaction | 1,000 | `DDB_MAX_TX_OPS` |

Queries that exceed these limits receive a `400 Bad Request` response with an explanation of which limit was exceeded.

---

[Previous: Self-Hosting](self-hosting.md) | [Next: Server Functions](server-functions.md) | [All Docs](README.md)
