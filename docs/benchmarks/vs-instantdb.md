# DarshanDB vs InstantDB: A Technical Comparison

Both DarshanDB and InstantDB store application data in a **triple store** (Entity-Attribute-Value model) on top of PostgreSQL. Both offer declarative relational queries from the client, real-time subscriptions, and optimistic mutations. DarshanQL is directly inspired by InstantDB's InstaQL.

This document is an honest, detailed comparison written from the perspective of someone who has read both codebases. Where InstantDB is better, we say so.

---

## 1. Data Model

Both systems reject the traditional "define a schema, run migrations, then write data" workflow. Instead, data is written as triples and schema emerges from usage.

### InstantDB

InstantDB stores triples in PostgreSQL using three core tables:

- **`triples`** -- `(entity_id, attribute_id, value, ea, av, vae)` with separate index columns for each access pattern (EA = entity-attribute lookup, AV = attribute-value scan, VAE = value-attribute-entity reverse lookup).
- **`attrs`** -- A catalog of known attributes with forward/reverse labels, cardinality (one/many), and value type.
- **`idents`** -- Human-readable namespace/name pairs mapped to internal integer IDs.

InstantDB normalizes attribute names into integer IDs and stores index orderings as separate columns rather than relying on composite B-tree indexes. This is a Datomic-influenced design: attributes are first-class entities with their own metadata.

Schema is defined in code via `i.graph()`:

```typescript
const graph = i.graph(
  {
    todos: i.entity({ title: i.string(), done: i.boolean() }),
    users: i.entity({ name: i.string(), email: i.string().unique() }),
  },
  { todosOwner: { forward: { on: "todos", has: "one", label: "owner" },
                   reverse: { on: "users", has: "many", label: "todos" } } }
);
```

### DarshanDB

DarshanDB uses a single `triples` table with typed value columns:

```sql
CREATE TABLE triples (
    entity_id   UUID NOT NULL,
    attribute   TEXT NOT NULL,          -- stored as string, not integer ID
    value_type  SMALLINT NOT NULL,      -- 0=null, 1=bool, 2=int, 3=float, 4=string, 5=json, 6=ref, 7=blob
    value_bool  BOOLEAN,
    value_int   BIGINT,
    value_float DOUBLE PRECISION,
    value_str   TEXT,
    value_json  JSONB,
    value_ref   UUID,
    namespace   TEXT NOT NULL DEFAULT 'default',
    version     BIGINT NOT NULL DEFAULT 0,
    retracted   BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at  TIMESTAMPTZ,            -- TTL support
    PRIMARY KEY (entity_id, attribute, version)
);
```

Key differences:

| Aspect | InstantDB | DarshanDB |
|--------|-----------|-----------|
| Attribute storage | Integer IDs via `attrs` table | Raw text strings |
| Value storage | Single JSONB `value` column | Typed columns (`value_bool`, `value_int`, etc.) |
| Index strategy | Composite columns (`ea`, `av`, `vae`) | Per-type partial indexes |
| Versioning | Transaction-based (Datomic style) | Version counter + soft-delete (`retracted`) |
| TTL support | No | Yes (`expires_at` column) |
| Entity typing | Via attribute metadata in `attrs` | Via `:db/type` attribute triple |

**Assessment:** InstantDB's integer-ID attribute normalization saves storage on wide datasets and makes attribute renames free. DarshanDB's typed value columns avoid JSONB comparison overhead for numeric filters (comparing `value_int >= 3` is faster than `value::jsonb >= '3'::jsonb`). Both are valid engineering tradeoffs. InstantDB's design is more faithful to Datomic's information model; DarshanDB's is more PostgreSQL-native.

---

## 2. Query Language

Both query languages use nested JSON objects where keys are entity types and values are query clauses. DarshanQL was directly inspired by InstaQL.

### Side-by-Side Syntax

**Fetch all todos:**

```typescript
// InstaQL (InstantDB)
db.useQuery({ todos: {} })

// DarshanQL (DarshanDB)
db.useQuery({ todos: {} })
```

Identical.

**Filter with conditions:**

