# DarshJDB vs. the BaaS Market: Feature Comparison Matrix

> Last updated: 2026-04-05 | Based on DarshJDB v0.1.0 (alpha) and publicly available documentation for each platform.

This document is a factual, feature-by-feature comparison of DarshJDB against every major Backend-as-a-Service platform. Checkmarks, crosses, and "partial" labels are applied honestly -- DarshJDB is alpha software and this table reflects that.

---

## Platforms Compared

| # | Platform | Version / Date | Primary Language | First Release |
|---|----------|---------------|-----------------|---------------|
| 1 | **DarshJDB** | 0.1.0-alpha | Rust | 2026 |
| 2 | **Supabase** | GA | TypeScript / Elixir | 2020 |
| 3 | **Firebase** | GA | Go / Java (proprietary) | 2012 |
| 4 | **Convex** | GA | Rust / TypeScript | 2022 |
| 5 | **Neon** | GA | C (Postgres fork) | 2022 |
| 6 | **PocketBase** | 0.x | Go | 2022 |
| 7 | **Appwrite** | 1.x | PHP / TypeScript | 2019 |
| 8 | **InstantDB** | GA | Clojure / TypeScript | 2023 |
| 9 | **Hasura** | 2.x / 3.x | Haskell / Rust | 2018 |
| 10 | **Directus** | 10.x | TypeScript | 2016 |

---

## Deployment & Infrastructure

| Feature | DarshJDB | Supabase | Firebase | Convex | Neon | PocketBase | Appwrite | InstantDB | Hasura | Directus |
|---------|-----------|----------|----------|--------|------|------------|----------|-----------|--------|----------|
| **Self-hosted** | Yes | Yes | No | No | No | Yes | Yes | No | Yes | Yes |
| **Single binary** | Yes | No | No | No | No | Yes | No | No | No | No |
| **Docker support** | Yes | Yes | N/A | N/A | N/A | Yes | Yes | N/A | Yes | Yes |
| **Kubernetes Helm chart** | Yes | Yes | N/A | N/A | N/A | Community | Yes | N/A | Yes | Community |
| **CI/CD workflows** | Yes (4) | Yes | Yes | Yes | Yes | Partial | Yes | Yes | Yes | Yes |
| **CLI tool** | Yes | Yes | Yes | Yes | Yes | No | Yes | Yes | Yes | No |
| **Open-source license** | MIT | Apache 2.0 | Proprietary | Proprietary | Apache 2.0 | MIT | BSD-3 | Apache 2.0 | Apache 2.0 | GPL-3 / BSL |

---

## Data Layer

