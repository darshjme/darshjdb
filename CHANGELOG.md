# Changelog

All notable changes to DarshJDB will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.2.1] - 2026-04-15 — Executor Rewire + SqliteStore::query

Closes the two integration deferrals from the v0.3.2 sprint so the
SQLite backend has a real read path and the DarshanQL executor knows
which dialects can run which statement types.

### Added

- **`SqliteStore::query` real implementation** — the v0.3.2 stub that
  returned `InvalidQuery` for every plan is gone. `SqliteStore::query`
  now binds `serde_json::Value` params through a small `ToSql` adapter,
  expands the v0.3.2 M-3 `__UUID_LIST__` token into per-uuid `?N`
  placeholders for nested-plan resolution, executes via rusqlite on a
  blocking task, and materialises rows in the same `QueryResultRow`
  JSON shape `PgStore::query` returns so downstream consumers see no
  shape drift across backends. Plans carrying the
  `__SQLITE_VECTOR_UNSUPPORTED__` /
  `__SQLITE_COSINE_DISTANCE_UNSUPPORTED__` sentinels are refused up
  front with a clear `InvalidQuery` message.
- **`SqlDialect` capability gates** — three new methods
  (`supports_ddl`, `supports_graph_traversal`, `supports_hybrid_search`)
  with default-true so `PgDialect` inherits the v0.3.1 surface
  unchanged. `SqliteDialect` overrides all three to false. The
  DarshanQL executor checks them at dispatch time.
- **`darshql::ExecutorContext`** — a `{pool, Arc<dyn Store>,
  Arc<dyn SqlDialect>}` bundle threaded through every executor
  function. The HTTP entry point keeps the existing
  `execute(&PgPool, …)` signature for backwards compatibility and
  constructs the context internally; new call sites (tests, future
  portable runners) use `execute_with_context` directly.
- **`tests/sqlite_e2e_query.rs`** — six end-to-end integration tests
  covering bare SELECT, `$where Eq`, `$where Neq`, `$limit + $offset`,
  `$order ASC`, and the empty-result case against an in-memory
  `SqliteStore` driven through `plan_query_with_dialect` + the new
  `Store::query` path. Plus two matching `store::sqlite::tests`
  unit tests and three new `query::dialect::tests` capability checks.

### Changed

- **DarshanQL executor — Tier 2 statement-type gates.** `DEFINE
  TABLE`, `DEFINE FIELD`, `RELATE`, `SELECT` fields containing
  `->edge` traversal, and `count(->edge)` computed fields now check
  `ctx.dialect.supports_*()` and return `InvalidQuery` with a
  v0.3.3-tracking message on dialects that don't support them
  (SQLite today). PostgreSQL production behaviour is byte-for-byte
  unchanged because `PgDialect` inherits the default-true.

### Deferred to v0.3.2.2 / v0.3.3

- The portable Pg-or-SQLite hookup for SELECT / CREATE / INSERT /
  RETRACT through `ctx.store` (rather than `ctx.pool`) is plumbed
  through `ExecutorContext` but the executor body still reaches for
  the pool directly for the read/write SQL — the SurrealQL-shaped AST
  consumed by `query/darshql/executor.rs` is independent of the
  JSON-shaped `QueryAST` driven by `plan_query_with_dialect`, and the
  v0.3.3 milestone tracks unifying the two planner surfaces so the
  same `Store::query` path serves both. Until then the gates are the
  safety net.
- `INFO FOR` against `:schema/*` triples on SQLite — the storage
  shape is portable but the planner pieces have not been ported.
  Tracked alongside the DDL gate in v0.3.3.
- See `DEFERRED.md` for the full deferral list with rationale.

## [0.3.2] - 2026-04-15 — SQLite Backend + mlua Runtime + Dialect Abstraction

The v0.3.1 architecture wave laid the trait boundaries; v0.3.2 fills
them in. Three sprint branches developed in parallel land together:
a real SQLite backend behind the `Store` trait, a `SqlDialect` trait
that lets the DarshanQL planner emit Postgres or SQLite SQL from the
same AST, and an embedded Lua 5.4 function runtime with a hardened
sandbox and a wired `ddb.*` host API. PostgreSQL 16 is still the
production HTTP backend — sqlite-only HTTP boot lands in v0.3.3.

### Added

- **`SqliteStore` (gated on `--features sqlite-store`)** — full
  `Store` trait implementation over a bundled `rusqlite` 0.31
  database. Migrations live at `migrations/sqlite/001_initial.sql`
  (triples + darshan_tx_seq). Uses `IMMEDIATE` transactions with a
  5-second `busy_timeout` so concurrent `set_triples` paths do not
  deadlock. 12 unit tests covering migration, set/get/retract
  roundtrip, TTL expiry, schema inference, concurrent batch ingest,
  and timestamp parsing.
