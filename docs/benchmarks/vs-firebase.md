# DarshJDB vs Firebase: An Honest Architectural Comparison

> **Disclosure**: This document was written by the DarshJDB team. We have made every effort to be factually accurate about both systems. Where Firebase excels, we say so plainly. Where DarshJDB is immature, we say that too. Claims about DarshJDB are verified against source code in the repository.

---

## Executive Summary

Firebase is a mature, battle-tested Backend-as-a-Service operated by Google. It powers hundreds of thousands of production applications and offers a breadth of integrated services (auth, database, storage, hosting, analytics, crash reporting, remote config) that no single open-source project can match today.

DarshJDB is an alpha-stage, self-hosted BaaS built in Rust over PostgreSQL. Its triple-store data model handles relational data natively, it runs as a single binary you own, and it will never send Google a bill. But it is new, unproven at scale, and missing ecosystem surface area that Firebase has spent a decade building.

The right choice depends on what you value: Firebase if you need production-grade infrastructure today with minimal ops. DarshJDB if you need data sovereignty, relational queries, vendor independence, or a $5/month deployment in a region Firebase does not serve.

---

## 1. Data Model

### Firebase

Firebase offers two databases, neither of which is relational:

- **Firestore**: Document-collection model. Documents are JSON-like maps organized into collections. You can nest subcollections but cannot join across them. Denormalization is the standard pattern -- you duplicate data across documents to avoid multiple reads.
- **Realtime Database**: A single JSON tree. All data lives under one root node. No collections, no schema, no joins. Reads at a node return all descendants, which makes deep nesting expensive.

Neither supports foreign keys, referential integrity, or cross-entity joins at the database level. The standard Firebase advice is "model your data for your queries" -- which means duplicating data and accepting the consistency burden.

### DarshJDB

DarshJDB uses a **triple store** (Entity-Attribute-Value) backed by PostgreSQL. Every fact is stored as an `(entity_id, attribute, value, value_type, tx_id)` tuple in an append-only `triples` table.

```
entity_id: uuid    | attribute: "user/email" | value: "alice@example.com" | tx_id: 42
entity_id: uuid    | attribute: "user/name"  | value: "Alice"             | tx_id: 42
entity_id: uuid    | attribute: "post/author"| value: <user-uuid>         | tx_id: 43
```

This architecture means:

- **Flexible schema**: Entities can have arbitrary attributes. No migrations needed during development. The `Schema` module (`triple_store/schema.rs`) supports strict mode for production lockdown.
- **Native references**: An attribute value can point to another entity (value_type discriminator for references). The query engine resolves these as joins across the triples table.
- **Temporal queries**: Every triple carries a `tx_id` and `created_at`. Retraction is a logical flag, not a physical delete. You can reconstruct entity state at any transaction.
- **TTL support**: Triples support an optional `expires_at` timestamp for automatic retraction.

The trade-off: EAV over a relational database is not the most storage-efficient model. A user with 10 attributes is 10 rows instead of 1. PostgreSQL handles this well with proper indexing, but at extreme scale (billions of triples) you pay more in storage and join cost than a columnar or document store would.

### Verdict

Firebase's document model is simpler to learn and faster for flat, read-heavy workloads. DarshJDB's triple store is fundamentally more flexible for relational data, temporal queries, and schema-fluid development. If your data has relationships (which most data does), DarshJDB's model eliminates the denormalization tax that Firebase imposes.

---

## 2. Real-Time Sync

### Firebase

Firebase pioneered real-time database sync. It remains the gold standard:

- **Realtime Database**: Sub-100ms updates via persistent WebSocket connections. Clients attach listeners to paths; changes propagate immediately to all listeners.
- **Firestore**: `onSnapshot` listeners on documents and queries. Uses a hybrid gRPC streaming protocol. Latency is slightly higher than Realtime Database but supports richer queries.
- **Conflict resolution**: Last-write-wins by default. Firestore supports transactions for atomic multi-document updates.

This is production-proven at scale: Firebase handles billions of concurrent connections across Google's infrastructure.

### DarshJDB

DarshJDB implements real-time via WebSocket subscriptions with a purpose-built sync architecture (source: `packages/server/src/sync/`):