| Feature | DarshJDB | Supabase | Firebase | Convex | Neon | PocketBase | Appwrite | InstantDB | Hasura | Directus |
|---------|-----------|----------|----------|--------|------|------------|----------|-----------|--------|----------|
| **Database engine** | PostgreSQL 16+ | PostgreSQL | Proprietary (Spanner-based) | Custom (proprietary) | PostgreSQL (serverless) | SQLite (embedded) | MariaDB | Custom (proprietary) | Postgres / SQL Server / BigQuery | Postgres / MySQL / SQLite / etc. |
| **Data model** | EAV Triple Store | Relational (SQL) | Document (NoSQL) | Document (reactive) | Relational (SQL) | Relational (SQL) | Document | Relational (triple-inspired) | Relational (SQL) | Relational (SQL) |
| **Schema approach** | Schema-optional (infer then lock) | Schema-first (migrations) | Schemaless | Schema-first (TypeScript) | Schema-first (migrations) | Schema-first (auto-migrations) | Schema-first | Schema-optional | Schema-first | Schema-first (auto-introspect) |
| **ACID transactions** | Yes (Postgres-backed) | Yes | No (eventual consistency) | Yes (serializable) | Yes | Yes (SQLite-level) | Partial | Yes | Yes | Yes |
| **Full-text search** | Yes (pg tsvector + GIN) | Yes (pg FTS) | No (use Algolia) | Yes (built-in) | Yes (pg FTS) | Yes (SQLite FTS5) | Yes (built-in) | No | Depends on source | Depends on source |
| **Vector / semantic search** | Yes (pgvector) | Yes (pgvector) | No | No | Yes (pgvector) | No | No | No | No | No |
| **Auto-embedding pipeline** | Yes (OpenAI / Ollama) | No | No | No | No | No | No | No | No | No |
| **TTL / auto-expiry** | Partial (cache + presence TTL; no data-level TTL) | No | Yes (Firestore TTL) | No | No | No | No | No | No | No |
| **Batch / pipeline API** | Yes (multi-op in one round-trip) | Yes (pg functions) | Yes (batched writes) | Yes (mutations) | N/A (raw SQL) | Partial | Partial | Yes | Yes (mutations) | No |
| **Merkle audit trail** | Yes (SHA-512 hash chain) | No | No | No | No | No | No | No | No | No |
| **Forward-chaining rules** | Yes (auto-inferred triples) | No (use pg triggers) | No | No | No (use pg triggers) | No | No | No | No | No (use custom hooks) |
| **Entity Pool (integer IDs)** | Yes (UUID-to-int mapping) | No (native types) | No (auto-generated) | No (auto-generated) | No (native types) | No (auto-increment) | No (auto-generated) | No | No (native types) | No (auto-increment) |

---

## Real-Time & Sync

| Feature | DarshJDB | Supabase | Firebase | Convex | Neon | PocketBase | Appwrite | InstantDB | Hasura | Directus |
|---------|-----------|----------|----------|--------|------|------------|----------|-----------|--------|----------|
| **Real-time subscriptions** | Yes (WebSocket + SSE) | Yes (Postgres CDC) | Yes (native) | Yes (native) | No | Yes (SSE) | Yes (WebSocket) | Yes (native) | Yes (subscriptions) | Partial (WebSocket ext.) |
| **WebSocket protocol** | Yes | Yes | Yes (proprietary) | Yes | No | No | Yes | Yes | Yes | Yes |
| **Pub/Sub events** | Yes (channel patterns with glob) | No (use Realtime channels) | Yes (FCM) | No | No | No | Partial (events) | No | No | No |
| **Presence tracking** | Yes (ephemeral per-room) | Yes (Realtime Presence) | No | No | No | No | No | Yes | No | No |
| **Offline-first client SDK** | Yes (IndexedDB queue + replay) | No | Yes (Firestore offline) | No | No | No | No | Yes (optimistic) | No | No |
| **Diff-based sync** | Yes (delta patches) | No (full row) | No (full document) | Yes (reactive) | No | No | No | Yes | No | No |

---

## Authentication & Security

| Feature | DarshJDB | Supabase | Firebase | Convex | Neon | PocketBase | Appwrite | InstantDB | Hasura | Directus |
|---------|-----------|----------|----------|--------|------|------------|----------|-----------|--------|----------|
| **Auth (built-in)** | Yes | Yes | Yes | External (Clerk) | No | Yes | Yes | Yes | No (delegate) | Yes |
| **Password auth** | Yes (Argon2id) | Yes (bcrypt) | Yes | External | No | Yes (bcrypt) | Yes (Argon2) | Yes | No | Yes (Argon2) |
| **OAuth2 providers** | Yes (Google, GitHub, Apple, Discord) | Yes (30+) | Yes (20+) | External | No | Yes (8+) | Yes (30+) | Yes (Google) | No | Yes (Google, GitHub, etc.) |
| **Magic link auth** | Yes | Yes | Yes | External | No | No | Yes | Yes | No | No |
| **MFA / TOTP** | Yes (TOTP + recovery codes) | Yes | Yes | External | No | Yes | Yes | No | No | Yes |
| **WebAuthn / Passkeys** | Partial (stubs) | Partial | No | External | No | No | No | No | No | No |
| **Row-level security** | Yes (triple-based rules) | Yes (Postgres RLS) | Yes (Security Rules) | Yes (function-based) | Yes (Postgres RLS) | Yes (API rules) | Yes (permissions) | Yes (permissions) | Yes (permissions) | Yes (permissions) |
| **Rate limiting** | Yes (per-endpoint) | Yes | Yes | Yes | N/A | No | Yes | Yes | Yes | No |

