# DarshJDB vs PocketBase

A technical comparison of two single-binary, self-hosted Backend-as-a-Service platforms.

Both projects share a core thesis: developers should be able to run one binary on a cheap VPS and get a complete backend. The implementations diverge sharply in almost every other decision. This document examines those divergences honestly.

---

## At a Glance

| Dimension | PocketBase | DarshJDB |
|-----------|-----------|-----------|
| **Language** | Go | Rust |
| **Storage engine** | SQLite (embedded) | PostgreSQL 16+ (external) |
| **Data model** | Relational tables with migrations | Triple store (EAV) over Postgres |
| **Binary size** | ~25 MB | Larger (Rust static linking + dependencies) |
| **External dependencies** | None | PostgreSQL instance required |
| **Auth** | Email/password, OAuth2 (8+ providers) | Email/password, magic link, OAuth2 (Google, GitHub, Apple, Discord), MFA (TOTP, recovery codes) |
| **Real-time** | SSE-based subscriptions | WebSocket subscriptions with diff-based delta push |
| **File storage** | Built-in (local + S3) | Pluggable backends (local, S3, R2, MinIO), signed URLs, resumable uploads |
| **Admin UI** | Built-in, embedded in binary | Separate React + Vite dashboard |
| **Query language** | SQL (via REST filters) | DarshanQL (purpose-built for triple stores) |
| **Vector search** | Not supported | pgvector integration with auto-embedding pipeline |
| **Rule engine** | Not supported | Forward-chaining inference rules |
| **SDKs** | JS/TS (official), community Dart/Swift/Kotlin | TypeScript (React, Angular, Next.js), Python (FastAPI, Django), PHP (Laravel) |
| **GitHub stars** | 43K+ | New project |
| **Maturity** | Stable, v0.22+ | Alpha |

---

## 1. Architecture

### PocketBase: Go + SQLite

PocketBase compiles to a single Go binary with SQLite linked in. No external processes, no network dependencies. Copy the binary to a server, run it, and you have a working backend with a database file on disk.

This is genuinely elegant. The operational burden is near-zero. Backups are `cp pb_data/ somewhere/`. Deployment is `scp` and restart. SQLite is the most deployed database engine on Earth and is tested to a degree most software never approaches.

The Go runtime provides garbage collection, goroutine concurrency, and fast compilation. PocketBase can be extended as a Go framework --- import it as a library and add custom routes, middleware, and hooks in Go.

### DarshJDB: Rust + PostgreSQL

DarshJDB compiles to a single Rust binary but requires a running PostgreSQL instance. This is a fundamental tradeoff: you gain Postgres's full feature set (MVCC, concurrent writers, extensions, replication) at the cost of operational complexity.

The Rust binary runs on Axum + Tokio for async I/O. Memory safety is enforced at compile time without a garbage collector, which means no GC pauses under load. The type system catches entire classes of bugs before the binary exists.

The Postgres dependency means DarshJDB will never match PocketBase's "copy one file and run" experience. But it also means DarshJDB inherits decades of PostgreSQL investment: pg_dump, logical replication, pgvector, PostGIS, pg_cron, and the entire Postgres extension ecosystem.

**Honest assessment:** PocketBase wins on operational simplicity. DarshJDB wins on raw capability of the underlying storage engine. For a solo developer deploying to a $5 VPS, the difference matters. For a team with a managed Postgres instance (Neon, Supabase Postgres, RDS), the Postgres dependency is a non-issue.

---

## 2. Data Model

### PocketBase: SQL Tables

PocketBase uses a traditional relational model. You define collections (tables) through the admin UI or API. Each collection has typed fields --- text, number, bool, email, URL, date, file, relation, JSON, select. Schema changes require migrations, but PocketBase handles this through the admin UI with automatic ALTER TABLE operations.

This is familiar to any developer who has used SQL. The mental model is rows and columns. Queries filter on columns. Relations are foreign keys. The schema is explicit and enforced.

### DarshJDB: Triple Store (EAV)

Every piece of data in DarshJDB is a triple: `(entity_id, attribute, value)`. An "entity" is a collection of triples sharing the same ID. A "collection" is triples grouped by entity type.

```
(e_01, "name",  "Alice")
(e_01, "email", "alice@example.com")
(e_01, "team",  "e_02")          -- reference to another entity
(e_02, "name",  "Engineering")
```