- **Broadcaster**: Listens for triple-store mutations, identifies affected queries by hash, re-executes with the subscriber's permission context, and pushes diffs.
- **DiffEngine** (`sync/diff.rs`): Computes minimal delta patches (`EntityPatch`, `QueryDiff`) between result snapshots rather than sending full results.
- **SubscriptionRegistry** (`sync/registry.rs`): Global query-hash-to-session-set mapping for fan-out deduplication. A mutation touching 1000 subscribers does not re-execute the query 1000 times if they share the same query shape and permission context.
- **Presence** (`sync/presence.rs`): Ephemeral per-room user state with auto-expiry and rate limiting.
- **PubSub** (`sync/pubsub.rs`): Additional pub/sub engine for custom event channels beyond query subscriptions.

What DarshJDB does not yet have:
- Proven behavior under thousands of concurrent connections (no published benchmarks).
- Graceful degradation under network partitions at scale.
- Multi-region fan-out (single-server architecture today).

### Verdict

Firebase's real-time is the industry benchmark. DarshJDB's architecture is sound -- diff-based broadcasting with deduplication is the right design -- but it is unproven at Firebase-level scale. For small-to-medium deployments (hundreds of concurrent connections), DarshJDB's approach should perform well. For tens of thousands of simultaneous connections with global distribution, Firebase is the safer choice today.

---

## 3. Offline-First

### Firebase

Firebase has excellent offline support across all platforms:

- **Firestore**: Transparent offline persistence. Reads hit local cache when offline. Writes queue locally and sync when connectivity returns. Conflict resolution is automatic (last-write-wins or transactions).
- **Realtime Database**: Disk persistence with `setPersistenceEnabled`. Queued writes replay in order.
- **Mobile SDKs**: iOS (Core Data-backed), Android (SQLite-backed), Web (IndexedDB-backed). All three are battle-tested in production.

This is one of Firebase's strongest features. Developers can build offline-capable apps without thinking about sync logic.

### DarshJDB

DarshJDB's client SDK includes a `SyncEngine` class (`packages/client-core/src/sync.ts`) with:

- **IndexedDB cache**: Query results cached locally by query hash. Three object stores: `queryCache`, `offlineQueue`, `meta`.
- **Optimistic updates**: `applyOptimistic()` / `confirmOptimistic()` / `rollbackOptimistic()` API for instant UI feedback before server confirmation.
- **Offline queue**: Mutations enqueued via `enqueue()` when offline. `replayQueue()` sends them in order when connectivity returns. Failed entries retry up to 5 times before being discarded.
- **Transaction cursor tracking**: `setLastTxId()` / `getLastTxId()` for reconnection catch-up.

What DarshJDB does not yet have:
- Mobile SDKs (no iOS or Android). The sync engine is JavaScript/TypeScript only.
- Automatic conflict resolution beyond last-write-wins.
- Transparent offline mode (the developer must call `enqueue` and `replayQueue` explicitly rather than having writes automatically queue when offline).
- Production testing of the offline path (the README states offline is "untested").

### Verdict

Firebase wins decisively here. Its offline support is automatic, cross-platform, and battle-tested. DarshJDB has the right building blocks (IndexedDB cache, offline queue, optimistic updates) but requires manual orchestration by the developer and lacks mobile SDK support entirely.

---

## 4. Authentication

### Firebase Auth

Firebase Auth is comprehensive and mature:

| Feature | Firebase |
|---------|----------|
| Email/password | Yes, with email verification |
| OAuth providers | Google, Apple, Facebook, Twitter/X, GitHub, Microsoft, Yahoo, plus generic OIDC/SAML |
| Phone (SMS OTP) | Yes, global coverage |
| Anonymous auth | Yes |
| Custom tokens | Yes (mint your own JWTs) |
| Multi-factor | SMS-based (GA), TOTP (beta) |
| Passkeys/WebAuthn | Yes (recent addition) |
| Email link (magic link) | Yes |
| Session management | Managed by Firebase, revocation supported |
| Rate limiting | Built-in |
| Admin SDK | Node.js, Python, Go, Java, C# |

Firebase Auth also integrates with Identity Platform for enterprise SAML/OIDC, tenant isolation, and blocking functions.

