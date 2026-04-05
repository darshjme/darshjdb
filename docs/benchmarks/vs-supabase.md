# DarshJDB vs Supabase: Architectural Comparison

A technically honest comparison between DarshJDB and Supabase, written for engineers evaluating both systems. Neither product is universally better -- each makes fundamental tradeoffs that favor different use cases.

**Versions compared**: DarshJDB 0.1.0 (single-binary Rust server) vs Supabase (managed platform / self-hosted Docker Compose).

---

## Table of Contents

1. [Architecture](#1-architecture)
2. [Data Model](#2-data-model)
3. [Real-Time](#3-real-time)
4. [Authentication](#4-authentication)
5. [Permissions](#5-permissions)
6. [Server Functions](#6-server-functions)
7. [Storage](#7-storage)
8. [Developer Experience](#8-developer-experience)
9. [Self-Hosting](#9-self-hosting)
10. [What Supabase Does Better](#10-what-supabase-does-better)
11. [What DarshJDB Does Better](#11-what-darshjdb-does-better)
12. [Migration Path](#12-migration-path-supabase-to-darshjdb)

---

## 1. Architecture

### Supabase: Microservice Ensemble

Supabase is a composition of five-plus independent open-source projects behind a unified API gateway (Kong or Envoy):

| Service | Language | Role |
|---------|----------|------|
| **PostgreSQL** | C | Primary data store |
| **PostgREST** | Haskell | Auto-generated REST API from schema |
| **GoTrue** | Go | Authentication (JWT, OAuth, MFA) |
| **Realtime** | Elixir | WebSocket subscriptions via Postgres LISTEN/NOTIFY |
| **Storage API** | TypeScript (Node) | S3-compatible file storage |
| **Edge Functions** | Deno (Rust) | Serverless compute |
| **pg_graphql** | Rust (PG extension) | GraphQL interface |
| **Kong/Envoy** | Lua/C++ | API gateway, routing, rate limiting |
| **Studio** | TypeScript (Next.js) | Admin dashboard |

**Total moving parts**: 8-12 containers in a self-hosted deployment. Each service has independent health, scaling, and failure modes.

### DarshJDB: Monolithic Binary

DarshJDB compiles to a single Rust binary that embeds all subsystems:

```
darshjdb (single process)
  +-- Triple Store (EAV on Postgres via sqlx)
  +-- DarshanQL Query Engine (LRU plan cache, parallel execution)
  +-- Auth Engine (JWT, OAuth2, MFA, rate limiting)
  +-- Permission Engine (composable rules, WHERE injection)
  +-- Sync Engine (WebSocket, subscriptions, diff, presence, pub/sub)
  +-- Function Runtime (Node subprocess isolation)
  +-- Storage Engine (local FS, S3, R2, MinIO backends)
  +-- Embedding Pipeline (OpenAI, Ollama auto-embed)
  +-- Connector System (webhooks, log, extensible)
  +-- Rule Engine (declarative triggers)
  +-- REST API + SSE + OpenAPI docs
  +-- Health / Readiness probes (K8s-compatible)
```

**External dependency**: PostgreSQL (and optionally pgvector for semantic search). Everything else is in-process.

### Tradeoff Analysis

| Dimension | Supabase | DarshJDB |
|-----------|----------|-----------|
| **Operational complexity** | High -- each service has its own config, logs, version matrix, failure modes | Low -- one binary, one config, one log stream |
| **Independent scaling** | Yes -- scale Realtime separately from PostgREST | No -- vertical scaling only (but Rust's efficiency pushes this surprisingly far) |
| **Failure blast radius** | Contained -- GoTrue crashing does not kill Realtime | Total -- a panic in any subsystem kills the process (mitigated by CatchPanicLayer and graceful shutdown) |
| **Deployment speed** | Minutes (Docker Compose pull + init) | Seconds (download binary, set DATABASE_URL, run) |
| **Resource consumption** | 2-4 GB RAM minimum for the full stack at rest | 30-80 MB RSS typical for a loaded DarshJDB instance |
| **Inter-service latency** | Present -- auth check goes process-to-process via HTTP/gRPC | Zero -- auth check is an in-process function call |
| **Version coordination** | Tight coupling between PostgREST schema cache refresh and GoTrue token format | Not applicable -- single version, single release |

**Verdict**: Supabase's microservice split is the right architecture for a managed cloud platform that needs to scale individual bottlenecks. DarshJDB's monolith is the right architecture for self-hosted deployments where operational simplicity and resource efficiency matter more than elastic scaling.

---

## 2. Data Model

### Supabase: SQL Tables (Relational)

Supabase exposes raw PostgreSQL. You define schemas with CREATE TABLE, add indexes, write migrations. PostgREST introspects `information_schema` and generates REST endpoints per table.

```sql
CREATE TABLE users (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  email TEXT UNIQUE NOT NULL,
  name TEXT,
  created_at TIMESTAMPTZ DEFAULT NOW()
);
```

Query via PostgREST:
```
GET /rest/v1/users?email=eq.alice@example.com&select=id,name
```

### DarshJDB: Triple Store (EAV)

All data is stored as `(entity_id, attribute, value, value_type, tx_id)` triples in a single `triples` table. Entity types are inferred from a `:db/type` attribute.

```json
POST /api/data
{
  "entity_type": "User",
  "attributes": {
    "email": "alice@example.com",
    "name": "Alice",
    "metadata": { "plan": "pro" }
  }
}
```

Query via DarshanQL:
```json
{
  "from": "User",
  "where": [{ "attribute": "email", "op": "=", "value": "alice@example.com" }],
  "select": ["email", "name"]
}
```

### Tradeoff Analysis

| Dimension | Supabase (SQL) | DarshJDB (Triple/EAV) |
|-----------|----------------|------------------------|
| **Schema flexibility** | Rigid -- ALTER TABLE for every change | Fluid -- add any attribute to any entity at any time, no migration |
| **Join performance** | Native SQL joins, index-driven, highly optimized by Postgres planner | Self-joins on the triples table for multi-attribute queries; relies on composite indexes `(entity_id, attribute)` |
| **Complex queries** | Full SQL power (CTEs, window functions, recursive queries, lateral joins) | DarshanQL covers 80% of cases; escape hatch to raw SQL for the rest |
| **Schema discovery** | `information_schema` -- always accurate | Inferred from live data via `schema::EntityType` introspection; the `MigrationGenerator` diffs snapshots |
| **Polymorphism** | Requires JSONB columns or table inheritance | Native -- same attribute can hold different `ValueType` discriminators across entities |
| **Time-travel queries** | Requires custom audit triggers or pgaudit | Built-in -- every triple has `tx_id` and `retracted` flag; query `as_of_tx` for point-in-time reads |
| **Bulk performance** | INSERT with VALUES or COPY -- extremely fast | UNNEST-based bulk insert (`bulk_load`) with measured throughput; still uses Postgres under the hood |
| **Raw SQL access** | Full -- you own the database | Limited -- the triple store abstracts over Postgres; you can query `triples` directly but lose the DarshanQL features |
| **Type safety** | Enforced at the column level by Postgres | Enforced per-triple via `value_type` discriminator (0-6: string, integer, float, boolean, timestamp, reference, json) |
| **TTL / expiry** | Manual -- write a cron job or use pg_cron | Built-in -- `expires_at` column with background reaper every 10 seconds |

**Verdict**: Supabase gives you the full power of relational modeling. If your data is naturally tabular and you need complex joins, aggregations, and window functions, SQL is unbeatable. DarshJDB's triple store trades query sophistication for schema flexibility and built-in time-travel -- it is better suited for applications where the data shape evolves rapidly (CMS, multi-tenant SaaS, collaborative tools) than for analytical workloads.

**Honest limitation**: The EAV model incurs a measurable performance penalty on multi-attribute filters compared to a properly indexed relational table. A query filtering on 5 attributes requires 5 self-joins on the triples table. DarshJDB mitigates this with plan caching and parallel query execution, but it will not outperform a single indexed scan on a dedicated column.

---

## 3. Real-Time

### Supabase: Postgres LISTEN/NOTIFY via Elixir

Supabase Realtime is a separate Elixir/Phoenix application that:

1. Subscribes to Postgres replication slot (logical decoding) or `LISTEN/NOTIFY` channels.
2. Parses WAL events or notification payloads.
3. Filters by table, schema, and optional row-level security policies.
4. Broadcasts to connected clients via Phoenix Channels (WebSocket).

Supports three modes:
- **Postgres Changes**: Subscribe to INSERT/UPDATE/DELETE on specific tables.
- **Broadcast**: Ephemeral pub/sub between clients (no persistence).
- **Presence**: Shared ephemeral state with conflict resolution.

### DarshJDB: In-Process Broadcast with Diff Engine

DarshJDB's sync engine is compiled into the server binary:

1. Every mutation emits a `ChangeEvent` on a Tokio broadcast channel (4096 buffer).
2. The `DependencyTracker` maps attribute changes to affected query hashes.
3. Affected queries are re-executed with the subscriber's permission context.
4. The `DiffEngine` computes a minimal delta (added/removed/updated entities with field-level patches).
5. Diffs are pushed over the WebSocket connection (JSON or MessagePack).

Additionally:
- **Presence**: Per-room ephemeral state with auto-expiry and rate limiting, built into the WS protocol.
- **Pub/Sub**: Keyspace notification channels (`entity:users:*`) with glob matching.
- **SSE fallback**: REST endpoint for environments where WebSockets are blocked.

### Latency Comparison

| Path | Supabase | DarshJDB |
|------|----------|-----------|
| Mutation to first subscriber notification | Postgres WAL decode -> Elixir process -> Phoenix Channel push. Typical: **50-200ms** depending on replication lag. | In-process broadcast channel -> tokio task -> WebSocket frame. Typical: **1-10ms** (no network hop, no WAL decode). |
| Fan-out to N subscribers | Elixir is excellent at this (BEAM scheduler). Scales to millions of connections per Realtime node. | Tokio task per affected subscription. Efficient for thousands; untested at millions per instance. |
| Granularity | Table-level or row-filter level. Sends full row payloads. | Query-level. Sends minimal diffs (only changed fields, not full entities). |

### Reliability

| Dimension | Supabase | DarshJDB |
|-----------|----------|-----------|
| **Guaranteed delivery** | No -- LISTEN/NOTIFY is fire-and-forget. Realtime can miss events during reconnect. Broadcast channel has no persistence. | No -- broadcast channel drops events when buffer is full (4096 capacity). Clients re-sync via full query on reconnect. |
| **Ordering** | WAL-ordered (strong, monotonic) | `tx_id` ordered (strong, monotonic within a single server) |
| **Backpressure** | Elixir process mailboxes can grow unbounded | Bounded channel with drop semantics; explicit `change_count` tracking per diff |

**Verdict**: DarshJDB has significantly lower latency because there is no inter-process hop or WAL decode step. The diff-based approach also reduces bandwidth -- clients receive only the fields that changed, not entire row payloads. However, Supabase's Elixir-based Realtime server is battle-tested at scale with millions of concurrent connections. DarshJDB's Tokio-based approach is efficient but unproven at that level.

---

## 4. Authentication

### Supabase: GoTrue

GoTrue is a standalone Go service providing:

- Email/password with confirmation flow
- Magic link (passwordless email)
- Phone OTP (SMS via Twilio/MessageBird)
- OAuth2 (Google, GitHub, Apple, Discord, Slack, Spotify, Twitter, Azure AD, ~20 providers)
- SAML SSO (enterprise)
- PKCE flow for SPAs
- MFA (TOTP via authenticator apps)
- Session management with refresh token rotation
- User metadata and custom claims
- Email templates (customizable)
- Webhook callbacks on auth events

### DarshJDB: Built-In Auth

DarshJDB's auth engine is compiled into the server:

- **Password**: Argon2id (64 MB memory cost, 3 iterations, 4-way parallelism -- OWASP recommended)
- **Magic Link**: 32-byte random token, SHA-256 hashed storage, 15-minute expiry, one-time use
- **OAuth2**: Generic provider abstraction with PKCE mandatory, HMAC-signed state. Concrete implementations for Google, GitHub, Apple, Discord
- **MFA**: TOTP (RFC 6238, HMAC-SHA1, 6-digit, 30s period, +/-1 step window), recovery codes (Argon2id hashed), WebAuthn stubs
- **Sessions**: RS256 JWT access tokens (15-minute lifetime), opaque refresh tokens (SHA-256 hashed, device-fingerprint bound, 30-day lifetime)
- **Key rotation**: Current + previous key pair; grace window for old tokens
- **Rate limiting**: In-process per-IP/per-user with configurable windows and automatic cleanup

### Feature Parity Matrix

| Feature | Supabase | DarshJDB |
|---------|----------|-----------|
| Email/password | Yes | Yes |
| Magic link | Yes | Yes |
| Phone OTP (SMS) | Yes | No |
| OAuth2 providers | ~20 built-in | 4 built-in (Google, GitHub, Apple, Discord) + generic provider trait |
| SAML SSO | Yes (enterprise) | No |
| TOTP MFA | Yes | Yes |
| Recovery codes | Yes | Yes |
| WebAuthn/Passkeys | Yes | Stubs only (registration + assertion interfaces defined, no implementation) |
| Email templates | Yes (customizable) | No -- relies on external email service |
| Custom claims | Yes | Yes (via `roles` in JWT) |
| Device fingerprinting | No | Yes (refresh tokens bound to device fingerprint) |
| Key rotation | Yes | Yes (current + previous key pair) |
| JWT algorithm | HS256 | RS256 (production), HS256 (dev), with documented PQC migration path |

**Verdict**: Supabase has broader auth coverage, especially for enterprise use cases (SAML, phone OTP, 20+ OAuth providers, email templates). DarshJDB has stronger cryptographic defaults (Argon2id with OWASP params, RS256 by default, device-fingerprint binding) and lower latency (no inter-service token validation hop). If you need phone OTP or SAML, Supabase wins outright. If you need tight auth-to-data integration in a self-hosted environment, DarshJDB's in-process auth eliminates an entire class of token-relay bugs.

---

## 5. Permissions

### Supabase: Row-Level Security (RLS)

Supabase leverages PostgreSQL's native RLS policies:

```sql
CREATE POLICY "Users can read own data"
  ON users
  FOR SELECT
  USING (auth.uid() = id);

CREATE POLICY "Admins can read all"
  ON users
  FOR SELECT
  USING (auth.jwt() ->> 'role' = 'admin');
```

Policies are:
- Enforced at the database level (cannot be bypassed by any client)
- Written in SQL (full expression power, can call functions)
- Composable with OR semantics (any matching policy grants access)
- Per-table, per-operation (SELECT, INSERT, UPDATE, DELETE)
- Dependent on `auth.uid()` and `auth.jwt()` helper functions set by PostgREST

### DarshJDB: Composable Permission Rules

DarshJDB uses a JSON-configurable permission engine with composable rules:

```json
{
  "documents": {
    "read": {
      "type": "composite",
      "operator": "and",
      "rules": [
        { "type": "role_check", "required_role": "editor" },
        { "type": "where_clause", "sql": "owner_id = $user_id" }
      ]
    },
    "create": { "type": "role_check", "required_role": "editor" },
    "delete": { "type": "deny" }
  }
}
```

Permission rules:
- `allow` / `deny` -- unconditional
- `role_check` -- requires specific role in AuthContext
- `where_clause` -- injects SQL fragment into read queries (parameterized, not interpolated)
- `field_restriction` -- allowed/denied field lists
- `composite` -- AND/OR combinators with arbitrary nesting

Enforcement paths:
- **Read path**: WHERE clause injection into query planner (row-level filtering)
- **Write path**: Pre-transaction allow/deny evaluation
- **Subscribe path**: Permission-scoped query re-execution for diffs
- **Default**: Deny-by-default for unconfigured entity types

### Comparison

| Dimension | Supabase RLS | DarshJDB Permissions |
|-----------|-------------|----------------------|
| **Enforcement level** | Database kernel (unbypassable) | Application layer (the server is the only Postgres client, so this is equivalent in practice) |
| **Expression power** | Full SQL (functions, subqueries, joins) | Limited DSL (role checks, WHERE fragments, field restrictions, composites) |
| **Bypass risk** | Zero -- even `psql` respects RLS (unless BYPASSRLS role) | Moderate -- direct Postgres access bypasses all DarshJDB permissions |
| **Debugging** | `EXPLAIN` shows policy application; `pg_policies` catalog | Structured `PermissionResult` with denial reasons and WHERE clause trace |
| **Performance** | Policies are optimized by the Postgres planner alongside the query | WHERE fragments are injected before execution; separate evaluation step adds overhead |
| **Field-level security** | Requires column-level grants (limited) or views | Native -- `field_restriction` rules with allowed/denied lists |
| **Subscription filtering** | Realtime checks RLS on each WAL event | Queries re-executed with subscriber's permission context for every diff |
| **Hot reload** | ALTER POLICY -- requires DDL permission | Reload JSON config -- no restart needed |

**Verdict**: Supabase RLS is more expressive and more secure by design -- policies are enforced at the database kernel level with full SQL power. DarshJDB's permission engine is more developer-friendly (JSON config, composable rules, field-level restrictions) but relies on the application layer for enforcement. For security-critical applications, the database-level enforcement of RLS is a meaningful advantage.

---

## 6. Server Functions

### Supabase: Edge Functions (Deno Deploy)

```typescript
// supabase/functions/hello/index.ts
Deno.serve(async (req) => {
  const { name } = await req.json();
  return new Response(JSON.stringify({ message: `Hello ${name}` }));
});
```

- Runtime: Deno (V8 isolate per invocation)
- Cold start: ~50-200ms on managed platform
- Limits: 150ms CPU time (free), 2s (pro) wall clock; 150 MB memory
- Deployment: `supabase functions deploy`
- Local dev: `supabase functions serve` (Deno runtime)
- Access to: Supabase client libraries, environment variables, fetch API

### DarshJDB: Server Functions (Node Subprocess)

```typescript
// darshan/functions/hello.ts
import { query, mutation, action } from "darshjdb";

export const hello = action({
  args: { name: "string" },
  handler: async (ctx, args) => {
    return { message: `Hello ${args.name}` };
  },
});
```

- Runtime: Node.js subprocess with a harness (`_darshan_harness.js`)
- Function types: `query` (read-only), `mutation` (read-write), `action` (side effects), `httpAction` (raw HTTP), `scheduled` (cron)
- Resource limits: Configurable CPU timeout, memory ceiling, max concurrency (semaphore-controlled)
- Registry: File-system discovery with hot reload (`notify` crate watches `darshan/functions/`)
- Argument validation: Schema-based via `ArgSchema` with type checking before execution
- Scheduler: Cron expressions with distributed locking and retry
- Context injection: `ctx.db` (triple store), `ctx.auth` (current user), `ctx.scheduler` (cron management)

### Comparison

| Dimension | Supabase Edge Functions | DarshJDB Server Functions |
|-----------|------------------------|---------------------------|
| **Isolation** | V8 isolate (strong sandbox) | OS process (moderate isolation, no sandbox) |
| **Cold start** | 50-200ms | Higher -- Node process spawn (~200-500ms for first invocation) |
| **Type variety** | HTTP handler only | 5 types: query, mutation, action, httpAction, scheduled |
| **Scheduled execution** | Via pg_cron or external scheduler | Built-in cron scheduler with distributed locking |
| **Hot reload** | Redeploy required | File watcher triggers automatic re-registration |
| **Ecosystem** | Deno (growing, Web Standards-aligned) | Node.js (massive npm ecosystem) |
| **Data access** | Via Supabase client SDK (network hop) | Direct `ctx.db` access (in-process IPC to triple store) |
| **Argument validation** | Manual | Declarative schema with pre-execution validation |

**Verdict**: Supabase Edge Functions have stronger isolation (V8 sandbox) and are better suited for untrusted or multi-tenant code. DarshJDB Server Functions are tighter integrated with the data layer (direct `ctx.db` access, built-in scheduling, type-differentiated functions) but lack sandbox isolation -- they trust the function author. For a self-hosted system where you control the function code, DarshJDB's model is more productive. For a managed platform, Supabase's isolation model is mandatory.

---

## 7. Storage

### Supabase Storage

- S3-compatible API behind a Node.js gateway
- Bucket-level access policies (RLS on `storage.objects` table)
- Image transformations via CDN (resize, crop, format conversion)
- Resumable uploads (TUS protocol)
- Signed URLs for time-limited access
- Dashboard integration for browsing files

### DarshJDB Storage

- Pluggable backend trait: `LocalFsBackend`, S3, R2, MinIO
- Signed URLs with HMAC authentication
- Image transform hooks (delegates to external processor or CDN)
- Resumable uploads (TUS-compatible)
- Upload hooks (pre/post callbacks for validation, virus scanning, metadata)
- Object metadata with user-defined key-value pairs

### Comparison

| Dimension | Supabase | DarshJDB |
|-----------|----------|-----------|
| **Backend options** | S3 only (managed), any S3-compatible (self-hosted) | Local FS, S3, R2, MinIO (pluggable trait) |
| **Access control** | RLS policies on `storage.objects` table | Application-level (via auth middleware on storage endpoints) |
| **Image transforms** | Built-in via CDN (managed) | Hooks for external processors -- not built-in |
| **Dashboard browsing** | Yes | No (API-only) |
| **Integration depth** | Separate service, accessed via REST | Same binary, same auth context, zero-hop access |

**Verdict**: Supabase Storage is more feature-complete, especially on the managed platform (CDN image transforms, dashboard UI). DarshJDB Storage is more operationally simple (same binary) and offers more backend flexibility (local FS for development, any S3-compatible for production). The local FS backend is a significant advantage for self-hosted deployments that do not want to run MinIO alongside Postgres.

---

## 8. Developer Experience

### SDKs

| SDK | Supabase | DarshJDB |
|-----|----------|-----------|
| JavaScript/TypeScript | `@supabase/supabase-js` (mature, >50K GitHub stars) | Planned (TypeScript SDK in design) |
| Python | `supabase-py` (community, well-maintained) | `sdks/python` (present in repo) |
| PHP | Community-maintained | `sdks/php` (present in repo) |
| Dart/Flutter | Official | Not yet |
| Swift | Official | Not yet |
| Kotlin | Official | Not yet |
| Rust | Community | Not needed -- direct Postgres access |

### CLI

| Feature | Supabase CLI | DarshJDB CLI |
|---------|-------------|---------------|
| Project init | `supabase init` | `ddb init` |
| Local dev | `supabase start` (full Docker Compose stack) | `ddb dev` (single binary + Postgres) |
| Migrations | `supabase db diff`, `supabase migration` | Schema inference + `MigrationGenerator` diffs |
| Function deploy | `supabase functions deploy` | File watcher (automatic) |
| Remote management | `supabase link`, `supabase db push` | Manual (planned) |

### Admin Dashboard

| Feature | Supabase Studio | DarshJDB |
|---------|----------------|-----------|
| Table editor | Yes (spreadsheet-like) | No -- docs describe an admin dashboard but it is not yet shipped |
| SQL editor | Yes (with autocomplete) | No |
| Auth user management | Yes | API-only |
| Log viewer | Yes | Structured logs (tracing) to stdout -- use Grafana/Loki |
| Storage browser | Yes | No |

**Verdict**: Supabase has a vastly more mature developer experience. The JavaScript SDK alone has more usage than DarshJDB's entire user base. Supabase Studio is a genuine competitive advantage for onboarding non-backend developers. DarshJDB's developer experience is early-stage -- functional but missing polish, dashboard, and client SDK breadth. This is the single largest gap.

---

## 9. Self-Hosting

### Supabase Self-Hosted

```yaml
# docker-compose.yml includes:
# postgres, postgrest, gotrue, realtime, storage-api,
# meta (dashboard API), studio, kong, vector (log aggregation),
# imgproxy, analytics, edge-runtime...
```

- **Container count**: 10-15 depending on feature flags
- **Minimum RAM**: 2-4 GB (more for production)
- **Configuration**: `.env` file with 30+ variables across services
- **Upgrades**: Pull new images, hope the version matrix is compatible
- **Networking**: Internal Docker network with Kong routing
- **SSL**: Separate reverse proxy (Nginx, Caddy, Traefik)
- **Backup**: Standard Postgres backup; each service may have its own state

### DarshJDB Self-Hosted

```bash
# Install
curl -L https://github.com/darshjme/darshjdb/releases/latest/download/darshjdb-$(uname -s)-$(uname -m) -o darshjdb
chmod +x darshjdb

# Run
DATABASE_URL=postgres://user:pass@localhost:5432/darshjdb ./darshjdb
```

- **Container count**: 1 (the binary) + 1 (Postgres, probably already running)
- **Minimum RAM**: 30-80 MB for the DarshJDB process
- **Configuration**: Environment variables (12-factor). Key ones: `DATABASE_URL`, `DDB_JWT_SECRET` or `DDB_JWT_PRIVATE_KEY`/`DDB_JWT_PUBLIC_KEY`, `DDB_PORT`
- **Upgrades**: Download new binary, restart
- **Networking**: Single port (default 7700) -- REST, WebSocket, and health on the same listener
- **SSL**: Same reverse proxy (or built-in TLS in a future release)
- **Backup**: Standard Postgres backup. All state lives in Postgres. The binary is stateless.
- **Docker option**: Also available as a 2-container Docker Compose (Postgres + DarshJDB)

### Comparison

| Dimension | Supabase Self-Hosted | DarshJDB Self-Hosted |
|-----------|---------------------|----------------------|
| **Time to running** | 10-30 minutes (Docker Compose, init scripts) | 60 seconds (download binary, set DATABASE_URL) |
| **Ops burden** | High -- 10+ containers, version matrix, inter-service debugging | Low -- one binary, standard Postgres admin |
| **Resource efficiency** | 2-4 GB RAM minimum | 100 MB RAM is generous |
| **Kubernetes** | Requires StatefulSets, service mesh, secrets management for each service | Single Deployment + existing Postgres. Health/readiness probes at `/health` and `/health/ready` |
| **Air-gapped deployment** | Difficult -- many container images to mirror | Trivial -- one binary + Postgres |
| **Debugging** | Logs from 10+ services, each with different format | Single structured log stream (tracing with JSON output) |

**Verdict**: DarshJDB's self-hosting story is dramatically simpler. This is not a marginal difference -- it is an order of magnitude less operational complexity. For teams that do not want to run a Kubernetes cluster just for their backend-as-a-service, DarshJDB's single binary is a compelling proposition.

---

## 10. What Supabase Does Better

Being honest about where Supabase is ahead:

1. **Ecosystem maturity**: The JavaScript SDK has millions of downloads. Client libraries exist for every major platform. Community tutorials, templates, and integrations are abundant. DarshJDB is in its infancy here.

2. **Admin dashboard (Studio)**: A genuinely excellent web interface for managing tables, auth users, storage, and running SQL. DarshJDB has no dashboard.

3. **Managed hosting**: Supabase Cloud handles infrastructure, backups, monitoring, SSL, and scaling. DarshJDB is self-hosted only.

4. **SQL power**: If you need complex analytical queries, window functions, CTEs, or lateral joins, Supabase gives you raw PostgreSQL. DarshJDB's DarshanQL covers common cases but cannot match SQL's expressiveness.

5. **Auth provider breadth**: 20+ OAuth providers, phone OTP, SAML SSO. DarshJDB has 4 OAuth providers and no phone/SAML support.

6. **Database extensions**: Supabase exposes the full PostgreSQL extension ecosystem (PostGIS, pg_cron, pg_stat_statements, pgvector, etc.). DarshJDB uses Postgres as a backing store but abstracts over it -- most extensions are not directly accessible through the DarshanQL interface.

7. **Edge Functions isolation**: V8 isolates provide genuine sandboxing. DarshJDB's Node subprocesses rely on OS-level controls, which are weaker.

8. **Community and funding**: Supabase has raised $116M+ in venture capital, employs 100+ engineers, and has a large open-source community. DarshJDB is a solo/small-team project.

9. **PostgreSQL ecosystem**: If your team already knows SQL, Supabase has zero learning curve for the data layer. DarshJDB requires learning the triple store model and DarshanQL.

10. **Horizontal scaling**: Supabase can scale individual services (add more Realtime nodes, more PostgREST instances). DarshJDB scales vertically only.

---

## 11. What DarshJDB Does Better

Being honest about where DarshJDB is ahead:

1. **Self-hosting simplicity**: One binary, one Postgres dependency. No Docker Compose with 10+ containers. No version matrix. No inter-service debugging. This is not a marginal improvement -- it eliminates an entire category of operational pain.

2. **Resource efficiency**: 30-80 MB RAM vs 2-4 GB. For edge deployments, VPS hosting, or cost-sensitive environments, this is a 25-50x difference.

3. **Real-time latency**: In-process change broadcast with diff computation delivers 1-10ms mutation-to-notification latency, vs 50-200ms for Supabase's WAL-based approach. The diff-based model also reduces bandwidth.

4. **Schema flexibility**: Add any attribute to any entity at any time. No migrations for schema evolution. The `MigrationGenerator` can diff schemas when you need structural awareness, but it is not required for day-to-day development.

5. **Time-travel queries**: Every triple has a transaction ID. Point-in-time queries are a first-class feature, not an afterthought requiring custom audit triggers.

6. **Built-in vector search**: pgvector integration with automatic embedding pipeline. Configure `DDB_EMBEDDING_PROVIDER=openai` and attributes auto-embed on write. Supabase supports pgvector but requires manual embedding and query construction.

7. **Server function integration**: Functions have direct `ctx.db` access to the triple store, built-in cron scheduling with distributed locking, and declarative argument validation. Supabase Edge Functions access data via HTTP client SDK.

8. **Field-level permissions**: The permission engine supports `field_restriction` rules that control which fields are visible or writable per role. Supabase RLS operates at the row level; column-level control requires views or column grants.

9. **TTL/expiry**: Built-in `expires_at` support with automatic background reaping. Supabase requires manual implementation (pg_cron + DELETE query).

10. **Cryptographic defaults**: RS256 JWT by default (not HS256), Argon2id with OWASP parameters, device-fingerprint binding on refresh tokens, documented PQC migration path. These are production-grade defaults, not afterthoughts.

11. **Unified protocol**: REST, WebSocket, SSE, health probes, and API docs all on a single port. No API gateway routing complexity.

12. **Connector system**: Built-in webhook and log connectors with a pluggable `Connector` trait. React to data changes without writing functions.

---

## 12. Migration Path: Supabase to DarshJDB

### Phase 1: Data Migration

```bash
# Export Supabase tables as JSON
for table in users posts comments; do
  curl "https://your-project.supabase.co/rest/v1/$table?select=*" \
    -H "apikey: YOUR_SERVICE_KEY" \
    -H "Authorization: Bearer YOUR_SERVICE_KEY" \
    > "${table}.json"
done
```

Convert relational rows to triple format:

```python
import json, requests

for table in ["users", "posts", "comments"]:
    rows = json.load(open(f"{table}.json"))
    for row in rows:
        entity_id = row.pop("id")
        attributes = {}
        for key, value in row.items():
            if value is not None:
                attributes[key] = value
        requests.post("http://localhost:7700/api/data", json={
            "entity_id": entity_id,
            "entity_type": table.rstrip("s").capitalize(),
            "attributes": attributes,
        }, headers={"Authorization": "Bearer YOUR_ADMIN_TOKEN"})
```

### Phase 2: Auth Migration

- Export Supabase `auth.users` table (password hashes are Bcrypt -- DarshJDB uses Argon2id)
- Option A: Re-hash on first login (store Bcrypt hash temporarily, verify with Bcrypt, then re-hash with Argon2id)
- Option B: Force password reset for all users
- OAuth tokens: Re-authenticate through DarshJDB's OAuth flow (tokens are not portable)

### Phase 3: Permission Migration

Convert RLS policies to DarshJDB permission rules:

```sql
-- Supabase RLS
CREATE POLICY "own_data" ON posts
  FOR SELECT USING (auth.uid() = user_id);
```

Becomes:

```json
{
  "Post": {
    "read": {
      "type": "where_clause",
      "sql": "owner_id = $user_id"
    }
  }
}
```

Note: Complex RLS policies using SQL functions or subqueries may not translate directly to DarshJDB's permission DSL. These require custom handler logic.

### Phase 4: Client SDK Migration

Replace Supabase client calls:

```typescript
// Before (Supabase)
const { data } = await supabase
  .from('posts')
  .select('*')
  .eq('user_id', userId);

// After (DarshJDB)
const { data } = await ddb.query({
  from: 'Post',
  where: [{ attribute: 'user_id', op: '=', value: userId }],
});
```

### Phase 5: Real-Time Migration

```typescript
// Before (Supabase)
supabase.channel('posts')
  .on('postgres_changes', { event: 'INSERT', schema: 'public', table: 'posts' },
    (payload) => console.log(payload.new))
  .subscribe();

// After (DarshJDB WebSocket)
ws.send(JSON.stringify({
  type: 'sub',
  id: 'posts-sub',
  query: { from: 'Post', order: [{ attribute: 'created_at', direction: 'desc' }], limit: 50 }
}));
// Receives diff messages with only changed fields
```

### Migration Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| **Data model mismatch** | High | Test query performance with realistic data volumes before committing. Multi-attribute queries on the triple store are slower than indexed relational scans. |
| **Missing SQL features** | Medium | Identify all queries using CTEs, window functions, or complex joins. These need rewriting for DarshanQL or direct Postgres access. |
| **Auth gap** | Medium | If using phone OTP or SAML, these are not available in DarshJDB. Find alternative providers or defer migration. |
| **SDK maturity** | High | DarshJDB's client SDKs are early-stage. Budget time for SDK issues and workarounds. |
| **No dashboard** | Low | Teams accustomed to Supabase Studio will need alternative tools (pgAdmin, DBeaver, or custom admin UI). |

---

## Summary

| Dimension | Choose Supabase When | Choose DarshJDB When |
|-----------|---------------------|----------------------|
| **Scale** | You need managed hosting and horizontal scaling | You need lightweight self-hosting and vertical efficiency |
| **Data model** | Your data is naturally relational with complex queries | Your schema evolves rapidly and you value flexibility over query power |
| **Team** | Frontend developers who want SQL familiarity | Backend teams comfortable with novel data models |
| **Auth** | You need SAML, phone OTP, or 20+ OAuth providers | You need tight auth-data integration with strong crypto defaults |
| **Real-time** | You need proven scale to millions of connections | You need sub-10ms mutation-to-notification latency |
| **Operations** | You have DevOps capacity for 10+ container orchestration (or use managed) | You want a single binary that runs on a $5/month VPS |
| **Ecosystem** | You need mature SDKs, dashboard, and community resources | You are willing to trade ecosystem maturity for architectural simplicity |

DarshJDB is not a drop-in Supabase replacement. It is a different architectural bet: that a single, efficient binary with a flexible data model can serve the 80% of use cases that do not need the full power of a managed Postgres platform. For the 20% that do, Supabase remains the better choice.
