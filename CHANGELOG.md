# Changelog

All notable changes to DarshJDB will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-04-14 — Grand Transformation

DarshJDB becomes a single self-contained Rust binary that replaces PostgreSQL + Redis + Pinecone + LangChain Memory + MCP simultaneously. 63 commits, 28 parallel slice branches merged under the Vyasa orchestrator dispatch.

### Added — Phase 0 · Security Hardening

- **Admin role enforcement**: real cryptographic JWT verification on every `/api/admin/*` route; the previous stub accepted forged tokens.
- **Magic-link auth**: SHA256 token hashing, 15-minute expiry, atomic single-use semantics, `lettre` SMTP + SendGrid + dev-log backends.
- **Session hardening**: 24h absolute timeout, overflow eviction (cap=5 active sessions per user), refresh-token SHA256 at rest, `(user_id, device_fingerprint)` unique index for re-login safety.
- **Login rate limiting**: exponential backoff after 5 failures, account lock at 10 failures for 3600s, per-email and per-IP tracking.
- **WS mutation transactional flow**: atomic tx_id assignment, rule engine evaluation inside the transaction, query-cache invalidation, change-event broadcast.
- **SSE subscription filter**: event filter now respects `entity_type` AND re-evaluates the query's WHERE clause against fresh entity state — no more phantom events.
- **Path-traversal fix** on `/api/storage/upload` — rejects `..`, absolute paths, null bytes, and over-long keys.

### Added — Phase 1 · Redis Superset (port 7701)

- **New `ddb-cache` crate**: L1 DashMap (sub-μs, lz4) + L2 Postgres (zstd ≥1KB) + unified read-through/write-through with Prometheus metrics.
- **New `ddb-cache-server` binary**: RESP3 protocol on port 7701, Redis-compatible — any Redis client works (`redis-cli -p 7701 PING`).
- **Supported commands**: GET / SET / DEL / EXPIRE / TTL / KEYS (glob), HSET / HGET / HGETALL / HDEL / HLEN, LPUSH / RPUSH / LPOP / RPOP / LRANGE, ZADD / ZRANGE / ZRANGEBYSCORE / ZRANK / ZREM / ZSCORE, XADD / XREAD / XRANGE streams, BFADD / BFEXISTS bloom filters, PFADD / PFCOUNT HyperLogLog, SUBSCRIBE / PUBLISH / UNSUBSCRIBE, HELLO 3, AUTH, PING, INFO, FLUSH.
- **HTTP REST mirror** at `/api/cache/*` for non-RESP3 clients.
- **Migrations**: `kv_store`, `kv_streams` with compression-tagged BYTEA storage and background expiry sweeper.

### Added — Phase 2 · Agent Memory (unlimited LLM context)

- **New `ddb-agent-memory` crate**: 4-tier hierarchy (working → episodic → semantic → archival) with importance scoring via Ebbinghaus-style forgetting curve + log-smoothed access count.
- **Schema**: `agent_sessions`, `memory_entries` with pgvector HNSW (m=16, ef_construction=64), `agent_facts` with cross-session upserts, role CHECK constraints.
- **ContextBuilder**: tiktoken-rs budget, reverse-chronological working + episodic, semantic recall via cosine similarity, agent facts injection — returns OpenAI messages[] ready to paste into any chat completion.
- **REST API**: `POST /api/agent/sessions`, `POST /sessions/:id/messages`, `GET /sessions/:id/context?max_tokens=&current_query=&include_facts=`, `POST /sessions/:id/search`, `GET /sessions/:id/timeline`, `GET /sessions/:id/stats`, `POST /facts`, `GET /facts`, `DELETE /sessions/:id`.
- **Embedding worker**: pluggable `EmbeddingProvider` trait with OpenAI, Ollama (nomic-embed-text), Anthropic (via OpenAI-compat gateway), and None backends; batch 50 rows every 5s.
- **LLM summariser (Slice 15)**: episodic-to-semantic compression at 50/100/200 thresholds. OpenAI / Anthropic / None `LlmClient` backends. 20 oldest episodic rows per session are summarised in one LLM call and atomically replaced with a single `tier='semantic'` row — **conversation history is infinite but the context window stays bounded.**

### Added — Phase 3 · Vector + Full-Text + Hybrid Search

- **pgvector** extension bootstrap migration with HNSW + IVFFlat cosine indexes on `embeddings(entity_id, attribute)`.
- **Full-text search** GIN index on `triples.value::text`.
- **`POST /api/search/semantic`** — cosine ANN with `:db/type` filter.
- **`GET /api/search/text?q=&entity_type=&limit=`** — `ts_rank`-ordered full-text search.
- **`POST /api/search/hybrid`** — Reciprocal Rank Fusion (k=60), 4x candidate window per side, per-side weights in the request body.