---

## Server-Side Logic

| Feature | DarshJDB | Supabase | Firebase | Convex | Neon | PocketBase | Appwrite | InstantDB | Hasura | Directus |
|---------|-----------|----------|----------|--------|------|------------|----------|-----------|--------|----------|
| **Server functions** | Partial (registry + scheduler; V8 runtime WIP) | Yes (Edge Functions) | Yes (Cloud Functions) | Yes (native) | No | Yes (Go hooks) | Yes (Cloud Functions) | No | No (use Actions) | Yes (Extensions / Flows) |
| **Scheduled / cron jobs** | Yes (cron scheduler with distributed lock) | Yes (pg_cron) | Yes (Cloud Scheduler) | Yes (cron) | No | Yes (Go hooks) | Yes | No | Yes (cron triggers) | Yes (Flows) |
| **Webhooks** | Yes (connector) | Yes | Yes | Yes | No | Yes | Yes | No | Yes | Yes |
| **Connector architecture** | Yes (pluggable: log, webhook, extensible) | No | No | No | No | No | No | No | No | No |

---

## File Storage

| Feature | DarshJDB | Supabase | Firebase | Convex | Neon | PocketBase | Appwrite | InstantDB | Hasura | Directus |
|---------|-----------|----------|----------|--------|------|------------|----------|-----------|--------|----------|
| **File storage** | Yes (local, S3, R2, MinIO) | Yes (S3) | Yes (Cloud Storage) | Yes (built-in) | No | Yes (local) | Yes (local, S3) | No | No | Yes (local, S3, GCS, Azure) |
| **Signed URLs** | Yes (HMAC) | Yes | Yes | Yes | No | Yes | Yes | No | No | Yes |
| **Image transforms** | Partial (delegated to CDN layer) | Yes (built-in) | Yes (via Extensions) | No | No | Yes (thumbs) | Yes (built-in) | No | No | Yes (built-in) |
| **Resumable uploads** | Yes (TUS-compatible) | Yes (TUS) | Yes | No | No | No | Yes | No | No | Yes |

---

## Client SDKs

| SDK | DarshJDB | Supabase | Firebase | Convex | Neon | PocketBase | Appwrite | InstantDB | Hasura | Directus |
|-----|-----------|----------|----------|--------|------|------------|----------|-----------|--------|----------|
| **React** | Yes (hooks) | Yes | Yes | Yes | Community | Community | Yes | Yes | Community | Community |
| **Angular** | Yes (signals + RxJS) | Community | Yes | Community | No | No | Yes | No | Community | No |
| **Next.js** | Yes (App + Pages Router) | Yes | Yes | Yes | Community | Community | Yes | Yes | Community | Yes |
| **PHP** | Yes (Composer + Laravel) | No | Yes (Admin SDK) | No | No | Community | Yes | No | No | No |
| **Python** | Yes (sync + async, FastAPI + Django) | Yes | Yes (Admin SDK) | Yes | Yes (psycopg) | Community | Yes | No | No | Yes |
| **JavaScript / TypeScript** | Yes (core client) | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| **Swift / iOS** | No | Yes | Yes | No | No | No | Yes | No | No | No |
| **Kotlin / Android** | No | Yes | Yes | No | No | No | Yes | No | No | No |
| **Flutter / Dart** | No | Yes | Yes | No | No | Community | Yes | No | No | No |

---

## API & Protocol