### DarshJDB Auth

DarshJDB implements authentication in Rust (source: `packages/server/src/auth/`):

| Feature | DarshJDB | Source verification |
|---------|-----------|---------------------|
| Email/password | Yes, Argon2id (64MB, 3 iterations, parallelism 4) | `providers.rs` -- `PasswordProvider` |
| OAuth providers | Google, GitHub, Apple, Discord | `providers.rs` -- `GenericOAuth2Provider` with per-provider configs |
| Magic link | Yes, 32-byte token, SHA-256 hashed storage, 15-min expiry | `providers.rs` -- `MagicLinkProvider` |
| TOTP (2FA) | Yes, RFC 6238, HMAC-SHA1, 30s period, +/-1 step window | `mfa.rs` -- `TotpManager` |
| Recovery codes | Yes, 10 codes, Argon2id-hashed | `mfa.rs` -- `RecoveryCodeManager` |
| WebAuthn/Passkeys | Stub only (registered in module, not functional) | `mfa.rs` -- `WebAuthnStub` |
| PKCE | Mandatory on all OAuth flows | `providers.rs` -- `pkce_pair()` |
| CSRF protection | HMAC-signed state parameter | `providers.rs` -- `sign_state()` / `verify_state()` |
| Rate limiting | Yes | `middleware.rs` -- `RateLimiter` |
| Session management | JWT with refresh rotation, device fingerprinting | `session.rs` -- `SessionManager`, `KeyManager` |
| Permission engine | Row-level, composable rules with WHERE injection | `permissions.rs` -- `PermissionEngine` |
| Timing-safe auth | Yes, dummy verify on unknown emails | `providers.rs` line 99-101 |

What DarshJDB auth does **not** have:
- Phone/SMS authentication
- Anonymous auth
- Facebook, Twitter/X, Microsoft OAuth (only Google, GitHub, Apple, Discord)
- SAML/OIDC enterprise federation
- Managed email verification flow (you must implement the email transport)
- Admin SDKs beyond the REST API

### Verdict

Firebase Auth covers more ground, especially for mobile (phone auth, anonymous auth) and enterprise (SAML, multi-tenant). DarshJDB's auth implementation is cryptographically solid -- Argon2id with OWASP parameters, mandatory PKCE, HMAC-signed state, timing-safe comparisons -- but covers fewer authentication methods. For web applications using email + OAuth + MFA, DarshJDB is sufficient. For mobile-first apps needing phone auth or enterprise SSO, Firebase Auth is necessary.

---

## 5. Pricing

### Firebase

Firebase uses a pay-as-you-go model that starts free but scales aggressively:

| Service | Free tier | After free tier |
|---------|-----------|-----------------|
| Firestore reads | 50K/day | $0.06 per 100K |
| Firestore writes | 20K/day | $0.18 per 100K |
| Firestore deletes | 20K/day | $0.02 per 100K |
| Firestore storage | 1 GiB | $0.18/GiB/month |
| Auth (phone) | 10K verifications/month | $0.01-0.06 per verification |
| Cloud Functions invocations | 2M/month | $0.40 per million |
| Cloud Storage | 5 GB | $0.026/GB/month |
| Hosting | 10 GB storage, 360 MB/day transfer | $0.026/GB stored, $0.15/GB transferred |

The cost curve is not linear. Real-world examples from the Firebase community:

- A social app doing 10M Firestore reads/day: ~$180/day = **$5,400/month**
- A chat app with heavy real-time writes: write costs dominate at scale
- Cloud Functions with cold starts add latency cost alongside dollar cost

Firebase's pricing penalizes read-heavy patterns, which is ironic given that its document model encourages denormalization (which means more reads).

### DarshJDB

DarshJDB is MIT-licensed, open-source, self-hosted:

| Component | Cost |
|-----------|------|
| Software | $0 (MIT license) |
| Hosting | Your server cost |
| Database | PostgreSQL (included in most hosting) |

Realistic hosting costs:

| Scale | Infrastructure | Monthly cost |
|-------|---------------|--------------|
| Prototype | $5 VPS (Hetzner, DigitalOcean) | $5 |
| Small production | $20-40 VPS + managed Postgres | $30-60 |
| Medium production | Dedicated server + Postgres | $50-200 |
| Large production | Multiple servers + Postgres cluster | $200-1000+ |

