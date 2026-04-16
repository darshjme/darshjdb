# Triple Store & EAV Deep Dive

DarshJDB stores all application data as triples in an Entity-Attribute-Value (EAV) model backed by PostgreSQL. This document covers the storage model, schema, query translation, indexing strategy, and audit chain.

---

## What is EAV / Triple Store?

A triple store represents every fact as a three-part assertion:

```
(Entity, Attribute, Value)
```

For example, a user record with an email becomes:

```
(user_abc123, "user/email", "alice@example.com")
(user_abc123, "user/name",  "Alice")
(user_abc123, "user/age",   30)
```

Traditional relational databases force you to define columns ahead of time. The EAV model inverts this: every field is a row, making schema evolution trivial. Adding a new field to an entity type requires no `ALTER TABLE` -- you simply write a new triple with a new attribute name.

### Why EAV for a BaaS?

1. **Schema-free by default.** Applications can store arbitrary fields without migrations.
2. **Fine-grained history.** Each triple carries a transaction ID, so point-in-time reads and undo are built into the storage layer.
3. **Sparse data is free.** Entities with different shapes coexist in the same table without NULL columns.
4. **Attribute-level permissions.** Row-level and field-level security operate on the same primitive.

The tradeoff is query complexity -- DarshJDB's query engine handles the reassembly of triples into JSON documents transparently.

---

## The `triples` Table Schema

All data lives in a single PostgreSQL table:

```sql
CREATE TABLE IF NOT EXISTS triples (
    id          BIGSERIAL   PRIMARY KEY,
    entity_id   UUID        NOT NULL,
    attribute   TEXT        NOT NULL,
    value       JSONB       NOT NULL,
    value_type  SMALLINT    NOT NULL DEFAULT 0,
    tx_id       BIGINT      NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    retracted   BOOLEAN     NOT NULL DEFAULT false,
    expires_at  TIMESTAMPTZ
);
```

### Column Reference

| Column | Type | Description |
|--------|------|-------------|
| `id` | `BIGSERIAL` | Auto-generated primary key. Internal use only. |
| `entity_id` | `UUID` | The entity this fact belongs to. All triples sharing an `entity_id` form one logical record. |
| `attribute` | `TEXT` | Attribute name, typically namespaced (e.g., `"user/email"`, `"post/title"`). Maximum 512 bytes. |
| `value` | `JSONB` | The value payload, stored as a JSON blob. Supports strings, numbers, booleans, arrays, and nested objects. |
| `value_type` | `SMALLINT` | Discriminator tag identifying the logical type. See ValueType enum below. |
| `tx_id` | `BIGINT` | Monotonically increasing transaction identifier. All triples written in a single mutation share the same `tx_id`. |
| `created_at` | `TIMESTAMPTZ` | Timestamp when the triple was written. Defaults to `now()`. |
| `retracted` | `BOOLEAN` | Soft-delete flag. Retracted triples are logically invisible but physically retained for history. |
| `expires_at` | `TIMESTAMPTZ` | Optional TTL expiry. When set, the triple is automatically retracted after this timestamp. |

### Append-Only Semantics

Triples are never physically deleted or updated in place. "Deletion" is expressed by setting `retracted = true` in a later transaction. "Updates" are expressed by retracting the old triple and inserting a new one with the updated value under a new `tx_id`. This append-only design enables:

- Full version history at any point in time
- Undo/restore by retracting the retraction
- Merkle audit trails over the transaction log

---

## ValueType Enum

Every triple carries a `value_type` discriminator (stored as `SMALLINT`) so the query engine can apply type-specific operators -- ordering, range scans, full-text search, and so on.

```rust
#[repr(i16)]
pub enum ValueType {
    String    = 0,  // UTF-8 string
    Integer   = 1,  // 64-bit signed integer
    Float     = 2,  // 64-bit IEEE 754 float
    Boolean   = 3,  // true / false
    Timestamp = 4,  // RFC 3339 timestamp
    Reference = 5,  // UUID reference to another entity
    Json      = 6,  // Arbitrary JSON blob
}
```

