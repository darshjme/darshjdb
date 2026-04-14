<div align="center">

<img src=".github/assets/hero.svg" alt="DarshJDB" width="100%" />

<br/>

[![License: MIT](https://img.shields.io/badge/License-MIT-F59E0B.svg?style=for-the-badge)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-B7410E.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![PostgreSQL 16+](https://img.shields.io/badge/PostgreSQL-16+-336791.svg?style=for-the-badge&logo=postgresql&logoColor=white)](https://www.postgresql.org)
[![Status: Alpha](https://img.shields.io/badge/Status-Alpha-orange.svg?style=for-the-badge)](https://github.com/darshjme/darshjdb)
[![CI](https://img.shields.io/github/actions/workflow/status/darshjme/darshjdb/ci.yml?style=for-the-badge&label=CI)](https://github.com/darshjme/darshjdb/actions)
[![Release: v0.3.0](https://img.shields.io/badge/Release-v0.3.0-F59E0B.svg?style=for-the-badge)](https://github.com/darshjme/darshjdb/releases)

<br/>

**Postgres + Redis + Pinecone + LangChain Memory + MCP in one Rust binary.**

Self-hosted Backend-as-a-Service. Triple-store EAV over PostgreSQL. Tiered agent memory with LLM summariser. RESP3-compatible cache. pgvector hybrid search. Model Context Protocol server. TimescaleDB hypertables. Merkle audit chain. One binary. One config. One port per service.

[Quick Start](#quick-start) | [Feature Grid](#feature-grid) | [DarshJQL](#darshjql) | [Agent Memory](#agent-memory-unlimited-llm-context) | [Redis Drop-in](#redis-drop-in-resp3-port-7701) | [MCP](#model-context-protocol) | [SDKs](#sdks) | [Roadmap](#roadmap)

</div>

---

## What is DarshJDB

DarshJDB is a self-hosted backend that ships as one Rust binary. It replaces the stack most modern apps end up stitching together -- Postgres for data, Redis for cache, Pinecone for vectors, a bespoke "agent memory" service, and an MCP bridge -- with a single process that speaks REST, WebSocket, RESP3, JSON-RPC, and Server-Sent Events. You connect from TypeScript, React, Next.js, Angular, Python, or PHP. It runs on a machine with 512MB of RAM.

This is alpha software. It has hundreds of integration tests across Rust, TypeScript, Python, and PHP. It works. It is not production-hardened yet.

---

## Why DarshJDB Exists

Started in 2021 in Ahmedabad, India. Darshankumar Joshi was building apps for clients at his company (GraymatterOnline) and got frustrated -- every project needed auth, real-time, storage, permissions, and a query engine. Then every AI project needed a vector DB, an agent memory service, and a way to expose tools to Claude Desktop or Cursor. Firebase was locked-in, Supabase was Postgres but heavy, Convex was cloud-only, Pinecone was a separate bill, every "agent memory" product was a thin wrapper over someone else's infra.

He wanted something he could run on a $5 VPS, own completely, and extend without limits. So he started building DarshJDB -- one Rust binary that handles everything. You are not fighting the rowboat when you build a backend in 2026. You are fighting the aircraft carrier of services that boot alongside it.

Five years in, DarshJDB absorbs ideas from PostgreSQL, GraphDB, Redis, pgvector, TimescaleDB, Bitcoin/Solana anchoring, InstantDB, Convex, and the Model Context Protocol -- and still runs on a machine with 512MB of RAM.

---

## Feature Grid

Every capability below is code that shipped in the Grand Transformation integration. No roadmap items. No fake features.

| Phase | Capability | What it replaces |
|-------|------------|------------------|
| **0 — Security** | Real admin RBAC, magic-link email auth, 24h session cap (max 5 per user), refresh-token SHA256 hashing, exponential login rate limit (lock after 10 fails / 3600s), WS mutation transactional broadcast, SSE subscription re-evaluation, path-traversal-safe uploads | Hand-rolled auth middleware, ad-hoc rate limiters |
| **1 — Redis Superset** | `ddb-cache` crate (L1 DashMap + lz4, L2 Postgres + zstd) and `ddb-cache-server` RESP3 binary on port 7701. GET/SET/DEL/EXPIRE/TTL/KEYS, HSET/HGET/HGETALL/HDEL/HLEN, LPUSH/RPUSH/LPOP/RPOP/LRANGE, ZADD/ZRANGE/ZRANGEBYSCORE/ZRANK/ZREM/ZSCORE, XADD/XREAD/XRANGE, BFADD/BFEXISTS, PFADD/PFCOUNT, SUBSCRIBE/UNSUBSCRIBE/PUBLISH, HELLO 3, AUTH, INFO. Mirrored at `/api/cache/*`. | Redis, RedisBloom, Dragonfly, KeyDB |
| **2 — Agent Memory** | `ddb-agent-memory` crate with 3-tier hierarchy (working DashMap / episodic Postgres / semantic pgvector HNSW) plus `agent_facts` injection. tiktoken-aware `ContextBuilder` with reverse-chron recall, semantic top-K, budget-bounded assembly. LLM-backed **episodic → semantic summariser** firing at 50 / 100 / 200 thresholds — the unlimited-context engine. | Pinecone, Weaviate, LangChain Memory, Zep, Mem0 |
| **3 — Hybrid Search** | pgvector HNSW + IVFFlat, ts_rank full-text, Reciprocal Rank Fusion (k=60, Cormack et al.) at `/api/search/hybrid`. | Elasticsearch + Pinecone + custom re-ranker |
| **4 — Self-Contained** | Embedded Postgres 16 via `pg-embed` behind the `embedded-db` feature. React admin dashboard bundled via `include_dir!`. One-line installer at `scripts/install.sh`. GH Actions release matrix: `x86_64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`. | Docker compose stacks, "bring your own Postgres" docs |
| **5 — Time + Graph + Anchor** | TimescaleDB hypertable `time_series` at `/api/ts/*` (insert/range/agg/latest). Graph `edges` table with BFS at `/api/graph/*`. Merkle anchor receipts with optional IPFS/Ethereum submission behind `anchor-ipfs` / `anchor-eth` feature flags. | TimescaleDB instance, Neo4j, anchor-as-a-service |
| **6 — MCP + Streaming** | JSON-RPC 2.0 Model Context Protocol at `POST /api/mcp` exposing 10 tools. SSE streaming agent endpoint `/api/agent/stream`. Works with Claude Desktop, Cursor, and any MCP client. | Custom MCP bridges per project |
| **7 — Multimodal** | Chunked/resumable uploads at `/api/storage/upload/{init,chunk,status}`. Image transforms pipeline (resize/crop/format/quality) with lz4 byte cache. WebSocket diff engine emits `{added, removed, updated}` buckets instead of full replays. | tus, uppy, imgproxy, hand-rolled diff logic |
| **9 — SurrealDB Parity** | Strict schema mode (`schema_definitions`, 422 on violation). LIVE SELECT auto-registers WS subscriptions from DarshanQL. SQL passthrough at `POST /api/sql` (whitelisted DML, admin-only, audit-logged). | A second database for "just the SQL case" |
| **10 — Observability** | Prometheus metrics at `/metrics` (IP-allowlisted). `/health`, `/ready`, `/live` probes. Structured JSON logging with request_id propagation. Legacy rich health moved to `/health/full`. | A Grafana stack you don't have time to configure |

---

## Agent Memory: Unlimited LLM Context

DarshJDB ships a tiered agent memory model that gives any LLM agent (Claude, GPT, Gemini, open-source) effectively unlimited working memory without shelling out to Pinecone, Weaviate, Qdrant, or a bespoke MCP memory server.

| Tier        | Storage                               | Purpose                                                     | Latency |
| ----------- | ------------------------------------- | ----------------------------------------------------------- | ------- |
| **Working** | In-process DashMap window             | Hot turns for the current conversation                      | <1ms    |
| **Episodic**| Postgres `memory_entries` rows        | Recent turns evicted from the working window                | <10ms   |
| **Semantic**| pgvector HNSW + LLM-backed summaries  | Long-term recall. Compressed by the summariser at 50/100/200 turn thresholds | <50ms   |
| **Facts**   | `agent_facts` key/value per session   | Structured facts injected directly into the context budget  | <10ms   |

`ContextBuilder` uses a tiktoken counter to assemble a budget-bounded prompt: reverse-chronological working window, semantic top-K recall against the current query, and optional fact injection. When the episodic tier grows past 50, 100, or 200 entries, the summariser fires, asks the configured LLM provider (OpenAI, Ollama, Anthropic, or None) to compress the oldest block into a semantic summary, writes the compressed row, and deletes the source rows in a single transaction. A 500-turn conversation stays inside a 4k-token prompt window. A 5000-turn conversation does too.

### End-to-end example

```bash
# 1. Create a session
curl -X POST http://localhost:7700/api/agent/sessions \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"agent_id":"claude-opus","model":"claude-opus-4","system_prompt":"You are a travel agent."}'
# → {"session":{"id":"3f1e...","agent_id":"claude-opus",...}}

# 2. Append a user turn
curl -X POST http://localhost:7700/api/agent/sessions/3f1e.../messages \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"role":"user","content":"Book me a flight to Tokyo next Friday."}'

# 3. Build a prompt for the next LLM call — reverse-chron + semantic recall
curl "http://localhost:7700/api/agent/sessions/3f1e.../context?max_tokens=4096&current_query=tokyo%20flight&recall_top_k=5&include_facts=true" \
  -H "Authorization: Bearer $TOKEN"
# → {"messages":[...], "total_tokens": 842, "budget_remaining": 3254}
```

```python
import httpx

ddb = httpx.Client(base_url="http://localhost:7700",
                   headers={"Authorization": f"Bearer {token}"})

# Create session
session = ddb.post("/api/agent/sessions",
                   json={"agent_id": "claude-opus",
                         "model": "claude-opus-4"}).json()["session"]
sid = session["id"]

# Log turns
for role, content in turns:
    ddb.post(f"/api/agent/sessions/{sid}/messages",
             json={"role": role, "content": content})

# Build the next prompt — unlimited history compressed into a 4k budget
ctx = ddb.get(f"/api/agent/sessions/{sid}/context",
              params={"max_tokens": 4096,
                      "current_query": user_query,
                      "recall_top_k": 5,
                      "include_facts": True}).json()

messages = ctx["messages"]  # Pass straight to Anthropic / OpenAI / Ollama
```

No external services. No per-token pricing. One Rust binary.

---

## Redis Drop-in (RESP3, port 7701)

The `ddb-cache-server` binary speaks native RESP3 on port 7701. Any Redis client works. `ddb-cache` writes through an L1 DashMap (lz4-compressed, sub-microsecond) to an L2 Postgres table (zstd-compressed for values >=1KB) so the data survives restarts.

```bash
# Point any redis-cli at DarshJDB
redis-cli -p 7701 PING
# → PONG

redis-cli -p 7701 HELLO 3
redis-cli -p 7701 SET user:42 '{"name":"Alice"}' EX 3600
redis-cli -p 7701 GET user:42
# → "{\"name\":\"Alice\"}"

redis-cli -p 7701 HSET session:abc user_id 42 csrf_token tok_9f
redis-cli -p 7701 HGETALL session:abc

redis-cli -p 7701 ZADD leaderboard 1500 alice 2200 bob
redis-cli -p 7701 ZRANGEBYSCORE leaderboard 0 3000 WITHSCORES

redis-cli -p 7701 XADD events '*' type signup user_id 42
redis-cli -p 7701 XRANGE events - +
```

**Supported commands:** `AUTH`, `HELLO 3`, `PING`, `QUIT`, `INFO`, `GET`, `SET`, `DEL`, `EXISTS`, `EXPIRE`, `TTL`, `KEYS`, `HSET`, `HGET`, `HGETALL`, `HDEL`, `HLEN`, `LPUSH`, `RPUSH`, `LPOP`, `RPOP`, `LRANGE`, `ZADD`, `ZRANGE`, `ZRANGEBYSCORE`, `ZRANK`, `ZREM`, `ZSCORE`, `SUBSCRIBE`, `UNSUBSCRIBE`, `PUBLISH`, `XADD`, `XREAD`, `XRANGE`, `BFADD`, `BFEXISTS`, `PFADD`, `PFCOUNT`.

| Operation            | Redis           | DarshJDB (`ddb-cache`)       |
| -------------------- | --------------- | ---------------------------- |
| `GET` / `SET`        | Yes             | Yes                          |
| Hashes               | Yes             | Yes                          |
| Lists                | Yes             | Yes                          |
| Sorted sets          | Yes             | Yes                          |
| Streams (`XADD`)     | Yes             | Yes                          |
| Bloom filters        | RedisBloom mod  | Native (`BFADD` / `BFEXISTS`)|
| HyperLogLog          | Yes             | Native (`PFADD` / `PFCOUNT`) |
| Pub/Sub              | Yes             | Yes (RESP3 + WebSocket + SSE)|
| TTL / expiry         | Yes             | Yes (background reaper)      |
| Persistence          | RDB + AOF       | Postgres write-through       |
| Process              | Separate daemon | Same binary as the DB        |

An HTTP mirror at `/api/cache/*` exposes the same commands for clients that do not want to open a second socket. One fewer service to run. One fewer service to monitor. One fewer service to secure.

---

## Quick Start

Three ways to get a local server. Pick one.

### 1. Zero-dep Rust (recommended for hacking)

Build with the `embedded-db` feature and DarshJDB will download, launch, and shut down a portable Postgres 16 server for you on a free port. No `DATABASE_URL`, no docker, no `brew install postgresql`.

```bash
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb
cargo run --bin ddb-server --features embedded-db
```

First run downloads ~20 MB of Postgres binaries (one-time); subsequent runs are instant. Data lives in `~/.darshjdb/data/pg` and persists across restarts. If `DATABASE_URL` is set, the embedded server is skipped and the configured URL wins.

### 2. One-line installer (latest release binary)

```bash
curl -fsSL https://raw.githubusercontent.com/darshjme/darshjdb/main/scripts/install.sh | bash
~/.darshjdb/bin/ddb start
```

The installer pulls the correct binary from the GH Actions release matrix (`x86_64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`) and drops it at `$HOME/.darshjdb/bin/ddb`. Override with `DARSH_INSTALL_DIR=...` if you want it elsewhere.

### 3. Docker compose (BYO Postgres image)

```bash
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb
docker compose up -d
curl http://localhost:7700/health
```

Once the server is up:

```bash
# REST data plane
curl -X POST http://localhost:7700/api/auth/signup \
  -H "Content-Type: application/json" \
  -d '{"email":"dev@example.com","password":"changeme123"}'

curl -X POST http://localhost:7700/api/data/tasks \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <token>" \
  -d '{"title":"Ship v1","status":"active","priority":1}'

# Redis-compatible cache (RESP3 on 7701)
redis-cli -p 7701 PING                 # → PONG

# Liveness / readiness / metrics
curl http://localhost:7700/health
curl http://localhost:7700/ready
curl http://localhost:7700/metrics     # Prometheus, IP-allowlisted
```

---

## Architecture

```mermaid
graph TB
    subgraph Clients["Client SDKs"]
        TS["TypeScript"]
        React["React"]
        Next["Next.js"]
        Angular["Angular"]
        Python["Python"]
        PHP["PHP"]
    end

    subgraph Server["DarshJDB Server -- Single Rust Binary"]
        API["REST + WebSocket\nAxum + Tokio"]
        AUTH["Auth Engine\nArgon2id, JWT RS256\nOAuth, MFA, Magic Links"]
        PERM["Permission Engine\nRow-Level Security"]
        QE["Query Engine\nDarshJQL"]
        TS2["Triple Store\nEAV"]
        RT["Real-Time\nLive Queries + Pub/Sub"]
        STORE["Storage\nS3 / R2 / Local"]
    end

    PG[("PostgreSQL 16+\npgvector + tsvector")]

    Clients --> API
    API --> AUTH
    AUTH --> PERM
    PERM --> QE
    QE --> TS2
    TS2 --> PG
    API --> RT
    API --> STORE

    style Server fill:#0d1117,stroke:#F59E0B,color:#c9d1d9
    style PG fill:#336791,stroke:#fff,color:#fff
    style Clients fill:#161b22,stroke:#F59E0B,color:#c9d1d9
```

### Request Lifecycle

```mermaid
sequenceDiagram
    participant C as Client
    participant R as Router (Axum)
    participant A as Auth
    participant P as Permissions
    participant Q as Query Engine
    participant T as Triple Store
    participant PG as PostgreSQL

    C->>R: HTTP / WebSocket
    R->>A: Validate JWT
    A->>P: Load permission rules
    P->>Q: Inject RLS clauses
    Q->>T: DarshJQL to SQL
    T->>PG: Execute
    PG-->>T: Rows
    T-->>Q: Triples to entities
    Q-->>P: Filter restricted fields
    P-->>C: JSON response
```

### Module Map

29 server modules under `packages/server/src/`:

| Module | Purpose |
|--------|---------|
| `api` | HTTP route handlers, request/response types |
| `api_keys` | API key generation, validation, scoping |
| `activity` | Activity log, user action tracking |
| `aggregation` | COUNT, SUM, AVG, MIN, MAX, GROUP BY |
| `audit` | Merkle tree hash chain, tamper detection |
| `auth` | Signup, signin, OAuth, MFA/TOTP, magic links, token refresh |
| `automations` | Scheduled tasks, cron triggers |
| `cache` | DashMap query cache, ChangeEvent invalidation |
| `collaboration` | Presence, cursor sharing |
| `connectors` | Webhook + log connectors, entity-level sync |
| `embeddings` | pgvector HNSW, OpenAI/Ollama/NIM providers |
| `events` | Change feed, mutation event bus |
| `fields` | Field definitions, type validation |
| `formulas` | Computed fields, formula evaluation |
| `functions` | Server functions — Node.js/Deno subprocess default, or embedded V8 via `--features v8` (VYASA) |
| `graph` | RELATE, traversals, multi-hop queries |
| `history` | Record version history, point-in-time queries |
| `import_export` | JSON/CSV import, full database export |
| `plugins` | Plugin registry, lifecycle hooks |
| `query` | DarshJQL parser, AST, optimizer, executor |
| `relations` | Foreign key relations, record links |
| `rules` | Forward-chaining rules, triggered triples |
| `schema` | SCHEMALESS / SCHEMAFULL / SCHEMAMIXED modes |
| `storage` | File uploads: local FS, S3, R2, MinIO |
| `sync` | Real-time diff engine, WebSocket broadcast |
| `tables` | Table definitions, namespace management |
| `triple_store` | EAV storage layer, entity pool (UUID to i64) |
| `views` | Materialized views, virtual tables |
| `webhooks` | Outbound webhooks on mutation events |

---

## DarshJQL

DarshJQL is the query language built for DarshJDB. It borrows from SQL, graph query languages, and document query builders.

### Basic Query

```sql
-- Define a table
DEFINE TABLE user SCHEMAFULL;
DEFINE FIELD name ON user TYPE string ASSERT $value != NONE;
DEFINE FIELD email ON user TYPE string ASSERT string::is::email($value);
DEFINE INDEX idx_email ON user FIELDS email UNIQUE;

-- Create records
CREATE user SET name = 'Alice', email = 'alice@example.com';
CREATE user:bob SET name = 'Bob', email = 'bob@example.com';

-- Query with conditions
SELECT * FROM user WHERE email CONTAINS 'example.com' ORDER BY name ASC LIMIT 10;
-- Returns: [{id: "user:...", name: "Alice", email: "alice@example.com"}, ...]
```

### Graph Traversal

```sql
-- Define a relation
DEFINE TABLE follows SCHEMAFULL TYPE RELATION IN user OUT user;

-- Create edges
RELATE user:alice -> follows -> user:bob SET since = time::now();
RELATE user:alice -> follows -> user:carol;

-- Who does Alice follow?
SELECT ->follows->user.name FROM user:alice;
-- Returns: ["Bob", "Carol"]

-- Who follows Bob? (reverse traversal)
SELECT <-follows<-user.name FROM user:bob;
-- Returns: ["Alice"]

-- Friends of friends (multi-hop)
SELECT ->follows->user->follows->user.name FROM user:alice;
```

### Full-Text Search

```sql
-- String and math functions
SELECT string::uppercase(name), math::mean(scores) FROM student;

-- Time-based queries
SELECT * FROM event WHERE created > time::now() - 7d;

-- Vector search (semantic similarity)
SELECT * FROM document WHERE embedding <|4|> $query_vector;

-- Geo queries
SELECT * FROM restaurant WHERE geo::distance(location, $user_location) < 5km;
```

### Real-Time Subscription

```sql
-- Subscribe to all changes on a table
LIVE SELECT * FROM user;

-- Subscribe with filters
LIVE SELECT * FROM user WHERE country = 'IN';

-- Diff mode -- only changed fields
LIVE SELECT DIFF FROM user;
```

When a mutation matches a LIVE SELECT, DarshJDB pushes the change over WebSocket. No polling, no external message broker. DarshanQL also supports a whitelisted SQL passthrough at `POST /api/sql` (admin-only, audit-logged) for the "just give me SQL" case.

---

## Hybrid Search (pgvector + FTS + RRF)

Three search endpoints, backed by pgvector HNSW indexes and Postgres `ts_rank` FTS:

```bash
# Pure semantic (cosine over HNSW)
curl -X POST http://localhost:7700/api/search/semantic \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "vector": [0.012, -0.084, ...],
    "entity_type": "article",
    "limit": 10
  }'

# Full-text
curl "http://localhost:7700/api/search/text?q=rust+database&entity_type=article&limit=10" \
  -H "Authorization: Bearer $TOKEN"

# Hybrid — Reciprocal Rank Fusion (k=60, Cormack et al.)
curl -X POST http://localhost:7700/api/search/hybrid \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query_text": "how does the triple store work",
    "vector": [0.012, -0.084, ...],
    "entity_type": "doc",
    "attribute": "body",
    "limit": 10,
    "weights": { "semantic": 1.0, "text": 1.0 }
  }'
```

The hybrid handler pulls a 4x candidate window from each side, fuses with `score(d) = Σ wᵢ / (k + rankᵢ(d))`, then returns the top `limit` rows. Embedding generation is caller-side on purpose — bring your OpenAI / Ollama / Anthropic / NIM provider of choice, or let the built-in embedding worker handle it.

---

## Model Context Protocol

DarshJDB is a native Model Context Protocol server. Point Claude Desktop, Cursor, or any MCP client at `POST /api/mcp` and it exposes 10 first-class tools:

| Tool | Purpose |
|------|---------|
| `ddb_query` | Execute a DarshJQL query |
| `ddb_mutate` | Apply a batch of create/update/delete operations |
| `ddb_semantic_search` | Vector similarity over embeddings |
| `ddb_memory_store` | Persist a chat-style memory turn |
| `ddb_memory_recall` | Retrieve chat memory for a session |
| `ddb_graph_traverse` | BFS/DFS over graph edges |
| `ddb_timeseries` | Time-bucketed aggregation of events |
| `ddb_cache_get` | Read a value from the hot KV cache |
| `ddb_cache_set` | Write a value into the hot KV cache |
| `ddb_kv_list` | List cache keys matching a pattern |

Drop this into `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "darshjdb": {
      "command": "ddb-server",
      "args": ["--embedded-db", "--mcp-stdio"],
      "env": {
        "DARSHJDB_BEARER": "<your-token>"
      }
    }
  }
}
```

Or point an HTTP MCP client directly at the running server:

```bash
curl -X POST http://localhost:7700/api/mcp \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

An SSE streaming endpoint at `/api/agent/stream` pushes intermediate tokens for clients that want to render model output progressively instead of waiting on a single response.

---

## Features

### Data

| Feature | Description |
|---------|-------------|
| Triple store (EAV) | Every record is a set of (entity, attribute, value) triples over PostgreSQL |
| Typed fields | String, number, boolean, datetime, record, array, object, geometry |
| Schema modes | SCHEMALESS (dev), SCHEMAFULL (prod), SCHEMAMIXED (migration), strict mode (`schema_definitions`, 422 on violation) |
| Views | Materialized views, virtual tables |
| Formulas | Computed fields, formula evaluation |
| Relations | Record links, foreign keys, graph edges |
| Aggregation | COUNT, SUM, AVG, MIN, MAX, GROUP BY |
| Full-text search | PostgreSQL tsvector + GIN indexes, `ts_rank` ordering |
| Vector search | pgvector HNSW + IVFFlat, cosine/euclidean/dot product |
| Hybrid search | Reciprocal Rank Fusion (`k=60`) over semantic + FTS |
| Entity Pool | UUID-to-i64 dictionary encoding for performance |
| SQL passthrough | Whitelisted DML at `POST /api/sql`, admin-only, audit-logged |

### AI & Agents

| Feature | Description |
|---------|-------------|
| Agent memory | 3-tier hierarchy (working / episodic / semantic) + `agent_facts` injection |
| Unlimited context | LLM-backed episodic → semantic summariser at 50/100/200 thresholds |
| Context builder | tiktoken budget, reverse-chron window, semantic top-K recall |
| Embedding worker | OpenAI, Ollama, Anthropic, None (pluggable), 5s batch tick |
| MCP server | JSON-RPC 2.0 at `/api/mcp`, 10 tools, Claude Desktop / Cursor compatible |
| Streaming agent | SSE endpoint at `/api/agent/stream` |
| Sessions API | `POST /api/agent/sessions`, `/messages`, `/context`, `/search`, `/timeline`, `/stats`, `/facts` |

### Caching & Streams (`ddb-cache`)

| Feature | Description |
|---------|-------------|
| RESP3 server | Port 7701, drop-in Redis client compatibility |
| L1 cache | In-process DashMap, lz4-compressed, sub-microsecond |
| L2 cache | Postgres-backed, zstd-compressed (values >=1KB), write-through |
| Data structures | Strings, hashes, lists, sorted sets, streams (`XADD`/`XREAD`/`XRANGE`) |
| Probabilistic | Bloom filter (`BFADD`/`BFEXISTS`), HyperLogLog (`PFADD`/`PFCOUNT`) |
| Pub/Sub | RESP3 `SUBSCRIBE`/`PUBLISH`, plus WS + SSE mirrors |
| HTTP mirror | `/api/cache/*` for clients that don't want a second socket |

### Time-series & Graph

| Feature | Description |
|---------|-------------|
| Hypertables | TimescaleDB `time_series` table at `/api/ts/{entity_type}` |
| TS endpoints | Insert, range, aggregate (bucket/window), latest |
| Graph edges | `edges` table with BFS traversal |
| Graph endpoints | `/api/graph/relate`, `/traverse`, `/neighbors`, `/outgoing`, `/incoming` |

### Auth

| Feature | Description |
|---------|-------------|
| Password auth | Argon2id (64MB memory, 3 iterations, OWASP recommended) |
| OAuth | Google, GitHub, Apple, Discord, and more |
| MFA / TOTP | Time-based one-time passwords |
| Magic links | Email-based passwordless login (SMTP + SendGrid + dev-log backends) |
| JWT RS256 | Asymmetric token signing, 15min access + 7day refresh |
| Token refresh | Automatic rotation with SHA256-hashed refresh tokens |
| Session hardening | 24h absolute timeout, max 5 concurrent sessions per user |
| Login rate limit | Exponential backoff after 5 failures, lock after 10 for 3600s |
| Admin RBAC | Cryptographic JWT validation of `admin` role — no stubs |

### Real-Time

| Feature | Description |
|---------|-------------|
| WebSocket subscriptions | LIVE SELECT pushes `{added, removed, updated}` diff buckets |
| WS diff engine | Incremental diffs only — full replays are opt-in |
| Presence | Online status, cursor positions |
| Pub/Sub | WebSocket channels + SSE, pattern matching |
| SSE subscriptions | entity_type + where-clause re-evaluation on every mutation |
| Permission-filtered | Each client only receives data they are authorized to see |

### Multimodal Storage

| Feature | Description |
|---------|-------------|
| File storage | S3, Cloudflare R2, MinIO, local filesystem |
| Path traversal safe | Sanitized paths on upload (Phase 0 fix) |
| Chunked uploads | `/api/storage/upload/{init, chunk, status}` for resumable transfers |
| Image transforms | Resize / crop / format / quality pipeline with lz4 byte cache |

### Infrastructure

| Feature | Description |
|---------|-------------|
| Webhooks | Outbound HTTP on mutation events |
| API keys | Scoped keys for service-to-service auth |
| Plugins | Plugin registry with lifecycle hooks |
| Automations | Cron-triggered scheduled tasks |
| Server functions | Node.js / Deno subprocess execution — or embedded V8 isolate via `cargo run --features v8` (VYASA, sub-ms cold start) |
| Connectors | Webhook + log connectors, entity-level sync |
| TTL / Expiry | Per-entity expiry with background reaper |
| Batch API | Multiple operations in a single request |

### DevEx

| Feature | Description |
|---------|-------------|
| Embedded Postgres | `cargo run --features embedded-db` — zero external deps |
| One-line installer | `scripts/install.sh` fetches the right release binary |
| Release matrix | linux-musl, darwin-aarch64, windows-msvc via GH Actions |
| CLI (`ddb`) | start, sql console, import, export, status |
| Admin dashboard | React + Vite + Tailwind, embedded via `include_dir!` |
| 7 SDKs | TypeScript, React, Next.js, Angular, Python, PHP, cURL |
| Import / Export | JSON and CSV, full database backup and restore |
| Forward-chaining rules | Trigger implied triples on mutation |

### Observability

| Feature | Description |
|---------|-------------|
| Prometheus metrics | `/metrics` (IP-allowlisted) |
| Health probes | `/health`, `/ready`, `/live` — plus legacy rich `/health/full` |
| Structured logging | JSON logs with `request_id` propagation |

### Integrity

| Feature | Description |
|---------|-------------|
| Merkle audit trail | SHA-512 hash chain, tamper detection, verification endpoints |
| Blockchain anchor | SHA3-Keccak aggregate Merkle roots, optional IPFS / Ethereum submission (`anchor-ipfs`, `anchor-eth` feature flags) |
| Row-level security | WHERE clause injection per user on every query |
| Field-level filtering | Restricted fields stripped from responses |
| Rate limiting | Token bucket per IP and per user |
| TLS | Native rustls via `DDB_TLS_CERT` / `DDB_TLS_KEY` |
| CORS | Environment-aware origin configuration |

---

## SDKs

### TypeScript

```typescript
import { DDB } from '@darshjdb/client';

const db = new DDB({ serverUrl: 'http://localhost:7700' });
await db.signin({ email: 'dev@example.com', password: 'changeme123' });

const task = await db.create('task', {
  title: 'Ship v1',
  status: 'active'
});

const tasks = await db.select('task', {
  where: { status: 'active' },
  orderBy: { created: 'desc' },
  limit: 10
});
```

### React

```tsx
import { useQuery, useMutation, useAuth } from '@darshjdb/react';

function TaskList() {
  const { data, loading } = useQuery({
    tasks: { $where: { status: 'active' } }
  });

  const [createTask] = useMutation('task');

  if (loading) return <p>Loading...</p>;
  return data.tasks.map(t => <div key={t.id}>{t.title}</div>);
}
```

### Next.js

```typescript
import { createServerClient } from '@darshjdb/nextjs';

// Server Component
export default async function Page() {
  const db = createServerClient();
  const tasks = await db.select('task', { where: { status: 'active' } });
  return <ul>{tasks.map(t => <li key={t.id}>{t.title}</li>)}</ul>;
}
```

### Angular

```typescript
import { injectDarshan } from '@darshjdb/angular';

@Component({ /* ... */ })
export class TaskComponent {
  private readonly db = injectDarshan();
  tasks = this.db.query('task', { where: { status: 'active' } });
}
```

### Python

```python
from darshjdb import DarshJDB, AsyncDarshJDB

db = DarshJDB("http://localhost:7700")
db.signin(email="dev@example.com", password="changeme123")

task = db.create("task", {"title": "Ship v1", "status": "active"})
tasks = db.select("task", where={"status": "active"}, limit=10)

# Async (FastAPI)
adb = AsyncDarshJDB("http://localhost:7700")

@app.get("/tasks")
async def get_tasks():
    return await adb.select("task", limit=50)
```

### PHP

```php
use Darshjdb\Client;

$db = new Client('http://localhost:7700');
$db->signin(['email' => 'dev@example.com', 'password' => 'changeme123']);

$task = $db->create('task', ['title' => 'Ship v1', 'status' => 'active']);
$tasks = $db->select('task', ['where' => ['status' => 'active'], 'limit' => 10]);
```

---

## Comparison

An honest comparison. "Yes" means the feature is implemented in-tree. Dashes mean it is not available.

| | DarshJDB | Firebase | Supabase | Convex | PocketBase |
|---|---|---|---|---|---|
| Self-hosted | Yes | No | Yes | No | Yes |
| Real-time | WS + SSE + RESP3 | WebSocket | WebSocket | WebSocket | SSE |
| Auth (built-in) | Yes | Yes | Yes | Yes | Yes |
| File storage | S3/R2/Local + chunked + transforms | Yes | Yes | Yes | Local |
| Custom query language | DarshJQL | No | SQL | JS/TS | No |
| Graph traversal | Yes | No | No | No | No |
| Vector search | pgvector HNSW + RRF hybrid | No | pgvector | No | No |
| Agent memory (unlimited context) | Yes (3 tiers + summariser) | No | No | No | No |
| Redis-compatible cache | Yes (RESP3 port 7701) | No | No | No | No |
| Time-series hypertables | Yes (Timescale) | No | Yes (Timescale ext) | No | No |
| MCP server | Yes (10 tools) | No | No | No | No |
| Blockchain anchoring | Yes (Merkle + IPFS/ETH) | No | No | No | No |
| Triple store / EAV | Yes | No | No | No | No |
| Schema modes (strict + flexible) | Yes | Flexible only | Strict only | Strict only | Flexible only |
| Single binary | Yes | Cloud | Multi-service | Cloud | Yes |
| Runs on 512MB RAM | Yes | N/A | No | N/A | Yes |
| Production-hardened | No (alpha) | Yes | Yes | Yes | Yes |
| Hosted option | No | Yes | Yes | Yes | No |
| Price | Free (MIT) | Free tier + pay | Free tier + pay | Free tier + pay | Free (MIT) |

---

## What Works and What Doesn't

Two columns. No hedging.

| Working | Alpha / Incomplete |
|---------|-------------------|
| REST API: full CRUD over triple store | npm / crates.io packages not yet published |
| Auth: signup, signin, JWT, refresh, magic links, admin RBAC | No hosted documentation site |
| OAuth: Google, GitHub, Apple, Discord | No performance benchmarks published |
| MFA / TOTP | No horizontal scaling / multi-node |
| Session hardening + exponential login rate limit | Mobile SDKs (Swift, Kotlin) not started |
| Row-level + field-level security | Embedded V8 runtime available via `--features v8` (VYASA), subprocess remains default |
| WebSocket diff engine ({added, removed, updated}) | Phone OTP auth not implemented |
| LIVE SELECT auto-registration | No production deployment under real traffic |
| DarshJQL parser, optimizer, executor | Auth test suite still relies on live Postgres (testcontainers migration pending) |
| SQL passthrough (admin-only, audit-logged) | Typed `DdbConfig` hierarchy (Slice 17) still on the roadmap |
| Graph relations, BFS traversal, `/api/graph/*` | Graph visualisation UI (Slice 22) not started |
| Full-text search (`ts_rank`) | |
| Vector search (pgvector HNSW + IVFFlat) | |
| Hybrid search (Reciprocal Rank Fusion, k=60) | |
| Agent memory tiers + summariser (50/100/200) | |
| Embedding worker (OpenAI / Ollama / Anthropic / None) | |
| RESP3 cache server on port 7701 | |
| Hashes, lists, sorted sets, streams, bloom, HLL | |
| TimescaleDB hypertable at `/api/ts/*` | |
| MCP JSON-RPC server + 10 tools | |
| SSE streaming agent endpoint | |
| Chunked / resumable uploads | |
| Image transforms pipeline with lz4 cache | |
| Strict schema mode (`schema_definitions`) | |
| Prometheus `/metrics` + `/health` / `/ready` / `/live` | |
| Structured JSON logging with `request_id` | |
| Embedded Postgres 16 (`--features embedded-db`) | |
| Admin dashboard embedded via `include_dir!` | |
| One-line installer (`scripts/install.sh`) | |
| GH Actions release matrix (linux-musl / darwin-aarch64 / windows-msvc) | |
| File storage (S3, R2, MinIO, local) | |
| Merkle audit trail + optional IPFS / Ethereum anchor | |
| Rate limiting (token bucket) | |
| CLI: start, sql, import, export | |
| 7 SDKs with tests | |

---

## Roadmap

v0.3.0 **Grand Transformation** — this release. Everything in the Feature Grid above.

Coming next:

- **Slice 17 — Typed `DdbConfig` hierarchy.** Defaults → `config.toml` → `DARSH_*` env override, threaded through `main.rs`. Ends the "read env var at the call site" pattern that survives today in the schema and anchor modules.
- **Slice 22 — Graph UI.** Visual editor for the `edges` table in the embedded admin dashboard: pan/zoom, live filter on `/api/graph/neighbors`, node inspector.
- **testcontainers for auth tests.** The auth test harness still requires a live Postgres and `unsafe` env mutation gated to `#[cfg(test)]`. Replacing it with testcontainers unblocks CI on hosts without Postgres preinstalled.
- **npm + crates.io publish.** The release binaries and install script are in place; the package metadata work landed in Slice 30. Actual `cargo publish` / `npm publish` runs are the next ship task.
- **Phase 8 — production hardening pass.** Soak tests, fuzz targets, chaos drills, and the first public performance benchmarks.

Looking further out: mobile SDKs (Swift, Kotlin), V8-in-process server functions, horizontal scaling via logical replication, and a hosted `db.darshj.me` control plane.

---

## Contributing

```bash
# Prerequisites: Rust 1.85+, optional Postgres 16+ (or use --features embedded-db), Node.js 20+

# Clone and build
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb
cargo build --workspace

# Option A — zero-dep: embedded Postgres
cargo run --bin ddb-server --features embedded-db

# Option B — bring your own Postgres
docker compose up postgres -d
DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshjdb \
  cargo run --bin ddb-server

# Rust tests (workspace: server, cache, cache-server, agent-memory, cli)
cargo test --workspace

# TypeScript SDK tests
cd packages/tests && npm install && npm test

# Python SDK tests
cd sdks/python && pip install -e . && pytest

# PHP SDK tests
cd sdks/php && composer install && composer test
```

Read [CONTRIBUTING.md](CONTRIBUTING.md) for code style, PR process, and architecture decisions. Read [SECURITY.md](SECURITY.md) for reporting vulnerabilities.

The project is alpha. There is real work to do. If you care about self-hosted infrastructure and developer tools, pull requests are welcome.

---

## License & Attribution

MIT. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

DarshJDB is the original creation of Darshankumar Joshi. The triple-store architecture over PostgreSQL, the DarshanQL query language, the 3-tier agent memory model, the Merkle audit chain, and all associated SDKs are original works. Workspace authors, homepage (`db.darshj.me`), repository, and keywords are wired into `Cargo.toml` at the workspace root.

---

<div align="center">

Built by [Darshankumar Joshi](https://darshj.ai) in Navsari and Ahmedabad, Gujarat, India.

[db.darshj.me](https://db.darshj.me) | [GitHub](https://github.com/darshjme/darshjdb) | [darshj.ai](https://darshj.ai)

</div>