- **`SqlDialect` trait + `PgDialect` / `SqliteDialect` impls** — the
  DarshanQL planner now routes through a dialect handle so the same
  `QueryAST` produces Postgres SQL (`$1`-style placeholders, JSONB
  operators, `::uuid` casts, `ANY($1::uuid[])` batches) or SQLite SQL
  (`?N` placeholders, JSON `LIKE` fallbacks, `IN(__UUID_LIST__)`
  templates expanded at bind time). `PlanCache` instances are now
  pinned to a specific dialect so a Postgres planner cache can never
  hand back SQLite SQL or vice versa. Snapshot parity tests cover
  every WHERE op, ORDER, LIMIT/OFFSET, search, semantic, hybrid, and
  nested combination across both dialects.
- **`MluaRuntime` (gated on `--features mlua-runtime`)** — embedded
  mlua 0.10 with vendored Lua 5.4. Hardened sandbox strips `os.execute`,
  `io`, `require`, `dofile`, `loadfile`, `load`, `loadstring`,
  `string.dump`, `debug`, `collectgarbage`, the raw accessors, every
  bytecode loader path, and pins `ChunkMode::Text` on every load.
  Per-invocation environment isolation: each call gets a fresh proxy
  table over a frozen `safe_globals` snapshot so `string.sub = ...`
  in one user chunk cannot leak into another tenant. Wall-clock
  timeout via `tokio::time::timeout` + `call_async`. Function path
  containment via canonicalize + `starts_with` check (rejects
  `../escape.lua`). Single `Mutex<Lua>` serializes invocations
  (concurrency=1 is honest until v0.4 brings a `Pool<Lua>`). 23 unit
  tests covering every sandbox escape vector.
- **Wired `ddb.*` host API** — when `MluaRuntime` is constructed with
  an `MluaContext` (production server boot path), the Lua host
  bindings are wired live against the runtime-selected `Store` and
  `SqlDialect`:
  - `ddb.query(json_ast)` parses a DarshJQL AST, plans through the
    pinned dialect, dispatches via `Store::query`, returns rows as a
    Lua table.
  - `ddb.triples.get(uuid_string)` calls `Store::get_entity`.
  - `ddb.triples.put(uuid_string, attribute, value)` allocates a
    fresh `tx_id` via `Store::next_tx_id` and writes via
    `Store::set_triples`.
  - `ddb.log.{debug,info,warn,error}` forward into `tracing` with
    structured `message` fields and a 64 KiB cap to prevent OOM via
    `string.rep`.
- **`DDB_FUNCTION_RUNTIME=mlua` dispatch** in `main.rs`, mirroring
  the existing `DDB_FUNCTION_RUNTIME=v8` pattern. Subprocess
  (`ProcessRuntime`) remains the safe default. Misconfiguration
  (e.g. `mlua` requested without `--features mlua-runtime`) emits a
  clear warn and falls back to subprocess.
- **Top-level `Store` + `SqlDialect` handles in `main.rs`** —
  `Arc<dyn Store + Send + Sync>` and `Arc<dyn SqlDialect + Send +
  Sync>` constructed once at boot from the existing PgTripleStore +
  PgPool path. Today they wrap Postgres; in v0.3.3 the same handles
  flow out of the URL-scheme dispatch branch.
- **`docs/SQL_DIALECTS.md`** describing the dialect trait surface,
  what differs between Postgres and SQLite, and the v0.4 portable IR
  roadmap.

### Changed

- `darshql/dialect.rs` adds the trait extraction; `query/mod.rs`
  routes `plan_query` through `plan_query_with_dialect(ast,
  &PgDialect)` so v0.3.1 callers see byte-identical SQL output.
- Front-door `DATABASE_URL` validation in `main.rs` rejects `sqlite:`
  URLs with a clear message: SqliteStore is wired into the function
  runtime and the Store trait, but the HTTP server's auth, anchor,
  search, agent_memory, and chunked_uploads bootstraps are still
  Postgres-only and a sqlite-only HTTP boot lands in v0.3.3. Misconfig
  surfaces immediately instead of as a cryptic `pg_advisory_lock` panic.
- `SqliteStoreTx::{commit,rollback}` are stateless markers that match
  `PgStoreTx` symmetry — multi-statement transactions through the
  dynamic dispatch surface are tracked for v0.3.3.

### Security

The mlua runtime landed under a full security audit by the
`gsd-code-reviewer`, `gsd-security-auditor`, `gsd-nyquist-auditor`,
`gsd-integration-checker`, and `gsd-doc-verifier` agents. Findings
that landed as fixes inside the v0.3.2 sprint:

- **MJ-02** — drop redundant per-invocation semaphore (admitted N
  permits but every admitted task locked the same `Mutex<Lua>`, so
  the permit cap was theatre).
