# DarshanDB vs Neon: A Honest Technical Comparison

Neon and DarshanDB occupy different layers of the backend stack. Neon is a serverless Postgres provider -- it makes PostgreSQL cheaper, more elastic, and more developer-friendly. DarshanDB is a Backend-as-a-Service -- it replaces the entire backend layer between your frontend and your database.

Comparing them directly is like comparing Heroku Postgres to Firebase. They overlap in that they both store data, but the problems they solve are fundamentally different.

This document examines where they genuinely compete, where they complement each other, and where each one wins decisively.

---

## 1. Positioning

| | Neon | DarshanDB |
|---|---|---|
| **Category** | Serverless Postgres hosting | Self-hosted BaaS |
| **Core promise** | Postgres that scales to zero and branches instantly | A single binary that replaces your entire backend |
| **Target user** | Teams that want managed Postgres without managing servers | Teams that want auth + permissions + real-time + data without writing backend code |
| **Business model** | Cloud service (free tier + paid plans) | Open-source (MIT), self-hosted |
| **Replaces** | RDS, Cloud SQL, self-managed Postgres | Firebase, Supabase, Convex, custom backend APIs |

Neon competes with Amazon RDS, Google Cloud SQL, PlanetScale, and CockroachDB. DarshanDB competes with Firebase, Supabase, Convex, and InstantDB.

The overlap is narrow: both involve PostgreSQL, and both serve application developers. But Neon gives you a better database. DarshanDB gives you a backend that happens to use a database internally.

---

## 2. Data Model

### Neon: Standard Relational SQL

Neon is Postgres. Full stop. You define tables with `CREATE TABLE`, enforce foreign keys, write SQL or use an ORM, and get the full power of PostgreSQL's type system, indexing, and query planner.

```sql
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    email TEXT UNIQUE NOT NULL,
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE todos (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID REFERENCES users(id),
    title TEXT NOT NULL,
    done BOOLEAN DEFAULT false
);

SELECT u.name, t.title
FROM users u
JOIN todos t ON t.user_id = u.id
WHERE u.email = 'alice@example.com';
```

### DarshanDB: Triple Store (EAV) over Postgres

DarshanDB stores data as `(entity_id, attribute, value)` triples. Collections are logical groupings. Relationships are triples where the value references another entity. Queries use DarshanQL, a purpose-built language that compiles down to SQL internally.

```bash
# Write data -- no schema required
POST /api/data/users
{"name": "Alice", "email": "alice@example.com"}

# Read it back -- DarshanQL handles the triple-to-entity reconstruction
GET /api/data/users?where=email:eq:alice@example.com
```

### Comparison for App Development

| Dimension | Neon (SQL) | DarshanDB (Triples) |
|---|---|---|
| **Schema flexibility** | Rigid -- schema defined upfront, migrations required | Flexible -- write first, enforce later with strict mode |
| **Query power** | Full SQL with CTEs, window functions, recursive queries | DarshanQL covers common CRUD patterns; complex analytics require workarounds |
| **JOIN performance** | Native, optimized by Postgres query planner | Entity reconstruction from triples adds overhead; graph traversals are natural |
| **Rapid prototyping** | Slower -- schema design before data | Faster -- data before schema |
| **Production strictness** | Inherent -- schema is the contract | Opt-in -- strict mode locks the model |
| **Tooling ecosystem** | Decades of SQL tooling, ORMs, migration frameworks | Purpose-built SDKs, admin dashboard |

**Honest assessment:** For analytics, reporting, and complex relational queries, Neon (standard Postgres) wins. For rapid application development where the schema is evolving daily, DarshanDB's schema-later approach removes friction. The triple store trades query expressiveness for development velocity.

---

## 3. Serverless Architecture

### Neon: Scale to Zero

Neon's defining feature is compute/storage separation. The compute layer (Postgres process) can scale to zero when idle and cold-start in ~500ms when a connection arrives. You pay nothing when nobody is querying.

Architecture:
- **Pageserver**: Stores data pages, serves them on demand
- **Safekeeper**: WAL durability layer (3-node quorum)
- **Compute**: Ephemeral Postgres instances, scale 0 to N

This is genuinely valuable for:
- Dev/staging environments that sit idle 90% of the time
- Side projects with sporadic traffic
- Multi-tenant platforms with thousands of low-traffic databases