### Type Semantics

| Tag | Label | JSONB encoding | Use case |
|-----|-------|---------------|----------|
| 0 | `string` | `"hello"` | Names, emails, text content |
| 1 | `integer` | `42` | Counts, IDs, quantities |
| 2 | `float` | `3.14` | Prices, measurements, scores |
| 3 | `boolean` | `true` | Flags, toggles |
| 4 | `timestamp` | `"2026-04-07T12:00:00Z"` | Dates, created_at, scheduled times |
| 5 | `reference` | `"550e8400-e29b-41d4-a716-446655440000"` | Foreign key to another entity_id |
| 6 | `json` | `{"nested": "data"}` | Arbitrary structured data, arrays, metadata |

The discriminator range is contiguous from 0 to 6. Values outside this range are rejected by `TripleInput::validate()` before any database write occurs.

### Polymorphic Attributes

An attribute can carry multiple value types across different entities. For example, a `"doc/payload"` attribute might be `String` on some entities and `Json` on others. Schema inference tracks all observed types per attribute.

---

## Entity Pool (UUID to i64 Encoding)

Entities are identified by UUIDs externally but indexed as `UUID` type in PostgreSQL. The `entity_id` column uses Postgres-native UUID storage (16 bytes), which provides:

- Globally unique identifiers without coordination
- Efficient B-tree indexing via Postgres UUID operators
- Compact storage compared to TEXT representation

Entity "types" are not enforced at the storage layer. Instead, type is inferred from the `:db/type` attribute or from the attribute namespace prefix. For example, attributes starting with `user/` are inferred to belong to the `User` entity type.

---

## How Queries Translate from DarshJQL to SQL

When a client sends a DarshJQL query, the query engine translates it into SQL against the `triples` table. Here is the translation pipeline:

### 1. Simple Entity Fetch

**DarshJQL:**
```json
{ "from": "users", "where": { "email": "alice@example.com" } }
```

**Generated SQL:**
```sql
SELECT DISTINCT t1.entity_id,
       t1.value AS email,
       t2.value AS name
FROM triples t1
JOIN triples t2 ON t2.entity_id = t1.entity_id
  AND t2.attribute = 'user/name' AND NOT t2.retracted
WHERE t1.attribute = 'user/email'
  AND t1.value = '"alice@example.com"'::jsonb
  AND NOT t1.retracted
```

### 2. Attribute Scan (Schema Inference)

```sql
SELECT attribute, value_type, COUNT(DISTINCT entity_id) AS cardinality
FROM triples
WHERE NOT retracted
GROUP BY attribute, value_type
```

This query powers `GET /api/admin/schema`, which builds the full schema by scanning live data.

### 3. Point-in-Time Read

**DarshJQL:**
```json
{ "from": "users", "id": "abc123", "asOf": 42 }
```

**Generated SQL:**
```sql
SELECT DISTINCT ON (attribute)
       entity_id, attribute, value, value_type, tx_id
FROM triples
WHERE entity_id = $1 AND tx_id <= $2
ORDER BY attribute, tx_id DESC
```

`DISTINCT ON (attribute)` with `ORDER BY tx_id DESC` picks the latest version of each attribute at or before the given transaction, enabling time-travel queries.

### 4. Bulk Write (UNNEST Insert)

Instead of individual INSERT statements, DarshJDB decomposes a batch of `TripleInput` values into columnar arrays and executes a single query:

```sql
INSERT INTO triples (entity_id, attribute, value, value_type, tx_id, expires_at)
SELECT * FROM UNNEST(
    $1::uuid[],
    $2::text[],
    $3::jsonb[],
    $4::smallint[],
    $5::bigint[],
    $6::timestamptz[]
)
```

This UNNEST-based approach is 10-50x faster than per-row inserts because it eliminates per-row round-trip overhead and allows PostgreSQL to optimize WAL writes across the entire batch.