At every scale tier, DarshJDB costs a fraction of equivalent Firebase usage. The trade-off is operational overhead: you manage the server, backups, monitoring, and upgrades yourself.

### Verdict

Firebase is cheaper to start (free tier is generous for prototypes). DarshJDB is dramatically cheaper at scale. The crossover point typically happens at 100K-500K daily active operations, where Firebase bills start climbing while DarshJDB's cost remains the fixed price of your server.

---

## 6. Vendor Lock-In

### Firebase

Firebase lock-in operates at multiple levels:

- **Data format**: Firestore documents are not portable. Export is possible (via `gcloud firestore export`) but produces a proprietary format that requires transformation for any other database.
- **Auth**: Firebase Auth tokens are Firebase-specific. User password hashes use scrypt with Firebase-specific parameters -- Google will export the hash parameters, but migration requires re-hashing or forcing password resets.
- **SDK coupling**: Firebase SDKs are tightly coupled to Firebase infrastructure. `firebase.firestore()` calls go to Google's servers. There is no local or alternative backend.
- **Cloud Functions**: Deployed to Google Cloud Functions. Your function code is portable JavaScript/TypeScript, but the deployment mechanism, environment variables, and triggers are Firebase-specific.
- **Hosting**: Standard static/SSR hosting, but the CI/CD pipeline and preview channels are Firebase-specific.
- **Analytics + Crashlytics**: Proprietary. No export path for historical analytics data.

If Google deprecates a Firebase product (as they did with Fabric, Firebase Predictions, Firebase Invites, and Firebase Job Dispatcher), you migrate on their timeline.

### DarshJDB

- **Data format**: Standard PostgreSQL. Your data is in a `triples` table you can query with `psql`. Export with `pg_dump`. Transform with SQL.
- **Auth**: Argon2id password hashes in standard PHC format. OAuth tokens are standard JWT. Any system that reads JWTs can validate DarshJDB tokens.
- **SDKs**: Open-source TypeScript, Python, PHP clients. If the project disappeared tomorrow, you still have PostgreSQL with your data in a documented schema.
- **Deployment**: Single Rust binary + PostgreSQL. Runs on any Linux/macOS server, any cloud provider, any VPS, bare metal, Raspberry Pi.
- **No telemetry**: DarshJDB sends nothing to any external service. Zero phone-home behavior.

### Verdict

This is not close. Firebase is deep lock-in to Google's ecosystem. DarshJDB is PostgreSQL underneath -- the most portable database on earth. If DarshJDB ceased to exist, your data is still in Postgres. If Firebase ceased to exist, you have a migration project.

---

## 7. Relational Queries

### Firebase

This is Firebase's most criticized limitation:

- **No joins**: Firestore cannot join documents across collections. If you need a user's posts with their comments with the commenter's profile, that is three separate queries assembled client-side.
- **No aggregations**: `COUNT(*)`, `SUM()`, `AVG()` were only added to Firestore recently (2023) and are limited. No `GROUP BY`.
- **Limited `where` clauses**: Firestore requires a composite index for every unique combination of filters and sort orders. Adding a new filter to a query often means creating a new index and waiting for it to build.
- **No subqueries**: No `WHERE user_id IN (SELECT ...)` patterns.
- **Denormalization overhead**: The standard Firebase pattern is to duplicate data across documents. This trades query simplicity for write complexity and consistency risk.

The Realtime Database is worse: no query engine at all beyond `.orderByChild()` and `.equalTo()` on a single field.

### DarshJDB

DarshJDB's query engine (`packages/server/src/query/`) compiles DarshanQL into SQL that joins across the triples table:

- **Cross-entity resolution**: Reference-typed attributes are followed as joins. "Get all posts by users in role X" is a single query.
- **WHERE clauses**: Standard comparison operators, combined with `And`/`Or` compositors. Permission rules inject additional WHERE clauses for row-level security.
- **Full-text search**: Integrated tsvector search via PostgreSQL.
- **Semantic search**: pgvector cosine similarity for vector queries.
- **Hybrid search**: Combines tsvector + pgvector via Reciprocal Rank Fusion (RRF).
- **Parallel execution**: The `query/parallel.rs` module supports parallel query execution.
- **Reactive queries**: The `query/reactive.rs` module ties queries to the real-time subscription system.
- **Plan caching**: LRU cache for compiled query plans to avoid re-parsing identical queries.

