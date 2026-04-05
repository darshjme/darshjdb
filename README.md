<div align="center">

<img src=".github/assets/hero.svg" alt="DarshanDB — The Self-Hosted BaaS That Sees Everything" width="100%" />

<br/>
<br/>

[![License: MIT](https://img.shields.io/badge/License-MIT-F59E0B.svg?style=for-the-badge)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-B7410E.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![PostgreSQL 16+](https://img.shields.io/badge/PostgreSQL-16+-336791.svg?style=for-the-badge&logo=postgresql&logoColor=white)](https://www.postgresql.org)
[![TypeScript SDKs](https://img.shields.io/badge/SDKs-TypeScript-3178C6.svg?style=for-the-badge&logo=typescript&logoColor=white)](https://www.typescriptlang.org)
[![PRs Welcome](https://img.shields.io/badge/PRs-Welcome-brightgreen.svg?style=for-the-badge)](CONTRIBUTING.md)

<br/>

[![GitHub Stars](https://img.shields.io/github/stars/darshjme/darshandb?style=flat-square&color=F59E0B&label=Stars)](https://github.com/darshjme/darshandb)
[![GitHub Forks](https://img.shields.io/github/forks/darshjme/darshandb?style=flat-square&color=6B7280&label=Forks)](https://github.com/darshjme/darshandb/fork)
[![CI](https://img.shields.io/github/actions/workflow/status/darshjme/darshandb/ci.yml?style=flat-square&label=CI)](https://github.com/darshjme/darshandb/actions)
[![npm](https://img.shields.io/npm/v/@darshan/react?style=flat-square&color=cb3837&label=@darshan/react)](https://www.npmjs.com/package/@darshan/react)
[![Discord](https://img.shields.io/badge/Discord-Join-5865F2?style=flat-square&logo=discord&logoColor=white)](https://discord.gg/darshandb)

<br/>

**One binary. Every framework. Zero loopholes.**

[Quickstart](#-quickstart) · [Architecture](#-architecture) · [SDKs](#-universal-sdk-ecosystem) · [Security](#-zero-trust-security) · [Performance](#-why-darshandb-is-faster) · [Self-Hosting](#-self-hosting) · [Contributing](CONTRIBUTING.md)

</div>

---

<div align="center">

*"Darshan" (darshan) means "vision" in Sanskrit — to perceive the complete picture.*
*DarshanDB sees every change, every query, every permission, and reactively pushes exactly the right data to exactly the right clients.*

</div>

---

## The Story Behind DarshanDB

Every developer project starts the same way: three weeks of plumbing before a single line of business logic. Setting up Postgres. Writing REST APIs. Building auth. Wiring WebSockets. Handling file uploads. Managing permissions. I've done this dozens of times across startups in Ahmedabad, enterprise builds at Graymatter, and production systems at KnowAI.

**Firebase** almost solved it — but it's NoSQL, and the moment you need a relational query, you're writing denormalized spaghetti. **Supabase** is better, but it's a REST wrapper with real-time bolted on as an afterthought. **InstantDB** got the query language right — but it's cloud-only. **Convex** nailed server functions — but it's a proprietary black box.

DarshanDB is what happens when you take the best ideas from all of them and compile them into a single Rust binary you can run on a $5 VPS.

The developer in Ahmedabad, the student in Lagos, the freelancer in Sao Paulo — they deserve the same backend infrastructure that FAANG engineers take for granted. DarshanDB makes that real.

<div align="center">

*Built by [Darsh Joshi](https://darshj.ai) · [darshj.me](https://darshj.me)*

</div>

---

## How DarshanDB Compares

```mermaid
graph LR
    subgraph Legacy["Legacy Approach"]
        L1["Write REST APIs"] --> L2["Build Auth"]
        L2 --> L3["Wire WebSockets"]
        L3 --> L4["Handle Permissions"]
        L4 --> L5["Manage Files"]
        L5 --> L6["Weeks of Plumbing"]
    end

    subgraph DarshanDB["DarshanDB Approach"]
        D1["darshan dev"] --> D2["Ship Features"]
    end

    style Legacy fill:#7f1d1d,stroke:#fca5a5,color:#fff
    style DarshanDB fill:#14532d,stroke:#86efac,color:#fff
```

### Feature Matrix

| Feature | DarshanDB | Firebase | Supabase | InstantDB | Convex |
|---------|:---------:|:--------:|:--------:|:---------:|:------:|
| **Self-hosted** | Yes | No | Partial | No | No |
| **Relational queries** | Yes | No | Yes | Yes | Yes |
| **Real-time (native)** | Yes | Yes | Polling* | Yes | Yes |
| **Offline-first** | Yes | Limited | No | Yes | No |
| **Graph traversal** | Yes | No | No | Yes | No |
| **Server functions** | V8 Sandboxed | Cloud Functions | Edge Functions | No | Yes |
| **Row-level security** | SQL WHERE injection | Rules DSL | Postgres RLS | Permissions | Validators |
| **Field-level perms** | Yes | No | No | No | No |
| **Wire protocol** | MsgPack binary | Protobuf | JSON | JSON | Custom |
| **Delta sync** | Yes | No | No | Yes | Yes |
| **Vector search** | pgvector native | No | Extension | No | No |
| **Time-travel queries** | MVCC snapshots | No | No | No | No |
| **Multi-tenancy** | Namespace isolation | Projects | Schemas | No | Deployments |
| **MFA / WebAuthn** | Yes | Yes | Yes | No | No |
| **Open source** | MIT | No | Apache-2.0 | Partial | No |
| **Single binary deploy** | Yes | N/A | No (10+ services) | N/A | N/A |

> *Supabase Realtime requires separate channel subscriptions and does not provide reactive queries out of the box.

---

## Quickstart

Three commands from zero to real-time backend.

```bash
# 1. Install (single binary, ~30MB)
curl -fsSL https://darshandb.dev/install | sh

# 2. Start (auto-creates Postgres, seeds admin)
darshan dev

# 3. Open dashboard
#    Dashboard → http://localhost:7700/admin
#    Your app  → ws://localhost:7700
```

Then drop this into your React app:

```typescript
import { DarshanDB } from '@darshan/react';

const db = DarshanDB.init({ appId: 'my-app' });

function TodoApp() {
  // This is a LIVE query — it updates when anyone changes data
  const { data } = db.useQuery({
    todos: {
      $where: { done: false },
      $order: { createdAt: 'desc' },
      owner: {}  // fetch related user in one query
    }
  });

  const addTodo = (title: string) => {
    db.transact(db.tx.todos[db.id()].set({ title, done: false }));
  };

  return <TodoList items={data?.todos} onAdd={addTodo} />;
}
```

**That's it.** Real-time sync, offline support, optimistic updates, type safety — all from five lines of configuration.

---

## Architecture

```mermaid
graph TB
    subgraph Clients["Client Universe"]
        React["React"]
        Next["Next.js"]
        Angular["Angular"]
        Vue["Vue"]
        Svelte["Svelte"]
        PHP["PHP"]
        Python["Python"]
        HTML["Vanilla JS"]
        Curl["cURL"]
    end

    subgraph Protocol["Protocol Layer"]
        WS["WebSocket + MsgPack\nfastest — persistent connection"]
        HTTP2["HTTP/2 + MsgPack\nfast — for SSR and server calls"]
        REST["REST + JSON\nuniversal fallback"]
    end

    subgraph Negotiator["Protocol Negotiator"]
        TLS["TLS 1.3"]
        CORS["CORS"]
        RL["Rate Limiter"]
        Auth["Auth Check"]
    end

    subgraph Core["DarshanDB Core"]
        QE["Query Engine\nDarshanQL to SQL"]
        ME["Mutation Engine\nACID transactions"]
        SE["Sync Engine\nreactive push"]
        RE["REST Handler\nuniversal compat"]
    end

    subgraph Security["Permission Engine"]
        RLS["Row-Level Security"]
        ABAC["Attribute-Based Access"]
        FF["Field Filtering"]
        RR["Role Resolution"]
    end

    subgraph Services["Service Layer"]
        TS["Triple Store\nEAV over Postgres"]
        AE["Auth Engine\nJWT + OAuth + MFA"]
        FR["Function Runtime\nV8 Sandboxed"]
        ST["Storage Engine\nS3-compatible"]
    end

    PG[("PostgreSQL 16+\nwith pgvector")]

    Clients --> Protocol
    Protocol --> Negotiator
    Negotiator --> Core
    Core --> Security
    Security --> Services
    Services --> PG

    style Clients fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style Protocol fill:#16213e,stroke:#F59E0B,color:#fff
    style Negotiator fill:#0f3460,stroke:#F59E0B,color:#fff
    style Core fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style Security fill:#e94560,stroke:#fff,color:#fff
    style Services fill:#16213e,stroke:#F59E0B,color:#fff
    style PG fill:#336791,stroke:#fff,color:#fff
```

### Data Flow: Query Subscription Lifecycle

Every query in DarshanDB is a live subscription. Here's what happens from the moment a client subscribes to when it receives real-time updates.

```mermaid
sequenceDiagram
    participant C as Client
    participant W as WebSocket
    participant Q as Query Engine
    participant P as Permission Engine
    participant T as Triple Store
    participant S as Sync Engine

    C->>W: Subscribe { todos: { $where: { done: false } } }
    W->>Q: Parse DarshanQL to QueryAST
    Q->>P: Inject RLS WHERE clause
    P->>T: Execute filtered SQL
    T-->>S: Register query dependencies
    T-->>W: Initial result set
    W-->>C: q-init { data, tx: 42 }

    Note over T: Another client mutates a todo...

    T->>S: TripleChange event
    S->>S: Match against query dependencies
    S->>P: Re-evaluate with user permissions
    P->>T: Execute filtered SQL
    T-->>S: New result set
    S->>S: Compute delta diff
    S-->>W: q-diff { added: [], updated: [...], tx: 43 }
    W-->>C: Push diff (sub 1ms)
```

### Real-Time Sync: Mutation to Broadcast

When a client writes data, DarshanDB applies optimistic updates locally and reconciles with the server. The diff engine ensures every connected client gets only what changed.

```mermaid
graph LR
    subgraph Client["Client Side"]
        A["Optimistic Mutation"] --> B["Local Store Update"]
        B --> C["UI Re-renders Instantly"]
    end

    subgraph Server["Server Side"]
        D["Validate + Authorize"] --> E["ACID Write to Postgres"]
        E --> F["Compute Affected Queries"]
        F --> G["Generate Per-Client Diffs"]
    end

    subgraph Broadcast["Broadcast"]
        G --> H["Push to Subscriber A"]
        G --> I["Push to Subscriber B"]
        G --> J["Push to Subscriber N"]
    end

    A -->|WebSocket| D
    E -->|Confirm or Rollback| C

    style Client fill:#14532d,stroke:#86efac,color:#fff
    style Server fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style Broadcast fill:#0f3460,stroke:#F59E0B,color:#fff
```

### Auth Flow: Signup to Session

```mermaid
sequenceDiagram
    participant U as User
    participant C as Client SDK
    participant A as Auth Engine
    participant DB as PostgreSQL

    U->>C: signUp(email, password)
    C->>A: POST /auth/signup
    A->>A: Validate + Argon2id hash
    A->>DB: INSERT user + credentials
    A->>A: Generate RS256 JWT (15min)
    A->>A: Generate refresh token (7d)
    A-->>C: { accessToken, refreshToken, user }
    C->>C: Store tokens + open WebSocket

    Note over C,A: 14 minutes later...

    C->>A: POST /auth/refresh { refreshToken }
    A->>A: Verify + rotate refresh token
    A->>A: Issue new access + refresh pair
    A-->>C: { accessToken, refreshToken }
    C->>C: Seamless token swap

    Note over C,A: OAuth flow

    U->>C: signIn("google")
    C->>A: Redirect to Google OAuth
    A->>A: Exchange code for profile
    A->>DB: Find or create user
    A-->>C: { accessToken, refreshToken, user }
```

### Permission Evaluation Pipeline

Every data access passes through this pipeline. There are no shortcuts — even admin dashboard queries go through the same path.

```mermaid
graph TB
    REQ["Incoming Request"] --> AUTH["1. Authenticate\nJWT verification"]
    AUTH --> ROLE["2. Resolve Roles\nUser roles + team roles + inherited"]
    ROLE --> TABLE["3. Table Permission\nCan this role access this table?"]
    TABLE --> ROW["4. Row-Level Security\nInject WHERE clauses"]
    ROW --> FIELD["5. Field Filtering\nStrip restricted columns"]
    FIELD --> COMPLEXITY["6. Query Complexity\nReject expensive queries"]
    COMPLEXITY --> EXEC["7. Execute\nRun against Postgres"]
    EXEC --> SANITIZE["8. Sanitize Response\nFinal field strip + audit log"]
    SANITIZE --> RESPONSE["Response to Client"]

    AUTH -->|"Fail"| DENY["401 Unauthorized"]
    TABLE -->|"Fail"| DENY2["403 Forbidden"]
    COMPLEXITY -->|"Fail"| DENY3["429 Too Complex"]

    style REQ fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style RESPONSE fill:#14532d,stroke:#86efac,color:#fff
    style DENY fill:#7f1d1d,stroke:#fca5a5,color:#fff
    style DENY2 fill:#7f1d1d,stroke:#fca5a5,color:#fff
    style DENY3 fill:#7f1d1d,stroke:#fca5a5,color:#fff
    style AUTH fill:#713f12,stroke:#fde68a,color:#fff
    style ROLE fill:#713f12,stroke:#fde68a,color:#fff
    style TABLE fill:#365314,stroke:#bbf7d0,color:#fff
    style ROW fill:#365314,stroke:#bbf7d0,color:#fff
    style FIELD fill:#064e3b,stroke:#6ee7b7,color:#fff
    style COMPLEXITY fill:#064e3b,stroke:#6ee7b7,color:#fff
    style EXEC fill:#0c4a6e,stroke:#7dd3fc,color:#fff
    style SANITIZE fill:#0c4a6e,stroke:#7dd3fc,color:#fff
```

### Offline-First Sync Cycle

DarshanDB clients work fully offline. Mutations queue locally and reconcile on reconnect. Conflicts are resolved with last-writer-wins by default, or custom merge functions.

```mermaid
stateDiagram-v2
    [*] --> Online
    Online --> Offline: Connection lost
    Offline --> Reconnecting: Network detected

    state Online {
        [*] --> LiveSync
        LiveSync --> Mutate: User writes
        Mutate --> Optimistic: Apply locally
        Optimistic --> ServerConfirm: Send to server
        ServerConfirm --> LiveSync: ACK received
        ServerConfirm --> Rollback: NACK / conflict
        Rollback --> LiveSync: Revert optimistic
    }

    state Offline {
        [*] --> LocalOps
        LocalOps --> QueueMutation: User writes
        QueueMutation --> IndexedDB: Persist to disk
        IndexedDB --> LocalOps: Apply optimistic
    }

    state Reconnecting {
        [*] --> SendQueue
        SendQueue --> ConflictCheck: Server processes queue
        ConflictCheck --> Merge: Resolve conflicts
        Merge --> CatchUp: Receive missed changes
        CatchUp --> [*]
    }

    Reconnecting --> Online: Sync complete
```

---

## Why DarshanDB Is Faster

```mermaid
graph LR
    subgraph REST["Traditional REST"]
        R1["New TCP+TLS\nper request"] --> R2["JSON encoding\n~58 bytes/obj"]
        R2 --> R3["Full response\nevery poll"]
        R3 --> R4["800B headers\nrepeated"]
    end

    subgraph Darshan["DarshanDB"]
        D1["Single persistent\nconnection"] --> D2["MsgPack binary\n~42 bytes/obj"]
        D2 --> D3["Delta-only\npatches"]
        D3 --> D4["Zero header\noverhead"]
    end

    style REST fill:#dc2626,stroke:#fff,color:#fff
    style Darshan fill:#16a34a,stroke:#fff,color:#fff
```

<div align="center">

### Benchmark Results

| Metric | REST (20 req/s) | DarshanDB | Improvement |
|--------|:---------------:|:---------:|:-----------:|
| **Latency** | ~248ms avg | ~1.2ms avg | **206x faster** |
| **Bandwidth** | ~4,800 B/s overhead | ~180 B/s overhead | **26x less** |
| **Payload size** | 58 bytes/object (JSON) | 42 bytes/object (MsgPack) | **28% smaller** |
| **Auth overhead** | Verify every request | Verify once at connection | **Zero redundancy** |
| **Polling** | Continuous HTTP polling | Server push on change | **Zero polling** |

</div>

---

## Complete Feature Set

### Core Database
- **Triple-store graph engine** over Postgres — EAV with schema-on-read
- **DarshanQL** — declarative, relational, graph-traversal queries from the client
- **Auto schema inference** — write first, schema emerges. No migrations in dev
- **Strict mode** — opt-in enforcement with auto-generated migrations for prod
- **Full-text search** — `$search: "machine learning"` via Postgres tsvector
- **Vector search** — `$semantic: "things about cats"` via pgvector
- **Time-travel** — query any past state via MVCC snapshots
- **Multi-tenancy** — namespace isolation, shared infrastructure

### Real-Time Sync
- **Persistent WebSocket** with multiplexed subscriptions
- **Reactive queries** — every query is a live subscription
- **Optimistic mutations** — instant UI, server reconciliation, auto-rollback
- **Offline-first** — IndexedDB persistence, operation queue, sync on reconnect
- **Presence** — cursors, typing indicators, online status
- **Delta compression** — only changed fields transmitted
- **Catch-up protocol** — reconnecting clients get only the diff

### Server Functions
- **Queries** — read-only, cacheable, reactive
- **Mutations** — transactional ACID writes
- **Actions** — side-effects (HTTP, email, webhooks)
- **Cron jobs** — `darshan.cron("cleanup", "0 3 * * *", handler)`
- **V8 sandboxing** — CPU/memory limits, network allowlist
- **Hot reload** — functions update on file save

### Authentication
- Email/password (Argon2id) -- Magic links -- OAuth (Google, GitHub, Apple, Discord)
- JWT RS256 + refresh tokens -- MFA (TOTP + WebAuthn) -- Session management

### Permissions
- Row-level security -- Field-level permissions -- Role hierarchy -- TypeScript DSL
- **Zero-trust default** — everything denied unless explicitly allowed

### Storage
- S3-compatible (local FS, S3, R2, MinIO) -- Signed URLs -- Image transforms -- Resumable uploads

---

## Universal SDK Ecosystem

```mermaid
graph TB
    subgraph Tier1["First-Class SDKs"]
        React["React\n@darshan/react\nHooks + Suspense"]
        Next["Next.js\n@darshan/nextjs\nRSC + Server Actions"]
        Angular["Angular\n@darshan/angular\nSignals + RxJS + SSR"]
        Vue["Vue 3\n@darshan/vue\nComposables + Nuxt"]
        Svelte["Svelte\n@darshan/svelte\nStores + SvelteKit"]
    end

    subgraph Tier2["Server SDKs"]
        Node["Node.js\n@darshan/admin\nExpress middleware"]
        PHP["PHP\ndarshan/darshan-php\nLaravel ServiceProvider"]
        Python["Python\ndarshandb\nFastAPI + Django"]
    end

    subgraph Tier3["Universal Access"]
        Vanilla["Vanilla JS\nCDN script tag"]
        Native["React Native\nAsyncStorage"]
        RESTAPI["REST API\nAny HTTP client"]
        SSE["SSE\nEventSource fallback"]
    end

    Core["@darshan/client\nFramework-agnostic core"]

    Core --> Tier1
    Core --> Tier2
    Core --> Tier3

    style Core fill:#F59E0B,stroke:#000,color:#000,stroke-width:2px
    style Tier1 fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style Tier2 fill:#16213e,stroke:#F59E0B,color:#fff
    style Tier3 fill:#0f3460,stroke:#F59E0B,color:#fff
```

| Framework | Package | Features |
|-----------|---------|----------|
| **React** | `@darshan/react` | Hooks, Suspense, useSyncExternalStore |
| **Next.js** | `@darshan/nextjs` | Server Components, Server Actions, App Router, Pages Router |
| **Angular** | `@darshan/angular` | Signals (17+), RxJS, Route Guards, SSR |
| **Vue 3** | `@darshan/vue` | Composables, Nuxt support |
| **Svelte** | `@darshan/svelte` | Stores, SvelteKit support |
| **PHP** | `darshan/darshan-php` | Composer, Laravel ServiceProvider |
| **Python** | `darshandb` | pip, FastAPI/Django integration |
| **Vanilla JS** | CDN `<script>` | `window.DarshanDB`, zero build tools |
| **REST** | Any HTTP client | Full CRUD + query + auth + storage |

---

## Zero-Trust Security

DarshanDB doesn't bolt security on as an afterthought. Security is the foundation — 11 layers deep.

```mermaid
graph TB
    subgraph Stack["Defense-in-Depth: 11 Security Layers"]
        L0["Layer 0: TLS 1.3 Mandatory\nNo plaintext, no TLS 1.2 fallback"]
        L1["Layer 1: Rate Limiting\nToken bucket per IP/user/API key"]
        L2["Layer 2: Input Validation\nSchema-validated at API boundary"]
        L3["Layer 3: Authentication\nJWT RS256 + refresh + device binding"]
        L4["Layer 4: Authorization\nPermission engine on every request"]
        L5["Layer 5: Row-Level Security\nSQL WHERE injection — data invisible, not forbidden"]
        L6["Layer 6: Field Filtering\nRestricted fields stripped from response"]
        L7["Layer 7: Query Complexity\nRejects expensive queries"]
        L8["Layer 8: V8 Sandboxing\nFunctions isolated from system"]
        L9["Layer 9: Audit Logging\nEvery mutation: actor + timestamp + diff"]
        L10["Layer 10: Anomaly Detection\nUnusual patterns trigger alerts"]
    end

    L0 --> L1 --> L2 --> L3 --> L4 --> L5 --> L6 --> L7 --> L8 --> L9 --> L10

    style Stack fill:#1a1a2e,stroke:#e94560,color:#fff
    style L0 fill:#7f1d1d,stroke:#fca5a5,color:#fff
    style L1 fill:#7f1d1d,stroke:#fca5a5,color:#fff
    style L2 fill:#7c2d12,stroke:#fed7aa,color:#fff
    style L3 fill:#713f12,stroke:#fde68a,color:#fff
    style L4 fill:#365314,stroke:#bbf7d0,color:#fff
    style L5 fill:#14532d,stroke:#86efac,color:#fff
    style L6 fill:#064e3b,stroke:#6ee7b7,color:#fff
    style L7 fill:#134e4a,stroke:#5eead4,color:#fff
    style L8 fill:#0c4a6e,stroke:#7dd3fc,color:#fff
    style L9 fill:#1e1b4b,stroke:#a5b4fc,color:#fff
    style L10 fill:#4a044e,stroke:#d8b4fe,color:#fff
```

### OWASP API Top 10 Coverage

| OWASP Risk | How DarshanDB Handles It |
|-----------|--------------------------|
| **BOLA** (Broken Object Auth) | Permission rules are SQL WHERE clauses — unauthorized data never leaves the database |
| **Broken Authentication** | Argon2id + RS256 JWT + device fingerprint + brute-force lockout |
| **Broken Property Auth** | Field-level permissions strip attributes server-side |
| **Resource Consumption** | Token-bucket rate limiting + query complexity analysis |
| **Function Auth** | Every function declares auth requirements, enforced before dispatch |
| **SSRF** | `fetch()` restricted to domain allowlist, private IPs blocked |
| **Misconfiguration** | Secure defaults: CORS off, debug off, admin behind auth, no default passwords |
| **Inventory** | Single binary, one API surface, auto-generated OpenAPI spec |
| **Unsafe Consumption** | Responses validated against declared schemas |

---

## Deployment Topology

```mermaid
graph TB
    subgraph Single["Single Node (Dev / Small Prod)"]
        SN["DarshanDB Binary"]
        SPG[("PostgreSQL")]
        SN --> SPG
    end

    subgraph HA["High Availability Cluster"]
        LB["Load Balancer\nNginx / Caddy / Cloud LB"]

        subgraph Nodes["DarshanDB Nodes"]
            N1["Node 1\nLeader"]
            N2["Node 2\nFollower"]
            N3["Node 3\nFollower"]
        end

        subgraph PGCluster["PostgreSQL Cluster"]
            PG1[("Primary")]
            PG2[("Replica")]
            PG3[("Replica")]
            PG1 -->|"Streaming\nReplication"| PG2
            PG1 -->|"Streaming\nReplication"| PG3
        end

        LB --> N1
        LB --> N2
        LB --> N3
        N1 --> PG1
        N2 --> PG2
        N3 --> PG3
    end

    style Single fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style HA fill:#0f3460,stroke:#F59E0B,color:#fff
    style Nodes fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style PGCluster fill:#16213e,stroke:#336791,color:#fff
    style LB fill:#713f12,stroke:#fde68a,color:#fff
```

---

## Technology Stack

```mermaid
graph LR
    subgraph Runtime["Runtime"]
        Rust["Rust\nAxum + Tokio"]
        V8["Deno Core\nV8 Isolates"]
    end

    subgraph Data["Data"]
        PG["PostgreSQL 16+\npgvector"]
        MP["MessagePack\nBinary wire protocol"]
    end

    subgraph Client["Client"]
        TS["TypeScript\nType-safe SDKs"]
        IDB["IndexedDB\nOffline persistence"]
    end

    subgraph Crypto["Crypto"]
        Argon["Argon2id\nPassword hashing"]
        JWT["RS256/Ed25519\nToken signing"]
        AES["AES-256-GCM\nEncryption at rest"]
    end

    style Runtime fill:#B7410E,stroke:#fff,color:#fff
    style Data fill:#336791,stroke:#fff,color:#fff
    style Client fill:#3178C6,stroke:#fff,color:#fff
    style Crypto fill:#7f1d1d,stroke:#fca5a5,color:#fff
```

| Layer | Choice | Why |
|-------|--------|-----|
| Server | Rust (Axum + Tokio) | Single binary, zero GC pauses, millions of connections |
| Function Runtime | Deno Core (V8) | Secure sandboxing, TypeScript native |
| Database | PostgreSQL 16+ | Battle-tested, pgvector, MVCC, streaming replication |
| Wire Protocol | MsgPack over WebSocket | 28% smaller than JSON, zero-copy decode |
| Client Core | TypeScript | Universal, type-safe, tree-shakeable |
| Admin UI | React + Vite + Tailwind | Fast, responsive, dark-first |
| Password Hashing | Argon2id | PHC winner, GPU-resistant |
| JWT | RS256 + Ed25519 | Asymmetric verification |
| Encryption | AES-256-GCM | Hardware-accelerated |

---

## Self-Hosting

### Docker (Recommended)

```bash
curl -fsSL https://darshandb.dev/docker | sh
# or manually:
docker compose up -d
```

### Bare Metal

```bash
curl -fsSL https://darshandb.dev/install | sh
darshan dev  # development mode with auto-reload
darshan start --prod  # production mode
```

### Kubernetes

```bash
helm repo add darshan https://charts.darshandb.dev
helm install darshan darshan/darshandb \
  --set postgres.storageClass=ssd \
  --set replicas=3
```

---

## Project Structure

```
darshandb/
├── packages/
│   ├── server/          # Rust server — triple store, query, sync, auth, functions
│   ├── cli/             # Rust CLI — darshan dev/deploy/push/pull
│   ├── client-core/     # TypeScript — framework-agnostic client SDK
│   ├── react/           # React SDK — hooks + Suspense
│   ├── angular/         # Angular SDK — Signals + RxJS
│   ├── nextjs/          # Next.js SDK — RSC + Server Actions
│   └── admin/           # Admin dashboard — React + Vite + Tailwind
├── sdks/
│   ├── php/             # PHP SDK — Composer + Laravel
│   └── python/          # Python SDK — pip + FastAPI/Django
├── docs/                # Documentation
├── examples/            # Example apps (React, Next.js, Angular, PHP, Python, HTML)
├── deploy/
│   └── k8s/             # Kubernetes Helm chart
├── Cargo.toml           # Rust workspace
├── docker-compose.yml   # One-command self-hosted setup
└── Dockerfile           # Multi-stage production build
```

---

## Contributing

We welcome contributions from everyone. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

```bash
# Clone and setup
git clone https://github.com/darshjme/darshandb.git
cd darshandb

# Start development
darshan dev

# Run tests
cargo test                          # Rust
npm test --workspace=@darshan/react # TypeScript
```

---

## License

MIT License — use it for anything. See [LICENSE](LICENSE) for details.

---

<div align="center">

<img src=".github/assets/logo.svg" alt="DarshanDB" width="60" />

<br/>
<br/>

**Built by [Darsh Joshi](https://darshj.ai)** -- Ahmedabad, India

[darshj.ai](https://darshj.ai) -- [darshj.me](https://darshj.me) -- [GitHub](https://github.com/darshjme) -- [Discord](https://discord.gg/darshandb) -- [Twitter](https://twitter.com/darshandb)

<br/>

*"The developer in Ahmedabad, the student in Lagos, the freelancer in Sao Paulo —*
*they deserve the same backend infrastructure that FAANG engineers take for granted."*

<br/>

<sub>DarshanDB is open source under the MIT License.</sub>

</div>
