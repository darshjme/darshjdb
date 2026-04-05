# DarshJDB — Code Reference

> Last updated: 2026-04-05
> Status: Alpha — end-to-end data path working, not production-ready

## What Actually Works

The server starts, connects to Postgres, and handles real HTTP requests:

```
curl POST /api/data/users -d '{"name":"Alice"}' → writes triples to Postgres → returns UUID
curl GET  /api/data/users                       → reads entities from triple store
curl POST /api/query -d '{DarshanQL}'           → parses, plans, executes against Postgres
curl POST /api/mutate -d '{ops}'                → batch insert/update/delete → returns tx_id
curl GET  /health                               → real pool stats + triple count
```

What does NOT work yet:
- WebSocket real-time subscriptions (handler exists, not wired to query results)
- Reactive push (sync engine exists, not connected to mutation flow)
- Client SDKs connecting to a running server (types built, no integration test)
- Auth flow (module exists, not enforced on routes in dev mode)
- Server functions (registry exists, no V8/Deno runtime — subprocess placeholder)
- Admin dashboard (renders mock data, not connected to live API)
- File storage (S3 backend code exists, no upload endpoint wired)
- CLI `ddb dev` (compiles, does not auto-start Postgres)

## Project Stats

| Metric | Count |
|--------|-------|
| Git commits | 34 |
| Tracked files | 255 |
| Total lines | 68,111 |
| Rust source files | 32 |
| TypeScript/TSX files | 78 |
| Python files | 11 |
| PHP files | 11 |
| Documentation (Markdown) | 50 |
| SQL migrations | 2 |
| Shell scripts | 6 |
| YAML configs | 16 |
| HTML files | 5 |

## Test Counts

| Language | Tests | Status |
|----------|-------|--------|
| Rust (cargo test) | 438 | All passing |
| TypeScript (vitest) | 92 | All passing |
| Python (pytest) | 141 | All passing |
| PHP (phpunit) | 52 | All passing |
| **Total** | **723** | **All passing** |

## CI Status (GitHub Actions)

| Check | Status |
|-------|--------|
| Rust (fmt + clippy + test) | Passing |
| TypeScript (build + lint + test) | Passing |
| Python (test) | Passing |
| PHP (test) | Passing |
| Docker (smoke build) | Passing |

## Architecture — What Exists

### Server (Rust — packages/server/)

```
src/
├── main.rs              # Server entry point — connects to Postgres, mounts routes
├── lib.rs               # Module re-exports
├── error.rs             # DarshanError enum (thiserror)
├── triple_store/
│   ├── mod.rs           # PgTripleStore — EAV storage over Postgres
│   └── schema.rs        # Schema inference, migration generation
├── query/
│   ├── mod.rs           # DarshanQL parser → query planner → executor
│   └── reactive.rs      # Dependency tracker for subscription invalidation
├── sync/
│   ├── session.rs       # WebSocket session management
│   ├── registry.rs      # Subscription fan-out registry
│   ├── broadcaster.rs   # Change event → affected query → diff → push
│   ├── diff.rs          # Delta diff computation (added/removed/updated)
│   └── presence.rs      # Ephemeral presence rooms with rate limiting
├── auth/
│   ├── providers.rs     # Email/password (Argon2id), magic links, OAuth
│   ├── session.rs       # JWT RS256 with key rotation, refresh tokens
│   ├── mfa.rs           # TOTP (RFC 6238), recovery codes
│   ├── permissions.rs   # Row-level security rules → SQL WHERE clauses
│   └── middleware.rs    # Axum middleware, rate limiting (token bucket)
├── functions/
│   ├── runtime.rs       # Function execution (subprocess-based, not V8)
│   ├── registry.rs      # Scan .ts/.js files, parse exports, hot reload
│   ├── validator.rs     # Argument schema validation
│   └── scheduler.rs     # Cron job scheduler with Postgres advisory locks
├── api/
│   ├── rest.rs          # All REST endpoints — WIRED TO REAL TRIPLE STORE
│   ├── ws.rs            # WebSocket handler (MsgPack + JSON protocol)
│   ├── error.rs         # Consistent error envelope
│   └── openapi.rs       # OpenAPI 3.1 spec generation
└── storage/
    └── mod.rs           # S3-compatible storage with signed URLs
```

### CLI (Rust — packages/cli/)

```
src/
├── main.rs              # clap CLI: dev, deploy, push, pull, seed, migrate, logs
└── config.rs            # Config file resolution
```

### Client SDK (TypeScript — packages/client-core/)

```
src/
├── client.ts            # DarshJDB class — connection state machine, reconnect
├── query.ts             # QueryBuilder with deduplication
├── transaction.ts       # Proxy-based tx.entity[id].set/merge/delete
├── sync.ts              # IndexedDB cache, optimistic updates, offline queue
├── presence.ts          # Presence rooms with throttled publish
├── auth.ts              # Auth client with token refresh
├── storage.ts           # File upload with progress tracking
├── rest.ts              # REST transport fallback (SSE for subscriptions)
└── types.ts             # All shared type definitions
```

### React SDK (packages/react/)

```
src/
├── provider.tsx         # DarshanProvider context
├── use-query.ts         # useQuery — useSyncExternalStore, Suspense
├── use-mutation.ts      # useMutation — stable reference, error state
├── use-presence.ts      # usePresence — auto join/leave
├── use-auth.ts          # useAuth — reactive auth state
└── use-storage.ts       # useStorage — upload with progress
```

