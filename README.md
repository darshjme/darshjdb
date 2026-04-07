<div align="center">

<img src=".github/assets/hero.svg" alt="DarshJDB" width="100%" />

<br/>

[![License: MIT](https://img.shields.io/badge/License-MIT-F59E0B.svg?style=for-the-badge)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-B7410E.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![PostgreSQL 16+](https://img.shields.io/badge/PostgreSQL-16+-336791.svg?style=for-the-badge&logo=postgresql&logoColor=white)](https://www.postgresql.org)
[![Status: Alpha](https://img.shields.io/badge/Status-Alpha-orange.svg?style=for-the-badge)](https://github.com/darshjme/darshjdb)
[![CI](https://img.shields.io/github/actions/workflow/status/darshjme/darshjdb/ci.yml?style=for-the-badge&label=CI)](https://github.com/darshjme/darshjdb/actions)
[![Tests: 731](https://img.shields.io/badge/Tests-731_passing-86efac.svg?style=for-the-badge)](https://github.com/darshjme/darshjdb)

<br/>

**The multi-model database for the next decade of applications.**
**Document + Graph + Relational + KV + Vector — one binary, zero dependencies.**

[Getting Started](#quick-start) | [DarshQL](#darshql) | [Multi-Model](#multi-model-storage) | [Architecture](#architecture) | [Documentation](docs/) | [Contributing](#contributing)

</div>

---

## The Story

I grew up in Navsari, a small town in southern Gujarat where Parsi fire temples stand next to Hindu mandirs and the evening chai tastes like monsoon rain. My grandfather would say *"darshan karo"* every morning — see clearly, perceive the truth of things before you act.

I didn't know it then, but that word would follow me across three countries.

London first. Business Computing at Greenwich, Advanced Diploma at Sunderland. The cold taught me discipline. The coursework taught me systems thinking. But what I actually learned was watching how software got built in the West — and how the tools were locked behind expensive cloud services and FAANG-tier engineering budgets.

Then Dubai. VFX production. I worked on pipelines for Aquaman, The Invisible Man, The Last of Us Part II, and India's first NFT-funded film. When a render farm processes terabytes and a creative team of forty people needs it to just work, you learn what failure costs. You learn to build systems that don't go down at 2am because someone's free tier expired.

Back to India. Ahmedabad. Founded GraymatterOnline in 2015. Then Graymatter International. Then Coeus Digital Media. Then KnowAI, where we run 60+ autonomous agents managing enterprise operations. Four companies across a decade. Every single one hit the same wall.

The backend.

Three weeks of plumbing before writing one line of business logic. Postgres setup. REST APIs. Auth. WebSockets. File uploads. Permissions. The same work, repeated, for every project.

Firebase gives you NoSQL spaghetti. Supabase bolts real-time onto REST. InstantDB is cloud-only. Convex is a black box. None of them let you run a single binary on a $5 VPS in Mumbai and own your data completely.

So I built what I wanted. I called it DarshJDB.

*"Darshan"* means to see, to perceive the complete picture. The database sees every change, every query, every permission boundary. It sees what each user is allowed to see. And it shows them exactly that — in real-time, the moment anything changes.

```mermaid
graph LR
    subgraph Journey["The Path"]
        N["Navsari\n<i>where it started</i>"] --> L["London\n<i>systems thinking</i>"]
        L --> D["Dubai\n<i>production systems</i>"]
        D --> A["Ahmedabad\n<i>four companies</i>"]
        A --> DB["DarshJDB\n<i>building the tool\nI always needed</i>"]
    end

    style N fill:#cc9933,color:#000
    style L fill:#1a1a2e,color:#fff
    style D fill:#0f3460,color:#fff
    style A fill:#14532d,color:#fff
    style DB fill:#B7410E,color:#fff
```

---

## The Philosophy

The Bhagavad Gita says: *karmanye vadhikaraste ma phaleshu kadachana*. You have the right to work, but never to the fruit of work.

Build because building is dharma. Ship because shipping serves others. Open-source because knowledge locked away is knowledge wasted.

The Sompura Brahmins of Gujarat carved stone into temples that outlasted empires. Modhera. Somnath. Dilwara. The tools changed — chisel became compiler, sandstone became silicon — but the intent stays the same. Build something permanent. Build something that serves.

```mermaid
flowchart TD
    D["Dharma\n<i>What must be built?</i>"] --> V["Vichara\n<i>Think from first principles</i>"]
    V --> K["Karma\n<i>Write the code</i>"]
    K --> S["Seva\n<i>Open-source it</i>"]
    S --> L["Loka\n<i>The world builds on top</i>"]
    L --> D

    style D fill:#cc9933,color:#000
    style V fill:#1a1a2e,color:#fff
    style K fill:#0d1117,color:#fff
    style S fill:#14532d,color:#fff
    style L fill:#1e3a5f,color:#fff
```

---

## What DarshJDB Is

A single Rust binary that replaces your entire backend stack. Authentication. Permissions. Real-time subscriptions. Query engine. Graph relations. Vector search. Admin dashboard. Connect from React, Angular, Next.js, PHP, Python, or cURL.

DarshJDB is a **multi-model database** — document store, graph database, relational tables, key-value cache, and vector search engine — unified under one query language called **DarshQL**. The data model is a triple store (Entity-Attribute-Value) over PostgreSQL. No rigid schemas, no migrations during development. Write data first — structure emerges from usage. When you're ready for production, switch to `SCHEMAFULL` mode and lock it down.

Three schema modes, one database:
- **`SCHEMALESS`** — write anything, schema inferred from data (development)
- **`SCHEMAFULL`** — strict types, validated on write (production)
- **`SCHEMAMIXED`** — defined fields are strict, unknown fields pass through (migration)

This is alpha software. It works. It has 731 tests proving it works. But it is not production-hardened yet. Use it to prototype, learn the architecture, and contribute. Don't put your startup's production data on it today.

---

## What Works Today

Evidence, not promises.

```mermaid
graph TB
    subgraph working["Working End-to-End"]
        direction TB
        REST["REST API\n<i>Write and read triples\nvia HTTP</i>"]
        AUTH["Authentication\n<i>Signup, signin, JWT\nArgon2id hashing</i>"]
        PERM["Row-Level Permissions\n<i>Every request evaluated\nagainst permission rules</i>"]
        QE["Query Engine\n<i>DarshanQL parses, plans\nand executes against Postgres</i>"]
        WS["WebSocket Subscriptions\n<i>Mutation broadcasts\ndiff push to clients</i>"]
        DASH["Admin Dashboard\n<i>Live data view\nfrom the running server</i>"]
    end

    subgraph partial["In Progress"]
        direction TB
        FN["Server Functions\n<i>Registry exists\nV8 runtime placeholder</i>"]
        PKG["Published Packages\n<i>npm / crates.io\nnot yet published</i>"]
    end

    style working fill:#14532d,stroke:#86efac,color:#fff
    style partial fill:#713f12,stroke:#fde68a,color:#fff
```

### The evidence

| Layer | What it does | Tests |
|-------|-------------|-------|
| **Rust server** | REST API, auth, permissions, query engine, WebSocket handler, admin endpoints | 446 |
| **TypeScript SDKs** | React hooks, Angular signals, Next.js App/Pages Router, core client | 92 |
| **Python SDK** | Sync/async client, FastAPI integration, Django support | 141 |
| **PHP SDK** | Composer package, Laravel integration | 52 |
| **Total** | | **731 tests passing** |

### What each piece actually does

- **Data path**: `POST /api/data/users -d '{"name":"Alice"}'` writes triples to Postgres. `GET /api/data/users` reads them back. Round-trip proven by integration tests across all SDKs.
- **Auth**: Signup hashes passwords with Argon2id (64MB memory, 3 iterations). Signin returns a JWT. Every protected route validates the token before touching data.
- **Permissions**: Every request evaluates row-level rules. Users see only their own data. Admins bypass. Rules are stored as data (triples), not config files.
- **Query engine**: DarshanQL — a purpose-built query language that parses, generates an execution plan, and runs against Postgres. Not SQL, not GraphQL, not a toy.
- **WebSocket subscriptions**: Clients subscribe to queries. When a mutation changes matching data, the server broadcasts diffs to connected clients.
- **Admin dashboard**: React + Vite + Tailwind. Shows live data from the API. Manages collections, users, permissions.

### What's not done yet

- Server function V8 runtime (subprocess placeholder exists, API surface validated)
- Published npm/crates.io packages
- Install script (`curl -fsSL ... | sh`)
- Hosted documentation site
- Performance benchmarks against Firebase/Supabase/Convex
- Horizontal scaling / multi-node

---

## DarshQL

DarshQL is the query language purpose-built for DarshJDB. It borrows the clarity of SQL, the traversal power of graph query languages, and the expressiveness of document query builders — then unifies them under one syntax that works across every data model.

### Define

```sql
-- Namespace and database
USE NS production DB myapp;

-- Define a schemafull table
DEFINE TABLE user SCHEMAFULL;
DEFINE FIELD name ON user TYPE string ASSERT $value != NONE;
DEFINE FIELD email ON user TYPE string ASSERT string::is::email($value);
DEFINE FIELD created ON user TYPE datetime DEFAULT time::now();
DEFINE INDEX idx_email ON user FIELDS email UNIQUE;

-- Define a schemaless table (anything goes)
DEFINE TABLE event SCHEMALESS;

-- Define a graph edge table
DEFINE TABLE follows SCHEMAFULL TYPE RELATION IN user OUT user;
DEFINE FIELD since ON follows TYPE datetime DEFAULT time::now();
```

### Create

```sql
-- Create with auto-generated ID
CREATE user SET name = 'Darsh', email = 'darsh@navsari.dev';

-- Create with specific ID
CREATE user:darsh SET name = 'Darsh Joshi', email = 'darsh@navsari.dev';

-- Insert multiple records
INSERT INTO user [
  { name: 'Alice', email: 'alice@example.com' },
  { name: 'Bob', email: 'bob@example.com' }
];
```

### Query

```sql
-- Simple select with conditions
SELECT * FROM user WHERE email CONTAINS 'example.com' ORDER BY created DESC LIMIT 10;

-- Nested field access
SELECT name, settings.theme, settings.notifications.email FROM user;

-- Aggregations
SELECT count() AS total, math::mean(age) AS avg_age FROM user GROUP BY country;
```

### Graph Relations

```sql
-- Create a relationship between two records
RELATE user:darsh -> follows -> user:alice SET since = time::now();
RELATE user:darsh -> wrote -> article:rust_is_great SET published = true;

-- Traverse the graph — who does Darsh follow?
SELECT ->follows->user.name FROM user:darsh;

-- Reverse traversal — who follows Alice?
SELECT <-follows<-user.name FROM user:alice;

-- Multi-hop — friends of friends
SELECT ->follows->user->follows->user.name FROM user:darsh;

-- Graph with conditions
SELECT ->follows->user WHERE age > 25 FROM user:darsh;
```

### LIVE SELECT — Real-Time Subscriptions

```sql
-- Subscribe to all changes on a table
LIVE SELECT * FROM user;

-- Subscribe with filters — only get notified about relevant changes
LIVE SELECT * FROM user WHERE country = 'IN';

-- Subscribe to graph changes
LIVE SELECT * FROM follows WHERE in = user:darsh;

-- Subscribe with diff mode — only receive the changed fields
LIVE SELECT DIFF FROM user;
```

When a mutation matches a LIVE SELECT, DarshJDB pushes the change to all subscribed clients over WebSocket. No polling. No webhooks. No external message broker.

### Embedded Functions

```sql
-- String functions
SELECT string::uppercase(name), string::slug(title) FROM article;

-- Math functions
SELECT math::mean(scores), math::median(scores) FROM student;

-- Time functions
SELECT * FROM event WHERE created > time::now() - 7d;

-- Crypto functions
SELECT crypto::argon2::generate(password) AS hash FROM $input;

-- Vector search — semantic similarity
SELECT * FROM document WHERE embedding <|4|> $query_vector;

-- Geo functions
SELECT * FROM restaurant WHERE geo::distance(location, $user_location) < 5km;

-- HTTP functions (server-side)
SELECT http::get('https://api.example.com/data') AS response;

-- Custom functions
DEFINE FUNCTION fn::greet($name: string) {
  RETURN string::concat('Namaste, ', $name, '!');
};
SELECT fn::greet(name) FROM user;
```

### Transactions

```sql
BEGIN TRANSACTION;
  LET $from = (UPDATE wallet:darsh SET balance -= 100 RETURN AFTER);
  LET $to = (UPDATE wallet:alice SET balance += 100 RETURN AFTER);
  IF $from.balance < 0 {
    CANCEL TRANSACTION;
  };
COMMIT TRANSACTION;
```

```mermaid
graph LR
    subgraph pipeline["DarshQL Query Pipeline"]
        direction LR
        Q["DarshQL\n<i>query string</i>"] --> P["Parser\n<i>lexer + grammar</i>"]
        P --> AST["AST\n<i>typed syntax tree</i>"]
        AST --> OPT["Optimizer\n<i>index selection\njoin reordering</i>"]
        OPT --> EXEC["Executor\n<i>parallel evaluation\npermission injection</i>"]
        EXEC --> STORE["Storage\n<i>Postgres / Memory\n/ RocksDB</i>"]
    end

    style Q fill:#cc9933,color:#000
    style P fill:#1a1a2e,color:#fff
    style AST fill:#1a1a2e,color:#fff
    style OPT fill:#0f3460,color:#fff
    style EXEC fill:#14532d,color:#fff
    style STORE fill:#336791,color:#fff
```

---

## Multi-Model Storage

DarshJDB is not five databases duct-taped together. It is one engine with five access patterns over the same storage layer.

| Model | How DarshJDB implements it | Example |
|-------|---------------------------|---------|
| **Document** | Schemaless tables, nested JSON, flexible fields | `CREATE article SET title = 'Hello', tags = ['rust', 'db']` |
| **Graph** | `RELATE` statements, `->` / `<-` traversals, typed edges | `RELATE user:darsh -> authored -> article:hello` |
| **Relational** | `SCHEMAFULL` tables, indexes, joins, foreign keys | `DEFINE TABLE invoice SCHEMAFULL; DEFINE FIELD customer ON invoice TYPE record<customer>` |
| **Key-Value** | Direct record access by ID, O(1) lookups | `SELECT * FROM config:smtp` / `UPDATE config:smtp SET host = 'mail.example.com'` |
| **Vector** | pgvector HNSW indexes, cosine/euclidean/dot product | `SELECT * FROM document WHERE embedding <\|4\|> $query ORDER BY dist ASC` |

```mermaid
graph TB
    subgraph models["Five Models, One Engine"]
        DOC["Document Store\n<i>schemaless JSON\nnested objects, arrays</i>"]
        GRAPH["Graph Database\n<i>RELATE, traverse\nmulti-hop queries</i>"]
        REL["Relational Tables\n<i>SCHEMAFULL, indexes\njoins, constraints</i>"]
        KV["Key-Value Cache\n<i>O(1) record access\nconfig, sessions</i>"]
        VEC["Vector Search\n<i>HNSW embeddings\nsemantic similarity</i>"]
    end

    subgraph storage["Unified Storage Layer"]
        TS["Triple Store\n<i>Entity-Attribute-Value</i>"]
        PG[("PostgreSQL 16+\n<i>pgvector + tsvector</i>")]
        TS --> PG
    end

    DOC --> TS
    GRAPH --> TS
    REL --> TS
    KV --> TS
    VEC --> TS

    style models fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style storage fill:#0f3460,stroke:#F59E0B,color:#fff
    style PG fill:#336791,stroke:#fff,color:#fff
```

---

## Real-Time Architecture

Every mutation in DarshJDB flows through a change feed. LIVE SELECT queries register interest in specific data patterns. When a mutation matches, the diff is pushed to all subscribers — filtered through row-level permissions so each client only sees what they are allowed to see.

```mermaid
sequenceDiagram
    participant W as Writer Client
    participant S as DarshJDB Server
    participant CF as Change Feed
    participant LQM as Live Query Manager
    participant P as Permission Engine
    participant R1 as Reader A (WebSocket)
    participant R2 as Reader B (WebSocket)

    W->>S: CREATE user SET name = 'Darsh'
    S->>S: Execute mutation + write to storage
    S->>CF: Emit ChangeEvent{table: user, action: CREATE, data: {...}}
    CF->>LQM: Match against registered LIVE SELECTs
    LQM->>P: Filter: which subscribers can see this record?
    P-->>LQM: Reader A: full access, Reader B: denied
    LQM-->>R1: WebSocket push: {action: CREATE, result: {name: 'Darsh'}}
    Note over R2: Reader B receives nothing — permission denied
```

---

## How DarshJDB Compares

No names. Just capabilities.

| Capability | DarshJDB | Traditional BaaS | Cloud Databases |
|-----------|----------|------------------|-----------------|
| Multi-model (doc + graph + relational + KV + vector) | Yes | Partial | Rare |
| Graph relations with traversal | Yes | No | Some |
| LIVE SELECT real-time queries | Yes | Polling or webhooks | Some |
| Single binary deployment | Yes | 3-7 services | Cloud-only |
| Self-hosted, your metal | Yes | Some | Cloud-first |
| Row-level permissions | Yes | Yes | Some |
| Embedded functions (string, math, geo, crypto, http) | Yes | Rare | Rare |
| Vector search (HNSW) | Yes | No | Some |
| Triple store / knowledge graph | Yes | No | No |
| Schema modes (strict, flexible, mixed) | Yes | Pick one | Pick one |
| WebSocket + SSE subscriptions | Yes | WebSocket only | Varies |
| Merkle audit trail | Yes | No | Enterprise only |
| Runs on a $5 VPS | Yes | Depends | No |

---

## Deployment

### Docker

```bash
# Single command — DarshJDB + Postgres
docker compose up -d

# Or pull the image directly
docker pull ghcr.io/darshjme/darshjdb:latest
docker run -d \
  --name darshjdb \
  -p 7700:7700 \
  -e DATABASE_URL=postgres://user:pass@host:5432/darshjdb \
  ghcr.io/darshjme/darshjdb:latest
```

### Docker Compose (production)

```yaml
version: '3.8'
services:
  darshjdb:
    image: ghcr.io/darshjme/darshjdb:latest
    ports:
      - "7700:7700"
    environment:
      DATABASE_URL: postgres://darshan:darshan@postgres:5432/darshjdb
      DDB_JWT_SECRET: your-secret-here
      DDB_CORS_ORIGINS: https://yourapp.com
    depends_on:
      postgres:
        condition: service_healthy
    restart: unless-stopped

  postgres:
    image: pgvector/pgvector:pg16
    environment:
      POSTGRES_USER: darshan
      POSTGRES_PASSWORD: darshan
      POSTGRES_DB: darshjdb
    volumes:
      - pgdata:/var/lib/postgresql/data
    healthcheck:
      test: pg_isready -U darshan
      interval: 5s
      retries: 5

volumes:
  pgdata:
```

### LXC Container

```bash
# On Proxmox or any LXC host
lxc launch ubuntu:22.04 darshjdb
lxc exec darshjdb -- bash

# Inside the container
curl -fsSL https://db.darshj.me/install.sh | sh
ddb start --bind 0.0.0.0:7700 --database postgres://localhost/darshjdb
```

### CLI

```bash
# Start the server
ddb start
ddb start --bind 0.0.0.0:7700 --log debug

# Interactive SQL console
ddb sql
ddb sql --ns production --db myapp

# Import/export data
ddb export --output backup.darshql
ddb import --input backup.darshql

# Health and status
ddb status
ddb version
```

---

## SDK Examples

### JavaScript / TypeScript

```typescript
import { DDB } from '@darshjdb/client';

// Connect
const db = new DDB({ serverUrl: 'http://localhost:7700' });

// Authenticate
await db.signin({ email: 'darsh@navsari.dev', password: 'chai-pani' });

// Create a record
const user = await db.create('user', {
  name: 'Darsh Joshi',
  email: 'darsh@navsari.dev',
  location: { city: 'Navsari', state: 'Gujarat' }
});

// Query with filters
const engineers = await db.select('user', {
  where: { 'location.state': 'Gujarat' },
  orderBy: { created: 'desc' },
  limit: 10
});

// Graph relation
await db.relate('user:darsh', 'follows', 'user:alice');

// Traverse
const following = await db.query('SELECT ->follows->user.name FROM user:darsh');

// Live subscription
const unsubscribe = db.live('user', { where: { role: 'admin' } }, (change) => {
  console.log(change.action, change.result);
  // CREATE { name: 'New Admin', role: 'admin' }
});

// React hook
import { useQuery } from '@darshjdb/react';

function UserList() {
  const { data, loading, error } = useQuery({
    users: { $where: { active: true }, profile: {} }
  });

  if (loading) return <p>Loading...</p>;
  return data.users.map(u => <div key={u.id}>{u.name}</div>);
}
```

### Python

```python
from darshjdb import DarshJDB, AsyncDarshJDB

# Synchronous client
db = DarshJDB("http://localhost:7700")
db.signin(email="darsh@navsari.dev", password="chai-pani")

# Create
user = db.create("user", {
    "name": "Darsh Joshi",
    "email": "darsh@navsari.dev"
})

# Query
results = db.select("user", where={"location.state": "Gujarat"}, limit=10)

# Raw DarshQL
articles = db.query("SELECT * FROM article WHERE ->authored<-user = user:darsh")

# Graph relation
db.relate("user:darsh", "follows", "user:alice")

# Async client (FastAPI)
from fastapi import FastAPI
app = FastAPI()
adb = AsyncDarshJDB("http://localhost:7700")

@app.get("/users")
async def get_users():
    return await adb.select("user", limit=50)

# Live subscription (async)
async for change in adb.live("user", where={"role": "admin"}):
    print(change.action, change.result)
```

---

## Storage Architecture

DarshJDB supports multiple storage backends. PostgreSQL is the default for production. Memory mode is available for testing and development. File-based storage is planned for embedded use cases.

```mermaid
graph TB
    subgraph engine["DarshJDB Engine"]
        QE["Query Engine\n<i>DarshQL execution</i>"]
        CACHE["Query Cache\n<i>DashMap, TTL-aware\nChangeEvent invalidation</i>"]
        IDX["Index Manager\n<i>B-tree, HNSW, GIN\ntsvector, trigram</i>"]
    end

    subgraph backends["Storage Backends"]
        MEM["Memory\n<i>HashMap + BTreeMap\ndev/test only</i>"]
        PG["PostgreSQL 16+\n<i>ACID, pgvector\ntsvector, JSONB</i>"]
        FILE["File Store\n<i>RocksDB / SQLite\nembedded mode</i>"]
    end

    subgraph external["External Storage"]
        S3["S3 / R2 / MinIO\n<i>file uploads\nobject storage</i>"]
    end

    QE --> CACHE
    CACHE --> IDX
    IDX --> MEM
    IDX --> PG
    IDX --> FILE
    QE --> S3

    style engine fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style backends fill:#14532d,stroke:#86efac,color:#fff
    style external fill:#0f3460,stroke:#F59E0B,color:#fff
    style PG fill:#336791,stroke:#fff,color:#fff
```

---

## Architecture

```mermaid
graph TB
    subgraph Clients["Client SDKs"]
        React["React\n<i>hooks</i>"]
        Next["Next.js\n<i>App + Pages Router</i>"]
        Angular["Angular\n<i>signals + RxJS</i>"]
        PHP["PHP\n<i>Composer + Laravel</i>"]
        Python["Python\n<i>pip + FastAPI/Django</i>"]
        Curl["cURL / HTTP"]
    end

    subgraph Server["DarshJDB — Single Rust Binary"]
        API["HTTP + WebSocket\n<i>Axum + Tokio</i>"]
        AUTH["Auth Engine\n<i>Argon2id + JWT RS256</i>"]
        PERM["Permission Engine\n<i>Row-Level Security</i>"]
        QE["Query Engine\n<i>DarshanQL → SQL</i>"]
        TS["Triple Store\n<i>EAV over Postgres</i>"]
        SYNC["Sync Engine\n<i>Mutation → Diff → Push</i>"]
        FN["Function Runtime\n<i>user-defined logic</i>"]
        STORE["Storage Engine\n<i>S3-compatible</i>"]
    end

    PG[("PostgreSQL 16+\n<i>pgvector</i>")]

    Clients --> API
    API --> AUTH
    AUTH --> PERM
    PERM --> QE
    QE --> TS
    TS --> PG
    API --> SYNC
    API --> FN
    API --> STORE

    style Server fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style PG fill:#336791,stroke:#fff,color:#fff
    style Clients fill:#0f3460,stroke:#F59E0B,color:#fff
```

### Request Lifecycle

Every request flows through the same pipeline. No shortcuts, no bypasses.

```mermaid
sequenceDiagram
    participant C as Client
    participant A as Axum Router
    participant AU as Auth Middleware
    participant P as Permission Engine
    participant Q as Query Engine
    participant T as Triple Store
    participant PG as PostgreSQL

    C->>A: HTTP Request
    A->>AU: Extract + validate JWT
    AU->>P: Load user's permission rules
    P->>Q: Authorized query with RLS clauses
    Q->>T: DarshanQL → SQL translation
    T->>PG: Execute against Postgres
    PG-->>T: Result rows
    T-->>Q: Triples → entities
    Q-->>P: Filter restricted fields
    P-->>AU: Permitted response
    AU-->>A: Attach response headers
    A-->>C: JSON response
```

---

## The Data Model

Traditional databases force you to define tables before writing data. DarshJDB inverts this.

```mermaid
graph LR
    subgraph Traditional["Traditional Database"]
        T1["1. Define schema\n<i>CREATE TABLE users\n  id UUID,\n  name TEXT,\n  email TEXT</i>"]
        T2["2. Write data\n<i>INSERT INTO users\n  VALUES (...)</i>"]
        T3["3. Schema changes\n<i>ALTER TABLE...\n  ADD COLUMN...\n  run migrations</i>"]
        T1 --> T2 --> T3
    end

    subgraph Triple["DarshJDB Triple Store"]
        D1["1. Write data\n<i>POST /api/data/users\n  {'name':'Alice'}</i>"]
        D2["2. Schema inferred\n<i>from existing triples\n  automatic</i>"]
        D3["3. Lock it down\n<i>strict mode\n  when ready for prod</i>"]
        D1 --> D2 --> D3
    end

    style Traditional fill:#7f1d1d,stroke:#fca5a5,color:#fff
    style Triple fill:#14532d,stroke:#86efac,color:#fff
```

### How triples work

Every piece of data in DarshJDB is a triple: `(entity_id, attribute, value)`.

```
(e_01, "name",  "Alice")
(e_01, "email", "alice@example.com")
(e_01, "role",  "admin")
```

An "entity" is just a collection of triples sharing the same ID. A "collection" is just triples grouped by type. Relationships are triples where the value points to another entity ID. This is how knowledge graphs work. This is how the Semantic Web works.

```mermaid
graph LR
    E1["Entity: e_01"] -->|name| V1["Alice"]
    E1 -->|email| V2["alice@example.com"]
    E1 -->|role| V3["admin"]
    E1 -->|team| E2["Entity: e_02"]
    E2 -->|name| V4["Engineering"]
    E2 -->|lead| E1

    style E1 fill:#cc9933,color:#000
    style E2 fill:#cc9933,color:#000
    style V1 fill:#1a1a2e,color:#fff
    style V2 fill:#1a1a2e,color:#fff
    style V3 fill:#1a1a2e,color:#fff
    style V4 fill:#1a1a2e,color:#fff
```

---

## Auth Flow

```mermaid
sequenceDiagram
    participant C as Client SDK
    participant S as DarshJDB Server
    participant P as PostgreSQL

    Note over C,P: Signup
    C->>S: POST /api/auth/signup {email, password}
    S->>S: Hash password (Argon2id, 64MB, 3 iterations)
    S->>P: INSERT user record
    S->>P: INSERT user entity as triples
    S->>S: Generate JWT (RS256, 15min expiry)
    S->>S: Generate refresh token (7 day expiry)
    S-->>C: {access_token, refresh_token, user}

    Note over C,P: Authenticated request
    C->>S: GET /api/data/todos (Authorization: Bearer ...)
    S->>S: Validate JWT signature + expiry
    S->>S: Load permission rules for this user
    S->>P: SELECT triples WHERE entity matches + RLS clauses
    P-->>S: Only rows this user is allowed to see
    S-->>C: Filtered, permitted JSON response

    Note over C,P: Token refresh
    C->>S: POST /api/auth/refresh {refresh_token}
    S->>S: Validate refresh token
    S->>S: Generate new JWT + new refresh token
    S-->>C: {access_token, refresh_token}
```

---

## Security Layers

Every request passes through seven layers. No shortcuts.

```mermaid
graph TB
    R["Incoming Request"] --> TLS["TLS 1.3\n<i>encrypted in transit</i>"]
    TLS --> RL["Rate Limiter\n<i>token bucket\nper IP + per user</i>"]
    RL --> IV["Input Validation\n<i>schema-checked\nat boundary</i>"]
    IV --> AU["Authentication\n<i>JWT RS256\nsignature verification</i>"]
    AU --> AZ["Authorization\n<i>permission rule\nevaluation</i>"]
    AZ --> RLS["Row-Level Security\n<i>WHERE clause injection\nper-user filtering</i>"]
    RLS --> FF["Field Filtering\n<i>restricted fields\nstripped from response</i>"]
    FF --> PG[("PostgreSQL\n<i>only permitted\ndata returned</i>")]

    style R fill:#7f1d1d,color:#fff
    style TLS fill:#7f1d1d,color:#fff
    style RL fill:#7c2d12,color:#fff
    style IV fill:#713f12,color:#fff
    style AU fill:#365314,color:#fff
    style AZ fill:#14532d,color:#fff
    style RLS fill:#064e3b,color:#fff
    style FF fill:#0c4a6e,color:#fff
    style PG fill:#336791,color:#fff
```

---

## Real-Time Subscription Flow

```mermaid
sequenceDiagram
    participant C1 as Client A
    participant C2 as Client B
    participant S as DarshJDB
    participant P as PostgreSQL

    C1->>S: WebSocket: subscribe("todos", {filter: "owner=me"})
    S->>P: Initial query (with RLS)
    S-->>C1: Full result set

    C2->>S: POST /api/data/todos {title: "Buy chai"}
    S->>P: INSERT triple
    S->>S: Diff engine: what changed?
    S->>S: Permission check: who can see this?
    S-->>C1: WebSocket push: {added: [{title: "Buy chai"}]}
    Note over C1: Only receives if permission rules allow
```

---

## Quick Start

```bash
# Clone
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb

# Start Postgres
docker compose up postgres -d

# Initialize database with sample data
./scripts/setup-db.sh --seed

# Start the server
DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshjdb \
  cargo run --bin ddb-server

# Health check
curl http://localhost:7700/health

# Write some data
curl -X POST http://localhost:7700/api/data/users \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer dev" \
  -d '{"name":"Darsh","email":"darsh@navsari.dev"}'

# Read it back
curl http://localhost:7700/api/data/users \
  -H "Authorization: Bearer dev"
```

### Run the tests

```bash
# Rust (446 tests)
cargo test --workspace

# TypeScript SDKs (92 tests)
cd packages/tests && npm test

# Python SDK (141 tests)
cd sdks/python && pytest

# PHP SDK (52 tests)
cd sdks/php && composer test

# End-to-end (20+ assertions)
./scripts/e2e-test.sh
```

---

## Project Structure

```
darshjdb/
├── packages/
│   ├── server/           # Rust: HTTP server, auth, permissions, query engine, triple store
│   ├── cli/              # Rust: ddb dev / deploy / push / pull
│   ├── client-core/      # TypeScript: framework-agnostic SDK core
│   ├── react/            # React hooks (useQuery, useMutation, useAuth)
│   ├── angular/          # Angular signals + RxJS observables
│   ├── nextjs/           # Next.js App Router + Pages Router support
│   ├── admin/            # Admin dashboard (React + Vite + Tailwind)
│   └── tests/            # Cross-SDK integration tests
├── sdks/
│   ├── php/              # PHP SDK + Laravel integration
│   └── python/           # Python SDK + FastAPI/Django integration
├── docs/                 # 12 guides + 5 strategy roadmaps
├── examples/             # Todo app, chat app, Next.js, Angular, PHP, Python, cURL
├── deploy/               # Docker Compose, Kubernetes Helm chart, Prometheus
└── scripts/              # Setup, seeding, e2e testing
```

---

## Technology

| Layer | Choice | Why |
|-------|--------|-----|
| **Runtime** | Rust (Axum + Tokio) | Memory safety without GC. Async without callbacks. |
| **Database** | PostgreSQL 16+ with pgvector | Battle-tested. Extensions for vectors, full-text, JSON. |
| **Auth** | Argon2id + JWT RS256 | Argon2id is the OWASP recommendation. RS256 for asymmetric verification. |
| **Query Language** | DarshanQL | Purpose-built for triple stores. Not SQL, not GraphQL. |
| **TypeScript SDKs** | React, Angular, Next.js | Framework-native patterns: hooks, signals, server components. |
| **Admin UI** | React + Vite + TailwindCSS | Fast dev, fast builds, looks good. |
| **PHP SDK** | Composer + Laravel | Because PHP still runs most of the web. |
| **Python SDK** | pip + FastAPI/Django | Because data teams live in Python. |

---

## SDK Overview

```mermaid
graph TB
    subgraph core["@darshjdb/client-core"]
        CC["HTTP Client\nWebSocket Client\nAuth State\nQuery Builder"]
    end

    subgraph frameworks["Framework SDKs"]
        R["@darshjdb/react\n<i>useQuery, useMutation\nuseAuth, usePresence</i>"]
        A["@darshjdb/angular\n<i>DarshanService\nsignals + RxJS</i>"]
        N["@darshjdb/nextjs\n<i>Server Components\nApp + Pages Router</i>"]
    end

    subgraph native["Native SDKs"]
        PH["darshjdb-php\n<i>Composer + Laravel</i>"]
        PY["darshjdb-python\n<i>pip + FastAPI/Django</i>"]
    end

    core --> R
    core --> A
    core --> N
    CC -.->|same protocol| PH
    CC -.->|same protocol| PY

    style core fill:#cc9933,color:#000
    style frameworks fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style native fill:#1a1a2e,stroke:#F59E0B,color:#fff
```

---

## Roadmap

Focused on what matters next, in order.

| Priority | What | Status |
|----------|------|--------|
| 1 | Publish SDKs to npm and crates.io | Not started |
| 2 | Install script (`curl ... \| sh`) | Not started |
| 3 | Server function V8 runtime | Placeholder exists |
| 4 | Performance benchmarks vs Firebase/Supabase/Convex | Not started |
| 5 | Hosted docs site | Not started |
| 6 | File storage (S3-compatible) | API designed |
| 7 | Horizontal scaling | Architecture planned |

Longer-term thinking on AI/ML integration (MCP server, embeddings, RAG), Web3 (wallet auth, token-gated permissions), and enterprise features (multi-tenancy, SOC2) lives in [`docs/strategy/`](docs/strategy/).

---

## Contributing

```bash
# Run the full test suite
cargo test --workspace   # 446 tests
npm test                 # 92 tests
pytest                   # 141 tests
composer test            # 52 tests
```

Read [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on code style, PR process, and architecture decisions.

The project is alpha. There's real work to do. If you care about self-hosted infrastructure and developer tools, pull requests are welcome.

---

## License

MIT. See [LICENSE](LICENSE).

---

<div align="center">

**[Darsh Joshi](https://darshj.ai)** | Navsari, Gujarat to the world.

CEO at [GraymatterOnline LLP](https://graymatteronline.com) | CTO at [KnowAI](https://knowai.biz)

*karmanye vadhikaraste ma phaleshu kadachana*
You have the right to work, but never to the fruit of work.

[darshj.ai](https://darshj.ai) | [darshj.me](https://darshj.me) | [darshjme@gmail.com](mailto:darshjme@gmail.com)

</div>