```typescript
// InstaQL
db.useQuery({ todos: { $: { where: { done: false, priority: { $gt: 3 } } } } })

// DarshanQL
db.useQuery({ todos: { $where: { done: false, priority: { $gt: 3 } } } })
```

InstaQL nests filters under `$` -> `where`. DarshanQL uses `$where` directly. Both support `$gt`, `$gte`, `$lt`, `$lte`, `$ne`, `$in`.

**Nested relations:**

```typescript
// InstaQL
db.useQuery({ users: { todos: {} } })

// DarshanQL
db.useQuery({ users: { todos: {} } })
```

Identical syntax. Both resolve references and return nested objects.

**Ordering and pagination:**

```typescript
// InstaQL
db.useQuery({ todos: { $: { order: { serverCreatedAt: "desc" }, limit: 10 } } })

// DarshanQL
db.useQuery({ todos: { $order: { createdAt: 'desc' }, $limit: 10, $offset: 0 } })
```

InstaQL groups all query modifiers under `$`. DarshanQL uses top-level `$order`, `$limit`, `$offset`, `$after` (cursor-based pagination).

### Capability Comparison

| Feature | InstaQL | DarshanQL |
|---------|---------|-----------|
| Basic filters (`$where`) | Yes | Yes |
| Comparison operators | `$gt`, `$gte`, `$lt`, `$lte`, `$ne` | Same set |
| Set operators (`$in`, `$nin`) | `$in` | `$in`, `$nin` |
| String operators | `$like` | `$contains`, `$startsWith`, `$endsWith` |
| Logical operators (`$or`, `$and`, `$not`) | `$or`, `$and`, `$not` | `$or`, `$and`, `$not` |
| Nested relations | Yes | Yes |
| Reverse relations | `_` prefix | `_` prefix |
| Offset pagination | `limit` + `offset` | `$limit` + `$offset` |
| Cursor pagination | `startCursor`, `endCursor` | `$after` cursor |
| Ordering | `order` | `$order` (including nested field sort) |
| Full-text search | No (planned) | `$search` (PostgreSQL tsvector) |
| Vector/semantic search | No | `$semantic` (pgvector cosine distance) |
| Hybrid search (text + vector) | No | `$hybrid` (RRF fusion) |
| Aggregations (`count`, `sum`, `avg`) | No | `$aggregate` with `$groupBy` |
| Multi-entity queries | Yes | Yes |
| Query complexity limits | Not documented | Configurable depth, result, and op limits |

**Assessment:** DarshanQL is a superset of InstaQL. The core relational query syntax is nearly identical (intentionally so), but DarshanDB adds full-text search, vector search, hybrid search, aggregations, and groupBy that InstantDB does not have. InstantDB's query language is cleaner in its minimalism -- everything under one `$` key is arguably more elegant. DarshanDB trades that elegance for more query power.

---

## 3. Mutations

**Both use a transactional builder pattern:**

```typescript
// InstantDB
db.transact(tx.todos[id()].update({ title: "Buy milk", done: false }))

// DarshanDB
db.transact(db.tx.todos[db.id()].set({ title: "Buy milk", done: false }))
```

```typescript
// InstantDB
db.transact(tx.todos[id].merge({ done: true }))

// DarshanDB
db.transact(db.tx.todos[id].merge({ done: true }))
```

```typescript
// InstantDB
db.transact(tx.todos[id].delete())

// DarshanDB
db.transact(db.tx.todos[id].delete())
```

```typescript
// InstantDB
db.transact(tx.users[userId].link({ todos: todoId }))

// DarshanDB
db.transact(db.tx.users[userId].link({ todos: todoId }))
```

The mutation APIs are nearly identical. Both support batch transactions (array of operations executed atomically) and optimistic updates with server reconciliation.

**Assessment:** Parity. DarshanDB's mutation API is a direct port of InstantDB's, with the same semantics.

---

## 4. Real-Time Architecture

### InstantDB

InstantDB uses a **Reactive Datalog** approach. Queries are compiled into a set of pattern dependencies. When a triple is written, the server identifies which active subscriptions are affected and pushes novelty (new facts) to clients. The client-side Datalog engine re-evaluates the query locally using the updated fact set.

