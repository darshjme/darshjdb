# Changelog

All notable changes to DarshJDB will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-04-15

### Changed
- Honest project framing — removed fabricated timeline, added AI-assisted development disclosure
- Synchronized all package versions to 0.4.0
- Split monolithic rest.rs (4,449 lines) into 15 focused handler modules
- Replaced fabricated competitor comparison docs with real criterion benchmarks

### Security
- Hardcoded dev-signing-key now requires DDB_DEV=1 in development
- pg_advisory_lock uses namespaced constant instead of magic number 42
- Docker Compose requires explicit REDIS_PASSWORD and POSTGRES_PASSWORD
- Added cargo audit and npm audit to CI pipeline
- Removed unsafe std::env::set_var calls

### Removed
- Vaporware strategy docs (quantum, blockchain, web3)
- Fabricated benchmark comparison pages
- Unimplemented security claims from SECURITY.md (TLS 1.3 mandatory, AES-256-GCM at rest, Ed25519)

## [0.1.0] - 2026-04-05

### Added

- **Core Database**: Triple-store graph engine over PostgreSQL with EAV architecture
- **DarshanQL**: Declarative query language with $where, $order, $limit, $search, $semantic operators
- **Real-Time Sync**: WebSocket-based reactive queries with delta compression
- **Optimistic Mutations**: Instant client-side updates with server reconciliation
- **Offline-First**: IndexedDB persistence with operation queue and sync on reconnect
- **Server Functions**: Queries, mutations, actions, cron jobs in V8 sandboxes
- **Authentication**: Email/password (Argon2id), magic links, OAuth (Google, GitHub, Apple, Discord), MFA
- **Permissions**: Row-level security, field-level permissions, role hierarchy, TypeScript DSL
- **File Storage**: S3-compatible with signed URLs, image transforms, resumable uploads
- **Presence System**: Rooms, peer state, typing indicators, cursor tracking
- **Admin Dashboard**: Data explorer, schema visualizer, function logs, user management
- **React SDK**: `@darshjdb/react` with hooks, Suspense, useSyncExternalStore
- **Next.js SDK**: `@darshjdb/nextjs` with Server Components, Server Actions, App Router
- **Angular SDK**: `@darshjdb/angular` with Signals (17+), RxJS, route guards, SSR
- **PHP SDK**: `darshan/darshan-php` with Laravel ServiceProvider
- **Python SDK**: `darshjdb` with FastAPI and Django integration
- **CLI**: `ddb dev`, `ddb deploy`, `ddb push`, `ddb pull`, `ddb seed`
- **Docker**: Single-command self-hosted setup with docker-compose
- **Kubernetes**: Helm chart for production deployment
- **REST API**: Full CRUD + query + auth + storage over HTTP with OpenAPI spec
- **SSE Fallback**: Server-Sent Events for environments without WebSocket
- **Security**: 11-layer defense-in-depth, OWASP API Top 10 coverage, zero-trust default
- **CI/CD**: GitHub Actions for Rust/TypeScript CI, multi-platform release builds, Docker image publishing
- **Examples**: React todo app, plain HTML example, cURL script collection