---

## Schema Inference

DarshJDB infers schema from live data rather than requiring explicit DDL. The schema layer (`triple_store::schema`) provides:

### EntityType Discovery

The `Schema` struct represents a point-in-time snapshot of the database structure:

```rust
pub struct Schema {
    pub entity_types: HashMap<String, EntityType>,
    pub as_of_tx: i64,
}

pub struct EntityType {
    pub name: String,
    pub attributes: HashMap<String, AttributeInfo>,
    pub references: Vec<ReferenceInfo>,
    pub entity_count: u64,
}

pub struct AttributeInfo {
    pub name: String,
    pub value_types: Vec<ValueType>,
    pub required: bool,
    pub cardinality: u64,
}
```

### Migration Generation

The `MigrationGenerator` diffs two schema snapshots and produces ordered migration actions:

1. **AddEntityType** -- new entity types appear (sorted alphabetically)
2. **AddAttribute** -- new attributes on existing or new types
3. **AlterAttribute** -- value type changes (e.g., Integer widened to Float)
4. **RemoveAttribute** -- attributes removed from existing types
5. **RemoveEntityType** -- entire entity types removed

This ordering ensures consumers can apply actions sequentially. The diff is deterministic -- running it multiple times with the same inputs always produces the same output.

### Reference Detection

References between entity types are discovered by scanning attributes with `ValueType::Reference` (tag 5). The `ReferenceInfo` struct captures:

- The attribute holding the reference UUID
- The inferred target entity type (best-effort, based on where the referenced UUIDs appear)
- Cardinality (how many entities carry this reference)

---

## Indexes and Their Purpose

Six indexes support the query patterns described above:

```sql
-- 1. Entity + Attribute lookup (most common query path)
CREATE INDEX idx_triples_entity_attr
    ON triples (entity_id, attribute)
    WHERE NOT retracted;

-- 2. JSONB value queries (contains, equality, @> operator)
CREATE INDEX idx_triples_value_gin
    ON triples USING gin (value)
    WHERE NOT retracted;

-- 3. Transaction ordering (change feed, cursor-based reads)
CREATE INDEX idx_triples_tx_id
    ON triples (tx_id);

-- 4. Point-in-time reads (entity history, time-travel)
CREATE INDEX idx_triples_entity_tx
    ON triples (entity_id, tx_id);

-- 5. Schema inference (attribute scan across all entities)
CREATE INDEX idx_triples_attribute
    ON triples (attribute)
    WHERE NOT retracted;

-- 6. TTL expiry scan (find expired triples efficiently)
CREATE INDEX idx_triples_expires
    ON triples (expires_at)
    WHERE expires_at IS NOT NULL AND NOT retracted;
```

### Partial Indexes

Indexes 1, 2, 5, and 6 use `WHERE NOT retracted` partial index predicates. This means retracted triples are excluded from these indexes entirely, keeping index sizes small and scans fast. Retracted data is only accessed through the full table (for history reconstruction).

### GIN Index

The `idx_triples_value_gin` GIN index enables JSONB containment queries (`@>`), key-exists checks, and path-based filtering without full table scans. This is critical for queries like "find all users where metadata contains key X".

---

## Transaction IDs and Ordering

Transaction IDs are allocated from a PostgreSQL sequence:

```sql
CREATE SEQUENCE darshan_tx_seq START WITH 1 INCREMENT BY 1;
```

Each call to `set_triples` or `bulk_load`:

1. Allocates a new `tx_id` via `SELECT nextval('darshan_tx_seq')`
2. Writes all triples in the batch with that same `tx_id`
3. Sends a `NOTIFY ddb_changes` with payload `{tx_id}:{entity_type}` inside the same database transaction

This guarantees:

- **Atomicity**: all triples in a mutation share one `tx_id`
- **Ordering**: `tx_id` values are strictly monotonic
- **Consistency**: the NOTIFY is sent atomically with the data write, so listeners never see a notification without the corresponding data