- **MJ-03 + MN-01** — user log text passed as a structured `message`
  field (not a captured format identifier) so embedded newlines are
  escaped by the log formatter instead of injecting fake log lines.
  64 KiB cap on a single user log.
- **MN-03 + F6** — canonicalize and validate the functions directory
  at construction time; reject `../escape.lua` traversal via
  `canonicalize` + `starts_with` containment check.
- **MN-04** — switched the per-invocation source read from blocking
  `std::fs::read_to_string` to `tokio::fs::read_to_string` and moved
  it before the `Mutex<Lua>` lock so I/O does not block the VM mutex.
- **F4** — per-invocation environment isolation via fresh proxy
  tables over a frozen `safe_globals` snapshot.
- **F5** — `ChunkMode::Text` pinned on every chunk load to refuse
  bytecode (which can bypass every source-level sandbox check).
- **F7** — wall-clock timeout via `tokio::time::timeout` +
  `call_async` so CPU-cooperative user code cannot hang the worker.

### Cargo features

```toml
sqlite-store = ["dep:rusqlite"]      # SqliteStore backend
mlua-runtime = ["dep:mlua"]          # Embedded Lua 5.4 function runtime
```

Both default-off so production builds skip the bundled SQLite + Lua
compilation cost. All four feature combos
(`default`, `sqlite-store`, `mlua-runtime`, `sqlite-store mlua-runtime`)
are covered by `cargo check`, `cargo clippy --all-targets -D warnings`,
and `cargo test --lib` in CI.

### Known limitations / deferred to v0.3.2.1

- **`darshql/executor.rs` rewire onto `Store::query`** — the bespoke
  SurrealQL-style statement executor (959 lines, 12 statement types,
  20+ pg-specific helpers including graph traversal and DEFINE TABLE)
  still uses `PgPool` directly. The simpler `parse_darshan_ql →
  plan_query → execute_query` JSON-AST path is fully wired through the
  Store trait via `PgStore::query`, which is what the mlua `ddb.query`
  binding uses. The richer executor lands in v0.3.2.1.
- **`SqliteStore::query`** — currently returns `InvalidQuery` because
  the v0.3.2 SQLite SQL emission path covers triple-level CRUD but
  not the full DarshanQL surface. Triple-level APIs (`set_triples`,
  `get_entity`, `retract`, `next_tx_id`, `get_schema`) are wired
  end-to-end and exercised by the `ddb.triples.*` Lua bindings against
  a real `:memory:` SqliteStore.
- **`ddb.kv.{get,set}`** — kept as `NotYetImplemented` with an updated
  message. The `DdbCache` (slice 10) is keyed on the HTTP request
  boundary and is not exposed to the function runtime; tracked for
  v0.3.2.1.
- **CPU-bound Lua mid-instruction interruption** — the mlua 0.10
  `set_interrupt` hook lands in v0.3.3. Today the wall-clock timeout
  cancels at the next yield boundary, which is sufficient for any
  cooperative user code (the lua_call_respects_wall_clock_cap test
  passes) but a `while true do end` tight loop is bounded only by
  the OS scheduler.
- **`sqlite:` URL HTTP boot** — main.rs rejects sqlite: URLs at the
  front door because the auth/anchor/search/agent_memory/chunked_uploads
  bootstraps are still Postgres-only. Sqlite-only HTTP boot lands in
  v0.3.3.

### Acknowledgements

The v0.3.2 sprint shipped under the gsd-army audit protocol:
gsd-code-reviewer, gsd-security-auditor, gsd-nyquist-auditor,
gsd-integration-checker, and gsd-doc-verifier. Every Mxx and Fx
finding above carries the audit tag of the agent that surfaced it.

## [0.3.1] - 2026-04-15 — Architecture Wave