### DarshanDB: Always-On Binary

DarshanDB is a single Rust binary (Axum + Tokio) that starts and stays running. It maintains WebSocket connections for real-time subscriptions, holds JWT signing keys in memory, and keeps permission rule caches warm.

There is no scale-to-zero. The binary needs to be running to serve requests.

### Tradeoffs

| Factor | Neon (serverless) | DarshanDB (always-on) |
|---|---|---|
| **Idle cost** | $0 (scales to zero) | $5-10/month minimum (VPS + Postgres) |
| **Cold start latency** | ~500ms on first connection | None -- already running |
| **WebSocket support** | Not applicable (raw Postgres wire protocol) | Native -- real-time subscriptions require persistent process |
| **Connection handling** | Built-in pooling via proxy | Application-level connection pool |
| **Burst scaling** | Auto-scales compute up to limits | Manual -- single binary, single machine (horizontal scaling planned) |

**Honest assessment:** If your workload is bursty or idle most of the time, Neon's serverless model saves real money. DarshanDB's always-on model is the correct tradeoff for real-time applications where WebSocket connections must persist -- you cannot scale a subscription server to zero without disconnecting every client.

---

## 4. Database Branching

Neon's branching creates instant, copy-on-write clones of your entire database. A branch shares pages with its parent until writes diverge. This enables:

- **Preview environments**: Every PR gets its own database branch with production-like data
- **Testing**: Run destructive tests against a branch, discard it
- **Development**: Each developer works against an isolated branch
- **Point-in-time recovery**: Branch from any moment in the WAL history

DarshanDB has no branching. To create a test environment, you stand up a second instance with a separate Postgres database and seed it.

### How Important Is This?

Branching is a workflow feature, not a data feature. It matters enormously for teams with:
- CI/CD pipelines that need isolated database state per test run
- Multiple developers working on schema changes simultaneously
- Preview deployments (Vercel-style) that need matching data

It matters less for:
- Solo developers or small teams
- Applications where test data is generated programmatically
- Projects using DarshanDB's schema-later model (no migrations to branch around)

**Honest assessment:** Branching is one of Neon's strongest differentiators in the Postgres hosting market. DarshanDB doesn't need it as urgently because its triple store model avoids the migration-heavy workflow that makes branching essential in traditional SQL. But it would still be useful, and its absence is a gap.

---

## 5. Features Neon Lacks (That DarshanDB Provides)

Neon is a database. DarshanDB is a backend. The feature gap reflects that difference:

| Feature | Neon | DarshanDB |
|---|---|---|
| **Authentication** | None -- bring your own auth service | Built-in: signup, signin, JWT (RS256), refresh tokens, Argon2id password hashing |
| **Row-level permissions** | Postgres RLS exists, but you write and maintain the policies yourself | Declarative permission rules stored as data, evaluated on every request automatically |
| **Real-time subscriptions** | None -- Postgres LISTEN/NOTIFY exists but no client-side push | WebSocket subscriptions with diff-based push, permission-filtered per user |
| **Client SDKs** | None -- use any Postgres client library | React hooks, Angular signals, Next.js (App + Pages Router), Python (FastAPI/Django), PHP (Laravel) |
| **Admin dashboard** | Neon Console (database management) | Application-level admin UI showing live data, collections, users, permissions |
| **Server functions** | None -- Postgres functions exist (PL/pgSQL) | User-defined functions with API surface (V8 runtime in progress) |
| **File storage** | None | S3-compatible storage API (designed, not yet shipped) |

To build a complete application backend on Neon, you still need:
- An auth service (Auth0, Clerk, Lucia, or custom)
- A backend framework (Express, Fastify, Django, Rails)
- An ORM or query builder
- WebSocket infrastructure for real-time
- Permission logic in your application code
- An admin interface

DarshanDB ships all of this in the binary.

---

## 6. Features DarshanDB Lacks (That Neon Provides)

| Feature | DarshanDB | Neon |
|---|---|---|
| **Scale to zero** | No -- always-on binary | Yes -- compute shuts down when idle |
| **Compute/storage separation** | No -- single process, standard Postgres | Yes -- independent scaling of compute and storage |
| **Database branching** | No | Yes -- instant copy-on-write branches |
| **Autoscaling compute** | No -- single binary, fixed resources | Yes -- auto-adjusts compute units based on load |
| **Point-in-time recovery** | Standard Postgres PITR (if configured) | Built-in, branch from any WAL position |
| **Read replicas** | Not yet | Yes -- with regional placement |
| **Connection pooling proxy** | Application-level | Infrastructure-level (PgBouncer integrated) |
| **Postgres extensions** | Whatever your local Postgres supports | Curated set of supported extensions |
| **Multi-region** | Not yet | Available on paid plans |