### Transaction-Based Features

| Feature | How tx_id is used |
|---------|-------------------|
| Point-in-time reads | `WHERE tx_id <= $target` |
| Change feed cursors | Resume from last-seen `tx_id` |
| Subscription diffs | Compare result sets between `tx_id` values |
| Undo | Retract all triples with `tx_id = $target` |
| Audit trail | Merkle root computed per `tx_id` |

---

## TTL / Expiry

Triples support time-to-live via the `expires_at` column. When creating a triple, set `ttl_seconds` in the `TripleInput`:

```json
{
  "entity_id": "550e8400-...",
  "attribute": "session/token",
  "value": "abc123",
  "value_type": 0,
  "ttl_seconds": 3600
}
```

The server computes `expires_at = NOW() + interval` at insert time. A background task periodically scans the `idx_triples_expires` index and retracts expired triples.

### Use Cases

- **Session tokens**: auto-expire after 1 hour
- **Cache entries**: stale after 5 minutes
- **Temporary state**: collaboration cursors, typing indicators
- **Invite links**: expire after 24 hours

---

## Merkle Audit Chain

Every transaction is hashed into a Merkle tree for tamper detection. The flow:

1. Before writing to PostgreSQL, the server computes a Merkle root from the in-memory `TripleInput` values and the assigned `tx_id`.
2. After the data write commits, the root is recorded in a separate `audit_trail` table.
3. Verification compares the stored root against a recomputed root from the actual triple data.

```
          root
         /    \
      h(0,1)  h(2,3)
      /  \      /  \
    h(t0) h(t1) h(t2) h(t3)
```

Each leaf `h(tN)` is the hash of `(entity_id, attribute, value, value_type, tx_id)` for a single triple. The tree is built bottom-up using SHA-256.

### Verification

To verify a transaction's integrity:

1. Fetch all triples with `tx_id = $target`
2. Recompute the Merkle root from those triples
3. Compare against the stored root in `audit_trail`

If the roots match, no triple in that transaction has been modified since it was written. If they diverge, the data has been tampered with.

### Design Decisions

- Merkle roots are computed **in-memory** before the database round-trip, avoiding a read-after-write.
- Root recording failures are logged but do not abort the transaction (the data write is more important than the audit record).
- The audit trail table is created by `crate::audit::ensure_audit_schema()` during server startup.

---

## Retraction vs. Deletion

DarshJDB distinguishes between retraction (soft delete) and physical deletion:

| Operation | What happens | History preserved? |
|-----------|-------------|-------------------|
| Retract | Sets `retracted = true` on matching triples | Yes -- the data remains in the table |
| Physical delete | Not supported via API | N/A |

Retracted triples:
- Are excluded from all active indexes (via partial index predicates)
- Are invisible to normal queries
- Are visible to history queries (`get_entity_at`, version reconstruction)
- Can be "un-retracted" to restore deleted data

This design ensures that no data is ever lost through the application layer. Physical deletion would only occur through direct database maintenance (e.g., GDPR compliance scripts).

---

## Input Validation

Every `TripleInput` is validated before touching the database:

```rust
pub fn validate(&self) -> Result<()> {
    // Attribute must be non-empty
    if self.attribute.is_empty() { ... }

    // Attribute must not exceed 512 bytes
    if self.attribute.len() > 512 { ... }

    // value_type must be a known discriminator (0..=6)
    if ValueType::from_i16(self.value_type).is_none() { ... }
}
```

Validation runs on every triple in a batch **before** any database write. If any triple fails validation, the entire batch is rejected -- no partial writes.

---

## Related Documentation

- [Architecture](architecture.md) -- How the triple store fits into the overall system
- [DarshJQL Reference](DARSHQL.md) -- Full query language specification
- [History & Snapshots](guide/history.md) -- Version reconstruction built on tx_id
- [Merkle Audit Trail](guide/audit.md) -- Detailed audit chain documentation
- [Performance](performance.md) -- Tuning the triple store for production workloads