### Added — Phase 4 · Self-Contained Binary

- **Embedded Postgres 16** via `pg-embed` behind the `embedded-db` Cargo feature — zero external dependencies in dev mode. `cargo run --features embedded-db` spins up a full Postgres on a random port under `~/.darshjdb/data/pg`.
- **Layered `DdbConfig`**: defaults → `config.toml` → `DARSH_*` environment variables (partial; full typed threading coming in v0.3.1).
- **Admin dashboard embedded** via `include_dir!` — the single binary serves the full React management UI at `/admin/*`.
- **`scripts/install.sh`** — one-liner installer detecting OS/arch, resolving the latest GitHub release, writing to `~/.darshjdb/bin/ddb`.
- **GitHub Actions release matrix**: `x86_64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc` on every `v*` tag.

### Added — Phase 5 · TimescaleDB · Graph · Blockchain Anchor

- **TimescaleDB hypertable** `time_series` partitioned by `entity_type` with 1-day chunks, optional 7-day compression policy, 90-day retention policy (all guarded by `DO $$ EXCEPTION WHEN undefined_function`).
- **`/api/ts/*`** — insert/range/aggregate/latest endpoints with `time_bucket` aggregation and `date_trunc` fallback on vanilla Postgres.
- **Graph `edges` table** with `(from_id, relation, to_id)` unique constraint and recursive-CTE BFS traversal.
- **`/api/graph/edges`** / **`/api/graph/traverse`** / **`/api/graph/path`** routes.
- **Blockchain anchor receipts**: SHA3-Keccak-256 aggregate Merkle roots over N transactions, pluggable `Anchorer` trait with `NoneAnchorer` / `IpfsAnchorer` / `EthereumAnchorer` (gated on `anchor-ipfs` and `anchor-eth` features).
- **`GET /api/admin/audit/anchors`** — paginated anchor receipt audit log.

### Added — Phase 6 · Model Context Protocol + Streaming

- **JSON-RPC 2.0 MCP server** at `POST /api/mcp` — works with Claude Desktop, Cursor, any MCP-aware LLM client.
- **10 tools exposed**: `ddb_query`, `ddb_mutate`, `ddb_semantic_search`, `ddb_memory_store`, `ddb_memory_recall`, `ddb_graph_traverse`, `ddb_timeseries`, `ddb_cache_get`, `ddb_cache_set`, `ddb_kv_list`.
- **MCP methods**: `tools/list`, `tools/call`, `resources/list`, `resources/read`, `prompts/list`.
- **SSE streaming agent endpoint** at `GET /api/agent/stream?session_id=&q=` — chunked result frames (25 rows per chunk) with `done:true` terminator.

### Added — Phase 7 · Multimodal + WebSocket Diff Engine

- **Chunked/resumable uploads** with new `chunked_uploads` table, init/chunk/status routes, atomic write-then-rename into `/tmp/darshjdb-uploads/{upload_id}/`, assembly on completion, 24h stale cleanup task.
- **Image transform pipeline**: resize (Lanczos3), crop (with bounds check), format conversion (JPEG / PNG / WebP / AVIF), JPEG quality control, lz4 byte cache with 24h TTL, SHA256-keyed cache entries.
- **WebSocket diff engine**: subscriptions now track per-(session, sub) snapshots in `subscription_snapshots` DashMap and emit structured `{added, removed, updated}` buckets instead of full result replays. Unchanged result sets emit nothing.

### Added — Phase 9 · SurrealDB Parity

- **Strict schema mode**: `schema_definitions` table with per-field `value_type` / `required` / `unique_index` / `validator` / `default_val`. Returns HTTP 422 with `{"errors":[{"field":"email","code":"REQUIRED"},...]}` on violation. Activated by `DARSH_SCHEMA__SCHEMA_MODE=strict`.
- **LIVE SELECT** in DarshanQL — queries prefixed with `LIVE ` auto-register as WebSocket subscriptions. HTTP callers need `X-Subscription-Upgrade: 1` and receive an `X-Subscription-Id` response header.
- **SQL passthrough** at `POST /api/sql` — whitelisted DML only (SELECT / INSERT / UPDATE / DELETE / WITH), DDL rejected with 400, admin-only, every call written to `admin_audit_log` with actor / sql / params / row_count / duration.

### Added — Phase 10 · Observability