Key characteristics:
- Client-side query evaluation (the client holds a local Datalog database)
- Server pushes raw triple changes, not pre-computed diffs
- Client re-derives query results from its local triple set
- This enables offline query evaluation without server round-trips

### DarshanDB

DarshanDB uses a **server-side dependency tracker** with delta diff compression:

1. When a query is registered, the `DependencyTracker` records which `(attribute, value_constraint)` pairs the query depends on.
2. When triples change, `get_affected_queries()` returns the set of query IDs whose results may have changed.
3. The server re-executes affected queries (with permission filters) and computes a minimal diff (`added`, `updated`, `removed` arrays).
4. Only the diff is pushed to the client over WebSocket (MsgPack-encoded).

Key characteristics:
- Server-side query re-evaluation (client receives pre-computed results)
- Push delta diffs, not raw triples
- Dependency tracking uses an inverted index for O(1) lookup of affected queries
- Client cache is IndexedDB, updated from diffs

### Comparison

| Aspect | InstantDB | DarshanDB |
|--------|-----------|-----------|
| Where queries re-evaluate | Client (Datalog) | Server (SQL) |
| What gets pushed | Raw triple novelty | Computed delta diffs |
| Client complexity | Higher (local Datalog engine) | Lower (apply diffs to cache) |
| Server load per mutation | Lower (just broadcast triples) | Higher (re-execute affected queries) |
| Offline query capability | Full (local Datalog can answer new queries) | Partial (cache has results, not a queryable database) |
| Permission enforcement | Mixed (server filters what triples to send) | Pure server-side (results always filtered before sending) |
| Bandwidth efficiency | Depends on query selectivity | High (98% reduction vs polling per DarshanDB docs) |

**Assessment:** InstantDB's approach is architecturally more elegant and enables richer offline capabilities. DarshanDB's approach is simpler to implement correctly (especially around permissions -- the server always sees the full picture) and produces smaller payloads for complex queries. For most applications, the difference is invisible to end users.

---

## 5. Self-Hosting

This is the clearest differentiator.

### InstantDB

- **Cloud-only.** Runs on InstantDB's managed infrastructure.
- No self-hosting option. No way to run on your own servers.
- Data lives on InstantDB's cloud. You depend on their uptime, pricing, and data residency.
- There is an open-source client SDK, but the server is proprietary.

### DarshanDB

- **Self-hosted by design.** Single binary + PostgreSQL. That's it.
- Docker Compose for one-command deployment.
- Horizontal scaling: run N instances behind a load balancer, all pointing at the same PostgreSQL.
- Full control over data residency, backups, encryption at rest, network isolation.
- Can run air-gapped. No phone-home. No license server.
- Monitoring via Prometheus `/metrics` endpoint + Grafana dashboards.

**Assessment:** If you need to own your data, run in a regulated environment, operate in a specific geographic region, or simply refuse to depend on a startup's cloud for your production database, DarshanDB is the only option. This is not a minor difference -- it is the fundamental reason DarshanDB exists.

---

## 6. Authentication

### InstantDB

- Email/password with magic links
- Google OAuth
- Custom auth (bring your own JWT)
- Guest access (anonymous users)
- No MFA
- No session management UI

### DarshanDB

- Email/password (Argon2id hashing, breach-password rejection)
- Magic links
- OAuth (Google, GitHub, Apple, Discord)
- TOTP MFA (Google Authenticator, Authy) with recovery codes
- Session management (list, revoke, revoke-all)
- Device fingerprint binding on refresh tokens
- Account lockout (5 attempts / 30 minutes)
- Custom JWT claims
- Framework integrations (Next.js middleware, Angular service)

**Assessment:** DarshanDB's auth is significantly more complete. InstantDB's is intentionally minimal -- they expect you to use it alongside external auth providers. DarshanDB ships a production-grade auth system because self-hosted users cannot rely on a separate auth SaaS being available.

---

## 7. Permissions

### InstantDB

InstantDB uses a CEL-like permissions language defined in the dashboard or in code:

```typescript
{
  todos: {
    allow: { read: "auth.id == data.creatorId" },
    bind: ["isOwner", "auth.id == data.creatorId"],
  }
}
```

- Row-level permissions via expression evaluation
- `bind` for reusable permission variables
- Evaluated server-side before sending triples to clients

### DarshanDB

```typescript
export default {
  todos: {
    read: (ctx) => ({ userId: ctx.auth.userId }),
    create: (ctx) => !!ctx.auth,
    update: (ctx) => ({ userId: ctx.auth.userId }),
    delete: (ctx) => ctx.auth.role === 'admin',
  },
  users: {
    read: {
      allow: true,
      fields: {
        email: (ctx, entity) => entity.id === ctx.auth.userId,
        passwordHash: false,
      },
    },
  },
};
```

- Row-level security via filter objects injected as SQL WHERE clauses
- Field-level permissions (hide specific attributes per role)
- Full JavaScript functions, not expression strings
- Role hierarchy support
- Multi-tenant isolation patterns
- Permission test helpers
- Debug logging

**Assessment:** DarshanDB's permission system is more powerful -- field-level permissions and filter-object injection are features InstantDB lacks. InstantDB's expression-based approach is more constrained but arguably safer (no arbitrary code execution in the permission layer). Both work well for typical use cases. DarshanDB's advantage grows with complex multi-tenant scenarios.

---

## 8. What InstantDB Does Better

Let's be honest about where InstantDB wins.

### TypeScript Type Inference

InstantDB's `i.graph()` schema definition produces fully typed query results. When you write `db.useQuery({ todos: {} })`, TypeScript knows the shape of every todo. This is best-in-class developer experience, comparable to Prisma's type generation.

DarshanDB does not have schema-driven type inference at the query level. You get `data.todos` as a generic type unless you manually annotate.

### Client-Side Datalog Engine

InstantDB's client holds a full triple database locally and can evaluate queries offline without server round-trips. This is architecturally superior for offline-first applications. DarshanDB's client cache stores query results, not a queryable database.

### Developer Experience Polish

InstantDB has had more engineering time devoted to DX polish:
- Instant CLI for project setup
- Dashboard for managing apps, viewing data, editing permissions
- Comprehensive React hooks with TypeScript generics
- Error messages tuned for common mistakes

DarshanDB has an admin dashboard and CLI, but the overall DX is less polished.

### YC Backing and Community

InstantDB is YC-backed with full-time engineers, a growing community, and the resources of a funded startup. DarshanDB is a solo open-source project. This matters for long-term maintenance, ecosystem growth, and third-party integrations.

### Battle-Testing

InstantDB is running production workloads for paying customers. DarshanDB is new. There is no substitute for production traffic when it comes to finding edge cases in a database.

---

## 9. What DarshanDB Does Better

### Self-Hosting

The obvious one. If your data cannot leave your infrastructure, InstantDB is not an option. DarshanDB is a single binary.

### Vector Search

DarshanDB has native `$semantic` and `$hybrid` search built into the query language, backed by pgvector. This enables AI/ML features (semantic search, RAG, embeddings) at the database level. InstantDB has no vector search capability.

### Full-Text Search

DarshanDB exposes PostgreSQL's tsvector full-text search via `$search` in DarshanQL. InstantDB does not have full-text search.

### Aggregations

DarshanDB supports `$aggregate` with `count`, `sum`, `avg`, `min`, `max`, and `$groupBy` directly in queries. InstantDB requires you to fetch all data and aggregate client-side.

### Server Functions

DarshanDB runs TypeScript functions in sandboxed V8 isolates (Deno Core) on the server: queries, mutations, actions, scheduled (cron), and internal functions. Each isolate has CPU time limits (30s), memory limits (128MB), network allowlists, and SSRF protection.

InstantDB does not have server-side functions. Business logic runs either on the client or in your own separate backend.

### Forward-Chaining Rules

DarshanDB includes a rule engine inspired by GraphDB's TRREE: when triples are inserted, rules can fire to produce derived triples in the same transaction. This enables computed attributes, value propagation, and counter updates without application code. InstantDB has no equivalent.