Because the underlying store is PostgreSQL, you can also bypass DarshanQL and run raw SQL against the triples table for complex analytical queries.

### Verdict

DarshJDB wins comprehensively on relational queries. Firebase's document model makes joins structurally impossible at the database level. DarshJDB's triple store treats references as first-class citizens, and the PostgreSQL foundation means you always have SQL as an escape hatch.

---

## 8. Storage

### Firebase

- **Cloud Storage**: Google Cloud Storage backend. Unlimited scale. Signed URLs. Security Rules for access control. Client SDKs for direct upload from mobile/web.
- **Image resizing**: Via Extensions (Firebase Resize Images extension runs a Cloud Function on upload).
- **CDN**: Integrated with Google's CDN for static hosting.

### DarshJDB

DarshJDB has a storage module (`packages/server/src/storage/mod.rs`) with:

- **Pluggable backends**: Local filesystem, S3, Cloudflare R2, MinIO.
- **Signed URLs**: HMAC-authenticated, time-limited download links.
- **Image transforms**: On-the-fly resize, crop, format conversion (delegated to external processor).
- **Upload hooks**: Pre/post-upload callbacks for validation.
- **Resumable uploads**: TUS protocol support.

However, the storage module is at an earlier stage of maturity than the auth or query modules. Firebase Cloud Storage is battle-tested at exabyte scale.

### Verdict

Firebase Storage is more mature and globally distributed. DarshJDB's storage has a solid interface design with pluggable backends (S3, R2, MinIO give you flexibility Firebase does not), but it needs production hardening.

---

## 9. Server Functions

### Firebase

Cloud Functions for Firebase:
- Node.js, Python runtime.
- Triggers: Firestore, Auth, Storage, Pub/Sub, HTTP, Scheduled.
- Auto-scaling, zero server management.
- Cold starts are a known pain point (can add seconds of latency).

### DarshJDB

DarshJDB has a functions module (`packages/server/src/functions/`) with:

- **Registry**: Discovers `.ts`/`.js` function files with hot reload.
- **Function kinds**: Queries, mutations, actions, scheduled jobs, HTTP endpoints.
- **Validator**: Schema-based argument validation.
- **Scheduler**: Cron-scheduled functions with distributed locking and retry.
- **Runtime**: Subprocess-based isolate execution. The V8 runtime is a placeholder (noted in the README as in-progress).

The architecture is designed but the runtime is not production-ready. Firebase Cloud Functions work today.

### Verdict

Firebase wins on server functions maturity. DarshJDB's function architecture is well-designed (the registry, validator, and scheduler are implemented) but the actual execution runtime is incomplete.

---

## 10. Ecosystem and SDKs

### Firebase

- **Client SDKs**: iOS (Swift), Android (Kotlin/Java), Web (JavaScript), Flutter, Unity, C++
- **Admin SDKs**: Node.js, Python, Go, Java, C#
- **Extensions**: 70+ official extensions (Stripe, Algolia, Mailchimp, etc.)
- **Emulator Suite**: Local development with full Firestore, Auth, Functions emulation
- **Console**: Web-based dashboard for all services
- **Documentation**: Extensive, with guides for every framework
- **Community**: Millions of developers, thousands of tutorials, StackOverflow answers for every edge case

### DarshJDB

- **Client SDKs**: React hooks, Angular signals, Next.js (App + Pages Router), core TypeScript client, Python (sync + async, FastAPI + Django), PHP (Composer, Laravel)
- **Admin**: React + Vite + Tailwind dashboard
- **CLI**: Rust-based CLI (`packages/cli`)
- **Tests**: 731 (446 Rust server, 92 TypeScript, 141 Python, 52 PHP)
- **Documentation**: In-repo docs, no hosted documentation site yet
- **Community**: Early stage, open-source contributors welcome

### Verdict

