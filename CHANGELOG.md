# Changelog

All notable changes to DarshJDB will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-04-09

### Added

- **Multi-Model Database**: Document + Graph + Relational + KV + Vector storage modes
- **DarshQL**: Full query language with DEFINE TABLE, DEFINE FIELD, SELECT, CREATE, INSERT, UPDATE, DELETE
- **Typed Fields**: String, Int, Float, Bool, Datetime, UUID, JSON, Array, Record, Vector with validation
- **Views System**: Grid, kanban, calendar, gallery, form views over the same data
- **Formula Engine**: Computed fields, rollups, aggregations evaluated server-side
- **Relations**: Link fields, lookup fields, rollup aggregations across linked records
- **Automations**: Event-triggered workflows with webhook, email, and field-update actions
- **Webhooks**: HTTP callbacks on CRUD events with exponential backoff retry
- **Import/Export**: CSV, JSON, DarshQL dump with bulk import support
- **Plugin System**: Extensible architecture with Slack, audit log, and data validation plugins
- **Version History**: Merkle-tree audit trail with SHA-512 hash chain and restore capability
- **Collaboration**: Workspace sharing with role-based access and activity feeds
- **GraphQL API**: async-graphql layer alongside REST
- **Graph Traversal**: Entity relationship traversal with forward-chaining rules
- **Presence System**: Real-time user presence with rooms and peer state
- **Schema Modes**: SCHEMALESS, SCHEMAFULL, SCHEMAMIXED with field-level validation and assertions

### Fixed

- All 17 compiler warnings resolved (unused imports, dead code, missing type annotations)
- All clippy lints resolved across the workspace
- Test compilation errors in 6 modules fixed (aggregation, auth, history, plugins, schema)
- Float equality check in field validation tests
- CI pipeline now passes: fmt, clippy, test, smoke test all green

### Changed

- Upgraded to Rust Edition 2024
- Modernized code patterns: `map_or` → `is_some_and`, `format!` → string literals where applicable
- Crate-level clippy configuration for intentional architectural patterns

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
