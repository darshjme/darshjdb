# DarshanQL Query Language Reference

DarshanQL is a declarative, relational query language designed for client-side use. Every query is automatically a live subscription — when data changes, your app updates instantly.

## Basic Query

```typescript
const { data } = db.useQuery({
  todos: {}
});
// data.todos → all todos
```

## Filtering with $where

```typescript
// Equality
{ todos: { $where: { done: false } } }

// Comparison operators
{ todos: { $where: { priority: { $gt: 3 } } } }
{ todos: { $where: { priority: { $gte: 3 } } } }
{ todos: { $where: { priority: { $lt: 3 } } } }
{ todos: { $where: { priority: { $lte: 3 } } } }
{ todos: { $where: { priority: { $ne: 0 } } } }

// Set operators
{ todos: { $where: { status: { $in: ['active', 'pending'] } } } }
{ todos: { $where: { status: { $nin: ['archived'] } } } }

// String operators
{ todos: { $where: { title: { $contains: 'buy' } } } }
{ todos: { $where: { title: { $startsWith: 'Important' } } } }
```

## Sorting with $order

```typescript
{ todos: { $order: { createdAt: 'desc' } } }
{ todos: { $order: { priority: 'desc', createdAt: 'asc' } } }
```

## Pagination

```typescript
{ todos: { $limit: 20, $offset: 40 } }
```

## Full-Text Search

```typescript
{ articles: { $search: 'machine learning tutorial' } }
```

## Semantic / Vector Search

```typescript
{ articles: { $semantic: { field: 'embedding', query: 'things about cats', limit: 10 } } }
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
```

## Mutations

```typescript
// Create
db.transact(db.tx.todos[db.id()].set({
  title: 'Buy groceries',
  done: false,
  createdAt: Date.now()
}));

// Update (merge)
db.transact(db.tx.todos[existingId].merge({ done: true }));

// Delete
db.transact(db.tx.todos[existingId].delete());

// Link relations
db.transact(db.tx.users[userId].link({ todos: todoId }));

// Unlink relations
db.transact(db.tx.users[userId].unlink({ todos: todoId }));

// Multiple operations in one transaction
db.transact([
  db.tx.todos[id1].merge({ done: true }),
  db.tx.todos[id2].delete(),
  db.tx.users[userId].unlink({ todos: id2 })
]);
```