- **Prometheus metrics** at `GET /metrics` via `metrics-exporter-prometheus`, IP-allowlisted via `DDB_METRICS_ALLOWED_IPS` (default `127.0.0.1,::1`). New series: `ddb_http_requests_total{method,path,status}`, `ddb_http_latency_seconds`, `ddb_ws_connections_active`, `ddb_query_duration_seconds{kind}`, `ddb_cache_l1_hits_total`, `ddb_cache_memory_bytes`, `ddb_agent_sessions_active`, `ddb_memory_entries_total{tier}`, `ddb_embeddings_generated_total`, `ddb_memory_compressions_total`, `ddb_tx_total`.
- **`GET /health`** — minimal `{status, version, author}` shape (legacy rich health moved to `/health/full`).
- **`GET /ready`** — acquires a pool connection inside a 500ms timeout, checks cache readiness predicate.
- **`GET /live`** — constant 200.
- **Structured JSON logging** via `tracing-subscriber` with `request_id` middleware (honors upstream `X-Request-Id`, echoes on response), span fields: `request_id`, `method`, `path`, `user_id`, `session_id`, `status`, `duration_ms`.

### Added — Packaging

- **`NOTICE`** at repo root with full attribution to Darshankumar Joshi (Navsari + Ahmedabad, GraymatterOnline LLP / KnowAI), Bhagavad Gita 2.47 verse preserved.
- **Workspace `Cargo.toml` metadata**: authors, homepage (db.darshj.me), repository, license MIT, keywords (`database`, `baas`, `agent-memory`, `redis`, `vector-search`), categories, and the canonical description line.
- **`publish = true`** on `ddb-server`, `ddb-cache`, `ddb-cache-server`, `ddb-agent-memory` for future crates.io publication.
- **README comprehensive refresh** covering all 10 phases with concrete examples for Redis drop-in, agent memory flow, MCP config, hybrid search.

### Changed

- **`AuthError`** enum gained `SessionExpired` and `TokenAlreadyUsed` variants; non-exhaustive matches propagated across `api/error.rs`, `auth/middleware.rs`, `api/rest.rs`.
- **`SessionManager::validate_token`** is now `async` (DB-backed for absolute-timeout + revocation check). `.await` propagated to all call sites in `rest.rs`, `middleware.rs`, `views/handlers.rs`, `activity/handlers.rs`, plus test harnesses.
- **`/health`** route renamed — the rich pool+triples+ws shape moved to `/health/full`. New minimal shape at `/health` per Phase 10 spec. **Dashboards scraping `/health` for liveness should update their target.**
- **`async-openai`** upgraded 0.23 → 0.34 with `features = ["full", "rustls"]` (0.34 uses granular feature flags; `full` is the simplest path to `chat-completion-types` + `embedding-types`). Type imports moved under `async_openai::types::chat::*`.
- **`ServerMessage::Sub`** new WS message variant with `{added, removed, updated}` buckets alongside existing `Diff` variant for backwards compatibility.
- **`DdbCache`** in `AppState` — three parallel cache types now coexist (`DdbCache` for RESP3/HTTP REST from Slice 11, `DdbUnifiedCache` for read-through + Prometheus metrics from Slice 10, raw `L1Cache`/`L2Cache` from Slices 8 & 9). Follow-up unification task in code comments.

### Fixed

- **Path traversal in storage upload** — Phase 0 audit finding #2 resolved; `sanitize_storage_path` is now the single source of truth and rejects `..`, `C:`, null bytes, absolute paths.
- **Image transforms previously discarded** — `storage_get` handler used to silently drop `params.transform`; now runs the real pipeline with byte cache.
- **WS subscription replay** — subscriptions previously emitted the entire result set on every change event; now compute a real diff against snapshot cache.

### Deprecated

- Legacy `sessions.revoked BOOLEAN` column — kept in lockstep with new `revoked_at TIMESTAMPTZ` for backwards compat, but all new code reads `revoked_at IS NULL`. To be removed in v0.4.0.

### Security

- Admin endpoint JWT signatures are now verified cryptographically against the server's `KeyManager` before role check; the prior stub only base64-decoded the payload and trusted whatever `roles` field appeared. This closed a trivial forge-any-JWT exploit.
- Refresh tokens are stored as `hex(sha256(raw))` and never in plaintext.
- Login attempts are logged and rate-limited exponentially, with hard lock at 10 failures.
- SQL passthrough is admin-only, whitelisted to DML, and audit-logged.
- Every integration touching external APIs is reviewable via the `/api/admin/audit/anchors` and `admin_audit_log` trails.

### Known Issues

- **4 `require_admin_auth_*` lib tests** fail in environments without a live Postgres because session validation now hits the DB. Resolution: testcontainers harness in v0.3.1.
- **Slice 17 (typed `DdbConfig` end-to-end threading)** deferred to v0.3.1. Current config reads env vars directly in many call sites.
- **Slice 22 (graph admin UI)** deferred to v0.3.1. Backend routes and traversal logic are in place; the admin dashboard page is the missing piece.
- **`ddb-cache` L2 integration tests** require `DATABASE_URL` to run; they use `#[sqlx::test]` and panic without a reachable Postgres.