### Multi-Language SDKs

DarshanDB ships Python and PHP SDKs alongside JavaScript/TypeScript. InstantDB is JavaScript/TypeScript only.

### Presence System

DarshanDB has a built-in real-time presence system (rooms, peers, cursor tracking, typing indicators). InstantDB has presence via their `room` primitive, so this is roughly at parity -- though DarshanDB's is built into the core server rather than being a separate service.

### Wire Protocol

DarshanDB uses MsgPack over WebSocket by default (28% smaller than JSON), with HTTP/2 + MsgPack for SSR and REST + JSON as a fallback. InstantDB uses JSON over WebSocket.

### Authentication Depth

As covered above: MFA, session management, device binding, account lockout, breach-password rejection, custom claims. InstantDB's auth is minimal by comparison.

---

## 10. Is "The Self-Hosted InstantDB" a Fair Positioning?

Yes and no.

**Yes, because:**
- The data model is the same (EAV triple store on PostgreSQL)
- The query language is intentionally similar (DarshanQL is inspired by InstaQL)
- The mutation API is nearly identical
- The core value proposition overlap is real: both give you a reactive, relational database with a declarative client-side query language
- The primary differentiator is deployment model: cloud vs self-hosted

**No, because:**
- DarshanDB has features InstantDB does not: vector search, full-text search, aggregations, server functions, forward-chaining rules, MFA, multi-language SDKs
- DarshanDB lacks features InstantDB has: TypeScript type inference, client-side Datalog, production battle-testing, funded team
- The real-time architectures are fundamentally different (server-side re-evaluation vs client-side Datalog)
- "Self-hosted X" implies X-but-you-run-it-yourself. DarshanDB is not a drop-in replacement for InstantDB. You cannot take an InstantDB app and point it at DarshanDB without code changes.

**A more precise positioning:**

> DarshanDB is a self-hosted reactive database that shares InstantDB's core insight -- EAV triple stores are the right foundation for real-time client-centric applications -- but extends it with vector search, server functions, and the ability to run on your own infrastructure.

The "self-hosted InstantDB" shorthand is useful for initial understanding. It tells people the right mental model. But DarshanDB should grow beyond that comparison. The vector search, rule engine, and server functions are capabilities that make DarshanDB a different product, not just a self-hosted clone.

---

## Summary Table

| Dimension | InstantDB | DarshanDB |
|-----------|-----------|-----------|
| Data model | EAV triple store (Postgres) | EAV triple store (Postgres) |
| Query language | InstaQL | DarshanQL (inspired by InstaQL) |
| Mutations | Transactional builder | Transactional builder (same API) |
| Real-time | Client-side Datalog re-evaluation | Server-side dependency tracking + delta diffs |
| Self-hosting | No | Yes (single binary + Postgres) |
| Full-text search | No | Yes (tsvector) |
| Vector search | No | Yes (pgvector) |
| Aggregations | No | Yes (count, sum, avg, min, max, groupBy) |
| Server functions | No | Yes (V8 isolates, 5 function types) |
| Forward-chaining rules | No | Yes |
| Auth | Basic (email, Google, custom JWT) | Full (email, OAuth x4, MFA, sessions, lockout) |
| Permissions | Row-level (expression strings) | Row-level + field-level (JS functions) |
| TypeScript inference | Yes (best-in-class) | No |
| Offline queries | Full (local Datalog) | Partial (cached results) |
| SDKs | JavaScript/TypeScript | JS/TS, Python, PHP |
| Wire protocol | JSON over WebSocket | MsgPack over WebSocket (+ HTTP/2, REST) |
| Presence | Yes (rooms) | Yes (rooms + peers) |
| Admin dashboard | Yes (cloud) | Yes (self-hosted) |
| Backing | YC-funded startup | Open-source solo project |
| Production maturity | Running customer workloads | Early stage |

---

*This comparison was written by examining DarshanDB's source code (`query/mod.rs`, `triple_store/mod.rs`, `sync/mod.rs`, `rules/mod.rs`) and InstantDB's public documentation and open-source client. Last updated: April 2026.*
