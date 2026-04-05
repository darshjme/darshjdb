# DarshJDB — Complete Research Sources for NotebookLM

All sources used during the design, architecture, audit, and implementation of DarshJDB. Organized by category for feeding into NotebookLM.

---

## 1. Triple Store & Graph Database Architecture

### Ontotext GraphDB (primary architectural influence)
- [GraphDB Architecture and Components](https://graphdb.ontotext.com/documentation/11.3/architecture-components.html) — TRREE engine, Entity Pool (integer ID mapping), Sail API integration
- [GraphDB Data Storage](https://graphdb.ontotext.com/documentation/master/storage.html) — PSO/POS dual index strategy, Entity Pool internals, global shared cache
- [GraphDB Rules Optimizations](https://graphdb.ontotext.com/documentation/master/rules-optimisations.html) — Forward-chaining rule engine, TRREE entailment
- [GraphDB Data Loading Optimizations](https://graphdb.ontotext.com/documentation/10.7/data-loading-query-optimisations.html) — 500K statements/sec, batch loading, index tuning
- [GraphDB Connectors for Full-Text Search](https://graphdb.ontotext.com/documentation/master/general-full-text-search-with-connectors.html) — Automatic entity-level sync to Elasticsearch/Lucene/Solr
- [Elasticsearch GraphDB Connector](https://graphdb.ontotext.com/documentation/11.0/elasticsearch-graphdb-connector.html) — Connector architecture, automatic index management
- [RDF-star and SPARQL-star](https://graphdb.ontotext.com/documentation/11.3/rdf-sparql-star.html) — Embedded triples, triples-about-triples, provenance
- [GraphDB 6.0 Release](https://www.ontotext.com/company/news/ontotext-releases-rdf-triplestore-graphdb-6-0/) — Replication cluster, connector improvements

### Triple Store Theory
- [What is an RDF Triplestore (Ontotext)](https://www.ontotext.com/knowledgehub/fundamentals/what-is-rdf-triplestore/) — EAV model, URIs, semantic web foundation
- [Large Triple Stores (W3C Wiki)](https://www.w3.org/wiki/LargeTripleStores) — Scaling benchmarks, billions of triples, BSBM/LUBM benchmarks
- [Triple Store Architectures (DBKDA Tutorial)](https://www.iaria.org/conferences2018/filesDBKDA18/IztokSavnik_Tutorial_3store-arch.pdf) — Academic survey of storage layouts, index strategies, query optimization
- [Triplestore (Wikipedia)](https://en.wikipedia.org/wiki/Triplestore) — SPO/PSO index patterns, row vs column storage tradeoffs
- [Understanding RDF Data Representations (CEUR)](https://ceur-ws.org/Vol-3194/paper11.pdf) — Academic paper on triple store internal representations
- [Competing for the JOB with a Triplestore](https://yyhh.org/blog/2024/09/competing-for-the-job-with-a-triplestore/) — Triple store vs relational for complex queries

### Virtuoso (alternative triple store reference)
- [Virtuoso RDF Triple Store Whitepaper](https://vos.openlinksw.com/owiki/wiki/VOS/VOSRDFWP) — SQL-based SPARQL execution, query optimizer reuse

---

## 2. BaaS Competitors (what DarshJDB replaces)

### Firebase
- Real-time database: NoSQL document model, event-driven subscriptions
- Limitations: no relational queries, denormalized data, vendor lock-in, cost scaling
- What we took: real-time subscription model, optimistic updates, offline-first pattern

### Supabase
- Postgres foundation, REST API via PostgREST, real-time via Postgres LISTEN/NOTIFY
- Limitations: real-time bolted on, no native query language, row-level security as SQL policies
- What we took: Postgres as foundation, row-level security concept, auth integration

### InstantDB
- Triple-store architecture (EAV), declarative relational queries from client
- DarshanQL is directly inspired by InstantDB's query syntax
- Limitations: cloud-only, no self-hosting
- What we took: EAV data model, client-side declarative query language

### Convex
- Server functions with transactional guarantees, reactive queries, TypeScript-native
- Limitations: proprietary, cloud-only black box
- What we took: server function concept (queries/mutations/actions/cron), reactive query pattern

### PlanetScale
- Serverless MySQL, schema branching, zero-downtime migrations
- What we took: schema inference concept (DarshJDB's schema-on-read parallels auto-schema)

---

## 3. Rust Ecosystem & Dependencies

### Core Server
- [Axum](https://docs.rs/axum/latest/axum/) — Rust web framework (Tower-based, async, type-safe extractors)
- [Tokio](https://tokio.rs/) — Async runtime (multi-threaded, io_uring support planned)
- [SQLx](https://docs.rs/sqlx/latest/sqlx/) — Async Postgres driver (compile-time checked queries, connection pooling)
- [Tower](https://docs.rs/tower/latest/tower/) — Middleware framework (timeout, rate limiting, CORS)
- [tower-http](https://docs.rs/tower-http/latest/tower_http/) — HTTP-specific middleware (CORS, catch-panic, tracing)

### Authentication & Security
- [Argon2](https://docs.rs/argon2/latest/argon2/) — Password hashing (PHC winner, memory-hard, GPU-resistant)
- [jsonwebtoken](https://docs.rs/jsonwebtoken/latest/jsonwebtoken/) — JWT RS256/HS256 encoding/decoding
- [oauth2](https://docs.rs/oauth2/latest/oauth2/) — OAuth 2.0 client (PKCE, state management)
- [DashMap](https://docs.rs/dashmap/latest/dashmap/) — Lock-free concurrent HashMap (rate limiting, caching)

### Serialization
- [serde](https://serde.rs/) — Serialization framework
- [serde_json](https://docs.rs/serde_json/latest/serde_json/) — JSON serialization
- [rmp-serde](https://docs.rs/rmp-serde/latest/rmp_serde/) — MessagePack serialization (28% smaller than JSON)

### Other
- [thiserror](https://docs.rs/thiserror/latest/thiserror/) — Derive macro for error types
- [tracing](https://docs.rs/tracing/latest/tracing/) — Structured logging and distributed tracing
- [uuid](https://docs.rs/uuid/latest/uuid/) — UUID generation (v4 random, v7 time-sortable)
- [cron](https://docs.rs/cron/latest/cron/) — Cron expression parsing for scheduled functions
- [notify](https://docs.rs/notify/latest/notify/) — File system watcher for hot reload
- [hmac](https://docs.rs/hmac/latest/hmac/) + [sha2](https://docs.rs/sha2/latest/sha2/) — HMAC-SHA256 for OAuth state signing

---

## 4. PostgreSQL Internals

- **tsvector/tsquery** — Full-text search with GIN indexes (used for $search operator)
- **pgvector** — Vector similarity search extension (used for $semantic operator)
- **MVCC** — Multi-version concurrency control (enables time-travel queries)
- **Advisory locks** — Used for distributed cron scheduler locking
- **COPY protocol** — Bulk data loading (10-50x faster than INSERT)
- **Streaming replication** — Used for read replicas and disaster recovery
- **pg_auto_failover** — Automatic failover for high availability

---

## 5. Security Standards & References

### OWASP
- [OWASP API Security Top 10](https://owasp.org/API-Security/) — Every risk addressed in DarshJDB's security design
- BOLA, broken auth, property-level auth, resource consumption, function-level auth, SSRF, misconfiguration, inventory, unsafe consumption

### Password Hashing
- [Password Hashing Competition](https://password-hashing.net/) — Argon2 winner
- Argon2id parameters: memory=64MB, iterations=3, parallelism=4 (OWASP recommended)

### JWT
- [RFC 7519](https://tools.ietf.org/html/rfc7519) — JSON Web Token standard
- RS256 (RSA-SHA256) for asymmetric signing
- HS256 (HMAC-SHA256) for dev mode

### OAuth 2.0
- [RFC 6749](https://tools.ietf.org/html/rfc6749) — OAuth 2.0 Authorization Framework
- [RFC 7636](https://tools.ietf.org/html/rfc7636) — PKCE (Proof Key for Code Exchange)
- [EIP-4361](https://eips.ethereum.org/EIPS/eip-4361) — Sign-In with Ethereum (SIWE) — in Web3 strategy

### TOTP
- [RFC 6238](https://tools.ietf.org/html/rfc6238) — Time-Based One-Time Password Algorithm

---

## 6. Client SDK References

### React
- [useSyncExternalStore](https://react.dev/reference/react/useSyncExternalStore) — Concurrent-safe external store subscription (used in useQuery)
- [Suspense](https://react.dev/reference/react/Suspense) — Async data loading boundary (supported by useQuery)

### Angular
- [Angular Signals](https://angular.dev/guide/signals) — Fine-grained reactivity (Angular 17+)
- [RxJS Observables](https://rxjs.dev/) — Stream-based reactive programming
- [DestroyRef](https://angular.dev/api/core/DestroyRef) — Lifecycle-aware cleanup

### Next.js
- [Server Components](https://nextjs.org/docs/app/building-your-application/rendering/server-components) — Server-side rendering with streaming
- [Server Actions](https://nextjs.org/docs/app/building-your-application/data-fetching/server-actions-and-mutations) — Form submissions and mutations
- [App Router](https://nextjs.org/docs/app) — File-based routing with layouts

---

## 7. DarshJDB Internal Documentation

### Core Docs (in docs/)
- `getting-started.md` — 5-minute quickstart, all framework examples
- `architecture.md` — System architecture with 8 Mermaid diagrams
- `query-language.md` — Full DarshanQL reference (operators, nesting, mutations)
- `server-functions.md` — Queries, mutations, actions, cron, argument validation
- `authentication.md` — Email/password, magic links, OAuth, MFA, sessions
- `permissions.md` — Row-level security, field-level, roles, multi-tenant patterns
- `api-reference.md` — Complete REST API with request/response schemas
- `security.md` — 11-layer defense-in-depth, OWASP coverage, compliance
- `performance.md` — Tuning guide, capacity planning, benchmarking
- `self-hosting.md` — Docker, bare metal, K8s, nginx/Caddy, monitoring
- `presence.md` — Real-time presence rooms, cursors, typing indicators
- `storage.md` — File upload, signed URLs, image transforms, resumable
- `migration.md` — Schema migrations, version upgrades
- `troubleshooting.md` — Common errors and solutions
- `admin-dashboard.md` — Admin UI guide
- `migrating-from-convex.md` — Convex → DarshJDB migration guide

### Strategy Docs (in docs/strategy/)
- `AI_ML_STRATEGY.md` — Auto-embedding, ctx.ai namespace, MCP server, RAG
- `WEB3_STRATEGY.md` — SIWE, token-gated permissions, IPFS, chain indexing
- `ENTERPRISE_STRATEGY.md` — SaaS dual-mode, multi-tenancy, SOC2/HIPAA, pricing
- `SCALABILITY_STRATEGY.md` — 4 scale tiers, NATS, Redis, Patroni DR
- `DX_STRATEGY.md` — 24 DX improvements, error codes, CLI enhancements
- `GRAPHDB_LEARNINGS.md` — Entity Pool, connectors, COPY, forward-chaining rules

### Audit Reports
- `SECURITY_AUDIT.md` — 10 findings across 4 severity levels
- `VERIFICATION.md` — PhD-verified test results and integration assessment
- `IMPROVEMENT_LOG.md` — All fixes applied during audit phase
- `SDK_AUDIT.md` — Cross-SDK quality audit (6 packages)
- `CODE.md` — Complete technical reference with honest status
- 7x module-level `AUDIT.md` files (triple_store, query, sync, auth, functions, api, storage, cli)

### HTML Guide
- `docs/guide/index.html` — 2,192-line self-contained developer guide with sidebar, code highlighting, SDK tabs

---

## 8. Infrastructure & DevOps

### Docker
- Multi-stage Rust build → Alpine runtime
- pgvector/pgvector:pg16 for Postgres with vector support
- Non-root user, binary hardening (chmod 555)
- docker-compose with health checks, resource limits, network isolation

### Kubernetes
- Helm chart with security contexts (runAsNonRoot, readOnlyRootFilesystem, drop ALL)
- HPA (HorizontalPodAutoscaler) for auto-scaling
- Startup/liveness/readiness probes
- ConfigMap checksum annotations for rollout on config change

### CI/CD (GitHub Actions)
- `ci.yml` — Rust (fmt + clippy + test), TypeScript (build + test), Python, PHP
- `release.yml` — Multi-platform binaries (linux/macOS/Windows, amd64/arm64)
- `docker.yml` — Multi-arch Docker image to ghcr.io
- `e2e.yml` — Integration tests against real Postgres

---

## 9. Dharmic Philosophy (founder narrative)

- **Bhagavad Gita 3.35** — "Sreyaan sva-dharmo vigunah para-dharmaat su-anushthitaat" — Better to walk your own path imperfectly than another's perfectly
- **Bhagavad Gita 2.47** — "Karmanye vadhikaraste ma phaleshu kadachana" — You have the right to work, not the fruit of work
- **Sompura Brahmins** — Temple builders of Gujarat (Modhera Sun Temple, Somnath, Dilwara)
- **Darshan** (दर्शन) — Sanskrit: "vision", "seeing", "perceiving the complete picture"
- **Dharma → Vichara → Karma → Seva → Loka** — The building cycle: duty → thought → action → service → world

---

## 10. Quantum Computing & Post-Quantum Cryptography

### Primary Research
- **"Quantum blockchain: Trends, technologies, and future directions"** — M. A. Khan et al., *IET Quantum Communication*, 2024. DOI: [10.1049/qtc2.12119](https://doi.org/10.1049/qtc2.12119) — Comprehensive survey of quantum threats to blockchain/database cryptography, lattice-based signature schemes, hybrid migration strategies

### NIST Post-Quantum Cryptography Standards
- [NIST PQC Standardization Project](https://csrc.nist.gov/projects/post-quantum-cryptography) — FIPS 203 (ML-KEM/Kyber), FIPS 204 (ML-DSA/Dilithium), FIPS 205 (SLH-DSA/SPHINCS+)
- [CRYSTALS-Dilithium](https://pq-crystals.org/dilithium/) — Lattice-based digital signature scheme; selected by NIST as ML-DSA (FIPS 204); DarshJDB's primary PQC candidate for JWT signing
- [Falcon](https://falcon-sign.info/) — NTRU lattice-based signatures; smaller signatures than Dilithium but more complex implementation; DarshJDB's backup PQC candidate
- [SPHINCS+](https://sphincs.org/) — Stateless hash-based signatures; conservative fallback requiring no lattice assumptions; largest signatures but highest confidence

### Quantum Attack Vectors Relevant to DarshJDB
- **Shor's algorithm** — Breaks RSA, ECDSA, and all integer-factorization/discrete-log crypto in polynomial time; directly threatens DarshJDB's RS256 JWT signing
- **Grover's algorithm** — Provides quadratic speedup for brute-force search; reduces SHA-256 from 256-bit to ~128-bit effective security; mitigated by upgrading to SHA-512
- **"Harvest now, decrypt later"** — Adversaries record encrypted traffic today for future quantum decryption; low risk for DarshJDB due to 15-minute JWT expiry and opaque refresh tokens

### DarshJDB Quantum Strategy
- See `docs/strategy/QUANTUM_STRATEGY.md` for the complete three-phase migration plan (immediate SHA-512 hardening, hybrid Dilithium+RSA, full PQC)

---

## 11. Founder Context

- **Darshankumar Joshi** — Born in Navsari, Gujarat
- London: Business Computing (Greenwich), Advanced Diploma IT (Sunderland)
- Dubai: VFX production (Aquaman, The Invisible Man, The Last of Us Part II)
- Ahmedabad: GraymatterOnline LLP (2015), Graymatter International (2018), Coeus Digital Media (2020), KnowAI (2024)
- Currently: CEO at GraymatterOnline LLP, CTO at KnowAI
- Credentials: Ph.D. Business CS, CCNA, MCSE, CEH
- Websites: darshj.ai, darshj.me