### Angular SDK (packages/angular/)

```
src/
├── ddb.module.ts    # NgModule with forRoot()
├── providers.ts         # provideDarshan() for standalone
├── query.signal.ts      # Signal-based queries (Angular 17+)
├── query.observable.ts  # Observable queries (RxJS)
├── auth.ts              # Auth guard + interceptor
├── presence.ts          # Presence directive
└── ssr.ts               # TransferState integration
```

### Next.js SDK (packages/nextjs/)

```
src/
├── server.ts            # queryServer(), mutateServer() — REST-based
├── provider.tsx         # DarshanProvider with SSR hydration
├── pages.ts             # getServerSideProps/getStaticProps wrappers
├── middleware.ts         # Route protection middleware
└── api.ts               # API route wrappers with auth context
```

### Admin Dashboard (packages/admin/)

```
src/
├── App.tsx              # Shell with sidebar routing
├── pages/
│   ├── DataExplorer.tsx # Data table with DarshanQL editor
│   ├── Schema.tsx       # ER diagram visualization
│   ├── Functions.tsx    # Function list with execution charts
│   ├── AuthUsers.tsx    # User management
│   ├── Storage.tsx      # File browser with upload
│   ├── Logs.tsx         # Structured log viewer
│   └── Settings.tsx     # Config, backups, webhooks
└── components/          # Sidebar, TopBar, DataTable, CommandPalette, etc.
```

### PHP SDK (sdks/php/)

```
src/
├── Client.php           # Main client — query, transact, fn, auth, storage
├── AuthClient.php       # Sign up/in/out, OAuth, token management
├── QueryBuilder.php     # Fluent query builder
├── StorageClient.php    # Upload, getUrl, delete
├── DarshanException.php # Typed exception with status code
└── Laravel/             # ServiceProvider + Facade
```

### Python SDK (sdks/python/)

```
src/darshjdb/
├── client.py            # DarshJDB class — query, transact, fn
├── auth.py              # AuthClient — sign_up/in/out, OAuth
├── storage.py           # StorageClient — upload, get_url, delete
├── admin.py             # DarshanAdmin — impersonation, SSE subscribe
└── exceptions.py        # DarshanError, DarshanAPIError
```

## Database Schema

The triple store uses a single table:

```sql
CREATE TABLE IF NOT EXISTS triples (
    id          BIGSERIAL PRIMARY KEY,
    entity_id   UUID NOT NULL,
    attribute   TEXT NOT NULL,
    value       JSONB NOT NULL,
    value_type  SMALLINT NOT NULL DEFAULT 0,
    tx_id       BIGINT NOT NULL DEFAULT nextval('darshan_tx_seq'),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    retracted   BOOLEAN NOT NULL DEFAULT false
);
```

Value types: 0=string, 1=integer, 2=float, 3=boolean, 4=null, 5=reference, 6=json

Indexes: entity+attribute (partial, non-retracted), attribute+value (GIN), tx_id, entity+tx, attribute (partial).

## Security Audit Findings (Fixed)

| Severity | Issue | Fix |
|----------|-------|-----|
| CRITICAL | LIKE wildcard injection in $search | Escape %, _, \ before wrapping |
| CRITICAL | Null byte injection in storage paths | Added \0 check |
| CRITICAL | Subprocess zombie on function timeout | Kill on drop |
| HIGH | No content-type validation on upload | Added blocklist |
| HIGH | No upload size limit | Added max_upload_size (100MB) |
| HIGH | Timing attack on signed URL verify | Constant-time HMAC |
| HIGH | Mutation bypassed entity name validation | Added check |
| HIGH | Rate limiter stored raw token prefix | SHA-256 hash |
| MEDIUM | TOCTOU race in subscription registry | Atomic remove_if |
| MEDIUM | Non-deterministic diff hashing | Canonical key sorting |
| MEDIUM | TOTP timing oracle | Constant-time comparison |
| MEDIUM | JWT missing audience validation | Added aud claim |

## Strategic Roadmaps

Written in `docs/strategy/`:

- **AI_ML_STRATEGY.md** — Auto-embedding, ctx.ai namespace, MCP server, RAG, NL-to-DarshanQL
- **WEB3_STRATEGY.md** — SIWE wallet auth, token-gated permissions, IPFS/Arweave, chain indexing
- **ENTERPRISE_STRATEGY.md** — Self-hosted + SaaS dual model, multi-tenancy, SOC2/HIPAA, pricing
- **SCALABILITY_STRATEGY.md** — 4 scale tiers (500→1M+), NATS, Redis cache, Patroni DR
- **DX_STRATEGY.md** — 24 DX improvements, error codes, ddb doctor, CLI enhancements

## How to Run

```bash
# Start Postgres
docker compose up postgres -d

# Initialize database + seed
./scripts/setup-db.sh --seed

# Start server
DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshjdb cargo run --bin ddb-server

# Test it
curl http://localhost:7700/health
curl -X POST http://localhost:7700/api/data/users \
  -H "Content-Type: application/json" \
  -d '{"name":"Alice","email":"alice@test.com"}'

# Full E2E test
./scripts/e2e-test.sh
```

## Repo

- GitHub: https://github.com/darshjme/darshjdb (public)
- License: MIT
- Built by: Darsh Joshi (born Navsari, Gujarat — now Ahmedabad)
- CEO at GraymatterOnline LLP, CTO at KnowAI