Firebase's ecosystem is orders of magnitude larger. This is the natural consequence of a decade of Google investment and millions of developers. DarshJDB has respectable SDK coverage for an alpha project (TypeScript, Python, PHP across multiple frameworks), but it cannot compete with Firebase's breadth today.

---

## 11. What Firebase Does Better

1. **Maturity**: 12+ years of production use. Billions of operations per day across its fleet. DarshJDB is alpha software with 731 tests but zero production deployments at scale.

2. **Mobile SDKs**: Native iOS and Android SDKs with offline persistence, push notifications integration, analytics, and crash reporting. DarshJDB has no mobile SDKs.

3. **Global infrastructure**: Firebase runs on Google Cloud's global network with automatic multi-region replication. DarshJDB runs on your single server.

4. **Analytics and observability**: Firebase Analytics, Crashlytics, Performance Monitoring, and Remote Config are integrated services with no self-hosted equivalent in DarshJDB.

5. **Zero ops**: No servers to manage, no PostgreSQL to tune, no backups to configure, no SSL certificates to rotate. Firebase handles all of this.

6. **Phone authentication**: SMS OTP with global carrier coverage is a service that requires carrier agreements and infrastructure DarshJDB cannot replicate.

7. **Emulator Suite**: Firebase's local emulator lets you develop against Firestore, Auth, and Functions without a network connection. DarshJDB requires a running PostgreSQL instance.

8. **Extensions marketplace**: Pre-built integrations with Stripe, SendGrid, Algolia, and dozens of other services.

---

## 12. What DarshJDB Does Better

1. **Self-hosted data sovereignty**: Your data lives on your server, in your jurisdiction, under your control. No third party can access, mine, or monetize it. For GDPR, HIPAA, or data residency requirements, this is not a feature -- it is a requirement.

2. **Relational data model**: The triple store handles relationships natively. No denormalization, no data duplication, no client-side joins. For any application with connected data (which is most applications), this eliminates an entire class of complexity that Firebase forces onto the developer.

3. **Cost at scale**: A $40/month Hetzner server running DarshJDB can handle workloads that would cost $5,000+/month on Firebase. The cost curve is flat, not exponential.

4. **No vendor lock-in**: PostgreSQL underneath. Standard JWT tokens. MIT-licensed code. You can fork it, extend it, or migrate away from it without asking anyone's permission.

5. **Temporal data**: Every triple carries a transaction ID and timestamp. You can reconstruct entity state at any point in time. Firebase has no built-in time-travel queries.

6. **Permission model**: Row-level security with WHERE clause injection means permissions are enforced at the database query level, not in application code. Firebase Security Rules are powerful but exist in a separate layer from the data model.

7. **Full-text and vector search**: Built-in tsvector + pgvector support. Firebase requires a third-party service (Algolia, Typesense) for full-text search and has no vector search.

8. **Single binary deployment**: `./darshjdb` and a PostgreSQL connection string. No SDK initialization, no project configuration, no Google account required.

9. **Open source**: Read the source, audit the security, fix a bug, add a feature. Firebase is a black box.

10. **Diff-based real-time**: DarshJDB computes and sends minimal diffs rather than full document snapshots. For large documents with small changes, this is significantly more efficient on bandwidth.

---

## 13. Migration Path: Firebase to DarshJDB

### When to consider migrating

- Your Firebase bill is growing faster than your revenue
- You need real joins and are tired of denormalization
- Data residency requirements make Google Cloud incompatible
- You want to self-host in a region Firebase does not serve
- You need temporal queries or time-travel debugging

### Migration steps

**1. Data export**

```bash
# Export Firestore to GCS
gcloud firestore export gs://your-bucket/firestore-export

# Download and transform
# Firestore export is in protobuf format -- you'll need a transformer
# to convert documents to DarshJDB triple format
```

**2. Schema mapping**

Firestore collections map to DarshJDB entity types. Document fields map to attributes. Subcollections become reference-typed attributes pointing to new entities.

```
Firestore:                          DarshJDB triples:
users/alice {                       (alice-uuid, "user/name", "Alice")
  name: "Alice",                    (alice-uuid, "user/email", "alice@...")
  email: "alice@..."                (post-uuid, "post/title", "Hello")
}                                   (post-uuid, "post/author", alice-uuid)  // reference
users/alice/posts/post1 {
  title: "Hello"
}
```

