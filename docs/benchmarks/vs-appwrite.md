# DarshJDB vs Appwrite

A technical comparison between two self-hosted Backend-as-a-Service platforms. Both are open-source, both let you own your data. The engineering philosophies could not be more different.

**Last updated:** April 2026
**DarshJDB version:** 0.1.0 (alpha)
**Appwrite version:** 1.6.x (stable)

---

## Executive Summary

Appwrite is a mature, Docker-based BaaS with 14+ SDKs, a large community, and built-in services for nearly every backend concern. It is the safe choice for teams that want a self-hosted Firebase alternative today.

DarshJDB is an alpha-stage Rust binary that bets on a different architecture: a triple store over PostgreSQL, a single process instead of a container fleet, and real-time by default rather than bolted on. It is not production-ready. It is worth understanding why it exists.

| Dimension | Appwrite | DarshJDB |
|-----------|----------|-----------|
| **Language** | PHP (Utopia framework) + Node.js workers | Rust (Axum + Tokio) |
| **Database** | MariaDB | PostgreSQL 16 + pgvector |
| **Data model** | Document collections (JSON) | Triple store (EAV) over Postgres |
| **Deployment** | Docker Compose (10+ containers) | Single binary + Postgres |
| **Real-time** | WebSocket via Realtime service | WebSocket with diff-based push |
| **Auth providers** | 30+ (email, phone, OAuth, anonymous) | 4 (password, magic link, OAuth2, TOTP MFA) |
| **SDKs** | 14+ platforms | 6 (React, Angular, Next.js, PHP, Python, cURL) |
| **Functions** | 14 runtimes (Node, Python, Deno, etc.) | Node/Deno subprocess (alpha) |
| **Storage** | Built-in with image transforms | Local FS + S3-compatible (alpha) |
| **Messaging** | Email, SMS, push notifications | Not implemented |
| **Status** | Production-stable, well-funded | Alpha, solo developer |

---

## 1. Architecture

### Appwrite: Microservice Container Fleet

Appwrite runs as a Docker Compose stack with 10+ containers:

```
appwrite                  # Main API (PHP/Swoole)
appwrite-realtime         # WebSocket server (Node.js)
appwrite-worker-*         # 8+ worker containers (webhooks, functions, mails, etc.)
appwrite-executor         # Function execution orchestrator
mariadb                   # Primary database
redis                     # Cache + pub/sub
traefik                   # Reverse proxy + TLS
```

Each service has its own process, its own memory footprint, its own failure mode. A default Appwrite installation consumes 1.5-3 GB of RAM idle. The architecture is operationally complex but well-understood: each concern lives in its own container, can be scaled independently (in theory), and communicates through Redis pub/sub.

The PHP core uses the Utopia framework (written by the Appwrite team). It is synchronous, process-per-request PHP behind Swoole's event loop. The real-time service is a separate Node.js process that subscribes to Redis channels and pushes to WebSocket clients.

### DarshJDB: Single Binary

DarshJDB is one Rust binary linked against Tokio (async runtime) and Axum (HTTP framework). Everything runs in a single process:

```
ddb-server          # HTTP + WebSocket + auth + query engine + sync
postgres                  # External dependency
```

Docker Compose for DarshJDB has two containers: the server and Postgres. Memory footprint idle is approximately 40-80 MB for the server process plus whatever Postgres needs. The entire system runs on a 1 GB VPS.

The trade-off is clear. Appwrite's architecture gives you service isolation and mature operational patterns. DarshJDB's architecture gives you simplicity and resource efficiency. A single process means a single point of failure but also a single thing to monitor, deploy, and debug.

**What this means in practice:**

| Concern | Appwrite | DarshJDB |
|---------|----------|-----------|
| Minimum RAM | ~1.5 GB | ~256 MB (server + Postgres) |
| Containers to manage | 10+ | 2 |
| Inter-service communication | Redis pub/sub | In-process channels (tokio::broadcast) |
| Cold start | 30-60s (all services) | 1-3s (binary + Postgres connection) |
| Horizontal scaling | Per-service scaling | Not yet supported |
| Service isolation | Strong (container boundaries) | None (single process) |

---

## 2. Database and Data Model

### Appwrite: MariaDB Documents

Appwrite stores data in MariaDB as JSON documents within collections. You define collections with typed attributes (string, integer, float, boolean, email, URL, enum, etc.) through the console or API. Indexes are created explicitly. Relationships between collections are supported (one-to-one, one-to-many, many-to-one, many-to-many).

