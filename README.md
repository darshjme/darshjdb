# DarshJDB

**A self-hosted backend server written in Rust that stores everything as append-only triples in PostgreSQL and exposes them over REST, WebSocket, SSE, and the Model Context Protocol.**

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.4.0-informational.svg)](CHANGELOG.md)

DarshJDB is one Axum/Tokio process (`ddb-server`) that gives an application auth, row-level
permissions, a query language, file storage, pgvector search, graph traversal, realtime
subscriptions, time-series endpoints, and an agent-memory API — all on top of a single Postgres
database. What makes it different from a Firebase clone is the storage model: every write lands as
an immutable `(entity, attribute, value)` triple carrying a transaction id, nothing is updated in
place, and the same store is addressable as documents, as a graph, and as MCP tool calls from an
LLM. A per-transaction Merkle root is recorded on a best-effort basis *after* commit — see
[Write path](#write-path) — so treat the audit chain as advisory, not as a guarantee.

One capability in that list does not work on a stock `docker compose up`: the TimescaleDB
`/api/ts/*` endpoints need a migration file that nothing in this repository applies for you. And
agent-memory *recall* is a SQL `LIKE` scan, not vector search, whatever the embedding columns
suggest. See [Quickstart](#quickstart) and [Known gaps](#known-gaps).

**This is alpha software at v0.4.0.** It is not published to crates.io, npm, PyPI, or Packagist.
It has not been run under production traffic, there are no published benchmarks in this repository,
and there is no compatibility guarantee between 0.x minors.
[Read the limitations](#what-is-implemented-vs-planned) before you plan work around it.

Author: **Darshankumar Joshi** ([github.com/darshjme](https://github.com/darshjme)) · MIT ·
project site: [db.darshj.me](https://db.darshj.me)

---

## Table of contents

- [Quickstart](#quickstart)
- [Architecture](#architecture)
- [Write path](#write-path)
- [Search and agent memory](#search-and-agent-memory)
- [Data model](#data-model)
- [API surface](#api-surface)
- [What is implemented vs planned](#what-is-implemented-vs-planned)
- [Deployment](#deployment)
- [Security hardening](#security-hardening)
- [Contributing](#contributing)
- [License](#license)

---

## Quickstart

> **Every command below assumes bash** — Linux, macOS, WSL, or Git Bash. The heredocs,
> `openssl rand`, `$(...)` substitution, `date +%F`, and both `scripts/*.sh` are POSIX shell.
> None of them run in Windows PowerShell as written.

### Prerequisites, stated plainly

`ddb-server` **requires PostgreSQL**. It rejects `sqlite:` connection URLs at startup
(`packages/server/src/main.rs:142`).

pgvector is **strongly recommended but not required to boot**. `ensure_search_schema`
(`packages/server/src/api/rest.rs:4633`) is in two parts: the FTS GIN index on `triples.value` is
created unconditionally and is fatal on failure; the `CREATE EXTENSION vector` + `embeddings` table
block is wrapped in `if let Err(e) = … { tracing::warn!(…) }` and returns `Ok(())` regardless. The
in-source comment says so explicitly — pg_embed's bundled Postgres on darwin-arm64 ships without
pgvector. Without the extension the server starts fine, logs a warning, and
`/api/search/semantic` and `/api/search/hybrid` fail at query time. (Note: the bootstrap comment
predicts a "clean 503", but `search_semantic` actually maps the sqlx error through
`ApiError::internal`, which is **HTTP 500**. `/api/search/text` keeps working either way.)

`ddb-server` also **requires `DDB_STORAGE_KEY`, or it panics** at
`packages/server/src/main.rs:750-763` — unless `DDB_DEV` is `1`/`true`, which substitutes an
insecure dev key and logs a warning.

Config resolution is a real hierarchy (`packages/server/src/config/mod.rs:743`): serde defaults →
`config.toml` → `config.local.toml` → `DDB__*` env → `DARSH__*` env. `load_config` also calls
`dotenvy::dotenv()`, so **the server itself reads `.env` from its working directory**. Your shell
does not.

### Option A — Docker Compose

`docker-compose.yml` is labelled **NOT FOR PRODUCTION** in its own header: one Postgres node, no
replica, no pgBouncer, no WAL archival. It refuses to start without a `.env` — four variables use
the fail-if-unset `${VAR:?}` form (`POSTGRES_PASSWORD`, `DDB_JWT_SECRET`, `DARSH_CACHE_PASSWORD`,
`REDIS_PASSWORD`).

```bash
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb

cat > .env <<EOF
POSTGRES_PASSWORD=$(openssl rand -hex 24)
DDB_JWT_SECRET=$(openssl rand -hex 32)
DARSH_CACHE_PASSWORD=$(openssl rand -hex 24)
REDIS_PASSWORD=$(openssl rand -hex 24)
DDB_STORAGE_KEY=$(openssl rand -hex 32)
EOF
```

The shipped compose file does not pass `DDB_STORAGE_KEY` through to the container, so add an
override rather than editing the tracked file:

```bash
cat > docker-compose.override.yml <<'EOF'
services:
  darshjdb:
    environment:
      DDB_STORAGE_KEY: ${DDB_STORAGE_KEY:?Set DDB_STORAGE_KEY}
EOF

docker compose up -d

# `up -d` returns as soon as containers are created. The darshjdb service waits on the
# postgres healthcheck (start_period: 30s) and then runs the schema bootstraps under an
# advisory lock, so poll rather than curling immediately. CI allows 60s for this.
until curl -sf http://localhost:7700/health; do sleep 2; done
```

`health_handler` (`packages/server/src/observability/health.rs:86`) returns HTTP 200 with
`{"status": "ok", "version": "…", "author": "…"}`. That is read from source — nobody in this
workflow booted a container, so treat it as the shape the code emits, not as an observed response.
The image's `HEALTHCHECK` is `curl -sf http://localhost:7700/health`, so `docker compose ps`
reporting `healthy` is the same signal.

Three things about this stack you should know before you trust it:

- **Only `001_initial.sql` is applied.** Compose mounts exactly one of the fifteen migrations in
  `packages/server/migrations/` into Postgres `initdb` (the directory holds sixteen files; the
  sixteenth is `seed.sql`, which is sample data, not a migration). The server never runs
  migrations — there is no `sqlx::migrate!` anywhere under `packages/`. It does run hand-written idempotent bootstraps at
  boot that cover `triples`, `users`, `sessions`, `oauth_identities`, `magic_link_tokens`,
  `login_attempts`, `tx_merkle_roots`, `embeddings`, `chunked_uploads`, `anchor_receipts` and the
  agent-memory tables, so auth, storage, search, mutation and agent memory work out of the box.
  What is **not** covered by any bootstrap is `time_series` (backing `/api/ts/*`) and `kv_store`
  (backing the L2 cache). Those live only in unapplied migration files. Apply them by hand:

  ```bash
  set -a; . ./.env; set +a
  URL="postgres://darshan:${POSTGRES_PASSWORD}@localhost:5432/darshjdb"
  for f in packages/server/migrations/*.sql; do
    [ "$(basename "$f")" = seed.sql ] && continue
    psql "$URL" -f "$f"
  done
  ```

  `scripts/setup-db.sh` does **not** do this. It applies exactly one file —
  `psql "$DATABASE_URL" -f "$MIGRATIONS_DIR/001_initial.sql"` (line 55) — plus `seed.sql` only
  when you pass `--seed`. That is the same file compose already mounted, so running it after
  `docker compose up` is a no-op. Note also that `initdb` scripts run only on an empty `pgdata`
  volume.
- **Redis and Qdrant still start.** They are unconditional services in this compose file, and the
  server is handed `DDB_REDIS_URL` and `DDB_QDRANT_URL`. Whatever the tagline says, the reference
  stack is four containers, not one.
- **Nothing listens on 7701.** Compose publishes the port and sets `DARSH_CACHE_PORT`, but the
  Docker image does not contain `ddb-cache-server`. See [Binaries](#binaries).

### Option B — build from source, zero external database

The `embedded-db` feature downloads and manages a portable PostgreSQL 16 under
`~/.darshjdb/data/pg` (`packages/server/src/embedded_pg.rs:62`).

```bash
env -u DATABASE_URL \
  DDB_STORAGE_KEY=$(openssl rand -hex 32) \
  DDB_JWT_SECRET=$(openssl rand -hex 32) \
  cargo run --bin ddb-server --features embedded-db
```

Two details that bite:

- Without `DDB_STORAGE_KEY` (or `DDB_DEV=1`) this **panics**. See Prerequisites.
- Embedded Postgres is used **only when neither `database.url` nor `DATABASE_URL` resolves**
  (`main.rs:96-102`). Because `load_config` calls `dotenvy::dotenv()`, a `DATABASE_URL` line in
  `.env` counts — you will silently get the external database with no indication the embedded one
  was skipped. Hence the `env -u DATABASE_URL` above.

For a throwaway run, `DDB_DEV=1 cargo run --bin ddb-server --features embedded-db` substitutes an
insecure dev key and refuses to bind anything but loopback.

### Option C — build against your own Postgres

`docker-compose.dev.yml` is the overlay that publishes Postgres on the host. It sets
`DDB_JWT_SECRET` and `DARSH_CACHE_PASSWORD` for the app container, but **not**
`POSTGRES_PASSWORD` or `REDIS_PASSWORD` — those are still `${VAR:?}` in the base file, so
**do the `.env` step from Option A first**, even though you are only starting `postgres`.

```bash
set -a; . ./.env; set +a          # bash does not read .env on its own

docker compose -f docker-compose.yml -f docker-compose.dev.yml up -d postgres

# Optional: setup-db.sh applies ONLY 001_initial.sql. Its default URL is
# postgres://darshan:darshan@localhost:5432/darshandb — wrong database name AND
# wrong password for this stack — so pass the URL explicitly or it fails all 30
# connect attempts and exits 1.
./scripts/setup-db.sh "postgres://darshan:${POSTGRES_PASSWORD}@localhost:5432/darshjdb"

DATABASE_URL="postgres://darshan:${POSTGRES_PASSWORD}@localhost:5432/darshjdb" \
DDB_JWT_SECRET="${DDB_JWT_SECRET}" \
DDB_STORAGE_KEY="${DDB_STORAGE_KEY}" \
cargo run --bin ddb-server
```

The server listens on **7700**. The admin SPA is compiled into the binary via
`include_dir!("$CARGO_MANIFEST_DIR/../admin/dist")` and served at `/admin`; if you change the
dashboard you must run `npm run build --workspace=packages/admin` before any `cargo` command that
touches the server lib.

### Installing a prebuilt binary

There is no `cargo install` and no `npm install`. `ddb-server` is not on crates.io;
`@darshjdb/client` and `@darshjdb/react` are not on npm. The `npm-publish.yml` workflow exists,
the registry entries do not.

`scripts/install.sh` resolves `releases/latest` from the GitHub API and downloads
`ddb-server-<tag>-<os>-<arch>`. As of this writing the latest published tag is **v0.3.3**, with
assets `ddb-server-v0.3.3-linux-x86_64`, `-darwin-aarch64`, and `-windows-x86_64.exe` (verified
against the GitHub Releases API). Five caveats:

- It installs **0.3.3**, not the 0.4.0 this README documents. There is no v0.4.0 release.
- Linux and macOS only — the script's `case "$OS" in linux|darwin) : ;; *) exit 1` rejects
  Windows despite a Windows asset being published.
- The uname-based matrix also accepts linux/aarch64 and darwin/x86_64, but the release workflow
  builds exactly three targets — linux-x86_64, darwin-aarch64, windows-x86_64 — so on a Linux ARM
  box or an Intel Mac the download 404s.
- **No checksum or signature verification.** The script pipes whatever the GitHub CDN returns
  straight to an executable file.
- It writes the `ddb-server` binary to disk under the name `ddb`
  (`$HOME/.darshjdb/bin/ddb` by default), which is the CLI's name in every other context.

---

## Architecture

```mermaid
graph TD
    subgraph CLIENTS["Clients"]
        SDK["SDKs: TS · Python · PHP"]
        UI["React / Next / Angular"]
        LLM["LLM agent via MCP"]
    end

    CLIENTS --> MW

    subgraph MW["Axum :7700 — middleware, outermost first"]
        M1["CORS"] --> M2["Timeout 30s → 504"]
        M2 --> M3["CatchPanic → 500"]
        M3 --> M4["Request logging"]
        M4 --> M5["RequestId + span"]
        M5 --> M6["HTTP metrics"]
    end

    MW --> ROOT["Root, unauthenticated<br/>/health /ready /live<br/>/health/full /health/ready /health/db<br/>/cluster/status<br/>/metrics (IP allowlist)<br/>/admin SPA"]
    MW --> WSN["/ws — upgrade is open,<br/>first frame must be<br/>auth + token, else close"]
    MW --> PUB["/api public<br/>auth signup, signin,<br/>oauth, refresh, docs"]
    MW --> PROT["/api protected<br/>rate limit + require_auth_middleware<br/>query · mutate · data · darshql<br/>storage · graph · search · schema"]
    MW --> SUBR["/api sub-routers<br/>own state + own auth layer<br/>tables · webhooks · api-keys<br/>plugins · automations · agent · mcp<br/>cache (+ admin-only guard)"]

    PROT --> CORE
    SUBR --> CORE
    PUB --> CORE
    WSN --> CORE

    subgraph CORE["Engines"]
        TS["Triple store<br/>append-only"]
        QE["DarshJQL planner<br/>+ permission filter"]
        QC["QueryCache<br/>in-process LRU"]
        RT["Broadcast bus<br/>capacity 4096"]
    end

    CORE --> PG[("PostgreSQL 16<br/>triples · embeddings<br/>auth · audit")]
    RT --> OUT["WebSocket /ws<br/>SSE /api/subscribe<br/>SSE /api/events<br/>webhooks"]

    DC["ddb-cache-server :7701<br/>RESP3, in-process DashMap<br/>separate binary"] -.->|"not in the<br/>Docker image"| MW

    classDef client fill:#0b5fa5,stroke:#04305c,color:#ffffff
    classDef mw fill:#57606a,stroke:#24292f,color:#ffffff
    classDef route fill:#8250df,stroke:#4a2a86,color:#ffffff
    classDef core fill:#1a7f37,stroke:#0d4720,color:#ffffff
    classDef store fill:#953800,stroke:#4d1d00,color:#ffffff
    classDef warn fill:#a40e26,stroke:#5c0813,color:#ffffff

    class SDK,UI,LLM client
    class M1,M2,M3,M4,M5,M6 mw
    class ROOT,WSN,PUB,PROT,SUBR route
    class TS,QE,QC,RT,OUT core
    class PG store
    class DC warn
```

Everything the API serves is nested under `/api`. Health, readiness, liveness, cluster status and
the admin SPA are at the root and are **not** behind auth; `/metrics` is guarded by an IP allowlist
(`DDB_METRICS_ALLOWED_IPS`, default `127.0.0.1, ::1`, `*` disables it) and returns 403 to everyone
else.

The `/ws` **upgrade** is unauthenticated, but the socket is not: `authenticate` (`ws.rs:452`)
requires the first frame to be `{"type": "auth", "token": …}` and passes it through the same
`SessionManager::validate_token` as REST — signature, expiry, and session state are all checked —
and any other first message closes the connection with `"first message must be auth"`. After
auth, `sub` and `mut` frames are authorized through the same `PermissionEngine` instance the REST
handlers use (`WsState.permissions`; `ws.rs::authorize` mirrors REST's `check_permission` and
fails closed when no rule matches). Later handlers reject with `"not authenticated"` /
`"session not authenticated"`.

`/api/cache/*` carries a second guard, `require_cache_admin_middleware`, because the backing cache
is process-wide and not tenant-namespaced.

### Binaries

| Binary | Crate | Listens | In the Docker image? |
|---|---|---|---|
| `ddb-server` | `packages/server` | 7700 (loopback-only in dev mode) | yes — `CMD ["ddb-server"]`, `EXPOSE 7700` |
| `ddb-cache-server` | `packages/cache-server` | 7701 (RESP3) | **no** — built and stripped in stage 2, never copied into the runtime stage |
| `ddb` | `packages/cli` | — | copied in, not started |

`docker-compose.yml` publishes 7701 and sets `DARSH_CACHE_PORT`, but nothing in the image binds it.
The only `ddb_cache_server::` symbol the server references is `cache_http_router` — it never starts
the RESP3 listener. To get RESP3 you run `ddb-cache-server` yourself from a source build. This is a
real defect, not a documentation nuance.

### Caching, precisely

Three cache implementations exist in-tree and none of them are connected to each other:

| Component | Where | What it actually is |
|---|---|---|
| `QueryCache` | `packages/server/src/cache` | In-process LRU of query results keyed by `{query, user_id, permission_where}`. `DDB_CACHE_SIZE` default 1000 entries, `DDB_CACHE_TTL` default 60s. One `Arc` is shared by the REST and WebSocket states, so a mutation on either invalidates both. |
| `DdbCache` | `packages/cache` | DashMaps for strings, hashes, lists, zsets, streams, channels, plus bloom filter and HyperLogLog. **The engine itself touches no database — a restart drops the whole keyspace.** (The crate does depend on sqlx, `packages/cache/Cargo.toml:25`, but only for `l2.rs` — see the next row.) |
| `L2Cache` / `DdbUnifiedCache` | `packages/cache/src/{l2,unified}.rs` | The Postgres-backed tier over `kv_store`. `grep -rn 'DdbUnifiedCache\|L2Cache\|BytesL2Cache' packages/server/src packages/cache-server/src` returns **zero hits** — it is library code, not part of the running system. The `kv_store` table it queries has no bootstrap either. |

There is also an instance split: the HTTP `/api/cache/*` routes are wired to a `OnceLock` static
(`rest.rs:1014-1015`), while `AppState.ddb_cache` — shared with the Lua `ddb.kv.*` host API — is a
different `DdbCache` built in `main.rs`. Writing a key over HTTP does not make it visible to server
functions, or vice versa.

---

## Write path

`POST /api/mutate` is the batch mutation endpoint. Below is what its handler
(`packages/server/src/api/rest.rs`, `async fn mutate`) actually does.

```mermaid
sequenceDiagram
    autonumber
    participant C as Client
    participant A as Axum + require_auth_middleware
    participant AU as SessionManager
    participant H as mutate handler
    participant PG as PostgreSQL
    participant B as In-process broadcast bus
    participant S as WS / SSE subscribers

    C->>A: POST /api/mutate<br/>Authorization: Bearer
    A->>AU: validate_token(token, ip, ua, fingerprint)
    break invalid or missing token
        A-->>C: 401 JSON error
    end
    AU-->>A: AuthContext{user_id, roles}
    A->>H: request + extensions
    H->>H: validate_entity_name<br/>per-mutation shape checks
    H->>PG: BEGIN
    H->>PG: nextval('darshan_tx_seq')
    H->>PG: retract_in_tx (update / delete ops)
    H->>PG: INSERT INTO triples<br/>SELECT * FROM UNNEST($1..$6)
    H->>PG: rule_engine.evaluate_and_write_in_tx
    H->>PG: COMMIT
    H->>H: QueryCache.invalidate_by_entity_type
    H->>B: ChangeEvent (this process only)
    B->>S: diff / event
    H-->>C: 200 {tx_id, affected, entity_ids}
```

Writes are append-only. Deletes and updates retract the prior triple
(`UPDATE triples SET retracted = true`, with `retracted_tx_id` recording *when*) and append a new
one. The whole batch, including rule-engine inferences, commits or rolls back atomically.

Two properties this path does **not** have, both of which are easy to assume it does:

- **No row-level authorization.** `check_permission` and `PermissionFilter` appear zero times in
  the `mutate` handler body (`rest.rs:2574`); it calls only `extract_bearer_token`. Authentication
  is enforced by the group's `require_auth_middleware`, but per-entity-type permissions are not
  consulted. By contrast `query` (`rest.rs:2479`), `data_list` (`:2827`), `data_create` (`:2870`),
  `data_patch` (`:3162`) and `data_delete` (`:3377`) all call `check_permission`, and even the
  WebSocket `mut` frame goes through the shared `PermissionEngine` (`ws.rs::authorize`). If you
  need row-level enforcement on writes over REST today, go through `/api/data/*`, not
  `/api/mutate`.
- **No cross-replica fan-out, and no Merkle root.** `pg_notify('ddb_changes', …)` and
  `record_merkle_root` live in `PgTripleStore::set_triples`
  (`triple_store/mod.rs:597` and `:605`). `/api/mutate` uses `set_triples_in_tx`
  (`triple_store/mod.rs:319`), which contains only the UNNEST insert. So a mutation committed
  on node A reaches subscribers on node A only, and produces no row in `tx_merkle_roots`.

The `LISTEN/NOTIFY` cross-replica path is real, it just applies to the callers that go through
`set_triples`: `POST /api/data/{entity}`, `POST /api/auth/signup`, and the collaboration
(share / workspace / collaborator) writers. `cluster::notify_listener` LISTENs on `ddb_changes` and
re-injects into that node's local broadcast channel. Paths that use `set_triples_in_tx` —
`/api/mutate`, `/api/batch`, WebSocket `mut`, MCP `ddb_mutate`, history restore and snapshot
restore — do not notify.

Merkle roots are likewise weaker than "every write is Merkle-rooted" suggests. Even on the
`set_triples` path the root is computed from in-memory inputs, recorded **after** commit on a
separate pool connection, and a failure is swallowed with `tracing::warn!`. A transient error
leaves committed triples with no row in `tx_merkle_roots`, which `/api/audit/verify` will later
trip over.

Subscription diffs are computed against per-subscription snapshots and filtered through row-level
permissions before they leave the process.

---

## Search and agent memory

```mermaid
flowchart LR
    subgraph ENT["Entity search — real pgvector"]
        ST["POST /api/embeddings/store<br/>caller supplies the vector"] --> EMB[("embeddings<br/>vector 1536<br/>HNSW cosine<br/>UNIQUE entity+attribute")]
        Q1["POST /api/search/semantic"] --> ANN["cosine ANN<br/>embedding &lt;=&gt; query"]
        Q2["GET /api/search/text"] --> FTS["plainto_tsquery<br/>+ ts_rank"]
        ANN --> RRF["POST /api/search/hybrid<br/>reciprocal rank fusion, k=60"]
        FTS --> RRF
        ANN --> EMB
        FTS --> TRP[("triples.value<br/>GIN tsvector")]
    end

    subgraph AM["Agent memory — 4 tiers"]
        W["working<br/>DashMap ring, volatile"] --> E["episodic<br/>memory_entries rows"]
        E --> SEM["semantic<br/>LLM summariser"]
        SEM --> AR["archival<br/>byte-compressed"]
        CTX["GET /sessions/{id}/context<br/>tiktoken budgeting"] --> REC["recall = SQL LIKE scan<br/>NOT vector search"]
        REC --> E
        REC --> SEM
    end

    subgraph AUTO["Auto-embedding — opt-in, and orphaned"]
        WK1["EmbeddingService<br/>DDB_EMBEDDING_PROVIDER"] --> EE[("entity_embeddings<br/>own cosine index<br/>no reader anywhere")]
        WK2["agent-memory worker<br/>DARSH_EMBEDDING_PROVIDER"] --> MEV[("memory_entries.embedding<br/>vector 1536 + HNSW<br/>written, never queried")]
    end

    classDef ok fill:#1a7f37,stroke:#0d4720,color:#ffffff
    classDef store fill:#953800,stroke:#4d1d00,color:#ffffff
    classDef route fill:#0b5fa5,stroke:#04305c,color:#ffffff
    classDef bad fill:#a40e26,stroke:#5c0813,color:#ffffff

    class ANN,FTS,RRF ok
    class EMB,TRP store
    class ST,Q1,Q2,CTX,W,E,SEM,AR,WK1,WK2 route
    class REC,EE,MEV bad
```

**Entity-level semantic search is real.** `/api/search/semantic` runs a genuine `<=>` ANN query
joined to the entity's `:db/type` triple, and `/api/search/hybrid` fuses the vector and full-text
rankings with `score = Σ wᵢ / (60 + rankᵢ)`. Four consequences worth designing around:

- **You supply the vectors.** `INSERT INTO embeddings` appears in exactly two places, both behind
  `POST /api/embeddings/store` (`api/handlers/search.rs:50`, `api/rest.rs:4741`). Nothing in the
  server embeds text into that table for you — see the orphaned-pipelines note below.
- The dimension is fixed at **1536**. A 768-dim model does not fit the column.
- There is one vector per `(entity, attribute)` — no chunk table, no chunker. Passage-level RAG
  means modelling each chunk as its own entity.
- The bootstrap creates **two** indexes on `embeddings`: an HNSW over `vector_cosine_ops`
  (`m = 16, ef_construction = 64`), and an IVFFlat over **`vector_l2_ops`**
  (`rest.rs:4686-4691`). Every semantic and hybrid query orders by `e.embedding <=> $n::vector`,
  which is cosine distance and binds to `vector_cosine_ops`. The planner cannot use an l2_ops
  index for that ordering, so HNSW is the only index serving ANN reads; the IVFFlat one is write
  amplification and disk. Tracked as a defect.

**Agent-memory recall is not vector search.** `AgentMemoryRepo::semantic_recall`
(`packages/server/src/agent_memory/repo.rs:224`) is:

```sql
SELECT id, session_id, tier, role, content, token_count, metadata, created_at
FROM memory_entries
WHERE session_id = $1
  AND tier IN ('episodic', 'semantic')
  AND lower(content) LIKE $2
ORDER BY length(content) ASC, created_at DESC
LIMIT $3
```

No `<=>` operator appears anywhere in the agent-memory read path. The tiering machinery around it
is substantial — four tiers, decay scoring, promotion/demotion, LLM summarisation of episodic
blocks into semantic ones, archival compression — but the retrieval step is keyword matching today.

**Both auto-embedding pipelines write to tables nothing reads.** This is the sharpest gap in the
project and it is worth stating precisely:

- `EmbeddingService` / `EmbeddingManager` (`packages/server/src/embeddings/mod.rs`) is opt-in via
  `DDB_EMBEDDING_PROVIDER`. When enabled it subscribes to the change stream and writes
  `entity_embeddings` with its own cosine index. That module contains no `SELECT` at all, and
  `entity_embeddings` is referenced nowhere else in `packages/server/src`. The search handlers read
  `embeddings`, a different table.
- The agent-memory worker (`packages/agent-memory/src/worker.rs`) is opt-in via
  `DARSH_EMBEDDING_PROVIDER` (default `none`, i.e. off). When enabled it fills
  `memory_entries.embedding` / `agent_facts.embedding`, `content_tokens` and `embedded_at`. The
  schema for those columns is real — `ensure_agent_memory_schema` runs a fatal `CORE_SQL` block and
  then a best-effort `VECTOR_SQL` block that creates the `vector` extension, drops any legacy
  non-`vector` `embedding` column, adds `embedding vector(1536)` to both tables, and builds partial
  HNSW `vector_cosine_ops` indexes. Nothing then queries those vectors, because
  `semantic_recall` is the `LIKE` scan above.

Without pgvector the `VECTOR_SQL` block logs a warning and those columns simply do not exist.
`packages/server/migrations/20260414055500_agent_memory.sql` and
`20260725010000_embedding_worker_columns.sql` define the same tables and columns a second time and
are applied by nothing, but they are now redundant with the bootstrap rather than in conflict
with it.

---

## Data model

```mermaid
erDiagram
    TRIPLES {
        bigserial id PK
        uuid entity_id
        text attribute
        jsonb value
        smallint value_type
        bigint tx_id
        boolean retracted
        bigint retracted_tx_id
        timestamptz expires_at
    }
    EMBEDDINGS {
        bigserial id PK
        uuid entity_id
        text attribute
        vector embedding "1536"
        text model
    }
    TX_MERKLE_ROOTS {
        bigint tx_id PK
        bytea merkle_root
        bytea chained_root
        bytea prev_root
        integer triple_count
    }
    USERS {
        uuid id PK
        text email UK
        text password_hash
        jsonb roles
    }
    SESSIONS {
        uuid session_id PK
        uuid user_id FK
        text refresh_token_hash
        timestamptz refresh_expires_at
        timestamptz absolute_expires_at
        boolean revoked
    }
    MEMORY_ENTRIES {
        uuid id PK
        uuid session_id FK
        text tier
        text content
    }
    AGENT_SESSIONS {
        uuid id PK
        uuid user_id
    }

    TRIPLES }o--o| EMBEDDINGS : "entity_id + attribute"
    TRIPLES }o--o| TX_MERKLE_ROOTS : "tx_id (best-effort)"
    USERS ||--o{ SESSIONS : "owns"
    AGENT_SESSIONS ||--o{ MEMORY_ENTRIES : "contains"
```

Both relationships involving `TRIPLES` are deliberately optional on the right. `triples` is
append-only, so many rows share an `(entity_id, attribute)` while `embeddings` carries
`UNIQUE(entity_id, attribute)` — many-to-at-most-one. And `tx_merkle_roots` is populated
best-effort on one of two write paths, so a large class of triples has no root row at all.

One table carries all user data. Transaction ids come from `SEQUENCE darshan_tx_seq`. Reads fetch
matching rows ordered `tx_id DESC` and pivot EAV back into documents in Rust, first row per
attribute wins; nested plans are batched with `WHERE entity_id = ANY($1)`, so it is 1+P queries,
not N+1.

**Good fit:** wide, sparse, frequently-reshaped entities; per-attribute history and audit; mixed
document/graph/KV access over one store; small self-hosted deployments where one Postgres is the
whole data tier.

**Poor fit:** analytical scans and column-heavy aggregates — every attribute is a row and the pivot
happens in application memory; workloads that want a fixed relational schema with real foreign
keys.

Note also that `entity_pool` (UUID→i64) and `attribute_pool` (TEXT→i32), the dictionary-encoding
tables, exist with `get_or_create`/`resolve` methods, but `triples` stores raw `entity_id UUID` and
`attribute TEXT`, and no code outside `triple_store/mod.rs` references either pool. Dictionary
encoding is scaffolding, not a property of the running system.

### Schema bootstrap

The server does not run migrations. At boot, `main.rs` takes
`pg_advisory_lock(0x4442_4A44_5348_4D49)` and runs hand-written idempotent bootstraps —
`PgTripleStore::new`, `ensure_auth_schema`, `ensure_anchor_schema`, `ensure_search_schema`,
`ensure_chunked_uploads_schema`, and non-fatally `ensure_agent_memory_schema` — then releases the
lock. Beyond that set, several subsystems carry their own `CREATE TABLE IF NOT EXISTS` blocks —
graph `_edges`, snapshots, `_schemas` / `schema_definitions`, `api_keys`, `ddb_events`,
`activity_log`, `comments`, `notifications`, `entity_embeddings`, and `admin_audit_log` (that last
one is invoked from `main.rs:1039`).

Of the SQL under `packages/server/migrations/`, only `001_initial.sql` has an automated applier
(`scripts/setup-db.sh`, and the compose `initdb` mount). The remaining fourteen migrations have no
applier. Most are redundant with a bootstrap; the ones that are not — `20260414090000_timescale.sql`
and `20260414002020_kv_store.sql` — must be applied by hand with psql. See
[Quickstart](#quickstart).

---

## API surface

Routes were enumerated from `api/rest.rs::build_router`, the sub-router modules, and `main.rs`.

**Root, no auth:** `GET /health` `/ready` `/live`, `GET /health/full` `/health/ready` `/health/db`
(legacy shapes), `GET /cluster/status`, `GET /metrics` (Prometheus, IP-allowlisted),
`GET /admin[/*]` (React SPA), `ANY /ws` (upgrade open, first frame must authenticate).

**Public `/api`:** `POST /auth/{signup,signin,magic-link,verify,refresh}`,
`POST /auth/oauth/{provider}`, `GET /auth/oauth/{provider}/callback`, `GET /openapi.json`,
`GET /docs`, `GET /types.ts`.

**Protected `/api`** (one `require_auth_middleware` over the group, behind a rate-limit layer
keyed by bearer token when one is present and by client IP otherwise — 100 requests/min
authenticated, 20/min anonymous, both with `Retry-After` on 429):

| Area | Routes |
|---|---|
| Query / mutate | `POST /darshql`, `POST /sql/darshql`, `POST /sql` (raw DML, admin-gated and audit-logged), `POST /query`, `POST /mutate` |
| Documents | `GET\|POST /data/{entity}`, `GET\|PATCH\|DELETE /data/{entity}/{id}` |
| Functions | `POST /fn/{name}` |
| Storage | `POST /storage/upload`, chunked `upload/init` + `PUT .../chunk/{i}` + `GET .../status`, `GET\|DELETE /storage/{*path}` |
| Realtime | `GET /subscribe` (SSE), `GET /events` (SSE), `POST /events/publish` |
| Search | `POST /embeddings/store`, `GET /embeddings/get`, `POST /search/semantic`, `GET /search/text`, `POST /search/hybrid` |
| Graph | `relate`, `traverse`, `neighbors`, `outgoing`, `incoming`, `DELETE /graph/edge/{id}` |
| Schema | tables, fields, indexes, migrations under `/schema/*`; `GET\|POST /admin/schema/{collection}` |
| Time-series | `POST\|GET /ts/{entity_type}`, `GET /ts/{entity_type}/agg`, `GET /ts/{entity_type}/latest` — **needs `20260414090000_timescale.sql` applied by hand; no bootstrap creates `time_series`** |
| Batch | `POST /batch`, `POST /batch/parallel`, `GET /batch/metrics` |
| Audit | `verify`, `chain`, `proof`, `anchors` |
| Collaboration | share, collaborators, workspaces, comments, activity, notifications |
| Relations | link, linked, lookup, rollup |
| History | history, version, restore, undo, undelete, snapshots CRUD + restore + diff |
| Import/export | CSV and JSON, with status polling |

**Sub-routers** (own state, own auth layer): `/tables` (CRUD + duplicate + stats), `/aggregate`
(+ summary, chart), `/webhooks` (CRUD + deliveries + test), `/api-keys` (CRUD + rotate), `/plugins`
(CRUD + marketplace + configure), `/automations` (CRUD + run + runs), `/cache/*` (admin-gated
writes and enumeration), `/agent/sessions` (+ messages, context, context/export, search, timeline,
stats) and `/agent/facts`, `POST /mcp` (JSON-RPC 2.0), `GET /agent/stream` (SSE).

**MCP.** `POST /api/mcp` advertises exactly **10 tools**: `ddb_query`, `ddb_mutate`,
`ddb_semantic_search`, `ddb_memory_store`, `ddb_memory_recall`, `ddb_graph_traverse`,
`ddb_timeseries`, `ddb_cache_get`, `ddb_cache_set`, `ddb_kv_list` — plus 3 MCP *prompts*
(`darshql_find_by_type`, `darshql_semantic_qa`, `darshql_graph_neighbors`). Prompts are not tools.

**WebSocket.** JSON envelope, `{"type": "..."}` kebab-case. Client messages: `auth`, `sub`,
`unsub`, `mut`, `pres-join`, `pres-state`, `pres-leave`, `live-select`, `kill`, `pub-sub`,
`pub-unsub`, `batch`. Auth is in-band and goes through the same `SessionManager` as REST.

The query language is **DarshJQL** (`/api/darshql`, parser at
`packages/server/src/query/darshql/`). Its syntax reference lives in
[docs/DARSHQL.md](docs/DARSHQL.md) rather than here — treat any construct not covered by a parser
test in that directory as unverified.

---

## What is implemented vs planned

### Implemented and exercised by the code paths above

Append-only triple store with retraction and point-in-time reads · DarshJQL parser and planner with
row-level permission filtering · JWT auth (RS256 when both key paths are configured, HS256 from
`DDB_JWT_SECRET` otherwise) with 15-minute access tokens, 30-day refresh expiry under a 24-hour
absolute session cap, SHA-256-hashed refresh tokens with rotation, Argon2 passwords, OAuth
providers, magic links, and login throttling · WebSocket and SSE realtime, with WS `sub`/`mut`
frames authorized through the same permission engine as REST · pgvector entity search
with HNSW cosine ANN and RRF hybrid fusion, over vectors the caller supplies · graph edges and
traversal · file storage with chunked uploads · webhooks, API keys, automations, plugins · MCP server with 10 tools and 3 prompts ·
embedded admin dashboard · typed configuration hierarchy (defaults → `config.toml` →
`config.local.toml` → `DDB__*` → `DARSH__*`) · RESP3 cache server, as a binary you run yourself.

Partially implemented, code path exists but is gated on something:

- **TimescaleDB time-series** — handlers are complete, but nothing creates the `time_series`
  hypertable. Apply `20260414090000_timescale.sql` by hand.
- **Cross-replica realtime over `LISTEN/NOTIFY`** — works for writes that go through
  `PgTripleStore::set_triples`, not for `/api/mutate`, `/api/batch`, WS `mut`, or MCP `ddb_mutate`.
  See [Write path](#write-path).
- **Per-transaction Merkle roots** — recorded best-effort after commit, on one of two write paths.
- **Automatic embedding generation** — two providers-backed pipelines exist and run, but both
  write to tables no read path queries. See [Search and agent memory](#search-and-agent-memory).

### Removed

- **S3 / R2 / MinIO storage** — removed, no longer in the tree. `S3Backend` was a complete
  aws-sdk-s3 implementation that nothing ever constructed: `main.rs` builds `LocalFsBackend`
  unconditionally and `AppState.storage_engine` is typed `Arc<StorageEngine<LocalFsBackend>>`.
  It was deleted along with the four AWS SDK dependencies, which held rustls 0.21 /
  rustls-webpki 0.101.7 in `Cargo.lock` (RUSTSEC-2026-0098, -0099, -0104). The `storage.backend`
  and `storage.bucket` config fields still parse but only `local` does anything; there is no
  `DDB_STORAGE_BACKEND` environment variable. File storage is local-filesystem only.

### Known gaps

Verified in-tree, not speculation.

- **The Docker image runs one binary.** `ddb-cache-server` is built and then not copied into the
  runtime stage, so nothing listens on 7701 in the image despite compose publishing that port.
- **The Postgres-backed L2 cache is dead code.** Neither server binary references `L2Cache` or
  `DdbUnifiedCache`, and its `kv_store` table has no bootstrap. The cache that actually runs is
  in-process and does not survive a restart.
- **`/api/mutate` performs no permission check.** Authentication yes, row-level authorization no.
- **The IVFFlat index on `embeddings` is unusable** by the queries the search handlers issue —
  `vector_l2_ops` against a `<=>` cosine ordering.
- **Both auto-embedding pipelines are orphaned.** `EmbeddingService` writes `entity_embeddings`,
  which nothing reads; the agent-memory worker writes `memory_entries.embedding`, which
  `semantic_recall` does not query. The `embeddings` table the search handlers actually read is
  populated only by callers via `POST /api/embeddings/store`.
- **`ddb migrate` in the CLI calls three routes the server does not expose** —
  `GET /api/admin/migrations` (`packages/cli/src/main.rs:834`),
  `POST /api/admin/migrations/rollback` (`:852`), `POST /api/admin/migrations/run` (`:893`).
  `grep -rn 'admin/migrations' packages/server/src` returns nothing. All three 404.
- **The SDKs and the server disagree on the wire contract.** The TypeScript client reads
  `accessToken ?? token` from the signin response (`sdks/typescript/src/client.ts:130`) while the
  server returns `access_token` (`api/handlers/auth.rs:198`); it POSTs `{"operations": …}` to
  `/api/batch` while the server's `BatchRequest` deserializes `ops` (`api/batch.rs:50`). The drift
  extends to the `/api/query` payload (the server expects a DarshanQL JSON value in `query`, not
  SQL text), the live-query message shape, and the provider/export names in the React, Angular,
  and Next.js packages. Treat every SDK call as unverified until you have run it against a live
  server.
- **The release workflow never builds the CLI.** `release.yml` runs
  `cargo build … -p ddb-server` only, so no published release contains a `ddb` binary.
- **The Helm chart does not render.** `deploy/k8s/templates/_helpers.tpl` defines `darshandb.*`
  helpers while every template `include`s `darshjdb.*`, so `helm template` fails; and the
  liveness/readiness/startup probes point at `/api/admin/health`, a route the server does not
  expose.
- **Fourteen of fifteen migrations have no applier** anywhere in the tree. Most duplicate a
  bootstrap, but `20260414090000_timescale.sql` and `20260414002020_kv_store.sql` do not.
- **Compose still runs Redis and Qdrant** as unconditional services.
- **Server functions** run as a subprocess by default; the in-process V8 runtime is behind
  `--features v8` and is unproven.
- **No published packages** on crates.io, npm, PyPI, or Packagist. The newest GitHub release is
  v0.3.3; there is no 0.4.0 artifact.
- **No benchmarks.** `packages/server/benches/darshql_parser.rs` exists and
  `cargo bench --bench darshql_parser --no-run` compiles in CI, but no measured numbers are
  published in this repository. `docs/performance.md` and `docs/architecture.md` still carry
  specific performance multipliers that the 0.4.0 CHANGELOG says were removed; they were not, and
  nothing in this repository substantiates them. Ignore them.
- **CI on `main` is failing.** The five most recent `ci.yml` runs on `main` all completed with
  `failure` (re-checked 2026-07-25). The gates in [Contributing](#contributing) pass on this
  working tree — see the verified run there — but those fixes have not landed on `main`.
- **Ten npm advisories sit in the dev/build toolchain.** `npm audit --omit=dev` — the gate CI
  actually runs (`ci.yml:186`) — reports `found 0 vulnerabilities`, so nothing ships to a consumer
  of these packages. But a full-tree `npm audit` reports 10 (2 low, 7 high, 1 critical) in
  `vitest`, `vite`, `esbuild`, `@babel/core`, `undici`, and the `@angular/*` and `next`/`postcss`
  peer sets. Four need only `npm audit fix`; the `@angular/*` and `next` ones are semver-major.
  Not suppressed and not yet taken.
- **No mobile SDKs, no phone-OTP auth, no hosted documentation site.**
- **`https://db.darshj.me` returns 200 but does not serve the API.** `/health` and `/api/docs` both
  return 404 there (checked), so the live site is a project page, not a running demo instance.

### Planned, in rough order

1. Publish to crates.io and npm, and cut a 0.4.0 release — the metadata and workflows exist, the
   publish does not.
2. Get CI green on `main`.
3. Connect the two auto-embedding pipelines to a reader: point `EmbeddingService` at the
   `embeddings` table the search handlers use, and point `AgentMemoryRepo::semantic_recall` at the
   `vector` column the worker already fills.
4. Add a permission check to `/api/mutate`, and route it through a write path that emits
   `pg_notify` and records a Merkle root.
5. Align the SDKs, CLI, Helm chart, and release workflow with the server's real contract — see
   the four wire-drift bullets in [Known gaps](#known-gaps).
6. Wire the L2 Postgres cache into the server, or delete it and stop describing it. (`S3Backend`
   was the other half of this item and has been deleted.)
7. Ship `ddb-cache-server` in the Docker image, or drop 7701 from compose.
8. Replace the IVFFlat `vector_l2_ops` index with something the planner can use, or drop it.
9. Testcontainers for the auth suite so CI stops needing an external Postgres.
10. Hardening pass — soak, fuzz, chaos, and the first published benchmarks. **1.0 is gated on this.**

No dates. This ordering is intent, not commitment.

---

## Deployment

### Ports

| Port | Process | Notes |
|---|---|---|
| 7700 | `ddb-server` | REST, WebSocket, SSE, MCP, admin SPA, health, metrics |
| 7701 | `ddb-cache-server` | RESP3. Not in the Docker image — run the binary yourself. |

`/health`, `/ready`, `/live`, `/cluster/status`, and the `/admin` SPA are unauthenticated;
`/metrics` is IP-allowlisted. Do not expose the unauthenticated set to the public internet without
a proxy rule.

### Resource budget

These are `docker-compose.yml` limits, not measurements. No RSS benchmark exists in this repo.

| Service | Memory limit | CPU limit |
|---|---|---|
| `darshjdb` | 512M | 2.0 |
| `postgres` (timescaledb-ha pg16) | 1G | 2.0 |
| `redis` | 192M | 0.5 |
| `qdrant` | 512M | 1.0 |

Roughly 2.2 GB budgeted for the default stack.

### Volumes and backups

| Volume | Contents | Back it up? |
|---|---|---|
| `pgdata` | Triples, users, sessions, embeddings, audit chain | **Yes — this is the only durable state that matters.** |
| `ddb_data` | Uploads. The server always uses `LocalFsBackend`, so this is where files land. | Yes |
| `redis_data`, `qdrant_data` | Regenerable | No |

There is no built-in backup command. Use `pg_dump` (bash — the `-Fc` output is binary, and
PowerShell's `>` will corrupt it):

```bash
set -a; . ./.env; set +a
docker compose exec -T -e PGPASSWORD="$POSTGRES_PASSWORD" postgres \
  pg_dump -U darshan -Fc darshjdb > "ddb-$(date +%F).dump"
```

`PGPASSWORD` is passed explicitly because `-T` gives pg_dump no TTY to prompt on.

The container runs `read_only: true` with a 64 MB `/tmp` tmpfs. The storage engine's configured
path defaults to `./darshan/storage`; if that directory cannot be created, `main.rs` falls back to
`/tmp/darshjdb-storage`, which under this compose file is both size-capped and volatile. Make sure
`ddb_data` is actually where uploads land before accepting them.

### TLS

`ddb-server` can terminate TLS natively via the `tls_cert_path` / `tls_key_path` config, but the
image ships no certificates. Put Caddy, nginx, or Cloudflare in front of `:7700`.

### High availability

`docker-compose.ha.yml` defines a 3-node Patroni cluster with etcd for leader election, HAProxy in
front of the leader, pgBouncer transaction pooling, WAL-G archival to S3/MinIO, and multiple
`ddb-server` replicas. Kubernetes manifests are in `deploy/k8s`, LXC in `deploy/lxc`, and the
topology is written up in [docs/HORIZONTAL_SCALING.md](docs/HORIZONTAL_SCALING.md). It has not been
soak-tested or failover-drilled. Treat it as a starting point.

---

## Security hardening

What the code does today:

- **Tokens.** RS256 when both `jwt_private_key_path` and `jwt_public_key_path` are set; HS256 from
  `DDB_JWT_SECRET` otherwise; ephemeral generated keys with a startup warning if neither. Access
  tokens are capped at 15 minutes, refresh tokens expire at 30 days under a 24-hour absolute
  session cap. Refresh tokens are stored only as SHA-256 hashes and the hash is replaced on
  rotation. Device fingerprints are hashed too.
- **Login throttling.** Failures are counted per email over a 15-minute window: five failures
  returns `429 {"error":"too_many_attempts","retry_after":"2^(n-5)"}`; ten returns
  `429 {"error":"account_locked","retry_after":"3600"}`. Both set a `Retry-After` header.
- **Dev bypass, fenced.** In dev mode a random `dev.<hex>` token is generated at startup, logged at
  warn level, and refused whenever `x-forwarded-for` / `x-real-ip` / `forwarded` is present — and
  `main.rs` refuses to bind a non-loopback address while `DDB_DEV` is on.
- **Raw SQL is admin-gated and audit-logged.** `POST /api/sql` writes to `admin_audit_log`.
- **Cache endpoints are admin-gated** for destructive and enumerating verbs, because the cache is
  not tenant-namespaced.
- **`/metrics` is IP-allowlisted** (`DDB_METRICS_ALLOWED_IPS`, default loopback only); other peers
  get 403.
- **Container posture.** `read_only: true`, `no-new-privileges:true`, non-root `darshan` user, tini
  as PID 1, Postgres not published to the host in the base compose file.

What you must do yourself:

- Set `DDB_STORAGE_KEY`, `DDB_JWT_SECRET`, `POSTGRES_PASSWORD`, `DARSH_CACHE_PASSWORD` to real
  random values. Never commit `.env`.
- Prefer RS256 by mounting a keypair.
- Keep `/health*`, `/cluster/status`, and `/admin` off the public internet, or behind a proxy that
  authenticates them.
- Set `DARSH_CACHE_PASSWORD` before exposing 7701 anywhere — the RESP3 server's `AUTH` gate is
  opt-in on that variable.
- Do not rely on `/api/mutate` for authorization. See [Known gaps](#known-gaps).

**Reporting a vulnerability:** email **security@darshj.me**. Do not open a public issue.

Note for maintainers: [SECURITY.md](SECURITY.md) currently points at
`https://github.com/darshjme/darshjdb/security/advisories/new` and `security@db.darshj.me`. Neither
works for an outside reporter — `repos/darshjme/darshjdb/private-vulnerability-reporting` returns
`{"enabled":false}`, so the advisory form 404s for non-maintainers, and `db.darshj.me` has no MX
record (the apex `darshj.me` does, via Google). Both should be corrected in SECURITY.md.

---

## Contributing

Prerequisites: **Rust stable** (edition 2024; CI uses `dtolnay/rust-toolchain@stable` — no MSRV is
pinned in any `Cargo.toml`, so any specific "1.8x+" would be a guess), **Node.js 20+**
(`engines` says `>=20.0.0`, CI builds on 22), and **PostgreSQL 16 with pgvector** unless you use
`--features embedded-db`.

Build the admin dashboard first — it is embedded into `ddb-server` at compile time:

```bash
npm ci --workspace=packages/client-core \
       --workspace=packages/react \
       --workspace=packages/admin
npm run build --workspace=packages/admin
```

Then the gates, taken from `.github/workflows/ci.yml`:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo install cargo-audit --locked && cargo audit
cargo bench --bench darshql_parser --no-run

# needs a reachable Postgres with pgvector; CI uses pgvector/pgvector:pg16
DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshjdb_test cargo test --workspace
```

`DATABASE_URL` is not optional. The `#[sqlx::test]` suites do **not** self-skip when it is
missing — `packages/cache/tests/l2_integration.rs` panics with
`DATABASE_URL must be set: EnvVar(NotPresent)` on all 11 of its tests. Because `cargo test`
fail-fasts on the first failing target, that also stops the run before the `ddb-server` suite
executes. Without a database, use `--no-fail-fast` and read past the `l2_integration` block.

JavaScript, Python, PHP — each in its own subshell so the working directory does not accumulate:

```bash
npm ci && npm run build --workspaces --if-present && npm test --workspaces --if-present
npm audit --omit=dev    # exits non-zero on any advisory; keep it out of the && chain

(cd sdks/typescript && npm ci && npm test)
(cd sdks/python     && pip install -e ".[dev]" && pytest)
(cd sdks/php        && composer install && vendor/bin/phpunit)
```

The `npm run build` step is not optional — CI runs it between `npm ci` and the tests with the
comment "generates types for cross-package refs", and workspace tests can fail without it. And note
that `sdks/php/composer.json` defines no `scripts` section, so `composer test` will fail; call
PHPUnit directly.

The workspace is five Rust crates (`server`, `cli`, `cache`, `cache-server`, `agent-memory`) and
five npm workspaces (`client-core`, `react`, `angular`, `nextjs`, `admin`), plus three SDKs under
`sdks/`. Everything is at version 0.4.0.

Test *declarations*, counted by grep: 1,846 `#[test]` / `#[tokio::test]` attributes under
`packages/`; 34 `it(`/`test(` calls in `sdks/typescript`, 48 `def test_` in `sdks/python`, 56
`public function test` in `sdks/php`.

Last full gate run against this exact tree, 2026-07-26, on Windows with **no** Postgres reachable
and `DATABASE_URL` unset. Exit codes observed, not inferred:

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo check --workspace --all-targets` | exit 0, zero warnings |
| `cargo clippy --workspace --all-targets` | exit 0, zero warnings |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo audit` | exit 0 — 0 vulnerabilities, 16 informational warnings |
| `cargo test --workspace --no-fail-fast` | 1,742 passed · 11 failed · 28 ignored, 31 targets |
| `npm run build` | exit 0 (all 5 workspaces) |
| `npm run typecheck` | exit 0 (all 5 workspaces) |
| `npm test` | exit 0 — 105 passed, 15 skipped |
| `npm audit --omit=dev` | exit 0 — found 0 vulnerabilities |
| `npm audit` (incl. dev) | exit 1 — 10 advisories, all build/test tooling |

All 11 Rust failures are the same environmental cause and are the whole of
`packages/cache/tests/l2_integration.rs`: `#[sqlx::test]` panicking with
`DATABASE_URL must be set: EnvVar(NotPresent)`. There were 11 panics in the run and all 11 were
that message — no other test failed. CI supplies `DATABASE_URL` (`ci.yml:71`), so that target
is expected to run there. The two unused-import warnings in `rest.rs` reported previously are
gone; `cargo check` and `cargo clippy` are now completely silent.

CI on GitHub `main` is still red because none of this has landed there.
See [CONTRIBUTING.md](CONTRIBUTING.md).

Good first contributions are the ten items in [Planned](#planned-in-rough-order).

Every claim here was checked against this tree, the workflow config, or a live HTTP response. The
`file.rs:NNN` references are accurate as of writing and will drift as the code moves — the symbol
names next to them are the durable part. If a claim is not traceable to code, config, or an
observed response, that is a bug in this README; please file it.

---

## License

MIT. Copyright 2024–2026 Darshankumar Joshi. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

Created by **Darshankumar Joshi** — [github.com/darshjme](https://github.com/darshjme) ·
[db.darshj.me](https://db.darshj.me)
