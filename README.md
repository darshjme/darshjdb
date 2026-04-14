<div align="center">

<img src=".github/assets/hero.svg" alt="DarshJDB" width="100%" />

<br/>

[![License: MIT](https://img.shields.io/badge/License-MIT-F59E0B.svg?style=for-the-badge)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-B7410E.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![PostgreSQL 16+](https://img.shields.io/badge/PostgreSQL-16+-336791.svg?style=for-the-badge&logo=postgresql&logoColor=white)](https://www.postgresql.org)
[![Status: Alpha](https://img.shields.io/badge/Status-Alpha-orange.svg?style=for-the-badge)](https://github.com/darshjme/darshjdb)
[![CI](https://img.shields.io/github/actions/workflow/status/darshjme/darshjdb/ci.yml?style=for-the-badge&label=CI)](https://github.com/darshjme/darshjdb/actions)
[![Tests: 731+](https://img.shields.io/badge/Tests-731_passing-86efac.svg?style=for-the-badge)](https://github.com/darshjme/darshjdb)

<br/>

**Self-hosted Backend-as-a-Service. Single Rust binary. Triple-store EAV over PostgreSQL.**

[Quick Start](#quick-start) | [DarshJQL](#darshjql) | [Architecture](#architecture) | [SDKs](#sdks) | [Comparison](#comparison) | [Contributing](#contributing)

</div>

---

## What is DarshJDB

DarshJDB is a self-hosted backend that ships as one Rust binary. It handles authentication, real-time subscriptions, permissions, file storage, and a query engine -- all backed by a triple-store (Entity-Attribute-Value) data model over PostgreSQL. You connect from TypeScript, React, Next.js, Angular, Python, or PHP. It runs on a machine with 512MB of RAM.

This is alpha software. It has 731+ tests. It works. It is not production-hardened yet.

---

## Why DarshJDB Exists

Started in 2021 in Ahmedabad, India. Darshankumar Joshi was building apps for clients at his company (GraymatterOnline) and got frustrated -- every project needed auth, real-time, storage, permissions, and a query engine. Firebase was locked-in, Supabase was Postgres but heavy, Convex was innovative but cloud-only. He wanted something he could run on a $5 VPS, own completely, and extend without limits.

So he started building DarshJDB -- one Rust binary that handles everything. Not because the world needed another database, but because the apps he was building needed a better backend.

Five years later, it absorbs ideas from 8 different systems (PostgreSQL, GraphDB, Redis, pgvector, Bitcoin, Solana, InstantDB, Convex) and runs on a machine with 512MB of RAM.

---

## Quick Start

```bash
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb
docker compose up -d
curl http://localhost:7700/health
# Redis-compatible cache (RESP3 protocol, Slice 11+).
redis-cli -p 7701 PING                 # → PONG
curl http://localhost:7700/api/cache/stats
```

Write data:

```bash
# Sign up
curl -X POST http://localhost:7700/api/auth/signup \
  -H "Content-Type: application/json" \
  -d '{"email":"dev@example.com","password":"changeme123"}'

# Create a record (use the token from signup response)
curl -X POST http://localhost:7700/api/data/tasks \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <token>" \
  -d '{"title":"Ship v1","status":"active","priority":1}'

# Query it back
curl http://localhost:7700/api/data/tasks \
  -H "Authorization: Bearer <token>"
```

Build from source:

```bash
docker compose up postgres -d
DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshjdb \
  cargo run --bin ddb-server
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
| `functions` | Server functions, Node.js/Deno subprocess execution |
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

When a mutation matches a LIVE SELECT, DarshJDB pushes the change over WebSocket. No polling, no external message broker.

---

## Features

### Data

| Feature | Description |
|---------|-------------|
| Triple store (EAV) | Every record is a set of (entity, attribute, value) triples over PostgreSQL |
| Typed fields | String, number, boolean, datetime, record, array, object, geometry |
| Schema modes | SCHEMALESS (dev), SCHEMAFULL (prod), SCHEMAMIXED (migration) |
| Views | Materialized views, virtual tables |
| Formulas | Computed fields, formula evaluation |
| Relations | Record links, foreign keys, graph edges |
| Aggregation | COUNT, SUM, AVG, MIN, MAX, GROUP BY |
| Full-text search | PostgreSQL tsvector + GIN indexes |
| Vector search | pgvector HNSW, cosine/euclidean/dot product |
| Entity Pool | UUID-to-i64 dictionary encoding for performance |

### Auth

| Feature | Description |
|---------|-------------|
| Password auth | Argon2id (64MB memory, 3 iterations, OWASP recommended) |
| OAuth | Google, GitHub, Apple, Discord, and more |
| MFA / TOTP | Time-based one-time passwords |
| Magic links | Email-based passwordless login |
| JWT RS256 | Asymmetric token signing, 15min access + 7day refresh |
| Token refresh | Automatic rotation of access and refresh tokens |

### Real-Time

| Feature | Description |
|---------|-------------|
| WebSocket subscriptions | LIVE SELECT pushes diffs to connected clients |
| Presence | Online status, cursor positions |
| Pub/Sub | WebSocket channels + SSE, pattern matching |
| Incremental diffs | Only changed fields are transmitted |
| Permission-filtered | Each client only receives data they are authorized to see |

### Infrastructure

| Feature | Description |
|---------|-------------|
| File storage | S3, Cloudflare R2, MinIO, local filesystem |
| Webhooks | Outbound HTTP on mutation events |
| API keys | Scoped keys for service-to-service auth |
| Plugins | Plugin registry with lifecycle hooks |
| Automations | Cron-triggered scheduled tasks |
| Server functions | Node.js / Deno subprocess execution |
| Connectors | Webhook + log connectors, entity-level sync |
| TTL / Expiry | Per-entity expiry with background reaper |
| Batch API | Multiple operations in a single request |

### DevEx

| Feature | Description |
|---------|-------------|
| CLI (`ddb`) | start, sql console, import, export, status |
| Admin dashboard | React + Vite + Tailwind, live data explorer |
| 7 SDKs | TypeScript, React, Next.js, Angular, Python, PHP, cURL |
| Import / Export | JSON and CSV, full database backup and restore |
| Forward-chaining rules | Trigger implied triples on mutation |

### Integrity

| Feature | Description |
|---------|-------------|
| Merkle audit trail | SHA-512 hash chain, tamper detection, verification endpoints |
| Row-level security | WHERE clause injection per user on every query |
| Field-level filtering | Restricted fields stripped from responses |
| Rate limiting | Token bucket per IP and per user |
| TLS | Native rustls via DDB_TLS_CERT / DDB_TLS_KEY |
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
import { DarshanService } from '@darshjdb/angular';

@Component({ /* ... */ })
export class TaskComponent {
  tasks = inject(DarshanService).query('task', {
    where: { status: 'active' }
  });
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
use Darshan\DarshJDB\Client;

$db = new Client('http://localhost:7700');
$db->signin(['email' => 'dev@example.com', 'password' => 'changeme123']);

$task = $db->create('task', ['title' => 'Ship v1', 'status' => 'active']);
$tasks = $db->select('task', ['where' => ['status' => 'active'], 'limit' => 10]);
```

---

## Comparison

An honest comparison. Checkmarks mean the feature is implemented and tested. Dashes mean it is not available.

| | DarshJDB | Firebase | Supabase | Convex | PocketBase |
|---|---|---|---|---|---|
| Self-hosted | Yes | No | Yes | No | Yes |
| Real-time | WebSocket + SSE | WebSocket | WebSocket | WebSocket | SSE |
| Auth (built-in) | Yes | Yes | Yes | Yes | Yes |
| File storage | S3/R2/Local | Yes | Yes | Yes | Local |
| Custom query language | DarshJQL | No | SQL | JS/TS | No |
| Graph traversal | Yes | No | No | No | No |
| Vector search | pgvector HNSW | No | pgvector | No | No |
| Triple store / EAV | Yes | No | No | No | No |
| Merkle audit trail | Yes | No | No | No | No |
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
| REST API: full CRUD over triple store | npm / crates.io packages not published |
| Auth: signup, signin, JWT, token refresh | No install script (`curl ... \| sh`) |
| OAuth: Google, GitHub, Apple, Discord | No hosted documentation site |
| MFA / TOTP | No performance benchmarks published |
| Magic links | No horizontal scaling / multi-node |
| Row-level + field-level security | Mobile SDKs (Swift, Kotlin) not started |
| WebSocket subscriptions with diffs | Server function V8 runtime is subprocess-based |
| DarshJQL parser, optimizer, executor | Phone OTP auth not implemented |
| Graph relations and traversals | No production deployment under real traffic |
| Full-text search (tsvector) | CI badge may be red (binary rename in progress) |
| Vector search (pgvector HNSW) | |
| File storage (S3, R2, local) | |
| Admin dashboard (React + Vite) | |
| Merkle audit trail | |
| Rate limiting (token bucket) | |
| CLI: start, sql, import, export | |
| 7 SDKs with tests | |
| Docker Compose deployment | |
| Entity Pool (UUID to i64 encoding) | |
| Query cache with invalidation | |
| Pub/Sub channels + SSE | |

---

## Contributing

```bash
# Prerequisites: Rust 1.85+, PostgreSQL 16+, Node.js 20+

# Clone and build
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb
cargo build --workspace

# Start Postgres
docker compose up postgres -d

# Run Rust tests (446 tests)
cargo test --workspace

# Run TypeScript SDK tests (92 tests)
cd packages/tests && npm install && npm test

# Run Python SDK tests (141 tests)
cd sdks/python && pip install -e . && pytest

# Run PHP SDK tests (52 tests)
cd sdks/php && composer install && composer test
```

Read [CONTRIBUTING.md](CONTRIBUTING.md) for code style, PR process, and architecture decisions. Read [SECURITY.md](SECURITY.md) for reporting vulnerabilities.

The project is alpha. There is real work to do. If you care about self-hosted infrastructure and developer tools, pull requests are welcome.

---

## License

MIT. See [LICENSE](LICENSE).

---

<div align="center">

Built by [Darshankumar Joshi](https://darshj.ai) in Ahmedabad, India.

[db.darshj.me](https://db.darshj.me) | [GitHub](https://github.com/darshjme/darshjdb) | [darshj.ai](https://darshj.ai)

</div>
