<div align="center">

<img src=".github/assets/hero.svg" alt="DarshanDB" width="100%" />

<br/>

[![License: MIT](https://img.shields.io/badge/License-MIT-F59E0B.svg?style=for-the-badge)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-B7410E.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![PostgreSQL 16+](https://img.shields.io/badge/PostgreSQL-16+-336791.svg?style=for-the-badge&logo=postgresql&logoColor=white)](https://www.postgresql.org)
[![Status: Alpha](https://img.shields.io/badge/Status-Alpha-orange.svg?style=for-the-badge)](https://github.com/darshjme/darshandb)
[![CI](https://img.shields.io/github/actions/workflow/status/darshjme/darshandb/ci.yml?style=for-the-badge&label=CI)](https://github.com/darshjme/darshandb/actions)
[![Tests: 731](https://img.shields.io/badge/Tests-731_passing-86efac.svg?style=for-the-badge)](https://github.com/darshjme/darshandb)

<br/>

**A self-hosted Backend-as-a-Service built in Rust.**
**Triple-store architecture over PostgreSQL. Real-time by default.**

[Getting Started](#quick-start) | [Architecture](#architecture) | [Documentation](docs/) | [Contributing](#contributing)

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

So I built what I wanted. I called it DarshanDB.

*"Darshan"* means to see, to perceive the complete picture. The database sees every change, every query, every permission boundary. It sees what each user is allowed to see. And it shows them exactly that — in real-time, the moment anything changes.

```mermaid
graph LR
    subgraph Journey["The Path"]
        N["Navsari\n<i>where it started</i>"] --> L["London\n<i>systems thinking</i>"]
        L --> D["Dubai\n<i>production systems</i>"]
        D --> A["Ahmedabad\n<i>four companies</i>"]
        A --> DB["DarshanDB\n<i>building the tool\nI always needed</i>"]
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

## What DarshanDB Is

A single Rust binary that gives you a complete backend. Authentication. Permissions. Real-time subscriptions. Query engine. Admin dashboard. Connect from React, Angular, Next.js, PHP, Python, or cURL.

The data model is a triple store (Entity-Attribute-Value) over PostgreSQL. No rigid schemas, no migrations during development. Write data first — structure emerges from usage. When you're ready for production, switch to strict mode and lock it down.

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

    subgraph Server["DarshanDB — Single Rust Binary"]
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

Traditional databases force you to define tables before writing data. DarshanDB inverts this.

```mermaid
graph LR
    subgraph Traditional["Traditional Database"]
        T1["1. Define schema\n<i>CREATE TABLE users\n  id UUID,\n  name TEXT,\n  email TEXT</i>"]
        T2["2. Write data\n<i>INSERT INTO users\n  VALUES (...)</i>"]
        T3["3. Schema changes\n<i>ALTER TABLE...\n  ADD COLUMN...\n  run migrations</i>"]
        T1 --> T2 --> T3
    end

    subgraph Triple["DarshanDB Triple Store"]
        D1["1. Write data\n<i>POST /api/data/users\n  {'name':'Alice'}</i>"]
        D2["2. Schema inferred\n<i>from existing triples\n  automatic</i>"]
        D3["3. Lock it down\n<i>strict mode\n  when ready for prod</i>"]
        D1 --> D2 --> D3
    end

    style Traditional fill:#7f1d1d,stroke:#fca5a5,color:#fff
    style Triple fill:#14532d,stroke:#86efac,color:#fff
```

### How triples work

Every piece of data in DarshanDB is a triple: `(entity_id, attribute, value)`.

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
    participant S as DarshanDB Server
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
    participant S as DarshanDB
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
git clone https://github.com/darshjme/darshandb.git
cd darshandb

# Start Postgres
docker compose up postgres -d

# Initialize database with sample data
./scripts/setup-db.sh --seed

# Start the server
DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshandb \
  cargo run --bin darshandb-server

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
darshandb/
├── packages/
│   ├── server/           # Rust: HTTP server, auth, permissions, query engine, triple store
│   ├── cli/              # Rust: darshan dev / deploy / push / pull
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
    subgraph core["@darshandb/client-core"]
        CC["HTTP Client\nWebSocket Client\nAuth State\nQuery Builder"]
    end

    subgraph frameworks["Framework SDKs"]
        R["@darshandb/react\n<i>useQuery, useMutation\nuseAuth, usePresence</i>"]
        A["@darshandb/angular\n<i>DarshanService\nsignals + RxJS</i>"]
        N["@darshandb/nextjs\n<i>Server Components\nApp + Pages Router</i>"]
    end

    subgraph native["Native SDKs"]
        PH["darshandb-php\n<i>Composer + Laravel</i>"]
        PY["darshandb-python\n<i>pip + FastAPI/Django</i>"]
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