| Feature | DarshJDB | Supabase | Firebase | Convex | Neon | PocketBase | Appwrite | InstantDB | Hasura | Directus |
|---------|-----------|----------|----------|--------|------|------------|----------|-----------|--------|----------|
| **REST API** | Yes | Yes | Yes | No (function calls) | N/A (SQL wire) | Yes | Yes | No | Yes | Yes |
| **GraphQL** | No | Yes (pg_graphql) | No | No | No | No | Yes | No | Yes (primary) | Yes |
| **Custom query language** | Yes (DarshanQL) | No (SQL + PostgREST) | No | No | No | No | No | Yes (InstaQL) | No | No |
| **OpenAPI spec** | Yes (auto-generated 3.1) | Yes | No | No | N/A | Yes | Yes | No | No | Yes |
| **MessagePack support** | Yes (content negotiation) | No | No | No | No | No | No | No | No | No |
| **SSE (Server-Sent Events)** | Yes | No | No | No | No | Yes | No | No | No | No |

---

## Admin & Observability

| Feature | DarshJDB | Supabase | Firebase | Convex | Neon | PocketBase | Appwrite | InstantDB | Hasura | Directus |
|---------|-----------|----------|----------|--------|------|------------|----------|-----------|--------|----------|
| **Admin dashboard** | Yes (React + Vite + Tailwind) | Yes | Yes | Yes | Yes | Yes (built-in) | Yes | Yes | Yes | Yes (primary interface) |
| **Connection pool stats** | Yes (lock-free histogram, p50/p95/p99) | Partial (pgBouncer) | No | No | Yes | No | No | No | No | No |
| **Prometheus metrics** | Yes (config provided) | Yes | Yes (Cloud Monitoring) | No | Yes | No | Yes | No | Yes | No |
| **Hot-reload functions** | Yes (file watcher) | Yes | Yes | Yes | N/A | Yes | Yes | N/A | Yes | Yes |

---

## Pricing & Community

| Factor | DarshJDB | Supabase | Firebase | Convex | Neon | PocketBase | Appwrite | InstantDB | Hasura | Directus |
|--------|-----------|----------|----------|--------|------|------------|----------|-----------|--------|----------|
| **Free tier** | Unlimited (self-hosted) | 500 MB DB, 1 GB storage | Spark (generous) | 1M function calls/mo | 0.5 GB storage | Unlimited (self-hosted) | Unlimited (self-hosted) | 10K monthly active | Free for self-hosted | Unlimited (self-hosted) |
| **Cloud offering** | No (self-hosted only) | Yes | Yes (primary) | Yes (primary) | Yes (primary) | No | Yes | Yes (primary) | Yes | Yes |
| **GitHub stars** | New (alpha) | 78K+ | N/A (proprietary) | 2K+ | 15K+ | 42K+ | 48K+ | 5K+ | 32K+ | 30K+ |
| **npm packages published** | No (not yet) | Yes | Yes | Yes | Yes | N/A | Yes | Yes | Yes | Yes |
| **Maturity** | Alpha | GA (production) | GA (production) | GA (production) | GA (production) | Stable (pre-1.0) | GA (production) | GA (production) | GA (production) | GA (production) |

---

## Unique to DarshJDB

Features that no other platform in this comparison offers:

