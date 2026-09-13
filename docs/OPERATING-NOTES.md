# Operating DarshJDB

These notes describe the source accompanying the v0.4.0 README. They are not a record of a production deployment or a fresh runtime test. Check the linked implementations when changing a deployment.

## Configuration and startup

The [configuration loader](../packages/server/src/config/mod.rs) merges defaults, `config.toml`, `config.local.toml`, `DDB__*` and `DARSH__*` variables. It also reads `.env`; unsetting a shell variable alone does not remove a value supplied by that file or TOML.

[`ddb-server`](../packages/server/src/main.rs) resolves a configured PostgreSQL URL before considering the optional `embedded-db` feature. SQLite store code exists at library level, but `sqlite:` URLs are rejected by the HTTP server. Embedded mode downloads/manages PostgreSQL and should not be described as an in-process SQLite database.

Set `DDB_JWT_SECRET` and `DDB_STORAGE_KEY` for the relevant authentication and storage paths. The server requires the storage key unless development mode supplies an insecure fallback. Use `DDB_DEV` only for disposable local development, not as a deployment shortcut. Keep secrets stable across restarts and out of version control.

## Compose and packaging

The current [Compose file](../docker-compose.yml) defines three services: `darshjdb`, `ddb-cache`, and `postgres`. It requires the Postgres, JWT and cache passwords. The README supplies a local override for `DDB_STORAGE_KEY`, which the base file does not forward to the application container.

The [Dockerfile](../Dockerfile) builds the dashboard and copies **all three binaries**—`ddb-server`, `ddb`, and `ddb-cache-server`—into its runtime image. The cache listens in a separate service on port 7701; the application uses 7700. Earlier claims that the cache executable was missing or that this stack always started Redis and Qdrant are outdated.

The default Compose topology is single-node. Back up PostgreSQL and local file data, preserve deployment secrets, and verify restore procedures before relying on it. A published architecture example is not evidence that high availability has been validated on your deployment.

## Migrations and optional extensions

The [startup migration runner](../packages/server/src/migrations.rs) embeds the listed schema migrations, excludes seed data, and records successful files in `_ddb_migrations`. Each file runs in a transaction. A failed file is rolled back, logged and retried on a later startup; startup can continue. `DDB_SKIP_MIGRATIONS=1` bypasses this runner.

Check startup logs and the migration ledger when enabling pgvector or TimescaleDB features. A basic health response does not establish that every extension, index or optional API is ready. Do not reapply the old README’s claim that only `001_initial.sql` runs automatically.

## API boundaries to review

- **Mutation authorization:** the general `mutate` handler in [REST](../packages/server/src/api/rest.rs) extracts a bearer token but does not use the same `check_permission` path as the entity CRUD handlers. Review this path before exposing it in a multi-user deployment. Authentication and row-level authorization are separate checks.
- **Memory recall:** [`semantic_recall`](../packages/server/src/agent_memory/repo.rs) uses case-insensitive SQL `LIKE` matching. Embedding generation and embedding columns do not make this endpoint a vector recall API.
- **Entity search:** semantic and hybrid handlers in [REST](../packages/server/src/api/rest.rs) are separate from agent-memory recall and use PostgreSQL vector queries. Confirm extension and schema availability for the selected endpoint.
- **File storage:** startup constructs [`LocalFsBackend`](../packages/server/src/main.rs). Configuration fields alone do not establish support for an S3-compatible backend.
- **Audit and realtime:** inspect the selected mutation route, [triple-store implementation](../packages/server/src/store/) and [subscriptions documentation](subscriptions.md) before assuming that every write has the same audit-root and cross-replica notification behavior.

## Verification

Run builds and tests locally or on an authorized server. GitHub Actions is disabled. The README rewrite checked these statements against source and validated documentation structure; it did not rerun the application suite, launch a database, or establish production-readiness claims.