**Honest assessment:** DarshanDB's infrastructure story is early-stage. It runs on one machine, on one Postgres instance, with no auto-scaling. For a BaaS used by individual developers or small teams building applications, this is fine. For a platform serving thousands of concurrent users across regions, you would need to put significant engineering effort into scaling -- or use a managed database like Neon underneath.

---

## 7. Performance Characteristics

### Neon

Neon runs actual Postgres with a modified storage layer. Query performance is essentially Postgres performance, with one caveat: the first query after a cold start incurs latency while compute spins up (~500ms) and pages are fetched from the pageserver over the network (vs. local disk in traditional Postgres).

For hot workloads, Neon performs within 5-15% of self-managed Postgres. The gap comes from network-attached storage versus local NVMe.

### DarshanDB

DarshanDB adds layers on top of Postgres:

1. **Triple store reconstruction**: A query for "all users" requires reconstructing entities from individual `(entity, attribute, value)` rows. A user with 5 fields requires 5 rows read and assembled. This is inherently more work than `SELECT * FROM users`.

2. **Permission evaluation**: Every request evaluates row-level rules. This adds latency proportional to rule complexity.

3. **DarshanQL compilation**: Queries are parsed, planned, and compiled to SQL. This adds a constant overhead per query.

4. **WebSocket diff computation**: For subscriptions, the server computes diffs between previous and current state. This is additional work that Neon never does (Neon doesn't do subscriptions).

### Estimated Overhead

| Operation | Neon (raw Postgres) | DarshanDB | Overhead Source |
|---|---|---|---|
| Simple read (1 entity, 5 fields) | ~1ms | ~3-5ms | Triple reconstruction + permission check |
| Bulk read (1000 entities) | ~5ms | ~15-30ms | 5000 triples reassembled + batch permission evaluation |
| Write (1 entity) | ~2ms | ~4-8ms | Triple decomposition + permission check + subscription notification |
| Complex join (3 tables) | ~5-10ms | ~20-50ms | Graph traversal through triples vs. native Postgres JOIN |

These are estimated ranges based on architectural analysis, not published benchmarks. DarshanDB's benchmark suite against Firebase, Supabase, and Convex is on the roadmap but not yet completed.

**Honest assessment:** DarshanDB will always be slower than raw Postgres for equivalent queries. The triple store abstraction, permission system, and real-time engine all add overhead. The question is whether that overhead matters -- for most CRUD applications, the difference between 3ms and 1ms is invisible to users. For analytical workloads processing millions of rows, use Postgres directly.

---

## 8. When to Use Neon

Choose Neon when:

- **You need raw Postgres.** Your team writes SQL, uses ORMs, and wants full control over schema design, indexing strategy, and query optimization.

- **Your workload is bursty or idle.** Dev environments, staging databases, side projects, or multi-tenant platforms where most databases are idle most of the time. Scale-to-zero saves real money.

- **You need database branching.** Your CI/CD pipeline requires isolated database state per PR, or your team needs instant clones for development.

- **You already have a backend.** You have a Django/Rails/Express/FastAPI application and you need a better Postgres host, not a replacement for your backend code.

- **You need advanced SQL features.** CTEs, window functions, recursive queries, materialized views, full-text search with custom dictionaries, PostGIS for geospatial.

- **You need multi-region or read replicas.** Neon supports these at the infrastructure level.

---

## 9. When to Use DarshanDB

Choose DarshanDB when:

- **You need a complete backend, not just a database.** Auth, permissions, real-time subscriptions, SDKs, admin dashboard -- all from a single binary. No glue code.

- **You want to self-host.** Run on a $5 VPS, your own hardware, or air-gapped infrastructure. No cloud dependency, no vendor lock-in, no monthly database bill.

- **You're building a real-time application.** Chat, collaborative editing, live dashboards, multiplayer features. DarshanDB's WebSocket subscription engine pushes diffs to clients as data changes.

- **Your schema is evolving rapidly.** Early-stage products where the data model changes daily. DarshanDB's triple store lets you write data first and formalize the schema when you're ready.

- **You want client-side SDKs that handle the plumbing.** React hooks, Angular signals, Next.js integration -- optimistic updates, auth state management, and subscription lifecycle handled by the SDK.

- **You need row-level permissions without writing Postgres RLS policies.** DarshanDB's permission rules are declarative and evaluated automatically.

---

## 10. Can They Work Together?

### DarshanDB on Top of Neon Postgres

DarshanDB uses PostgreSQL 16+ as its storage engine. There is no hard coupling to a specific Postgres deployment -- any Postgres instance accessible via `DATABASE_URL` works.

Running DarshanDB against a Neon Postgres instance is technically feasible:

```bash
DATABASE_URL=postgres://user:password@ep-cool-name-123456.us-east-2.aws.neon.tech/neondb \
  cargo run --bin darshandb-server
```

### What You'd Gain

- **Neon's branching for DarshanDB data.** Branch the underlying Neon database to create instant snapshots of your entire DarshanDB state -- entities, permissions, user accounts, everything.
- **Neon's storage scaling.** Neon handles storage growth, compaction, and durability. DarshanDB focuses on the application layer.
- **Neon's PITR.** Recover DarshanDB to any point in time via Neon's WAL-based recovery.
- **Neon's read replicas.** If DarshanDB adds read-replica-aware connection routing, Neon could serve read traffic from regional replicas.

### What You'd Lose

- **Scale-to-zero becomes irrelevant.** DarshanDB is always-on. The Neon compute can't sleep because DarshanDB maintains a persistent connection pool. You'd be paying for always-on Neon compute, eliminating the cost advantage.
- **Latency.** Network hop between DarshanDB (your server) and Neon (their cloud) adds latency to every triple operation. Co-locating in the same region helps but doesn't eliminate it.
- **Self-hosted simplicity.** One of DarshanDB's selling points is running everything on a single machine. Adding a cloud dependency for the database layer undermines that.

### Verdict

DarshanDB on Neon works for teams that want DarshanDB's BaaS features but prefer managed Postgres over self-managed Postgres. The combination makes the most sense when:

1. You're already using Neon for other services
2. You want Neon's branching for DarshanDB development workflows
3. You don't mind the network latency and cost of always-on compute

For most DarshanDB users, running Postgres locally (or via Docker) alongside the DarshanDB binary on the same machine is simpler, cheaper, and faster.

---

## Summary Table

| Dimension | Neon | DarshanDB |
|---|---|---|
| **What it is** | Serverless Postgres | Self-hosted BaaS |
| **Data model** | Relational SQL | Triple store (EAV) over Postgres |
| **Auth** | None | Built-in (Argon2id + JWT RS256) |
| **Permissions** | Postgres RLS (manual) | Declarative, automatic, per-request |
| **Real-time** | None | WebSocket subscriptions with diff push |
| **Client SDKs** | None | React, Angular, Next.js, Python, PHP |
| **Serverless** | Yes (scale to zero) | No (always-on) |
| **Branching** | Yes (instant CoW) | No |
| **Self-hosted** | No (cloud service) | Yes (single binary) |
| **Open source** | Yes (core engine) | Yes (MIT) |
| **Maturity** | Production-ready | Alpha |
| **Query language** | Full SQL | DarshanQL (purpose-built) |
| **Scaling** | Auto-scaling compute, read replicas | Single binary, single machine (horizontal planned) |
| **Price (idle)** | Free tier available | $5+/month (VPS + Postgres) |
| **Best for** | Teams that need managed Postgres | Teams that need a complete backend |

---

## The Honest Bottom Line

Neon and DarshanDB are not competitors. They solve different problems at different layers.

If you need a database, use Neon (or Supabase, or RDS, or self-managed Postgres). If you need a backend, use DarshanDB (or Firebase, or Convex, or Supabase).

The comparison becomes interesting only at the margins: Neon + your own auth + your own API + your own WebSocket layer + your own permission system will eventually approximate what DarshanDB gives you out of the box. The question is whether you want to build that yourself or use a system designed for it.

DarshanDB is alpha. Neon is production-hardened. That gap matters today and will matter less over time. Choose based on what your project needs now, not what either project promises for the future.