The query interface uses Appwrite's own `Query` class with methods like `Query.equal()`, `Query.greaterThan()`, `Query.search()`. It maps to SQL internally. You do not write SQL. You do not have direct MariaDB access by default.

Appwrite enforces a 50 attribute limit per collection (configurable) and a ~16 MB document size limit from MariaDB. Nested documents are not natively supported (you use relationships instead).

### DarshJDB: Triple Store over PostgreSQL

DarshJDB stores everything as (entity_id, attribute, value) triples in a single PostgreSQL table. There are no collections in the traditional sense. A "collection" is just a query filter on entity type. There are no schemas to define before writing data.

```sql
-- What Appwrite stores as a document:
-- { "name": "Alice", "email": "alice@example.com", "role": "admin" }

-- DarshJDB stores as triples:
-- (e_01, "name",  "Alice")
-- (e_01, "email", "alice@example.com")
-- (e_01, "role",  "admin")
```

The query engine (DarshanQL) is a purpose-built JSON query language that compiles to SQL:

```json
{
  "from": "users",
  "where": { "role": "admin" },
  "order": [{ "field": "name", "direction": "asc" }],
  "limit": 10
}
```

This compiles to self-joins across the triples table with appropriate indexes.

The triple store has real advantages. Schema changes require no migrations. References between entities are first-class (a triple's value can point to another entity). The data model is inherently a knowledge graph. With pgvector, semantic search is built into the same query engine.

It also has real disadvantages. Self-joins across a single table are inherently slower than querying a purpose-built relational table. Complex queries with many attributes require many joins. The query planner must work harder. At scale (millions of triples), careful indexing and query planning are essential.

**Honest comparison:**

| Capability | Appwrite | DarshJDB |
|-----------|----------|-----------|
| Schema enforcement | Required upfront | Optional (strict mode available) |
| Migration story | Console + API | Not needed (add attributes freely) |
| Relationship modeling | Explicit (4 types) | Implicit (value references) |
| Full-text search | Built-in | Built-in (tsvector) |
| Vector / semantic search | Not built-in | Built-in (pgvector) |
| Hybrid search (text + vector) | No | Yes (Reciprocal Rank Fusion) |
| Nested queries | Via relationships | Via nested query resolution |
| Raw SQL access | No | Postgres is right there |
| Query performance at scale | Mature, well-optimized | Unproven at scale |
| Maximum document/entity size | ~16 MB | Postgres limits (effectively unlimited) |

---

## 3. Real-Time

### Appwrite

Appwrite's real-time is a separate Node.js service (appwrite-realtime) that subscribes to Redis pub/sub channels. Clients connect via WebSocket and subscribe to "channels" like `databases.{db}.collections.{col}.documents` or `account`. When a document changes, the API server publishes an event to Redis, the realtime service picks it up, checks permissions, and pushes to subscribed clients.

The event is the full document (not a diff). The client receives the entire updated document and replaces its local state. This is simple and reliable but bandwidth-intensive for large documents with small changes.

Appwrite supports subscribing to specific document IDs, entire collections, or account events. Permission checking happens at the realtime service level using the same permission model as the REST API.

### DarshJDB

DarshJDB's real-time is in-process. The sync engine uses `tokio::broadcast` channels (no Redis hop). When a mutation occurs in the triple store, a `ChangeEvent` is emitted containing the affected entity IDs and attributes. The broadcaster:

1. Identifies which active subscriptions might be affected (via dependency tracking on entity types and attributes).
2. Re-executes the subscription's query with the subscriber's permission context.
3. Computes a diff (added, removed, updated entities with field-level patches) against the cached result.
4. Pushes only the delta to the client.

This is architecturally more sophisticated. The diff engine (`sync/diff.rs`) uses hash-based change detection to avoid deep comparisons. The `EntityPatch` type carries only changed fields, not the full entity. For applications with large entities and frequent small updates, this saves significant bandwidth.

The trade-off: DarshJDB's approach re-executes queries on every mutation that touches relevant entities. At high write throughput with many active subscriptions, this could become a bottleneck. Appwrite's approach is simpler (publish full document) but scales more predictably because it does not re-run queries.

**Comparison:**

| Aspect | Appwrite | DarshJDB |
|--------|----------|-----------|
| Transport | WebSocket | WebSocket |
| Event granularity | Full document | Field-level diff |
| Subscription model | Channel-based (collection, document, account) | Query-based (arbitrary filters) |
| Permission enforcement | At push time | At query re-execution time |
| Infrastructure | Separate Node.js service + Redis | In-process (tokio channels) |
| Bandwidth efficiency | Sends full document | Sends minimal delta |
| Presence tracking | Not built-in | Built-in (per-room, auto-expiry) |
| Pub/sub (custom channels) | Not built-in | Built-in (keyspace notifications) |

---

## 4. Authentication

### Appwrite

Appwrite's auth is comprehensive. 30+ providers out of the box:

- Email + password
- Phone (SMS OTP via Twilio, Vonage, etc.)
- Magic URL (email link)
- Anonymous sessions
- OAuth2: Google, GitHub, Apple, Discord, Facebook, Microsoft, Spotify, Slack, and 20+ more
- JWT (custom tokens for server-side auth)
- Teams and team memberships
- Roles and labels
- Session management with device tracking
- Account recovery (email-based)
- Email verification
- Password history (configurable)
- Personal data access / GDPR compliance endpoints

This is the most complete auth system in any open-source BaaS. Period. It handles edge cases that most teams do not think about until production: account enumeration prevention, brute-force lockout, session limits per user, and MFA.

### DarshJDB

DarshJDB's auth covers the fundamentals:

- **Password**: Argon2id with OWASP parameters (64 MB memory, 3 iterations, parallelism 4). This is actually stronger default hashing than Appwrite's bcrypt.
- **Magic link**: 32-byte random token, hashed before storage, 15-minute expiry, one-time use.
- **OAuth2**: Trait-based provider abstraction with implementations for Google, GitHub, Apple, Discord. PKCE mandatory, HMAC-signed state parameter.
- **MFA**: TOTP (RFC 6238) with +/-1 step window, 10 recovery codes (Argon2id-hashed), WebAuthn stubs.
- **JWT**: RS256 for production (asymmetric key verification), HS256 for development. 15-minute access tokens, 7-day refresh tokens with rotation.
- **Rate limiting**: Token bucket per IP and per user.
- **Session management**: Device fingerprinting, session revocation.

What DarshJDB does NOT have:
- Phone/SMS authentication
- Anonymous sessions
- Teams/organizations/memberships
- Account recovery flows
- Email verification flows
- Password history
- GDPR compliance endpoints
- 20+ of Appwrite's OAuth providers

**The honest take:** Appwrite's auth is production-grade. DarshJDB's auth has strong cryptographic foundations (better password hashing, better JWT defaults) but lacks the breadth of flows that real applications need. A startup building a B2B SaaS would hit DarshJDB's auth limitations within weeks.

---

## 5. Server Functions

### Appwrite

Appwrite Functions supports 14 runtimes: Node.js (multiple versions), Python, PHP, Ruby, Dart, Swift, Kotlin, Java, .NET, Bun, Deno, Go, C++. Functions are deployed as code packages, built in isolated containers using the Open Runtime engine, and executed in Docker containers with resource limits.

Triggers include: HTTP endpoints, scheduled (cron), event-based (document created, user signed up, etc.), and manual invocation. Functions have access to the Appwrite SDK for server-side operations (reading/writing databases, managing users, sending messages).

The function build and execution pipeline is battle-tested. Thousands of open-source projects depend on it. Cold starts vary by runtime (Node.js: 1-3s, Python: 2-5s) but warm invocations are fast.

### DarshJDB

DarshJDB's function system exists at the architecture level but is not production-ready. The code shows:

- **Registry** (`functions/registry.rs`): File-system-based discovery of `.ts`/`.js` files with hot reload via the `notify` crate. Function metadata (kind, arguments, schedule) parsed from file conventions.
- **Runtime** (`functions/runtime.rs`): A `RuntimeBackend` trait with a `ProcessRuntime` implementation that spawns Node.js or Deno subprocesses. Resource limits (CPU timeout, memory limit, concurrency semaphore) are defined but enforcement depends on OS-level controls.
- **Scheduler** (`functions/scheduler.rs`): Cron-based scheduling with distributed locking.
- **Validator** (`functions/validator.rs`): Argument validation against declared schemas.

Function kinds defined: `query`, `mutation`, `action`, `scheduled`, `http_endpoint`. This mirrors Convex's function model, which is a good design.

What is missing:
- No embedded V8 isolate (subprocess-based only, meaning cold start on every invocation)
- No function build pipeline
- No multi-runtime support (Node/Deno only, no Python/Go/Ruby/etc.)
- No managed dependency installation
- Not tested at scale

**Comparison:**

| Capability | Appwrite | DarshJDB |
|-----------|----------|-----------|
| Runtimes | 14 | 2 (Node.js, Deno) |
| Execution model | Docker container | OS subprocess |
| Cold start | 1-5s (warm: <100ms) | Every invocation is cold |
| Triggers | HTTP, cron, events, manual | HTTP, cron, events (planned) |
| Resource isolation | Container-level | OS-level (ulimit) |
| Dependency management | Built-in (npm, pip, etc.) | Manual |
| Function build pipeline | Yes | No |
| Server-side SDK access | Full Appwrite SDK | ctx.db / ctx.auth (planned) |

---

## 6. SDKs

### Appwrite

14+ SDKs generated from OpenAPI specs:
- **Web**: JavaScript/TypeScript
- **Mobile**: Flutter, Android (Kotlin), iOS (Swift), React Native
- **Server**: Node.js, Python, PHP, Ruby, Dart, Kotlin, Swift, .NET, Go

SDK quality is consistent because they are code-generated. They cover all API endpoints. The web SDK includes a `Realtime` class for WebSocket subscriptions. TypeScript types are complete.

The trade-off of code generation: SDKs feel like API wrappers rather than framework-native integrations. The React experience is "use the JS SDK inside useEffect" rather than purpose-built hooks.

### DarshJDB

6 SDKs, hand-written for framework-native patterns:

- **@darshjdb/client-core**: Framework-agnostic TypeScript core (HTTP client, WebSocket client, auth state, query builder).
- **@darshjdb/react**: `useQuery`, `useMutation`, `useAuth`, `usePresence`, `useStorage` hooks. Follows React conventions (Suspense-compatible, automatic refetch on focus).
- **@darshjdb/angular**: Signals + RxJS observables via `DarshanService`. Angular-native DI and change detection.
- **@darshjdb/nextjs**: Server Components support, App Router + Pages Router, server-side data fetching with client hydration.
- **darshjdb-php**: Composer package with Laravel service provider, Blade directives, Eloquent-like query builder.
- **darshjdb-python**: Sync + async client, FastAPI dependency injection, Django integration.

The SDK count is lower, but each SDK is designed for its framework rather than generated from a spec. The React SDK does not require `useEffect` boilerplate. The Angular SDK uses signals, not callbacks. The Next.js SDK understands server components.

**Comparison:**

| Dimension | Appwrite | DarshJDB |
|-----------|----------|-----------|
| Total SDKs | 14+ | 6 |
| Mobile SDKs | Flutter, Android, iOS, React Native | None |
| Server SDKs | Node, Python, PHP, Ruby, Dart, Kotlin, Swift, .NET, Go | Python, PHP |
| Generation method | OpenAPI codegen | Hand-written |
| Framework integration depth | Wrapper-style | Framework-native |
| TypeScript types | Generated (complete) | Hand-written (complete) |
| React hooks | Community (not official) | First-class |
| Published to registries | Yes (npm, pip, etc.) | Not yet |
| Test coverage | SDK-level tests | 92 TS + 141 Python + 52 PHP |

---

## 7. Self-Hosting Complexity

### Appwrite

```bash
docker run -it --rm \
  --volume /var/run/docker.sock:/var/run/docker.sock \
  --volume "$(pwd)"/appwrite:/usr/src/code/appwrite:rw \
  --entrypoint="install" \
  appwrite/appwrite:1.6.0
```

This runs an interactive installer that generates a `docker-compose.yml` with all 10+ services. Requirements: Docker, Docker Compose, at least 2 GB RAM (4 GB recommended). The installer asks for domain, SMTP settings, and other configuration.

Upgrades involve pulling new images and running a migration command. The team maintains upgrade guides between versions. Breaking changes are documented.

The operational burden is real. Monitoring 10+ containers, managing Redis and MariaDB, handling Docker networking, TLS termination via Traefik. For small teams or solo developers, this is significant overhead.

### DarshJDB

```bash
# Option 1: Binary
DATABASE_URL=postgres://user:pass@localhost:5432/darshjdb \
  ./ddb-server

# Option 2: Docker Compose (2 containers)
docker compose up -d
```

DarshJDB needs Postgres. That is the only external dependency. The binary is statically linked Rust. The Docker Compose file has two services: the server and `pgvector/pgvector:pg16`. Total RAM footprint: 256 MB is comfortable.

What DarshJDB does NOT provide yet:
- One-line install script (`curl | sh`)
- Pre-built binaries for download
- Published Docker images on Docker Hub
- Automatic TLS termination
- Built-in reverse proxy

**The honest take:** Appwrite is harder to run but gives you everything out of the box. DarshJDB is trivial to start but leaves TLS, reverse proxy, backups, and monitoring to you. For a $5 VPS running a side project, DarshJDB's simplicity wins. For a production SaaS with a team, Appwrite's completeness wins.

---

## 8. What Appwrite Does Better

These are areas where Appwrite is objectively ahead and DarshJDB has no near-term path to matching.

### Maturity and Stability

Appwrite has been in development since 2019, is backed by venture funding, has a full-time team, and is used in production by thousands of projects. DarshJDB has 731 tests and zero production deployments. This gap does not close with clever architecture.

### SDK Ecosystem

14+ SDKs including mobile (Flutter, Android, iOS, React Native) versus 6 web/server SDKs. Mobile SDKs are essential for many projects. DarshJDB has no mobile story.

### Messaging

Appwrite 1.5+ includes built-in messaging: email (SMTP, Mailgun, SendGrid), SMS (Twilio, Vonage, Textmagic), and push notifications (FCM, APNS). DarshJDB has no messaging capability. Building this from scratch is months of work.

### Teams and Organizations

Appwrite has first-class team management: create teams, invite members, assign roles, team-level permissions on resources. DarshJDB has user-level permissions but no team/organization abstraction. For B2B SaaS, this is a hard requirement.

### Console UI

Appwrite's web console is polished, mature, and covers all operations: database management, user management, function deployment, storage browsing, real-time monitoring. DarshJDB's admin dashboard exists but covers basic data viewing only.

### Geographic Deployment

Appwrite Cloud offers multiple regions. Self-hosted Appwrite can run anywhere Docker runs. DarshJDB has no horizontal scaling, no multi-region support, and no managed cloud offering.

### Community and Documentation

Appwrite has 40k+ GitHub stars, active Discord, comprehensive docs, video tutorials, and a marketplace. DarshJDB has documentation but no community yet.

---

## 9. What DarshJDB Does Better

These are areas where DarshJDB's architecture provides genuine advantages, not just theoretical ones.

### Resource Efficiency

A DarshJDB instance (server + Postgres) runs comfortably in 256 MB RAM. An Appwrite instance needs 1.5-3 GB minimum. For developers in regions where cloud costs matter (India, Southeast Asia, Africa, South America), running on a $5 VPS versus a $20 VPS is the difference between building and not building.

This is not a minor point. Appwrite's Docker fleet makes self-hosting expensive for individuals and small teams, which is the exact audience that self-hosted BaaS is supposed to serve.

### PostgreSQL as Foundation

Appwrite uses MariaDB. It works, but it limits what the database layer can do. DarshJDB uses PostgreSQL, which means:

- **pgvector**: Semantic search and vector similarity built into the query engine. No external service needed. Appwrite has no vector search capability.
- **Full-text search**: PostgreSQL's tsvector/tsquery is more capable than MariaDB's FULLTEXT indexes.
- **JSON operations**: PostgreSQL's jsonb operators are more mature.
- **Extensions**: PostGIS for geospatial, pg_trgm for fuzzy matching, pg_cron for scheduling, TimescaleDB for time series. The extension ecosystem is a multiplier.
- **Direct access**: You can always connect to Postgres directly with psql, run analytics queries, set up logical replication, or use any tool in the Postgres ecosystem. Appwrite's MariaDB is internal and not designed for direct access.

### Triple Store / Knowledge Graph

The EAV data model is genuinely different from document storage. Advantages:

- **Schema flexibility**: Add any attribute to any entity without migrations. Useful during rapid prototyping.
- **Relationship modeling**: Entity references are natural (a triple's value points to another entity ID). No need to define relationship types upfront.
- **Graph queries**: The triple model maps directly to knowledge graph patterns. SPARQL-like queries over your application data are architecturally possible.
- **Temporal queries**: Every triple has a transaction ID. Historical state reconstruction is inherent to the model. Appwrite tracks document history via audit logs but does not support temporal queries.
- **Append-only semantics**: Triples are never updated, only retracted and re-asserted. This provides a built-in audit trail.

### Diff-Based Real-Time

DarshJDB pushes field-level diffs instead of full documents. For a 50-field entity where one field changes, DarshJDB sends the one changed field. Appwrite sends all 50 fields. Over a WebSocket connection with hundreds of subscriptions and frequent updates, this difference is significant.

DarshJDB also includes built-in presence tracking (who is online in a room) and pub/sub (custom event channels), which Appwrite does not provide natively.

### Embedding Pipeline

DarshJDB has a built-in auto-embedding pipeline. When text triples are written, embeddings are generated via OpenAI or Ollama and stored in pgvector. This enables semantic search (find similar items), hybrid search (combine text + vector ranking via Reciprocal Rank Fusion), and RAG patterns without external infrastructure.

Appwrite has no AI/ML integration. Building semantic search on Appwrite requires an external vector database, a separate embedding service, and custom glue code.

### Single Binary Deployment

One binary. No Docker required (though Docker Compose is provided for convenience). No Redis. No Traefik. No worker containers. The operational surface area is minimal.

This matters for:
- Edge deployment (IoT gateways, kiosks, local servers)
- Development environments (start everything in 2 seconds)
- CI/CD pipelines (spin up a test instance in milliseconds)
- Air-gapped environments (copy one binary)

### Rust Performance Characteristics

Appwrite's PHP core, even behind Swoole, has different performance characteristics than Rust:
- No garbage collection pauses
- Predictable memory usage (no GC heap growth)
- Zero-cost abstractions for the permission engine and query compiler
- Native async I/O without the overhead of a runtime VM

DarshJDB does not have published benchmarks yet. The performance advantage is architectural, not proven.

---

## 10. When to Use Which

### Use Appwrite when:

- You need a production-ready BaaS today
- Your project requires mobile SDKs (Flutter, iOS, Android)
- You need messaging (email, SMS, push)
- You need team/organization management
- Your team can afford 2+ GB RAM for the server
- You want a mature console UI
- You need the backing of a funded company and active community

### Use DarshJDB when:

- You want to run on minimal infrastructure ($5 VPS, Raspberry Pi, edge device)
- Your data model benefits from a knowledge graph / triple store
- You need vector search and semantic queries integrated with your application data
- You want direct PostgreSQL access alongside the BaaS layer
- You are comfortable with alpha software and want to contribute
- You value architectural simplicity (understanding the entire system)
- You are building a prototype or internal tool where SDK breadth matters less

### Use neither when:

- You need a managed service with zero operational overhead (use Supabase, Firebase, or Convex)
- You need SOC2/HIPAA compliance today (neither platform is certified)
- You need horizontal scaling across regions (Appwrite supports it in theory, DarshJDB does not)

---

## 11. Architectural Decision Matrix

For architects evaluating both platforms, here is a decision matrix weighted by common BaaS concerns:

| Criterion | Weight | Appwrite | DarshJDB | Notes |
|-----------|--------|----------|-----------|-------|
| Production readiness | 10 | 9/10 | 3/10 | Appwrite is battle-tested. DarshJDB is alpha. |
| Self-hosting simplicity | 8 | 5/10 | 9/10 | 2 containers vs 10+. |
| Auth completeness | 8 | 9/10 | 5/10 | 30 providers vs 4. |
| Data model flexibility | 7 | 6/10 | 8/10 | Triple store wins for schema-free dev. |
| Real-time sophistication | 7 | 6/10 | 8/10 | Diff-based > full-document push. |
| SDK ecosystem | 7 | 9/10 | 5/10 | 14 vs 6. No mobile for DarshJDB. |
| AI/ML integration | 6 | 2/10 | 7/10 | pgvector + auto-embedding is real. |
| Resource efficiency | 6 | 4/10 | 9/10 | 10x difference in RAM. |
| Function runtime | 5 | 8/10 | 3/10 | 14 runtimes vs subprocess-only. |
| Community / ecosystem | 5 | 9/10 | 1/10 | 40k stars vs just started. |

---

## 12. A Note on Honesty

DarshJDB is alpha software built by one developer. Appwrite is a well-funded company with a full engineering team, years of production hardening, and a large community. Any comparison that declares DarshJDB "better" in aggregate would be dishonest.

What DarshJDB has is a different architectural thesis: that a single Rust binary with a triple store over PostgreSQL, diff-based real-time, and built-in vector search is a better foundation for the next decade of application development. Whether that thesis holds depends on execution, community, and time.

Appwrite is the practical choice today. DarshJDB is an experiment worth watching.

---

*This comparison was written from reading both codebases. Appwrite's source is at [github.com/appwrite/appwrite](https://github.com/appwrite/appwrite). DarshJDB's source is at [github.com/darshjme/darshjdb](https://github.com/darshjme/darshjdb). If any claims are inaccurate, please open an issue.*