This is fundamentally a knowledge graph stored in Postgres. The triples table has a `value_type` discriminator, transaction IDs for append-only history, TTL support via `expires_at`, and logical retraction instead of physical deletion.

**What the triple store gives you:**

- **Schema flexibility during development.** Write data first, add structure later. No migrations during prototyping. When ready for production, switch to strict mode.
- **Temporal queries.** Every triple has a `tx_id`. You can reconstruct the state of any entity at any point in time. PocketBase does not have built-in time-travel queries.
- **Natural graph traversal.** Relationships are just triples where the value is another entity ID. Graph queries (follow reference chains, find connected entities) are first-class operations, not JOIN gymnastics.
- **Forward-chaining rules.** When a triple is inserted, inference rules can fire automatically and produce derived triples in the same transaction. This enables computed fields, value propagation across references, and counter updates without application logic.

**What it costs you:**

- **Query performance on wide entities.** Reconstructing an entity with 20 attributes requires joining 20+ rows from the triples table, compared to a single row scan in a relational model. DarshJDB uses query plan caching and Postgres indexes to mitigate this, but the fundamental overhead exists.
- **Unfamiliar mental model.** Most developers think in rows and columns. The EAV model requires adjustment. DarshJDB's REST API abstracts this (you POST JSON objects, not raw triples), but the underlying model surfaces in query behavior and debugging.
- **Storage overhead.** Each attribute-value pair is a row with metadata (entity_id, attribute name, value_type, tx_id, timestamps). A 20-field entity produces 20 rows instead of one. Postgres handles this fine at moderate scale, but it is more storage than a normalized relational model.

**Honest assessment:** PocketBase's relational model is simpler, more familiar, and more storage-efficient for conventional CRUD. DarshJDB's triple store is more powerful for graph-shaped data, temporal queries, schema evolution, and automated inference --- but most applications do not need these capabilities. The triple store is a bet on a more expressive data model at the cost of simplicity.

---

## 3. Scalability

### PocketBase: SQLite Constraints

SQLite uses a single-writer model. Only one write can proceed at a time (WAL mode allows concurrent reads during writes, which PocketBase enables by default). This is fine for most indie projects and small teams.

The practical ceiling is well-documented: SQLite handles hundreds of thousands of reads per second and thousands of writes per second on modern hardware. For a BaaS serving a mobile app with 10K users, this is more than enough.

Horizontal scaling is not possible with SQLite. You cannot run two PocketBase instances against the same database. Vertical scaling (bigger machine) is the only option, and SQLite tops out when write contention becomes the bottleneck.

Backups and replication are file-level. Litestream can stream WAL changes to S3 for near-real-time backups, but this is append-only disaster recovery, not read replicas.

### DarshJDB: PostgreSQL Concurrency

PostgreSQL uses MVCC (Multi-Version Concurrency Control) with row-level locking. Multiple writers can operate concurrently on different rows without blocking each other. This is a fundamentally different concurrency model.

DarshJDB also implements Solana-inspired parallel query execution: non-conflicting batch operations are grouped into waves that execute concurrently via `tokio::join_all`, while conflicting operations (same entity type, at least one write) are serialized.

Postgres provides native horizontal read scaling via streaming replication. Write scaling requires more sophisticated approaches (Citus, partitioning), but the read-replica pattern covers most growth scenarios.

**Honest assessment:** For a solo developer's side project, SQLite's single-writer model is not a limitation --- and the operational simplicity is a genuine advantage. For an application that needs concurrent writes from multiple users modifying different data simultaneously, Postgres is architecturally superior. DarshJDB's scalability ceiling is much higher, but reaching that ceiling requires operational maturity that the target user may not have.

---

## 4. Real-Time

### PocketBase: Server-Sent Events

PocketBase uses SSE (Server-Sent Events) for real-time subscriptions. Clients subscribe to collection changes and receive notifications when records are created, updated, or deleted.

SSE is HTTP-based, works through most proxies and firewalls without configuration, and is simpler than WebSockets. The tradeoff is that SSE is one-directional (server to client only) and sends full record payloads on each change.

### DarshJDB: WebSocket Subscriptions with Diff Push

DarshJDB uses WebSocket connections for real-time sync. The architecture is more involved:

1. Clients subscribe to queries (not just collections).
2. When a mutation occurs, the `Broadcaster` identifies affected subscriptions via the `SubscriptionRegistry`.
3. The query is re-executed with the subscriber's permission context (row-level security applied).
4. A `DiffEngine` computes the minimal delta between the previous result set and the new one.
5. Only the diff is pushed to the client.

This means:

- **Bandwidth efficiency.** Diffs are smaller than full payloads, which matters on mobile or metered connections.
- **Permission-aware push.** Two users subscribed to the same query can receive different results because their permission rules differ. PocketBase also filters by collection rules, but DarshJDB's per-query subscription with RLS injection is more granular.
- **Presence system.** DarshJDB has a built-in presence engine (who's online, cursor positions, typing indicators) with auto-expiry and rate limiting (20 updates/sec per room). PocketBase does not have presence.

**Honest assessment:** PocketBase's SSE approach is simpler, requires no special proxy configuration, and is adequate for most use cases ("record changed, here's the new version"). DarshJDB's diff-based WebSocket system is more sophisticated and more bandwidth-efficient, but adds implementation complexity and requires WebSocket-compatible infrastructure. The presence system is a genuine differentiator for collaborative applications.

---

## 5. Authentication

### PocketBase

- Email/password with verification emails
- OAuth2 providers: Google, Facebook, GitHub, GitLab, Discord, Microsoft, Spotify, Kakao, Twitch, Strava, LiveChat, Gitea, Gitee, Patreon, mailcow, OpenID Connect (generic), Apple, Instagram, VK, Yandex
- API key authentication
- Admin accounts (separate from user accounts)
- Collection-level auth rules (expressions evaluated per-request)

The breadth of OAuth2 providers is impressive. The auth rules use a filter expression language evaluated at the collection level.

### DarshJDB

- Email/password (Argon2id: 64MB memory, 3 iterations, 4 parallelism --- OWASP recommended)
- Magic link authentication (32-byte tokens, hashed storage, 15-minute expiry, one-time use)
- OAuth2 providers: Google, GitHub, Apple, Discord (with PKCE mandatory, HMAC-signed state)
- MFA: TOTP with recovery codes, WebAuthn stubs
- JWT RS256 with refresh token rotation
- Device fingerprint binding on refresh tokens
- Rate limiting per-IP and per-user (token bucket)
- Row-level permission engine (rules stored as data, not config)

DarshJDB has fewer OAuth2 providers but deeper security features: MFA, device fingerprinting, refresh token rotation, and HMAC-signed OAuth state parameters. The permission engine stores rules as triples in the database, which means permissions are data --- queryable, versionable, and auditable.

**Honest assessment:** PocketBase has broader OAuth2 coverage. DarshJDB has deeper security primitives. For most indie projects, PocketBase's auth is more than sufficient. For applications where MFA, device binding, or auditable permission rules matter (enterprise, fintech, healthcare), DarshJDB's auth stack is more appropriate.

---

## 6. Developer Experience

### PocketBase

PocketBase is praised for good reason. The experience is:

1. Download a binary.
2. Run it.
3. Open `localhost:8090/_/` and you have an admin dashboard.
4. Create collections, define fields, set rules --- all from the UI.
5. Your API is live. Start calling it from your frontend.

No Docker. No database setup. No configuration files. No environment variables for basic operation. The SDK is a single npm package (`pocketbase`) that handles auth, CRUD, and real-time subscriptions.

The admin UI is polished and functional. You can manage data, view logs, configure auth settings, and test API calls --- all from the browser.

PocketBase can also be used as a Go framework: import it, add custom routes, and build your extended binary. This is a powerful pattern for developers who need custom server logic.

### DarshJDB

The current experience requires more setup:

1. Clone the repository.
2. Start PostgreSQL (Docker Compose provided).
3. Run the setup script.
4. Start the Rust server with a `DATABASE_URL` environment variable.
5. The admin dashboard is a separate React app.

This is not bad, but it is more steps than PocketBase. The Postgres dependency is the primary friction point.

Where DarshJDB's DX diverges is in SDK breadth. Five SDKs ship out of the box:

- **TypeScript core** + framework-specific packages for React (hooks), Angular (signals + RxJS), and Next.js (App Router + Pages Router + Server Components)
- **Python** with FastAPI and Django integration
- **PHP** with Laravel integration

Each SDK uses framework-native patterns. React gets `useQuery` and `useMutation` hooks. Angular gets signal-based reactivity and RxJS observables. Next.js gets Server Component support. This is more investment in the SDK layer than PocketBase, which provides a single JS client.

DarshanQL is a purpose-built query language with semantic search (`$semantic`), hybrid search (tsvector + pgvector via RRF), nested entity resolution, and full-text search. This is more expressive than PocketBase's filter syntax but has a learning curve.

**Honest assessment:** PocketBase has a better out-of-box experience for the first 10 minutes. DarshJDB has a more complete SDK story for production applications, especially polyglot teams using Python or PHP backends alongside JS frontends. The gap in initial setup friction is real and should not be minimized.

---

## 7. Community and Maturity

This is not a close comparison.

PocketBase was released in 2022 and has accumulated 43,000+ GitHub stars, an active Discord, extensive community tutorials, third-party SDKs (Dart, Swift, Kotlin, C#, Python), and production deployments. It has gone through 20+ releases with breaking changes handled via documented migration paths. The bus factor is low (primarily one developer, Gani Georgiev), but the codebase is clean and well-understood.

DarshJDB is a new project in alpha. It has 731 tests, comprehensive documentation, and multi-language SDK coverage, but no community yet. No production deployments. No third-party ecosystem.

**Honest assessment:** If community support and ecosystem maturity are deciding factors, PocketBase wins by a large margin. DarshJDB must earn its community through technical merit and reliability over time.

---

## 8. What PocketBase Does Better

### Zero-dependency deployment

One binary. No Docker. No database server. Copy to a VPS, run, done. This is the single most compelling feature of PocketBase and the hardest to replicate with a Postgres-backed architecture.

### SQLite portability

The entire database is a single file. Back it up by copying. Move it between machines by copying. Test locally with the exact same engine that runs in production. No `pg_dump`, no connection strings, no user management.

### Admin UI polish

PocketBase's admin dashboard is embedded in the binary, loads instantly, and provides a complete management interface. It is polished in a way that reflects years of iteration.

### Go extensibility

Import PocketBase as a Go library and add custom routes, middleware, and hooks. This is a clean extension model that doesn't require forking. The Go ecosystem provides access to a massive standard library and package ecosystem.

### Proven at scale (within its niche)

Thousands of indie developers have shipped production applications on PocketBase. The failure modes are known. The workarounds are documented. The community has answered the questions you will have.

### Simplicity as a feature

PocketBase does fewer things and does them well. It does not try to be a knowledge graph, a vector database, or a rule engine. For developers who need a backend for a mobile app or a SaaS dashboard, this focus is a virtue.

---

## 9. What DarshJDB Does Better

### PostgreSQL as a foundation

Everything Postgres gives you, DarshJDB inherits: MVCC concurrency, streaming replication, pgvector for embeddings, PostGIS for geospatial, full-text search with tsvector, JSONB operators, window functions, CTEs, and 30 years of performance optimization. SQLite is excellent, but Postgres operates at a different scale.

### Vector search and auto-embeddings

DarshJDB integrates pgvector for semantic search with an auto-embedding pipeline. Configure an OpenAI or Ollama endpoint, specify which attributes to embed, and vector embeddings are generated automatically when text triples are written. Queries support `$semantic` (vector similarity), `$search` (full-text), and `$hybrid` (RRF fusion of both).

PocketBase has no vector search capability.

### Triple store flexibility

The EAV model with transaction IDs enables:

- **Temporal queries**: Reconstruct any entity at any historical point.
- **Schema-free prototyping**: Write first, structure later.
- **Graph traversal**: Follow reference chains natively.
- **Append-only audit trail**: Every change is a new triple with a transaction marker. Nothing is physically deleted.

### Forward-chaining rule engine

Inspired by GraphDB's TRREE engine, DarshJDB's rule system fires when matching triples are inserted and produces inferred triples in the same transaction. Supported actions: computed attributes (concat, copy, literal), value propagation across references, and counter updates on related entities. Rules chain up to a configurable depth (default 3).

This enables patterns like "when a user is added to a team, automatically update the team's member_count" or "when an order's status changes to shipped, propagate the tracking number to the customer entity" --- without application code.

PocketBase has no equivalent.

### Multi-language SDK coverage

Official SDKs for TypeScript (React, Angular, Next.js), Python (FastAPI, Django), and PHP (Laravel). Each uses framework-native patterns. PocketBase's official SDK is JS-only; other language support comes from community packages of varying quality.

### Diff-based real-time sync

The WebSocket subscription system computes minimal diffs between query result snapshots and pushes only changes. Combined with per-subscriber permission filtering, this is more bandwidth-efficient and security-correct than SSE full-payload delivery.

### Presence system

Built-in room-based presence (who's online, cursor positions, typing indicators) with auto-expiry and rate limiting. This is essential for collaborative applications (document editing, chat, multiplayer) and absent from PocketBase.

### Parallel query execution

Solana-inspired wave scheduling groups non-conflicting batch operations for concurrent execution. Read-only queries on different entity types always parallelize. This is relevant for batch-heavy API calls.

### Deeper auth primitives

MFA with TOTP and recovery codes, device fingerprint binding, HMAC-signed OAuth state, mandatory PKCE, and permission rules stored as auditable data.

---

## 10. Target Audience

### PocketBase is for:

- **Indie developers** shipping a mobile app or SaaS product who want a backend running in 30 seconds.
- **Prototypers** who need a complete backend for a hackathon or MVP.
- **Small teams** (1-5 developers) who value operational simplicity over feature depth.
- **Go developers** who want an extensible backend framework.
- **Projects where the data model is conventional CRUD** (users, posts, comments, orders).

### DarshJDB is for:

- **Developers building on graph-shaped or highly relational data** where the triple store model is a natural fit (knowledge management, CRM, ERP, social graphs, content management with complex taxonomies).
- **Applications that need vector/semantic search** integrated with their transactional data (AI-powered search, recommendation engines, RAG pipelines).
- **Teams that need Postgres** --- because they already run it, because they need its extension ecosystem, or because they need concurrent write throughput beyond SQLite's ceiling.
- **Polyglot backend teams** using Python, PHP, and TypeScript together who want first-class SDK support in all three.
- **Applications requiring temporal queries** (audit trails, compliance, versioned data, undo/redo).
- **Collaborative applications** that need presence, diff-based sync, and permission-aware real-time updates.
- **Developers willing to trade initial setup simplicity for a more powerful foundation** and who plan to grow into that power.

---

## Choosing Between Them

Use this decision tree:

```
Do you need vector search or semantic/hybrid queries?
  YES --> DarshJDB
  NO  -->

Is your data naturally graph-shaped (entities referencing entities, traversal queries)?
  YES --> DarshJDB
  NO  -->

Do you need concurrent writes from many users modifying different data?
  YES --> DarshJDB (Postgres MVCC) 
  NO  -->

Do you need Python or PHP SDKs as first-class citizens?
  YES --> DarshJDB
  NO  -->

Do you need temporal queries or an append-only audit trail?
  YES --> DarshJDB
  NO  -->

Do you need to be running in production this week with community support?
  YES --> PocketBase
  NO  -->

Is zero-dependency deployment (no Docker, no Postgres) critical?
  YES --> PocketBase
  NO  -->

Are you comfortable with alpha software and willing to contribute?
  YES --> DarshJDB
  NO  --> PocketBase
```

---

## An Honest Summary

PocketBase is a mature, polished, community-proven BaaS that solves the "I need a backend for my app" problem with minimal friction. It has earned its 43K stars through genuine developer experience quality.

DarshJDB is an alpha-stage project that makes a different architectural bet: PostgreSQL instead of SQLite, triple store instead of relational tables, diff-based WebSockets instead of SSE, forward-chaining rules instead of application-level hooks. These choices trade simplicity for power in specific dimensions.

If PocketBase fits your use case, it is the safer choice today. It is more stable, better documented by the community, and battle-tested in production.

If your application needs what DarshJDB offers --- vector search, graph data, temporal queries, multi-language SDKs, Postgres-grade concurrency, inference rules --- then DarshJDB addresses a gap that PocketBase does not fill. But you are adopting alpha software, and that comes with the risks that word implies.

The goal is not to replace PocketBase. The goal is to serve the developers who have outgrown SQLite's concurrency model, who need more than relational tables, or who are building AI-native applications where vector search is not optional. Different tools for different problems.
