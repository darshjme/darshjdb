<div align="center">

<img src=".github/assets/hero.svg" alt="DarshanDB" width="100%" />

<br/>

[![License: MIT](https://img.shields.io/badge/License-MIT-F59E0B.svg?style=for-the-badge)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-B7410E.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![PostgreSQL 16+](https://img.shields.io/badge/PostgreSQL-16+-336791.svg?style=for-the-badge&logo=postgresql&logoColor=white)](https://www.postgresql.org)
[![Status: Alpha](https://img.shields.io/badge/Status-Alpha-orange.svg?style=for-the-badge)](https://github.com/darshjme/darshandb)
[![CI](https://img.shields.io/github/actions/workflow/status/darshjme/darshandb/ci.yml?style=for-the-badge&label=CI)](https://github.com/darshjme/darshandb/actions)

<br/>

**A self-hosted Backend-as-a-Service built in Rust.**
**Triple-store architecture over PostgreSQL. Real-time by default.**

</div>

---

## The Journey

I grew up in Navsari, a small town in southern Gujarat where the Parsi fire temples stand next to Hindu mandirs and the evening chai tastes like monsoon rain. My grandfather would say *"darshan karo"* every morning. See clearly. Perceive the truth of things before you act.

I didn't know it then, but that word would follow me across continents.

From Navsari I went to London for my studies. Business Computing at Greenwich. Advanced Diploma at Sunderland. The cold taught me discipline. The coursework taught me systems thinking. But the real education was watching how software got built in the West and realizing the tools were gatekept behind expensive cloud services and FAANG salaries.

Then Dubai. Media production. VFX pipelines for Aquaman, The Invisible Man, The Last of Us Part II. I learned what it means to build systems that cannot fail, that process terabytes without flinching, that serve creative teams who don't care about your architecture and just need it to work.

Back to India. Ahmedabad. Founded GraymatterOnline in 2015. Then Coeus Digital Media. Then KnowAI with 60+ autonomous agents managing enterprise operations. Each company, each product, each system, I hit the same wall.

The backend.

Three weeks of plumbing before writing a single line of business logic. Postgres setup. REST APIs. Auth. WebSockets. File uploads. Permissions. The same work, every time, for every project.

Firebase is NoSQL spaghetti. Supabase is REST with real-time bolted on. InstantDB is cloud-only. Convex is a black box. None of them let you run a single binary on a $5 VPS in Mumbai and own your data completely.

So I built what I wanted. I called it DarshanDB.

*"Darshan"* means to see, to perceive the complete picture. The database sees every change, every query, every permission. It sees what each user is allowed to see. And it shows them exactly that, in real-time, the moment anything changes.

```mermaid
graph LR
    subgraph Journey["The Path"]
        N["Navsari\n<i>where it started</i>"] --> L["London\n<i>systems thinking</i>"]
        L --> D["Dubai\n<i>production systems</i>"]
        D --> A["Ahmedabad\n<i>building companies</i>"]
        A --> DB["DarshanDB\n<i>building the tool</i>"]
    end

    style N fill:#cc9933,color:#000
    style L fill:#1a1a2e,color:#fff
    style D fill:#0f3460,color:#fff
    style A fill:#14532d,color:#fff
    style DB fill:#B7410E,color:#fff
```

---

## The Philosophy

The Bhagavad Gita says: *karmanye vadhikaraste ma phaleshu kadachana*. You have the right to work, but never to the fruit of work. Build because building is dharma. Ship because shipping serves others. Open-source because knowledge locked away is knowledge wasted.

The Sompura Brahmins of Gujarat carved stone into temples that outlasted empires. The tools changed. Chisel became compiler. Sandstone became silicon. But the intent is the same: build something permanent. Build something that serves.

```mermaid
flowchart TD
    D["Dharma\nWhat must be built?"] --> V["Vichara\nThink from first principles"]
    V --> K["Karma\nWrite the code"]
    K --> S["Seva\nOpen-source it"]
    S --> L["Loka\nThe world builds on top"]
    L --> D

    style D fill:#cc9933,color:#000
    style V fill:#1a1a2e,color:#fff
    style K fill:#0d1117,color:#fff
    style S fill:#14532d,color:#fff
    style L fill:#1e3a5f,color:#fff
```

---

## What DarshanDB Is

A single Rust binary that gives you a complete backend. Real-time queries. Authentication. Permissions. File storage. Server functions. Admin dashboard. Connect from React, Angular, Next.js, PHP, Python, or plain cURL.

The data model is a triple store (Entity-Attribute-Value) over PostgreSQL. This means no rigid schemas, no migrations in development. Write data first, structure emerges. When you're ready for production, switch to strict mode.

### What works today

```mermaid
graph TB
    subgraph Working["Working End-to-End"]
        REST["REST API\ncurl → Postgres → response"]
        AUTH["Authentication\nsignup → Argon2id → JWT"]
        PERM["Permissions\nrow-level security on every request"]
        DASH["Admin Dashboard\nlive data from Postgres"]
        SDK["Client SDK\nintegration tests prove round-trip"]
    end

    subgraph InProgress["In Progress"]
        WS["WebSocket Subscriptions\nmutation → broadcast → live push"]
        FN["Server Functions\nregistry exists, runtime placeholder"]
    end

    style Working fill:#14532d,stroke:#86efac,color:#fff
    style InProgress fill:#713f12,stroke:#fde68a,color:#fff
```

- **Data path**: `POST /api/data/users -d '{"name":"Alice"}'` writes triples to Postgres, `GET /api/data/users` reads them back
- **Auth**: signup with Argon2id password hashing, signin returns JWT, middleware enforces on protected routes
- **Permissions**: every request evaluates row-level rules, users see only their own data, admin bypasses
- **Query engine**: DarshanQL parses, plans, and executes against real Postgres
- **Admin dashboard**: shows live data from the API, falls back to mock when server is down
- **446 Rust tests**, 92 TypeScript tests, 141 Python tests, 52 PHP tests, all passing

### What's not done yet

- WebSocket real-time subscriptions (handler exists, reactive pipeline being wired)
- Server function V8 runtime (subprocess placeholder, API surface validated)
- Published npm/crates packages
- Install script and hosted docs site

## Architecture

```mermaid
graph TB
    subgraph Clients["Any Client"]
        React["React"]
        Next["Next.js"]
        Angular["Angular"]
        PHP["PHP"]
        Python["Python"]
        Curl["cURL"]
    end

    subgraph Server["DarshanDB — Single Rust Binary"]
        API["REST API + WebSocket"]
        AUTH["Auth Engine\nArgon2id + JWT RS256"]
        PERM["Permission Engine\nRow-Level Security"]
        QE["Query Engine\nDarshanQL → SQL"]
        TS["Triple Store\nEAV over Postgres"]
        SYNC["Sync Engine\nReactive Push"]
        FN["Function Runtime"]
        STORE["Storage Engine\nS3-compatible"]
    end

    PG[("PostgreSQL 16+")]

    Clients --> API
    API --> AUTH --> PERM
    PERM --> QE --> TS --> PG
    API --> SYNC
    API --> FN
    API --> STORE

    style Server fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style PG fill:#336791,stroke:#fff,color:#fff
    style Clients fill:#0f3460,stroke:#F59E0B,color:#fff
```

### The Data Model

```mermaid
graph LR
    subgraph Traditional["Traditional DB"]
        T1["CREATE TABLE users\n  id UUID,\n  name TEXT,\n  email TEXT"]
        T2["ALTER TABLE...\n  ADD COLUMN...\n  DROP COLUMN..."]
        T1 --> T2
    end

    subgraph DarshanDB["DarshanDB Triple Store"]
        D1["Write any data\nNo schema needed"]
        D2["Schema inferred\nfrom existing data"]
        D3["Strict mode\nwhen ready for prod"]
        D1 --> D2 --> D3
    end

    style Traditional fill:#7f1d1d,stroke:#fca5a5,color:#fff
    style DarshanDB fill:#14532d,stroke:#86efac,color:#fff
```

Every piece of data is a triple: `(entity_id, attribute, value)`. An entity is just a collection of triples sharing the same ID. Relationships are triples where the value points to another entity. This is how knowledge graphs work. This is how the Semantic Web works. This is how your brain works.

### The Auth Flow

```mermaid
sequenceDiagram
    participant C as Client
    participant S as DarshanDB
    participant P as PostgreSQL

    C->>S: POST /api/auth/signup {email, password}
    S->>S: Argon2id hash (64MB, 3 iter)
    S->>P: INSERT INTO users
    S->>P: INSERT triples (user entity)
    S->>S: Generate JWT (RS256, 15min)
    S-->>C: {access_token, refresh_token}

    C->>S: GET /api/data/todos (Bearer token)
    S->>S: Validate JWT
    S->>S: Evaluate permissions
    S->>P: SELECT with RLS WHERE clause
    S-->>C: Only data this user can see
```

### Security Layers

```mermaid
graph TB
    R["Request arrives"] --> TLS["TLS 1.3"]
    TLS --> RL["Rate Limiter\ntoken bucket per IP/user"]
    RL --> IV["Input Validation\nschema-checked at boundary"]
    IV --> AU["Authentication\nJWT RS256 verification"]
    AU --> AZ["Authorization\npermission rule evaluation"]
    AZ --> RLS["Row-Level Security\nSQL WHERE injection"]
    RLS --> FF["Field Filtering\nrestricted fields stripped"]
    FF --> PG["PostgreSQL\nonly permitted data returned"]

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

# Test it
curl http://localhost:7700/health
curl -H "Authorization: Bearer dev" \
  -X POST http://localhost:7700/api/data/users \
  -H "Content-Type: application/json" \
  -d '{"name":"Darsh","email":"darsh@navsari.dev"}'
```

### End-to-End Test

```bash
./scripts/e2e-test.sh
```

20+ assertions testing auth, CRUD, queries, mutations, permissions, and error handling.

## Project Structure

```
darshandb/
├── packages/
│   ├── server/          # Rust: the complete backend
│   ├── cli/             # Rust: darshan dev/deploy/push/pull
│   ├── client-core/     # TypeScript: framework-agnostic SDK
│   ├── react/           # React hooks
│   ├── angular/         # Angular signals + RxJS
│   ├── nextjs/          # Next.js App/Pages Router
│   └── admin/           # Admin dashboard (React + Vite + Tailwind)
├── sdks/
│   ├── php/             # PHP + Laravel
│   └── python/          # Python + FastAPI/Django
├── docs/                # 12 guides + 5 strategy roadmaps
├── examples/            # Todo app, chat app, Next.js, cURL scripts
└── deploy/              # Docker, Kubernetes Helm chart
```

## Technology

| Layer | Choice |
|-------|--------|
| Server | Rust (Axum + Tokio) |
| Database | PostgreSQL 16+ with pgvector |
| Auth | Argon2id + JWT RS256 |
| Client SDKs | TypeScript (React, Angular, Next.js) |
| Admin UI | React + Vite + TailwindCSS |
| PHP SDK | Composer + Laravel |
| Python SDK | pip + FastAPI/Django |

## Roadmap

The immediate focus: wire WebSocket subscriptions so a client can subscribe to a query and receive live updates when data changes. After that: publish to npm, build the install script, benchmark real performance.

See `docs/strategy/` for longer-term thinking on AI/ML integration (MCP server, embeddings, RAG), Web3 (wallet auth, token-gated permissions), enterprise (multi-tenancy, SOC2), and scalability (horizontal scaling, distributed cache).

## Contributing

```bash
cargo test --workspace   # 446 tests
npm test                 # 92 tests
```

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE).

---

<div align="center">

**[Darsh Joshi](https://darshj.ai)** — Navsari, Gujarat to the world.

CEO at [GraymatterOnline LLP](https://graymatteronline.com) | CTO at [KnowAI](https://knowai.biz)

*karmanye vadhikaraste ma phaleshu kadachana*

[darshj.ai](https://darshj.ai) · [darshj.me](https://darshj.me)

</div>