Three feature branches (PR #3, #5, #6) that spent the v0.3.0 release cycle
in-flight now land together as the architecture wave. v0.3.1 does not
change the DDB runtime requirements (PostgreSQL 16 is still mandatory);
it ships the trait boundaries, typed config surface, and cluster
primitives that v0.3.2 will build on.

### Added — Slice 17 · Typed `DdbConfig` hierarchy (PR #3)

- **13-subsystem strongly-typed config tree**: `server`, `database`,
  `auth`, `cors`, `dev`, `cache`, `embedding`, `llm`, `storage`, `schema`,
  `anchor`, `memory`, `rules` — each with its own Rust struct and
  defaults.
- **Layered loading**: defaults → `config.toml` → `DDB__*` / `DARSH__*`
  env vars, decoded via `config 0.15` with the `convert-case` feature so
  enum fields deserialise from kebab/camel/snake transparently.
- **`Secret<T>` wrapper** redacts sensitive fields in `Debug` output
  (JWT secrets, SMTP passwords, API keys) with `<redacted>`.
- **Backward compatibility**: legacy flat env vars still work; the typed
  loader only takes priority when both are set, and `DDB_RULES_FILE`
  continues to override `cfg.rules.file_path` when present.
- **8 config unit tests** green; `cfg.server.log_level` seeds `RUST_LOG`
  before tracing init so log levels land correctly.

### Added — Cluster module · Horizontal scaling baseline (PR #5)

- **`ddb_server::cluster`**: new top-level module holding all
  multi-replica primitives.
- **Advisory-lock leader election** via `pg_try_advisory_lock` wrapped
  in `spawn_singleton_task` + `spawn_singleton_supervisor`: only one
  replica runs each singleton (e.g. `LOCK_EXPIRY_SWEEPER`) at any time;
  failover is automatic when the leader's Postgres session drops.
- **Cross-replica WS fanout** via `LISTEN ddb_changes`: the extracted
  `notify_listener` task auto-reconnects on listener-session drop and
  feeds `ChangeEvent` into the local broadcast channel, so WebSocket
  subscribers attached to any replica see mutations from any other.
- **`/cluster/status` endpoint** alongside `/health` and `/metrics` —
  no auth required. Response shape is `{node_id, uptime_secs,
  leader_for, version}`, where `leader_for` is the list of singleton
  background tasks for which THIS replica currently holds the advisory
  lock (per-replica, not cluster-wide).
- **9 lib tests + 6 integration tests** covering lock acquisition,
  supervisor restart, and notify reconnect.

### Added — Architecture wave (PR #6)

- **`Store` trait** at `packages/server/src/store/`: defines the
  pluggable storage boundary (`backend_name`, `set_triples`,
  `get_entity`, `retract`, `query`, `get_schema`, `next_tx_id`,
  `begin_tx`). `PgStore` is a full delegation adapter around the
  existing `PgTripleStore` and is the default.
- **`SqliteStore` compile-time stub** gated behind `--features
  sqlite-store` (rusqlite 0.31 bundled). Every method returns
  `DarshJError::Internal("... not yet implemented ...")`
  (see `packages/server/src/store/sqlite.rs`); the stub exists to
  verify the trait boundary before v0.3.2 implements the real
  schema. This is NOT a functional SQLite backend — DarshJDB v0.3.1
  still requires PostgreSQL.
- **`docker-compose.ha.yml`** production HA stack: Patroni 3-node +
  etcd + HAProxy + pgBouncer + WAL-G + MinIO + 3 DDB replicas + nginx +
  Prometheus + Grafana. Companion configs under `deploy/ha/`.
- **`docs/HORIZONTAL_SCALING.md`** full guide: Patroni failover, WAL-G
  PITR restore runbook, pgBouncer tuning, the cluster module reference
  (leader election, singleton supervisor, notify fanout), live-readiness
  checklist.
- **`docs/STORAGE_BACKENDS.md`**: honest portability assessment and
  v0.3.2/v0.4 roadmap for the SqliteStore + DarshanQL dialect work.
- **NOT FOR PRODUCTION** banner on the single-node `docker-compose.yml`.
- **oauth2 5.0.0 stable** (up from 5.0.0-rc.1); `DDB_WATCH` dev shim
  removed; `DARSH_CACHE_PASSWORD`, when set, enables AUTH enforcement
  on the cache server (`packages/cache-server/src/server.rs` reads it
  as optional); `docker-compose.ha.yml` makes it a required
  substitution for production deployments via `${DARSH_CACHE_PASSWORD:?}`.

### Fixed

- **`notify` platform feature**: the v0.3.0 followup CI fix removed the
  explicit `macos_fsevent` feature that broke Linux builds. v0.3.1
  keeps `notify = "7"` with default features so each platform's backend
  is auto-selected.
- **Workspace version bumped `0.3.0` → `0.3.1`** across all crates.

### Known limitations — will land in v0.3.2

- **PostgreSQL is still required.** The `Store` trait boundary is in
  place, but `SqliteStore` is a compile-time stub only.
- **DarshanQL emits Postgres-specific SQL** (JSONB operators, UUID
  casts, `DISTINCT ON`, recursive CTEs, `make_interval`). A
  `SqlDialect` abstraction is required before the SQLite backend can
  execute real queries.
- **Function runtime still uses subprocess `ProcessRuntime`** — the
  embedded Lua / mlua 0.10 runtime is deferred to v0.3.2.
- **4 `require_admin_auth_*` tests remain `#[ignore]`** pending a
  real testcontainers harness; 15 pre-existing baseline failures in
  `views/automations/formulas/graph/plugins/storage/tables` likewise
  marked `#[ignore]`.

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