### Metrics

- **63 commits** on `integration/grand-transformation` (`origin/main`..`HEAD`).
- **~280 new tests** across the workspace (1373 `ddb-server` lib tests passing — 15 fail in offline mode, all DB-dependent or pre-existing baseline).
- **~91K lines of Rust** total across 5 workspace crates.
- **5 workspace crates**: `ddb-server`, `ddb-cli`, `ddb-cache`, `ddb-cache-server`, `ddb-agent-memory`.
- **Release binary**: ~39 MB stripped.
- **Build**: `cargo check --workspace` green on Rust stable 1.92.

## [0.2.0] - 2026-04-09

### Added

- **Multi-Model Database**: Document + Graph + Relational + KV + Vector storage modes
- **DarshQL**: Full query language with DEFINE TABLE, DEFINE FIELD, SELECT, CREATE, INSERT, UPDATE, DELETE
- **Typed Fields**: String, Int, Float, Bool, Datetime, UUID, JSON, Array, Record, Vector with validation
- **Views System**: Grid, kanban, calendar, gallery, form views over the same data
- **Formula Engine**: Computed fields, rollups, aggregations evaluated server-side
- **Relations**: Link fields, lookup fields, rollup aggregations across linked records
- **Automations**: Event-triggered workflows with webhook, email, and field-update actions
- **Webhooks**: HTTP callbacks on CRUD events with exponential backoff retry
- **Import/Export**: CSV, JSON, DarshQL dump with bulk import support
- **Plugin System**: Extensible architecture with Slack, audit log, and data validation plugins
- **Version History**: Merkle-tree audit trail with SHA-512 hash chain and restore capability
- **Collaboration**: Workspace sharing with role-based access and activity feeds
- **GraphQL API**: async-graphql layer alongside REST
- **Graph Traversal**: Entity relationship traversal with forward-chaining rules
- **Presence System**: Real-time user presence with rooms and peer state
- **Schema Modes**: SCHEMALESS, SCHEMAFULL, SCHEMAMIXED with field-level validation and assertions

### Fixed

- All 17 compiler warnings resolved (unused imports, dead code, missing type annotations)
- All clippy lints resolved across the workspace
- Test compilation errors in 6 modules fixed (aggregation, auth, history, plugins, schema)
- Float equality check in field validation tests
- CI pipeline now passes: fmt, clippy, test, smoke test all green

### Changed

- Upgraded to Rust Edition 2024
- Modernized code patterns: `map_or` → `is_some_and`, `format!` → string literals where applicable
- Crate-level clippy configuration for intentional architectural patterns

## [0.1.0] - 2026-04-05

### Added

- **Core Database**: Triple-store graph engine over PostgreSQL with EAV architecture
- **DarshanQL**: Declarative query language with $where, $order, $limit, $search, $semantic operators
- **Real-Time Sync**: WebSocket-based reactive queries with delta compression
- **Optimistic Mutations**: Instant client-side updates with server reconciliation
- **Offline-First**: IndexedDB persistence with operation queue and sync on reconnect
- **Server Functions**: Queries, mutations, actions, cron jobs in V8 sandboxes
- **Authentication**: Email/password (Argon2id), magic links, OAuth (Google, GitHub, Apple, Discord), MFA
- **Permissions**: Row-level security, field-level permissions, role hierarchy, TypeScript DSL
- **File Storage**: S3-compatible with signed URLs, image transforms, resumable uploads
- **Presence System**: Rooms, peer state, typing indicators, cursor tracking
- **Admin Dashboard**: Data explorer, schema visualizer, function logs, user management
- **React SDK**: `@darshjdb/react` with hooks, Suspense, useSyncExternalStore
- **Next.js SDK**: `@darshjdb/nextjs` with Server Components, Server Actions, App Router
- **Angular SDK**: `@darshjdb/angular` with Signals (17+), RxJS, route guards, SSR
- **PHP SDK**: `darshan/darshan-php` with Laravel ServiceProvider
- **Python SDK**: `darshjdb` with FastAPI and Django integration
- **CLI**: `ddb dev`, `ddb deploy`, `ddb push`, `ddb pull`, `ddb seed`
- **Docker**: Single-command self-hosted setup with docker-compose
- **Kubernetes**: Helm chart for production deployment
- **REST API**: Full CRUD + query + auth + storage over HTTP with OpenAPI spec
- **SSE Fallback**: Server-Sent Events for environments without WebSocket
- **Security**: 11-layer defense-in-depth, OWASP API Top 10 coverage, zero-trust default
- **CI/CD**: GitHub Actions for Rust/TypeScript CI, multi-platform release builds, Docker image publishing
- **Examples**: React todo app, plain HTML example, cURL script collection