| Feature | Description | Status |
|---------|-------------|--------|
| **Triple-store (EAV) data model** | Schema-optional architecture -- write data first, infer structure, lock down for production. No migrations during prototyping. | Working |
| **Merkle audit trail** | Bitcoin-inspired SHA-512 hash chain over every transaction. Tamper detection with O(log n) inclusion proofs. | Working |
| **Forward-chaining rule engine** | Automatic triple inference on write -- when a triple is inserted, rules fire and produce derived triples in the same transaction. | Working |
| **Auto-embedding pipeline** | Configure `DDB_AUTO_EMBED_ATTRIBUTES` and every matching text write generates a vector embedding via OpenAI or Ollama, stored in pgvector automatically. No application code needed. | Working |
| **Entity Pool** | UUID-to-integer mapping table for systems that need compact integer IDs alongside UUIDs. | Working |
| **Connector plugin architecture** | Pluggable event sinks (log, webhook, extensible) that receive hydrated entity change events from the triple-store broadcast channel. | Working |
| **DarshanQL** | Purpose-built query language with JSON syntax, inspired by Datomic pull expressions. Parsed, planned, and executed against Postgres with LRU plan caching. | Working |
| **MessagePack content negotiation** | Send `Accept: application/msgpack` and responses serialize as MessagePack instead of JSON. Request bodies follow `Content-Type`. | Working |
| **Single Rust binary** | The entire server -- API, auth, permissions, query engine, WebSocket handler, admin, storage, sync -- compiles to one binary. Deploy to a $5 VPS. | Working |

---

## Where DarshJDB is Behind

Honest assessment of gaps relative to established platforms:

| Gap | Who does it better | DarshJDB status |
|-----|--------------------|------------------|
| **Mobile SDKs (iOS, Android, Flutter)** | Firebase, Supabase, Appwrite | Not started |
| **GraphQL API** | Hasura, Appwrite, Supabase | Not planned (DarshanQL is the query interface) |
| **Cloud-hosted offering** | All cloud platforms | Not planned for alpha |
| **OAuth provider breadth** | Supabase (30+), Appwrite (30+) | 4 providers (Google, GitHub, Apple, Discord) |
| **Ecosystem / community size** | Supabase, Firebase, PocketBase | Brand new -- alpha stage |
| **Server function runtime** | Convex, Firebase, Supabase | Registry and scheduler exist; V8 isolate runtime is WIP |
| **Published packages (npm, crates.io, PyPI)** | All GA platforms | Not yet published |
| **Production hardening** | All GA platforms | Alpha -- not recommended for production data |
| **Horizontal scaling** | Neon, Firebase, Convex | Single-node only |
| **Data-level TTL** | Firebase (Firestore TTL) | Cache and presence TTL only; no per-document expiry |

---

## Methodology

Every DarshJDB feature claim in this document was verified by reading the source code at `packages/server/src/`, `packages/*/`, and `sdks/*/`. Specifically:

- **Triple store**: `packages/server/src/triple_store/mod.rs`
- **Auth**: `packages/server/src/auth/` (providers.rs, mfa.rs, middleware.rs, permissions.rs)
- **Real-time**: `packages/server/src/sync/` (broadcaster.rs, diff.rs, pubsub.rs, presence.rs)
- **Embeddings**: `packages/server/src/embeddings/` (mod.rs, provider.rs)
- **Storage**: `packages/server/src/storage/mod.rs`
- **Functions**: `packages/server/src/functions/` (registry.rs, runtime.rs, scheduler.rs)
- **Audit**: `packages/server/src/audit/mod.rs`
- **Rules**: `packages/server/src/rules/mod.rs`
- **Batch API**: `packages/server/src/api/batch.rs`
- **Cache**: `packages/server/src/cache/mod.rs`
- **Connectors**: `packages/server/src/connectors/` (mod.rs, webhook.rs)
- **Entity Pool**: `packages/server/src/triple_store/mod.rs` (EntityPool struct)
- **Query engine**: `packages/server/src/query/mod.rs` (FTS via tsvector, hybrid search)
- **OpenAPI**: `packages/server/src/api/openapi.rs`
- **SDKs**: `packages/react/`, `packages/angular/`, `packages/nextjs/`, `sdks/python/`, `sdks/php/`
- **Admin**: `packages/admin/`
- **CLI**: `packages/cli/`
- **Docker**: `Dockerfile`, `docker-compose.yml`
- **Helm**: `deploy/k8s/`
- **CI/CD**: `.github/workflows/` (ci.yml, docker.yml, e2e.yml, release.yml)

Competitor features were verified against official documentation as of April 2026. Where a feature's status was ambiguous, the more conservative label was applied.
