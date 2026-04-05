# DarshJDB vs Convex: Technical Comparison

> Written: 2026-04-05
> Status: Honest assessment based on code review of both systems
> Audience: Engineers evaluating backend-as-a-service platforms

## Executive Summary

Convex is a cloud-native reactive database with excellent TypeScript DX, deterministic server functions, and automatic query reactivity. DarshJDB is a self-hosted BaaS built on a triple store (EAV) data model with multi-language SDK support. They share similar goals -- making real-time backends easy -- but take fundamentally different architectural approaches.

**Convex's strengths:** Developer experience, type safety, deterministic execution, battle-tested reactivity.
**DarshJDB's strengths:** Self-hosting from day one, data model flexibility, multi-language SDKs, no vendor lock-in.

Neither is universally better. This document is written by the DarshJDB team and we will be honest about where Convex is ahead.

---

## 1. Data Model

### Convex: Document-Based with Schemas

Convex uses a document model similar to MongoDB, but with enforced schemas via `defineSchema()` and `defineTable()`. Each table holds JSON documents with an auto-generated `_id` and `_creationTime`. Schema validation is compile-time (TypeScript) and runtime (server-side). Relations are expressed through document ID references.

```typescript
// Convex schema definition
export default defineSchema({
  messages: defineTable({
    body: v.string(),
    author: v.id("users"),
    channel: v.id("channels"),
  }).index("by_channel", ["channel"]),
});
```

### DarshJDB: Triple Store (EAV)

DarshJDB stores everything as `(entity_id, attribute, value)` triples in a single Postgres table. The schema is inferred from data rather than declared upfront. Entities are grouped by a `:db/type` attribute. Values are JSONB with a type discriminator (string, integer, float, boolean, null, reference, json).

```sql
-- Every fact is a row
(entity_id: UUID, attribute: "name",  value: "Alice",     value_type: 0)
(entity_id: UUID, attribute: "email", value: "a@test.com", value_type: 0)
(entity_id: UUID, attribute: ":db/type", value: "User",   value_type: 0)
```

### Comparison

| Aspect | Convex | DarshJDB |
|--------|--------|-----------|
| Schema enforcement | Compile-time + runtime | Runtime inference, optional |
| Adding a field | Requires schema migration | Just write the triple |
| Removing a field | Schema migration | Stop writing it; old triples persist |
| Relations | Document ID references | First-class reference value type (5) |
| Nested queries | `.collect()` then manual joins | Built into query AST (`nested` field) |
| Point-in-time reads | Not exposed | `get_entity_at(entity_id, tx_id)` |
| Schema evolution | Explicit migrations | Schemaless by default |

**Honest assessment:** Convex's document model is more familiar to most developers. The triple store gives DarshJDB genuine flexibility -- adding attributes requires zero migration, and point-in-time reads are inherent to the append-only design -- but it comes at the cost of query complexity and join performance. For most CRUD applications, Convex's model is easier to reason about. DarshJDB's model shines for knowledge graphs, audit-heavy systems, and applications where schema evolution is constant.

---

## 2. Reactive Queries

### Convex: Automatic Re-Execution

Convex tracks every data dependency of a query function automatically. When any underlying data changes, the query is re-executed end-to-end and the new result is pushed to all subscribers. This is completely transparent -- developers write normal query functions and reactivity "just works."

```typescript
// Convex: reactivity is automatic
export const list = query({
  handler: async (ctx) => {
    return await ctx.db.query("messages").order("desc").take(50);
  },
});
```

The re-execution model means Convex does not compute diffs. It re-runs the entire query and sends the full result. This is simple and correct but can be wasteful for large result sets where only one row changed.

### DarshJDB: Change Event + Dependency Tracking + Diff Push

DarshJDB uses a three-stage pipeline (implemented in `packages/server/src/sync/` and `packages/server/src/query/reactive.rs`):

1. **ChangeEvent emission:** When triples are written, a `ChangeEvent` is broadcast via `tokio::sync::broadcast` containing the affected entity IDs, attributes, and entity type.