**3. Auth migration**

Firebase uses scrypt for password hashing. You have two options:
- Force password resets for all users (cleanest, most secure)
- Implement a scrypt verification shim that re-hashes to Argon2id on first successful login

OAuth users can re-authenticate since the tokens are provider-issued, not Firebase-specific.

**4. Real-time subscriptions**

Replace `onSnapshot` listeners with DarshJDB WebSocket subscriptions. The subscription model is similar (query-based, push updates) but the client API differs.

**5. Cloud Functions**

Firebase Cloud Functions map to DarshJDB server functions. The function types align:
- `onCall` -> `action`
- `onRequest` -> `httpEndpoint`
- `onDocumentWritten` -> `mutation` trigger
- `pubsub.schedule` -> `scheduled` with cron

Note: DarshJDB's function runtime is not production-ready. Plan to run functions externally (FastAPI, Express, etc.) until the V8 runtime matures.

**6. Storage**

Firebase Cloud Storage -> DarshJDB storage with S3/R2/MinIO backend. Security rules need manual translation to DarshJDB permission rules.

### What you lose

- Phone authentication (implement via Twilio/Vonage)
- Analytics (implement via Plausible/PostHog/self-hosted Matomo)
- Crash reporting (implement via Sentry)
- Remote Config (implement via feature flags in the triple store)
- Push notifications (implement via Firebase Cloud Messaging separately, or use ntfy/Pushover)
- Global CDN (use Cloudflare in front of DarshJDB)

### What you gain

- Predictable, flat-rate infrastructure costs
- Full SQL access to your data
- Relational queries without denormalization
- Data sovereignty and regulatory compliance
- The ability to run `pg_dump` and have a complete backup of everything

---

## Summary Table

| Dimension | Firebase | DarshJDB | Winner |
|-----------|----------|-----------|--------|
| Data model | Document (NoSQL) | Triple store (EAV over Postgres) | DarshJDB (relational flexibility) |
| Real-time sync | Industry gold standard | WebSocket + diff broadcasting | Firebase (maturity) |
| Offline-first | Automatic, cross-platform | IndexedDB + manual queue | Firebase (significantly) |
| Auth methods | 10+ providers, phone, anonymous | Email, OAuth (4), magic link, TOTP | Firebase (breadth) |
| Auth security | Managed, opaque | Argon2id, PKCE, HMAC state, auditable | DarshJDB (transparency) |
| Pricing (prototype) | Free tier | $5/month VPS | Firebase |
| Pricing (scale) | $1K-10K+/month | $40-200/month | DarshJDB (dramatically) |
| Vendor lock-in | Deep (Google) | None (PostgreSQL + MIT) | DarshJDB |
| Relational queries | Not possible | Native triple references + SQL | DarshJDB |
| Full-text search | Requires Algolia/Typesense | Built-in tsvector | DarshJDB |
| Vector search | Not available | Built-in pgvector | DarshJDB |
| Server functions | Production-ready | Architecture designed, runtime WIP | Firebase |
| Mobile SDKs | iOS, Android, Flutter, Unity | None | Firebase |
| Ecosystem | Massive | Early stage | Firebase |
| Observability | Analytics, Crashlytics, Perf | Bring your own | Firebase |
| Data sovereignty | Google's servers | Your servers | DarshJDB |
| Time-travel queries | Not available | Native (tx_id history) | DarshJDB |

---

## The Honest Bottom Line

If you are building a mobile app that needs to ship next month, use Firebase. Its mobile SDKs, offline support, and managed infrastructure will save you months of work.

If you are building a web application with relational data, care about cost at scale, or need data sovereignty, DarshJDB is the architecture you want. But understand that you are adopting alpha software. You will encounter rough edges. You will need to self-host. You will be an early adopter, not a consumer of a finished product.

The two systems are not truly competitors today -- they serve different values and different stages of project maturity. Firebase sells convenience and speed-to-market. DarshJDB offers ownership and architectural correctness. The market needs both.

---

*Last updated: 2026-04-05. DarshJDB version: 0.1.0 (alpha). Firebase references based on publicly documented features and pricing as of early 2026.*
