# DarshJQL Query Language -- Complete Reference

DarshJQL is a declarative, JSON-based query language purpose-built for DarshJDB's entity-attribute-value triple store. It compiles to optimized SQL plans that join across the `triples` table, supports full-text and vector search, resolves nested entity references inline, and powers real-time subscriptions with zero additional syntax. Every query you write is also a live query.

**Design philosophy.** SQL assumes fixed-schema tables. GraphQL assumes a typed schema and a resolver tree. DarshJQL assumes neither. It operates over a schema-on-read triple store where entities are bags of attribute-value pairs. The syntax is inspired by Datomic's pull API (declarative attribute selection) and GraphQL (nested resolution), but expressed as plain JSON so it can be sent from any language without a parser, a schema file, or a build step. You POST a JSON object, you get a JSON array back.

---

## Table of Contents

1. [Basic Queries](#1-basic-queries)
2. [Search](#2-search)
3. [Relations](#3-relations)
4. [Mutations](#4-mutations)
5. [Real-time Subscriptions](#5-real-time-subscriptions)
6. [Formulas](#6-formulas)
7. [Aggregation](#7-aggregation)
8. [Views](#8-views)
9. [Advanced](#9-advanced)

---

## 1. Basic Queries

All queries are sent as `POST /api/query` with a JSON body.

### 1.1 Fetch All Entities of a Type

The only required field is `type`, which specifies the entity type to query.

```json
{
  "type": "User"
}
```

```bash
curl -X POST http://localhost:4000/api/query \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"type": "User"}'
```

**Response:**

```json
[
  {
    "entity_id": "a1b2c3d4-...",
    "attributes": {
      "name": "Darsh",
      "email": "darsh@darshj.me",
      "role": "admin",
      "created_at": "2026-03-20T10:00:00Z"
    },
    "nested": {}
  }
]
```

### 1.2 Filtering with WHERE Clauses

The `$where` field accepts an array of predicates. All predicates are combined with AND logic.

```json
{
  "type": "User",
  "$where": [
    { "attribute": "role", "op": "Eq", "value": "admin" },
    { "attribute": "created_at", "op": "Gte", "value": "2026-01-01T00:00:00Z" }
  ]
}
```

#### Supported Operators

| Operator    | Meaning                        | SQL Equivalent     | Example Value              |
|-------------|--------------------------------|--------------------|----------------------------|
| `Eq`        | Exact equality                 | `=`                | `"admin"`                  |
| `Neq`       | Not equal                      | `!=`               | `"inactive"`               |
| `Gt`        | Greater than                   | `>`                | `100`                      |
| `Gte`       | Greater than or equal          | `>=`               | `"2026-01-01T00:00:00Z"`   |
| `Lt`        | Less than                      | `<`                | `50`                       |
| `Lte`       | Less than or equal             | `<=`               | `99.99`                    |
| `Contains`  | JSON containment               | `@>`               | `["tag1", "tag2"]`         |
| `Like`      | Case-insensitive pattern match | `ILIKE`            | `"darsh%"`                 |

**Contains** checks whether the stored JSON value contains the query value using PostgreSQL's `@>` operator. Use it for array membership or nested object matching.

**Like** uses PostgreSQL's `ILIKE` for case-insensitive prefix or pattern matching. The `%` wildcard matches any sequence of characters.

```json
{
  "type": "Post",
  "$where": [
    { "attribute": "title", "op": "Like", "value": "darsh%" },
    { "attribute": "tags", "op": "Contains", "value": ["rust", "database"] }
  ]
}
```

### 1.3 Ordering

The `$order` field accepts an array of sort clauses. Each clause specifies an attribute and a direction (`Asc` or `Desc`). Multiple clauses are applied in sequence as tiebreakers.

```json
{
  "type": "Post",
  "$order": [
    { "attribute": "created_at", "direction": "Desc" },
    { "attribute": "title", "direction": "Asc" }
  ]
}
```

```bash
curl -X POST http://localhost:4000/api/query \
  -H "Content-Type: application/json" \
  -d '{
    "type": "Post",
    "$order": [{"attribute": "created_at", "direction": "Desc"}],
    "$limit": 20
  }'
```

### 1.4 Pagination

Use `$limit` and `$offset` for cursor-free pagination. Pagination is applied in Rust after grouping rows by entity_id, so limits count entities, not raw database rows.

```json
{
  "type": "Invoice",
  "$where": [{ "attribute": "status", "op": "Eq", "value": "paid" }],
  "$order": [{ "attribute": "amount", "direction": "Desc" }],
  "$limit": 25,
  "$offset": 50
}
```

Page 3 of 25 results per page. The engine fetches all matching entities from Postgres, groups them by `entity_id`, sorts, then slices the offset/limit window in Rust for correctness (since one entity may span multiple `triples` rows).

---

## 2. Search

DarshJDB supports three search modes. They can be combined with `$where` filters, ordering, and pagination.

### 2.1 Full-Text Search (`$search`)

Uses PostgreSQL's `tsvector`/`tsquery` with a GIN index for efficient ranked full-text matching.

```json
{
  "type": "Article",
  "$search": "rust async programming"
}
```

```bash
curl -X POST http://localhost:4000/api/query \
  -H "Content-Type: application/json" \
  -d '{"type": "Article", "$search": "rust async programming", "$limit": 10}'
```

The engine joins a `t_search` alias with `to_tsvector('english', value) @@ plainto_tsquery('english', $term)`. This means stemming, stop-word removal, and ranking happen server-side via PostgreSQL's full-text engine.

### 2.2 Semantic / Vector Search (`$semantic`)

Uses pgvector's cosine distance operator (`<=>`) against pre-computed embeddings stored in the `embeddings` table.

**Short form** (text query -- requires an embedding API to be configured):

```json
{
  "type": "Article",
  "$semantic": "how to build a database from scratch"
}
```

**Rich form** (pre-computed vector):

```json
{
  "type": "Article",
  "$semantic": {
    "vector": [0.023, -0.114, 0.882, "...768 floats..."],
    "limit": 5
  }
}
```

When a vector is supplied, results are ordered by ascending cosine distance (most similar first). Explicit `$order` clauses are appended as tiebreakers after the distance sort.

Default limit for semantic search is 10 results.

### 2.3 Hybrid Search (`$hybrid`)

Combines full-text and vector search using Reciprocal Rank Fusion (RRF). The engine executes two CTEs -- one for text ranking via `ts_rank_cd`, one for vector ranking via cosine distance -- then merges them with weighted RRF scores.

```json
{
  "type": "Article",
  "$hybrid": {
    "text": "database design patterns",
    "vector": [0.023, -0.114, 0.882, "..."],
    "text_weight": 0.3,
    "vector_weight": 0.7,
    "limit": 20
  }
}
```

```bash
curl -X POST http://localhost:4000/api/query \
  -H "Content-Type: application/json" \
  -d '{
    "type": "Article",
    "$hybrid": {
      "text": "database design patterns",
      "vector": [0.023, -0.114, 0.882],
      "text_weight": 0.3,
      "vector_weight": 0.7,
      "limit": 20
    }
  }'
```

**RRF scoring formula:** `score = (text_weight / (60 + text_rank)) + (vector_weight / (60 + vector_rank))`. The constant 60 is the standard RRF `k` parameter from the literature. The engine oversamples by 3x (`limit * 3`) for each CTE to ensure good fusion quality.

| Default Parameter   | Value |
|---------------------|-------|
| `text_weight`       | 0.3   |
| `vector_weight`     | 0.7   |
| `limit`             | 10    |

### 2.4 When to Use Which

| Use Case                                     | Search Mode  |
|----------------------------------------------|--------------|
| Keyword search, exact phrase matching         | `$search`    |
| Meaning-based similarity, "find things like X"| `$semantic`  |
| Both keyword precision and semantic recall    | `$hybrid`    |

---

## 3. Relations

### 3.1 Nested Queries (`$nested`)

The `$nested` clause resolves entity references inline. When an entity has an attribute whose value is a UUID pointing to another entity, `$nested` fetches that entity and embeds it in the result.

```json
{
  "type": "User",
  "$where": [{ "attribute": "role", "op": "Eq", "value": "admin" }],
  "$nested": [
    { "via_attribute": "org_id" }
  ]
}
```

**Response:**

```json
[
  {
    "entity_id": "a1b2c3d4-...",
    "attributes": {
      "name": "Darsh",
      "org_id": "f5e6d7c8-..."
    },
    "nested": {
      "org_id": {
        "name": "DarshJ Labs",
        "plan": "enterprise",
        "seats": 50
      }
    }
  }
]
```

### 3.2 Multi-Level Nesting

Nested queries support sub-queries for multi-level resolution, up to a maximum depth of 3 to prevent query explosion.

```json
{
  "type": "Todo",
  "$nested": [
    {
      "via_attribute": "owner_id",
      "sub_query": {
        "entity_type": "User",
        "nested": [
          { "via_attribute": "org_id" }
        ]
      }
    }
  ]
}
```

This resolves `Todo -> owner (User) -> org (Organization)` in a single query. Nested resolution uses batched fetching: all referenced UUIDs are collected and fetched in a single `WHERE entity_id = ANY($1::uuid[])` query per nesting level, turning the classic N+1 problem into 1+P queries where P is the number of nested plans (typically 1-3).

### 3.3 Graph Traversal (DarshQL)

For graph-style queries, DarshJDB also supports a SurrealDB-inspired SQL-like syntax called DarshQL. Graph traversals use arrow notation:

```
SELECT *, ->works_at->company.name AS employer FROM user:darsh
```

Inbound traversals use the back-arrow:

```
SELECT *, <-follows<-user AS followers FROM user:darsh
```

Multi-hop:

```
SELECT ->friends->friends AS fof FROM user:darsh
```

### 3.4 RELATE Statements

Create edges between entities:

```
RELATE user:darsh->works_at->company:darshjlabs SET role = "founder", since = "2024-01-01"
```

---

## 4. Mutations

### 4.1 Creating Entities

```bash
curl -X POST http://localhost:4000/api/data/User \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "name": "Alice",
    "email": "alice@example.com",
    "role": "viewer",
    "org_id": "f5e6d7c8-..."
  }'
```

**Response:**

```json
{
  "entity_id": "new-uuid-...",
  "attributes": {
    "name": "Alice",
    "email": "alice@example.com",
    "role": "viewer",
    "org_id": "f5e6d7c8-..."
  }
}
```

### 4.2 Updating Entities

Partial update -- only specified attributes are modified. Unmentioned attributes are left unchanged.

```bash
curl -X PATCH http://localhost:4000/api/data/User/a1b2c3d4-... \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "role": "editor",
    "updated_at": "2026-04-07T12:00:00Z"
  }'
```

In the triple store, updates are append-only: a new triple is written and the old triple is marked `retracted = true`. This gives you a full audit trail for free.

Supports `$ttl` in the PATCH body to set a time-to-live on the entity.

### 4.3 Deleting Entities

```bash
curl -X DELETE http://localhost:4000/api/data/User/a1b2c3d4-... \
  -H "Authorization: Bearer $TOKEN"
```

Deletion retracts all triples for the entity. The data remains in the `triples` table with `retracted = true` for audit purposes.

### 4.4 Batch Mutations (`POST /api/mutate`)

Submit multiple mutations in a single HTTP round-trip. All mutations share a single Postgres transaction for atomicity -- either all succeed or all roll back.

```bash
curl -X POST http://localhost:4000/api/mutate \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "mutations": [
      {
        "op": "insert",
        "type": "Invoice",
        "data": { "amount": 500, "status": "draft", "customer_id": "c1d2..." }
      },
      {
        "op": "update",
        "type": "Invoice",
        "id": "existing-uuid-...",
        "data": { "status": "sent" }
      },
      {
        "op": "delete",
        "type": "Invoice",
        "id": "old-uuid-..."
      }
    ]
  }'
```

### 4.5 Batch Operations (`POST /api/batch`)

Execute multiple heterogeneous operations (queries, mutations, function calls) in a single HTTP round-trip. Operations execute sequentially within the batch so that a mutation in op N is visible to a query in op N+1.

```json
{
  "ops": [
    {
      "type": "mutate",
      "id": "m1",
      "body": {
        "mutations": [
          { "op": "insert", "type": "User", "data": { "name": "Bob" } }
        ]
      }
    },
    {
      "type": "query",
      "id": "q1",
      "body": { "type": "User", "$where": [{ "attribute": "name", "op": "Eq", "value": "Bob" }] }
    },
    {
      "type": "fn",
      "id": "f1",
      "name": "sendWelcomeEmail",
      "args": { "name": "Bob" }
    }
  ]
}
```

**Limits:** Maximum 50 operations per batch, maximum 200 total mutations across all mutate ops.

---

## 5. Real-time Subscriptions

DarshJDB provides three real-time mechanisms over WebSocket.

### 5.1 Live Queries

Register a query as a live subscription. When any mutation touches the entity type and satisfies the filter, the change is pushed immediately.

**Subscribe:**

```json
{
  "type": "live-select",
  "id": "req-1",
  "query": "LIVE SELECT * FROM users WHERE role = 'admin'"
}
```

**Server acknowledgement:**

```json
{
  "type": "live-select-ok",
  "id": "req-1",
  "live_id": "uuid-of-live-query"
}
```

**Live event (pushed on matching mutation):**

```json
{
  "type": "live-event",
  "live_id": "uuid-of-live-query",
  "action": "CREATE",
  "result": {
    "name": "NewAdmin",
    "role": "admin",
    "email": "new@example.com"
  },
  "tx_id": 42
}
```

**Unsubscribe:**

```json
{
  "type": "kill",
  "id": "req-2",
  "live_id": "uuid-of-live-query"
}
```

Actions are one of `CREATE`, `UPDATE`, or `DELETE`. Live query filters support `=`, `!=`, `>`, `>=`, `<`, `<=`, `CONTAINS`, `IN`, and compound `AND`/`OR` predicates.

### 5.2 Reactive Dependency Tracking

Under the hood, the `DependencyTracker` powers live query invalidation. When a query is registered, its filter predicates are extracted as dependency edges on `(attribute, optional value-constraint)` pairs. When new triples arrive, the tracker computes which live queries are affected:

- **Eq** predicates create exact-match dependencies (only triggered when the exact value changes).
- All other operators create wildcard dependencies (triggered by any change to that attribute).
- `$search` queries create a sentinel `"*"` dependency that matches any attribute change.
- Entity type filtering ensures a change to `Post.title` never triggers a live query on `User`.

### 5.3 Presence Rooms

Track which users are present in a room with arbitrary ephemeral state (cursor position, typing status, selection).

**Join a room:**

```json
{
  "type": "presence-join",
  "room_id": "doc:abc-123",
  "user_id": "user-uuid",
  "state": { "cursor": { "x": 100, "y": 200 }, "status": "active" }
}
```

**Update state:**

```json
{
  "type": "presence-update",
  "room_id": "doc:abc-123",
  "user_id": "user-uuid",
  "state": { "cursor": { "x": 150, "y": 250 }, "status": "typing" }
}
```

Presence entries expire automatically after 60 seconds if not refreshed. Rate-limited to 20 updates per room per second.

### 5.4 Pub/Sub Channels

Redis-style channel subscriptions with glob pattern matching.

**Channel patterns:**

| Pattern                     | Matches                          |
|-----------------------------|----------------------------------|
| `entity:*`                  | All entity changes               |
| `entity:users:*`            | All user entity changes          |
| `entity:users:<uuid>`       | Specific entity changes          |
| `mutation:*`                | All mutations                    |
| `auth:*`                    | Auth events (signup, signin, signout) |
| `custom:<topic>`            | User-defined channels            |

**Subscribe:**

```json
{
  "type": "subscribe",
  "id": "sub-1",
  "channel": "entity:users:*"
}
```

---

## 6. Formulas

DarshJDB includes a spreadsheet-grade formula engine. Formulas are stored as computed field definitions and re-evaluated when their dependencies change.

### 6.1 Syntax

Formulas use a familiar expression language with field references enclosed in curly braces:

```
IF(AND({Status} = "Done", {Priority} > 3), "High", "Low")
```

**Grammar (informal):**

```
expr       = or_expr
or_expr    = and_expr ( "OR" and_expr )*
and_expr   = eq_expr  ( "AND" eq_expr )*
eq_expr    = cmp_expr ( ("=" | "!=" | "<>") cmp_expr )*
cmp_expr   = add_expr ( ("<" | "<=" | ">" | ">=") add_expr )*
add_expr   = mul_expr ( ("+" | "-" | "&") mul_expr )*
mul_expr   = unary    ( ("*" | "/" | "%") unary )*
unary      = ("NOT" | "-") unary | call_expr
call_expr  = IDENT "(" args ")" | primary
primary    = NUMBER | STRING | BOOL | field_ref | "(" expr ")"
field_ref  = "{" FIELD_NAME "}"
```

### 6.2 Field References

Reference other fields on the same entity with `{Field Name}`. The formula engine extracts all field references for dependency tracking so computed fields are re-evaluated only when their inputs change.

```
{First Name} & " " & {Last Name}
```

### 6.3 Operators

| Operator | Meaning             | Precedence |
|----------|---------------------|------------|
| `OR`     | Logical OR          | 1 (lowest) |
| `AND`    | Logical AND         | 2          |
| `=`, `!=`, `<>` | Equality    | 3          |
| `<`, `<=`, `>`, `>=` | Comparison | 4       |
| `+`, `-`, `&` | Add, subtract, string concat | 5 |
| `*`, `/`, `%` | Multiply, divide, modulo    | 6 |
| `NOT`, `-` (unary) | Negation   | 7 (highest)|

The `&` operator concatenates strings. The `<>` operator is an alias for `!=`.

### 6.4 Built-in Functions

Functions are case-insensitive. `IF`, `if`, and `If` are all valid.

#### Logic

| Function   | Signature                         | Description                              |
|------------|-----------------------------------|------------------------------------------|
| `IF`       | `IF(cond, then, else)`            | Conditional branch. `else` is optional (defaults to null). |
| `AND`      | `AND(a, b, ...)`                  | True if all arguments are true.          |
| `OR`       | `OR(a, b, ...)`                   | True if any argument is true.            |
| `NOT`      | `NOT(value)`                      | Logical negation.                        |
| `SWITCH`   | `SWITCH(expr, pat1, val1, ..., default)` | Pattern matching with fallback. |
| `BLANK`    | `BLANK()`                         | Returns null / empty.                    |

#### Text

| Function     | Signature                        | Description                            |
|--------------|----------------------------------|----------------------------------------|
| `CONCAT`     | `CONCAT(a, b, ...)`             | Concatenate strings.                   |
| `UPPER`      | `UPPER(text)`                    | Convert to uppercase.                  |
| `LOWER`      | `LOWER(text)`                    | Convert to lowercase.                  |
| `TRIM`       | `TRIM(text)`                     | Remove leading/trailing whitespace.    |
| `LEFT`       | `LEFT(text, n)`                  | First n characters.                    |
| `RIGHT`      | `RIGHT(text, n)`                 | Last n characters.                     |
| `MID`        | `MID(text, start, length)`       | Substring extraction.                  |
| `LEN`        | `LEN(text)`                      | Character count.                       |
| `FIND`       | `FIND(needle, haystack)`         | Position of first occurrence.          |
| `SUBSTITUTE` | `SUBSTITUTE(text, old, new)`     | Replace all occurrences.               |
| `REPT`       | `REPT(text, n)`                  | Repeat text n times.                   |

#### Math

| Function   | Signature                 | Description                     |
|------------|---------------------------|---------------------------------|
| `ABS`      | `ABS(n)`                  | Absolute value.                 |
| `ROUND`    | `ROUND(n, decimals)`      | Round to n decimal places.      |
| `FLOOR`    | `FLOOR(n)`                | Round down.                     |
| `CEILING`  | `CEILING(n)`              | Round up.                       |
| `MOD`      | `MOD(n, divisor)`         | Remainder.                      |
| `POWER`    | `POWER(base, exp)`        | Exponentiation.                 |
| `SQRT`     | `SQRT(n)`                 | Square root.                    |
| `LOG`      | `LOG(n)`                  | Natural logarithm.              |
| `MIN`      | `MIN(a, b, ...)`          | Minimum of arguments.           |
| `MAX`      | `MAX(a, b, ...)`          | Maximum of arguments.           |

#### Date

| Function     | Signature                   | Description                        |
|--------------|-----------------------------|------------------------------------|
| `NOW`        | `NOW()`                     | Current timestamp.                 |
| `TODAY`      | `TODAY()`                   | Current date (no time).            |
| `YEAR`       | `YEAR(date)`                | Extract year.                      |
| `MONTH`      | `MONTH(date)`               | Extract month (1-12).              |
| `DAY`        | `DAY(date)`                 | Extract day of month.              |
| `DATEADD`    | `DATEADD(date, n, unit)`    | Add n units to a date.             |
| `DATEDIF`    | `DATEDIF(start, end, unit)` | Difference between two dates.      |

#### Aggregate (used in rollup/summary contexts)

| Function   | Signature           | Description                          |
|------------|---------------------|--------------------------------------|
| `SUM`      | `SUM(values)`       | Numeric sum.                         |
| `AVG`      | `AVG(values)`       | Arithmetic mean.                     |
| `COUNT`    | `COUNT(values)`     | Count of non-null values.            |
| `COUNTA`   | `COUNTA(values)`    | Count of non-blank values.           |

#### System

| Function     | Signature           | Description                        |
|--------------|---------------------|------------------------------------|
| `RECORD_ID`  | `RECORD_ID()`      | Current entity UUID.               |
| `CREATED_AT` | `CREATED_AT()`     | Entity creation timestamp.         |
| `MODIFIED_AT`| `MODIFIED_AT()`    | Last modification timestamp.       |

### 6.5 Error Values

When a formula cannot be evaluated, it produces a typed error value:

| Error       | Meaning                                                |
|-------------|--------------------------------------------------------|
| `#ERROR`    | General evaluation failure.                            |
| `#REF`      | Referenced field does not exist on this entity.        |
| `#VALUE`    | Type mismatch (e.g., adding a string to a number).     |
| `#DIV/0`    | Division by zero.                                      |

### 6.6 Examples

**Priority label:**
```
IF(AND({Status} = "Done", {Priority} > 3), "High", "Low")
```

**Full name:**
```
CONCAT(UPPER({First}), " ", LOWER({Last}))
```

**Days until due:**
```
DATEDIF(NOW(), {Due Date}, "days")
```

**Revenue classification:**
```
SWITCH({Region}, "US", {Revenue} * 1.0, "EU", {Revenue} * 0.92, {Revenue} * 0.85)
```

---

## 7. Aggregation

DarshJDB supports server-side aggregation via `POST /api/aggregate`.

### 7.1 Basic Aggregation

```bash
curl -X POST http://localhost:4000/api/aggregate \
  -H "Content-Type: application/json" \
  -d '{
    "entity_type": "Order",
    "group_by": ["status"],
    "aggregations": [
      { "field": "amount", "function": {"fn": "Sum"}, "alias": "total_amount" },
      { "field": "amount", "function": {"fn": "Avg"}, "alias": "avg_amount" },
      { "field": "id", "function": {"fn": "Count"}, "alias": "order_count" }
    ]
  }'
```

**Response:**

```json
{
  "groups": [
    {
      "key": { "status": "active" },
      "values": { "total_amount": 15000, "avg_amount": 500, "order_count": 30 },
      "count": 30
    },
    {
      "key": { "status": "closed" },
      "values": { "total_amount": 8000, "avg_amount": 400, "order_count": 20 },
      "count": 20
    }
  ],
  "totals": {
    "total_amount": 23000,
    "avg_amount": 460,
    "order_count": 50
  }
}
```

### 7.2 All 18 Aggregate Functions

| Function        | JSON `fn` Value      | Description                                  |
|-----------------|----------------------|----------------------------------------------|
| Count           | `"Count"`            | Count of non-null values.                    |
| CountDistinct   | `"CountDistinct"`    | Count of distinct non-null values.           |
| Sum             | `"Sum"`              | Numeric sum.                                 |
| Avg             | `"Avg"`              | Arithmetic mean.                             |
| Min             | `"Min"`              | Minimum (numeric or lexicographic).          |
| Max             | `"Max"`              | Maximum (numeric or lexicographic).          |
| StdDev          | `"StdDev"`           | Population standard deviation.               |
| Variance        | `"Variance"`         | Population variance.                         |
| Median          | `"Median"`           | 50th percentile.                             |
| Percentile      | `{"fn":"Percentile","arg":0.95}` | Arbitrary percentile (0.0 to 1.0). |
| First           | `"First"`            | First value by tx_id ordering.               |
| Last            | `"Last"`             | Last value by tx_id ordering.                |
| ArrayAgg        | `"ArrayAgg"`         | Collect all values into a JSON array.        |
| StringAgg       | `{"fn":"StringAgg","arg":", "}` | Concatenate strings with separator. |
| CountEmpty      | `"CountEmpty"`       | Count of null or empty values.               |
| CountFilled     | `"CountFilled"`      | Count of non-null, non-empty values.         |
| PercentEmpty    | `"PercentEmpty"`     | Percentage of null or empty values.          |
| PercentFilled   | `"PercentFilled"`    | Percentage of non-null, non-empty values.    |

### 7.3 Pre-Aggregation Filters

Apply WHERE-style filters before aggregation:

```json
{
  "entity_type": "Order",
  "group_by": ["region"],
  "aggregations": [
    { "field": "amount", "function": {"fn": "Sum"}, "alias": "total" }
  ],
  "filters": [
    { "attribute": "created_at", "op": "Gte", "value": "2026-01-01" },
    { "attribute": "status", "op": "Neq", "value": "cancelled" }
  ]
}
```

### 7.4 HAVING Clause

Filter on aggregated results:

```json
{
  "entity_type": "Order",
  "group_by": ["customer_id"],
  "aggregations": [
    { "field": "amount", "function": {"fn": "Sum"}, "alias": "total_spend" }
  ],
  "having": {
    "alias": "total_spend",
    "op": "Gt",
    "value": 10000
  }
}
```

HAVING operators: `Eq`, `Neq`, `Gt`, `Gte`, `Lt`, `Lte`. The `alias` must reference an aggregation alias defined in the same query.

### 7.5 Multi-Column GROUP BY

Group by multiple attributes. Group keys are stored internally as `"val1|||val2"` and deserialized automatically.

```json
{
  "entity_type": "Sale",
  "group_by": ["region", "product_type"],
  "aggregations": [
    { "field": "revenue", "function": {"fn": "Sum"}, "alias": "total_revenue" },
    { "field": "revenue", "function": {"fn": "Avg"}, "alias": "avg_deal_size" }
  ]
}
```

### 7.6 Time-Series / Chart Queries

For dashboard charts, use `POST /api/aggregate/chart`:

```json
{
  "entity_type": "Order",
  "date_field": "created_at",
  "value_field": "amount",
  "function": "sum",
  "bucket": "month",
  "group_by": "region",
  "filters": [
    { "attribute": "status", "op": "Eq", "value": "completed" }
  ]
}
```

**Bucket options:** `day`, `week`, `month`, `quarter`, `year`. Uses PostgreSQL `date_trunc` for bucketing.

**Chart aggregate functions:** `count`, `sum`, `avg`, `min`, `max`.

### 7.7 Summary Endpoint

Quick summary statistics for all numeric fields of an entity type:

```bash
curl http://localhost:4000/api/aggregate/summary/Invoice
```

Returns count, count_distinct, count_empty, count_filled, sum, avg, min, max per attribute -- no query needed.

---

## 8. Views

Views in DarshJDB are saved query configurations with filters, sorts, and field visibility. When you query through a view, its filters and sorts are applied automatically before your additional query parameters.

### 8.1 Querying Through a View

```bash
curl -X POST http://localhost:4000/api/query \
  -H "Content-Type: application/json" \
  -d '{
    "type": "Task",
    "view_id": "view-uuid-...",
    "$limit": 50
  }'
```

The view's saved `$where` and `$order` clauses are merged with any clauses you provide. Your clauses are appended (AND'd with the view's filters, added as secondary sorts).

### 8.2 View Types and Query Behavior

| View Type  | Behavior                                                          |
|------------|-------------------------------------------------------------------|
| Grid       | Standard tabular query with all visible fields.                   |
| Kanban     | Groups results by a status/category field automatically.          |
| Calendar   | Filters to entities with date fields, ordered by date.            |
| Gallery    | Includes image/cover field in projection.                         |
| Form       | Write-only view; queries return the form schema, not data.        |

---

## 9. Advanced

### 9.1 Query Caching and Invalidation

DarshJDB uses a Redis-inspired in-memory hot cache backed by `DashMap` for lock-free concurrent reads. Cache behavior:

- **Key derivation:** SHA-256 hash of the QueryAST JSON.
- **TTL + LRU eviction** to bound memory usage.
- **Entity-type keyed invalidation:** when a mutation touches entity type `User`, only cached queries targeting `User` are flushed. Queries on `Post` remain valid.
- **Transaction-aware:** each cache entry stores the `tx_id` at cache time for stale detection.

Configure via environment variables:

| Variable          | Default | Description                     |
|-------------------|---------|---------------------------------|
| `DDB_CACHE_TTL`   | 60s     | Time-to-live for cache entries  |
| `DDB_CACHE_MAX`   | 10000   | Maximum entries in the cache    |

### 9.2 Permission Injection (Row-Level Security)

DarshJDB evaluates SurrealDB-style row-level permissions at query time. When a table has `DEFINE TABLE ... PERMISSIONS` rules:

```
DEFINE TABLE posts PERMISSIONS
  FOR select WHERE published = true OR user = $auth.id
  FOR create WHERE $auth.role = "admin"
  FOR update WHERE user = $auth.id
  FOR delete WHERE $auth.role = "admin"
```

- **Reads:** The permission expression is injected as an additional WHERE clause. The query engine adds it to the SQL plan so only authorized rows are returned.
- **Writes:** The expression is evaluated as a gate. If it evaluates to false, the mutation is rejected with 403.
- **Security:** All `$auth.*` references are resolved to bind parameters, never interpolated as raw strings. Unknown `$auth.*` fields resolve to NULL, which fails closed.

### 9.3 Parallel Batch Execution

Inspired by Solana's Sealevel runtime, the batch executor analyzes conflict sets and schedules non-conflicting operations in parallel:

**Conflict model:**
- Two operations conflict if they touch the same entity type AND at least one is a mutation.
- Read-only queries never conflict with each other, even on the same entity type.

**Wave scheduling:**
1. Extract the entity types each operation touches.
2. Walk the operation list. If no conflict with existing wave members, add to the current wave.
3. If conflict, start a new wave.
4. Execute each wave with `tokio::join_all` (parallel within wave).
5. Waves execute sequentially to preserve causal ordering.

This means a batch of `[query(User), query(Post), mutate(Invoice), query(Order)]` executes all four operations in a single parallel wave. A batch of `[mutate(User), query(User)]` splits into two waves.

### 9.4 Content Negotiation

All JSON-producing handlers inspect the `Accept` header. When the client sends `Accept: application/msgpack`, responses are serialized with MessagePack instead of JSON. Request bodies follow `Content-Type`.

```bash
curl -X POST http://localhost:4000/api/query \
  -H "Content-Type: application/json" \
  -H "Accept: application/msgpack" \
  -d '{"type": "User"}'
```

### 9.5 Rate Limiting

Every response includes rate-limit headers:

| Header                    | Description                     |
|---------------------------|---------------------------------|
| `X-RateLimit-Limit`       | Maximum requests per window     |
| `X-RateLimit-Remaining`   | Requests remaining in window    |
| `X-RateLimit-Reset`       | Seconds until window resets     |

### 9.6 Performance Tips

1. **Use `$limit` always.** Without it, the engine fetches every matching entity. Even `$limit: 1000` prevents unbounded scans.

2. **Use `$where` over `$search` when possible.** Exact attribute filters generate targeted index joins. Full-text search scans more broadly.

3. **Nest sparingly.** Each nesting level adds one batched query. Three levels is the maximum. If you need deeper traversal, use DarshQL graph queries or restructure your data model.

4. **Pre-compute embeddings.** Pass `$semantic.vector` directly instead of `$semantic.query` to avoid an embedding API round-trip on every query.

5. **Batch operations.** Use `POST /api/batch` to combine multiple queries and mutations in one round-trip. The parallel executor will schedule non-conflicting operations concurrently.

6. **Use the cache.** The hot cache serves repeated queries in sub-millisecond time. Structure queries to be cache-friendly by avoiding random offsets and using stable filter patterns.

7. **Aggregate server-side.** Use `POST /api/aggregate` instead of fetching all entities and computing in your client. The SQL aggregation runs directly against the triple store index.

---

## Appendix: Query Execution Pipeline

```
DarshJQL JSON Object
    |
    v
parse_darshan_ql()          -- Validate and parse JSON into QueryAST
    |
    v
plan_query()                -- Compile AST into SQL with bind params
  or plan_hybrid_query()    -- For $hybrid queries (RRF CTE)
    |
    v
execute_query()             -- Run SQL via sqlx, group rows by entity_id
    |
    v
batch_resolve_nested()      -- Fetch referenced entities (1+P queries)
    |
    v
Apply offset / limit        -- Rust-side pagination over grouped entities
    |
    v
Vec<QueryResultRow>         -- Final result: entity_id + attributes + nested
```

All query plans are cached in an LRU keyed by the SHA-256 hash of the plan shape, so repeated queries with different bind values skip the planning step entirely.