2. **Dependency matching:** The `DependencyTracker` maintains an inverted index mapping `(attribute, optional_value_constraint)` pairs to registered query IDs. It resolves affected queries in O(changes x matching_deps) time, with entity-type filtering to avoid false positives.

3. **Diff computation:** Affected queries are re-executed and the result is diffed against a cached snapshot. The `QueryDiff` contains `added`, `removed`, and `updated` (field-level `EntityPatch`) arrays. Only the delta is pushed over WebSocket.

```rust
// DarshJDB: explicit dependency extraction
fn extract_dependencies(ast: &QueryAST) -> Vec<Dependency> {
    // Eq predicates -> exact (attribute, value) constraint
    // Range/order/nested -> wildcard (attribute, None) constraint
    // $search -> star wildcard ("*", None)
}
```

### Comparison

| Aspect | Convex | DarshJDB |
|--------|--------|-----------|
| Developer effort | Zero (automatic) | Zero (subscribe to query, receive diffs) |
| Granularity | Full result re-send | Field-level diffs |
| Bandwidth efficiency | Lower (full payloads) | Higher (delta only) |
| Correctness guarantee | Very high (re-execute = always correct) | Good but unproven at scale |
| Implementation maturity | Production-tested | **Not wired to mutation flow yet** |

**Honest assessment:** Convex's approach is simpler and more reliable. Re-executing the whole query eliminates an entire class of bugs around stale caches and diff correctness. DarshJDB's diff-based approach is theoretically more bandwidth-efficient, especially for large result sets, but:

1. **The reactive pipeline is not connected end-to-end.** CODE.md explicitly states: "WebSocket real-time subscriptions (handler exists, not wired to query results)" and "Reactive push (sync engine exists, not connected to mutation flow)." The code for each stage exists and is individually tested (reactive.rs has 20+ tests passing), but the pipeline is not integrated.

2. Convex has years of production usage validating their reactivity model. DarshJDB has zero production deployments.

**Advantage: Convex, decisively.** DarshJDB's architecture has better theoretical efficiency, but Convex ships a working product.

---

## 3. Server Functions

### Convex: Deterministic V8 Isolates

Convex runs server functions in a deterministic JavaScript runtime. Functions are categorized into three types:

- **Queries** (read-only, deterministic, cached, reactive)
- **Mutations** (read-write, deterministic, serializable transactions)
- **Actions** (non-deterministic, can call external APIs, no direct DB access)

Determinism means queries produce identical results given the same database state. This enables aggressive caching and reliable reactivity. The V8 isolate patches non-deterministic APIs (`Date.now()`, `Math.random()`) to return deterministic values during query/mutation execution.

### DarshJDB: Subprocess-Based Execution

DarshJDB defines a `RuntimeBackend` trait with a `ProcessRuntime` implementation that spawns Deno or Node subprocesses per invocation. Functions are categorized into queries, mutations, and actions (mirroring Convex's taxonomy). Resource isolation is via OS-level controls (CPU time limit, memory limit, concurrency semaphore).

```rust
// DarshJDB: pluggable runtime backend
pub trait RuntimeBackend: Send + Sync + 'static {
    fn execute(...) -> Pin<Box<dyn Future<Output = RuntimeResult<Value>> + Send + '_>>;
}

// Default: subprocess per invocation
pub struct ProcessRuntime {
    binary: PathBuf,      // deno or node
    timeout: Duration,    // CPU time limit
    memory_mb: u32,       // Memory ceiling
    semaphore: Arc<Semaphore>, // Concurrency limit
}
```

### Comparison

| Aspect | Convex | DarshJDB |
|--------|--------|-----------|
| Runtime | Embedded V8 with determinism patches | Subprocess (Deno/Node) |
| Cold start | ~5ms (isolate reuse) | ~50-200ms (process spawn) |
| Determinism | Enforced (patched `Date`, `Math.random`) | Not enforced |
| Isolation | V8 isolate sandbox | OS process boundary |
| Hot reload | Automatic on deploy | Registry-based file watching |
| Function types | query / mutation / action | query / mutation / action |
| Status | Production | **Subprocess placeholder, not V8** |

**Honest assessment:** Convex's deterministic V8 runtime is a genuine technical achievement. The determinism guarantee makes caching and reactivity reliable by construction. DarshJDB's subprocess approach is a pragmatic starting point, but the per-invocation spawn cost makes it unsuitable for high-throughput workloads. The codebase explicitly notes "subprocess-based, not V8" and the function runtime is listed as not working in CODE.md.

**Advantage: Convex, significantly.** DarshJDB has the right abstractions (the `RuntimeBackend` trait allows swapping in an embedded V8 later), but today it is a subprocess placeholder versus Convex's production-grade isolate system.

---

## 4. Transactions

### Convex

Convex provides serializable ACID transactions. Mutations are automatically transactional -- the entire mutation function runs in a single transaction. If any part fails, the entire mutation is rolled back. Convex uses optimistic concurrency control (OCC) -- if a conflict is detected, the mutation is retried automatically.

### DarshJDB

DarshJDB transactions are backed by Postgres. The `set_triples()` method writes a batch of triples under a single transaction ID obtained from a Postgres sequence (`darshan_tx_seq`). Retraction (soft delete) sets `retracted = true` rather than physically deleting rows.

The client-side transaction API uses a Proxy-based builder:

```typescript
await transact(db, (tx) => {
  tx.users['user-123'].set({ name: 'Alice' });
  tx.users['user-123'].link('org', 'org-456');
  tx.messages[generateId()].set({ body: 'Hello', author: 'user-123' });
});
```

Operations are collected during the callback, then submitted atomically to `/api/mutate`.

### Comparison

| Aspect | Convex | DarshJDB |
|--------|--------|-----------|
| Isolation level | Serializable | Postgres default (read committed) |
| Conflict handling | Automatic OCC retry | Postgres row-level locks |
| Scope | Full mutation function | Batch of triple writes |
| Client API | Function-level (implicit) | Explicit `transact()` builder |
| Reads within tx | Yes (consistent snapshot) | No (writes only, reads are separate) |
| Automatic retry | Yes | No |

**Honest assessment:** Convex transactions are more powerful because mutation functions can read and write within the same serializable transaction. DarshJDB's transactions are write-only batches -- you cannot read a value, make a decision, and write based on it within a single atomic operation. This is a meaningful limitation for patterns like "transfer funds if balance sufficient."

The Postgres foundation gives DarshJDB a solid base to build on (Postgres serializable isolation is available), but the current implementation does not expose transactional reads to function code.

**Advantage: Convex.** More complete transaction model with automatic conflict resolution.

---

## 5. Type Safety

### Convex

Convex achieves end-to-end type safety through code generation. The schema definition produces TypeScript types that flow through queries, mutations, and client code. The `api` object provides fully typed function references. Changes to the schema immediately surface type errors in client code.

```typescript
// Convex: full type inference from schema to client
const messages = useQuery(api.messages.list, { channel: channelId });
// messages is typed as Array<{ body: string, author: Id<"users">, ... }>
```

### DarshJDB

DarshJDB schemas are inferred from data at runtime via `get_schema()`, which scans triple data to discover entity types, their attributes, value types, and reference relationships. The TypeScript SDK uses generic type parameters for queries:

```typescript
// DarshJDB: generic type parameter
const result = await db.query<User>('users').where('active', '=', true).exec();
// result.data is User[] -- but the User type is manually defined
```

### Comparison

| Aspect | Convex | DarshJDB |
|--------|--------|-----------|
| Type source | Schema definition (single source of truth) | Manual type definitions + runtime inference |
| End-to-end safety | Yes (schema -> query -> client) | Partial (client types are manual) |
| Code generation | Automatic | Not implemented |
| Runtime validation | Schema-enforced | Schema-optional |
| Refactoring safety | High (type errors cascade) | Low (string-based attribute names) |

**Honest assessment:** Convex's type safety story is best-in-class among BaaS platforms. DarshJDB's approach of generic type parameters provides some safety but relies on developers keeping types in sync manually. The triple store's string-based attribute names (`"email"`, `"name"`) mean typos become runtime errors, not compile-time errors.

**Advantage: Convex, clearly.** This is one of Convex's strongest selling points.

---

## 6. Self-Hosting

### Convex

Convex was cloud-only until they open-sourced parts of the backend. Self-hosting is now possible but is relatively new. The managed Convex service handles scaling, backups, monitoring, and upgrades. Self-hosted Convex requires operating their Rust backend, which has non-trivial operational complexity.

### DarshJDB

DarshJDB was designed for self-hosting from the start. The server is a single Rust binary that connects to Postgres. Docker Compose files are provided for both development and production. The operational requirements are minimal: Postgres + the DarshJDB binary.

```yaml
# DarshJDB docker-compose.yml -- that's the whole deployment
services:
  postgres:
    image: postgres:16
  darshjdb:
    build: .
    environment:
      DATABASE_URL: postgres://darshan:darshan@postgres:5432/darshjdb
```

### Comparison

| Aspect | Convex | DarshJDB |
|--------|--------|-----------|
| Managed cloud | Yes (primary offering) | Not yet |
| Self-hosted | Recently available | Day-one capability |
| Operational complexity | Higher (custom storage engine) | Lower (just Postgres) |
| Backup story | Managed (cloud) / manual (self-hosted) | Postgres pg_dump / streaming replication |
| Scaling | Managed auto-scaling | Postgres scaling (Patroni, read replicas) |
| Data sovereignty | Cloud regions | Full control |
| Vendor lock-in | Medium (proprietary runtime) | Low (Postgres underneath) |

**Honest assessment:** Self-hosting is genuinely where DarshJDB has a structural advantage. Building on Postgres means operators can use existing Postgres expertise, tooling, and infrastructure. Convex's custom storage engine means self-hosting it requires learning Convex-specific operations.

That said, DarshJDB's self-hosting advantage is somewhat offset by the fact that many features listed in the codebase are not yet wired end-to-end (real-time, functions, auth enforcement). Self-hosting a feature-incomplete system is less valuable than using Convex's fully working managed service.

**Advantage: DarshJDB architecturally, Convex practically.** DarshJDB wins on the self-hosting design; Convex wins on having a working product to host.

---

## 7. Developer Experience

### Convex

Convex's DX is widely praised in the developer community. Key DX features:

- `npx convex dev` starts a local dev server with hot reload
- Schema changes auto-generate types
- Dashboard with data browser, function logs, and deployment management
- Error messages include the exact query that failed with context
- Zero-config reactivity (just use `useQuery`)
- Comprehensive documentation with working examples

### DarshJDB

DarshJDB provides:

- CLI with `ddb dev` (compiles but does not auto-start Postgres)
- Admin dashboard (renders mock data, not connected to live API)
- Multi-framework SDKs (React, Angular, Next.js, Python, PHP)
- REST API with OpenAPI 3.1 spec generation
- QueryBuilder with fluent API
- Proxy-based transaction API

### Comparison

| Aspect | Convex | DarshJDB |
|--------|--------|-----------|
| Getting started | `npx convex dev` -- works | Requires manual Postgres setup |
| Hot reload | Automatic | Registry-based file watching (not wired) |
| Dashboard | Full-featured, live data | Exists but shows mock data |
| Error messages | Excellent (context-rich) | Standard Rust error types |
| Documentation | Comprehensive | Strategic docs exist, tutorials minimal |
| Framework SDKs | React (primary), React Native | React, Angular, Next.js, Python, PHP |
| CLI | Polished, single command | Compiles, limited functionality |

**Honest assessment:** Convex has one of the best developer experiences of any backend platform. Their investment in DX -- from error messages to documentation to the dashboard -- is evident. DarshJDB has broader language coverage (Python, PHP, Angular SDKs) but the DX of each individual SDK is less polished. The admin dashboard rendering mock data is a significant gap.

**Advantage: Convex, substantially.** DarshJDB's breadth (multi-language, multi-framework) is notable, but Convex's depth in TypeScript DX is hard to match.

---

## 8. What Convex Does Better

Being honest about where Convex is ahead:

1. **Reactivity that works today.** Convex's reactive queries are production-tested. DarshJDB's sync pipeline exists as separate components that are not connected.

2. **Deterministic function execution.** Patching `Date.now()` and `Math.random()` for deterministic caching is a clever engineering decision that DarshJDB has no equivalent for.

3. **End-to-end type safety.** Schema-to-client type inference eliminates an entire category of bugs. DarshJDB's string-based attribute names are a step backward.

4. **Transactional reads + writes.** Being able to read, decide, and write within one serializable transaction is fundamental. DarshJDB only batches writes.

5. **Onboarding experience.** `npx convex dev` to a working reactive backend in under 60 seconds. DarshJDB requires Postgres, environment variables, and understanding the triple store model.

6. **Battle-tested at scale.** Convex serves production traffic for real applications. DarshJDB has zero production deployments and explicitly states it is "not production-ready."

7. **Automatic conflict resolution.** OCC with automatic retry means developers never think about write conflicts. DarshJDB leaves this to Postgres defaults.

---

## 9. What DarshJDB Does Better

Where DarshJDB has genuine advantages:

1. **Data model flexibility.** The triple store can represent any relationship without schema migrations. Adding a field to an entity type is a single triple write, not a migration. For rapidly evolving schemas or graph-like data, this is a real advantage.

2. **Point-in-time reads.** `get_entity_at(entity_id, tx_id)` is built into the storage layer. Every mutation is append-only with a transaction ID. Convex has no equivalent for historical queries.

3. **Audit trail by design.** The append-only, retract-based deletion model means every state change is preserved. Combined with the Merkle audit trail table, this satisfies regulatory audit requirements that Convex cannot easily match.

4. **Multi-language SDKs.** Python, PHP, Angular, and Next.js SDKs alongside the TypeScript client. Convex is TypeScript-first and while there are community SDKs for other languages, they are not first-party. DarshJDB's PHP/Laravel and Python SDKs are first-party with 52 and 141 tests respectively.

5. **Self-hosted from day one.** No managed service dependency. Data stays on your infrastructure. The Postgres foundation means any Postgres-compatible tool (pgAdmin, pg_dump, Patroni) works without adaptation.

6. **No vendor lock-in.** Data is in Postgres tables. If DarshJDB disappears tomorrow, your data is still queryable with standard SQL. Convex data export requires their tooling.

7. **Bandwidth-efficient reactivity (when connected).** The diff-based push model sends only changed fields, not full result sets. For large datasets with small incremental changes, this is meaningfully more efficient.

8. **TTL support.** Built-in `expires_at` on triples with automatic retraction. Convex requires manual cleanup via scheduled functions.

9. **Security depth.** Row-level security rules compiled to SQL WHERE clauses, Argon2id password hashing, TOTP MFA with recovery codes, JWT RS256 with key rotation, rate limiting with SHA-256 hashed tokens. The security audit in CODE.md shows 12 vulnerabilities found and fixed -- evidence of proactive security review.

---

## 10. ConvexCompat Layer Evaluation

**File:** `packages/client-core/src/convex-compat.ts` (260 lines)

### What It Provides

The `ConvexCompat` class wraps a `DarshJDB` client instance and exposes Convex-style methods:

| ConvexCompat Method | Maps To | Convex Equivalent |
|--------------------|---------|--------------------|
| `query(table, filter, options)` | `QueryBuilder.exec()` | `db.query(table).collect()` |
| `mutation(table, data)` | `transact() -> tx[table][id].set()` | `db.insert(table, data)` |
| `patch(table, id, fields)` | `transact() -> tx[table][id].merge()` | `db.patch(id, fields)` |
| `remove(table, id)` | `transact() -> tx[table][id].delete()` | `db.delete(id)` |
| `watch(table, filter, cb)` | `QueryBuilder.subscribe()` | `useQuery()` |
| `id()` | `generateId()` | Auto-generated `_id` |

### Filter Translation

The compat layer translates Convex-style filter objects to DarshJDB `where()` calls:

```typescript
// Convex-style
{ done: false }                    // -> builder.where("done", "=", false)
{ priority: { $gt: 3 } }          // -> builder.where("priority", ">", 3)
{ status: { $in: ["a", "b"] } }   // -> builder.where("status", "in", ["a", "b"])
```

The operator mapping covers: `$eq`, `$ne`, `$gt`, `$gte`, `$lt`, `$lte`, `$in`, `$nin`, `$contains`, `$startsWith`.

### Quality Assessment

**Strengths:**
- Clean, well-documented code with JSDoc and examples
- Proper operator mapping with 10 operators covered
- Uses the real DarshJDB transaction API (not a parallel implementation)
- Watch/subscribe properly delegates to QueryBuilder subscriptions
- ID generation uses UUID v7 (time-ordered, same as Convex's IDs)

**Gaps:**
- **No index hints.** Convex's `.withIndex("by_channel", q => q.eq("channel", id))` has no equivalent. DarshJDB relies on Postgres query planning.
- **No pagination cursors.** Convex supports cursor-based pagination. ConvexCompat only has `limit`/`offset`.
- **No `replace` operation.** Convex's `db.replace(id, data)` fully replaces a document. ConvexCompat has `mutation` (insert) and `patch` (merge) but no full replace on existing documents.
- **No function references.** Convex's `api.messages.list` typed function references are not represented. ConvexCompat uses string table names.
- **No auth context forwarding.** Convex functions receive `ctx.auth` automatically. ConvexCompat does not thread auth context.
- **Watch relies on unfinished reactivity.** `watch()` delegates to `QueryBuilder.subscribe()`, which depends on the WebSocket subscription pipeline that is not wired end-to-end.

**Verdict:** The ConvexCompat layer is a reasonable starting point for teams migrating simple CRUD patterns. It covers the 80% case (queries, inserts, patches, deletes, live subscriptions) but misses the 20% that makes Convex powerful (typed function references, index-aware queries, cursor pagination, transactional reads). Teams with complex Convex usage will need manual migration for those patterns.

**Rating: 6/10** -- Functional for basic operations, insufficient for advanced Convex patterns.

---

## 11. Migration Script Evaluation

**File:** `scripts/migrate-from-convex.ts` (393 lines)

### What It Does

The script reads exported Convex data (directory of JSON files or a single combined JSON file) and imports it into DarshJDB via the REST API.

**Data flow:**
1. Parse CLI args (`--input`, `--url`, `--token`, `--batch-size`, `--dry-run`)
2. Load Convex export: directory of `{table}.json` files or single `{ table: [...] }` file
3. Convert each Convex document to a DarshJDB mutation op:
   - `_id` becomes the entity ID
   - `_creationTime` becomes `:db/createdAt`
   - All other fields become triple attributes
   - `:db/type` is set to the table name
4. Send in batches via `/api/mutate` or `/api/admin/bulk-load` (auto-detected)

**Fast path:** The script probes for `/api/admin/bulk-load` and uses UNNEST-based batch inserts (claimed 10-50x faster) when available, falling back to standard `/api/mutate` batches.

### Quality Assessment

**Strengths:**
- Dual input mode (directory or single file) handles both Convex export formats
- Automatic bulk-load detection with graceful fallback
- Dry-run mode for validation before actual migration
- Progress bar with percentage display
- Error reporting includes failed document IDs for debugging
- Batch size is configurable (default 100, auto-increased to 1000 for bulk-load)
- `:db/type` attribute preserves Convex table grouping

**Gaps:**
- **No reference resolution.** Convex document IDs like `j571hp2a5p6ej7vhq7bwfq1mzh6g2pxv` are used as-is. If DarshJDB expects UUID format for references, cross-document references will break. The script does not detect or convert reference fields.
- **No schema migration.** Convex schemas (validators, indexes) are not translated into DarshJDB equivalents. Only data is migrated.
- **No file storage migration.** Convex file storage (`_storage` table) is not handled.
- **No incremental migration.** Running the script twice will create duplicate entities. There is no idempotency check (e.g., "skip if entity already exists").
- **No nested document handling.** Convex documents can contain nested objects and arrays. These are stored as JSON blobs in a single triple rather than being decomposed into sub-entities.
- **No validation of imported data.** The script trusts the Convex export completely. Malformed documents will produce malformed triples.
- **No rollback mechanism.** If the migration fails midway, there is no cleanup of partially imported data.

**Verdict:** The script handles the straightforward case well: flat Convex documents with primitive fields migrated to DarshJDB triples. It falls short on reference resolution, schema translation, and operational safety (no idempotency, no rollback). A production migration would need additional tooling around this script.

**Rating: 5/10** -- Works for simple datasets, needs significant enhancement for production Convex migrations.

---

## Summary Table

| Dimension | Convex | DarshJDB | Winner |
|-----------|--------|-----------|--------|
| Data model familiarity | Documents (familiar) | Triples (unusual) | Convex |
| Schema flexibility | Rigid, safe | Flexible, risky | Depends on use case |
| Reactive queries | Working, production-tested | Designed, not connected | Convex |
| Server functions | Deterministic V8 isolates | Subprocess placeholder | Convex |
| Transactions | Serializable, read+write | Write-only batches | Convex |
| Type safety | End-to-end generated types | Manual generics | Convex |
| Self-hosting | Recently available | Day-one design | DarshJDB |
| Vendor lock-in | Medium | Low (Postgres) | DarshJDB |
| Multi-language | TypeScript-primary | TS, Python, PHP, Angular | DarshJDB |
| Audit/history | Not built-in | Append-only + point-in-time | DarshJDB |
| Production readiness | Yes | No (alpha) | Convex |
| DX polish | Excellent | Work in progress | Convex |
| Bandwidth efficiency | Full re-send | Delta diffs (when working) | DarshJDB (theoretical) |
| Security depth | Managed + auth providers | 12 CVEs found and fixed, MFA, RLS | DarshJDB |
| Migration path from Convex | N/A | ConvexCompat (6/10) + script (5/10) | Partial |

## When to Choose Convex

- You want a working product today, not a promising architecture
- TypeScript is your primary (or only) backend language
- You value DX above all else
- You are comfortable with a managed service
- Your data model is document-oriented
- You need production-grade reactivity now

## When to Choose DarshJDB

- Self-hosting is a hard requirement
- You need multi-language backend support (Python, PHP)
- Your data model is graph-like or requires constant schema evolution
- Audit trails and point-in-time reads are regulatory requirements
- You want to build on Postgres (existing team expertise, tooling, infrastructure)
- You are willing to contribute to an alpha project and grow with it
- Vendor independence matters more than immediate feature completeness

## Closing Note

This comparison is written honestly. Convex is ahead on most dimensions that matter for shipping products today. DarshJDB's architectural choices -- the triple store, append-only writes, diff-based reactivity, multi-language SDKs -- represent bets on flexibility and self-hosting that may pay off as the project matures. But "may pay off" is not the same as "works today."

The ConvexCompat layer and migration script demonstrate intent to provide a path for Convex users, but both need work before they can support non-trivial migrations. Teams considering DarshJDB should evaluate the current state of the code (CODE.md is unflinchingly honest about what works and what does not) rather than the roadmap.
