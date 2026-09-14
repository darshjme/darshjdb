# Operating DarshJDB

These notes describe the source accompanying the v0.4.0 README. They are not a record of a production deployment or a fresh runtime test. Check the linked implementations when changing a deployment.

## Configuration and startup

The [configuration loader](../packages/server/src/config/mod.rs) merges defaults, `config.toml`, `config.local.toml`, `DDB__*` and `DARSH__*` variables. It also reads `.env`; unsetting a shell variable alone does not remove a value supplied by that file or TOML.

[`ddb-server`](../packages/server/src/main.rs) resolves a configured PostgreSQL URL before considering the optional `embedded-db` feature. SQLite store code exists at library level, but `sqlite:` URLs are rejected by the HTTP server. Embedded mode downloads/manages PostgreSQL and should not be described as an in-process SQLite database.

Set `DDB_JWT_SECRET` and `DDB_STORAGE_KEY` for the relevant authentication and storage paths. The server requires the storage key unless development mode supplies an insecure fallback. Use `DDB_DEV` only for disposable local development, not as a deployment shortcut. Keep secrets stable across restarts and out of version control.

## Compose and packaging

The current [Compose file](../docker-compose.yml) defines three services: `darshjdb`, `ddb-cache`, and `postgres`. It requires the Postgres, JWT, storage-signing and cache secrets. The base file now forwards `DDB_STORAGE_KEY`; the README override remains compatible.

The [Dockerfile](../Dockerfile) builds the dashboard and copies **all three binaries**—`ddb-server`, `ddb`, and `ddb-cache-server`—into its runtime image. The cache listens in a separate service on port 7701; the application uses 7700. Earlier claims that the cache executable was missing or that this stack always started Redis and Qdrant are outdated.

The default Compose topology is single-node. Back up PostgreSQL and local file data, preserve deployment secrets, and verify restore procedures before relying on it. A published architecture example is not evidence that high availability has been validated on your deployment.

## Migrations and optional extensions

The [startup migration runner](../packages/server/src/migrations.rs) embeds the listed schema migrations, excludes seed data, and records successful files in `_ddb_migrations`. Each file runs in a transaction. A failed file is rolled back, logged and retried on a later startup; startup can continue. `DDB_SKIP_MIGRATIONS=1` bypasses this runner.

Check startup logs and the migration ledger when enabling pgvector or TimescaleDB features. A basic health response does not establish that every extension, index or optional API is ready. Do not reapply the old README’s claim that only `001_initial.sql` runs automatically.

## API boundaries to review

- **Mutation authorization:** general mutations and sequential/parallel batch queries now verify sessions and apply permission rules. Mutations check the existing entity type, row predicates before and after changes, field restrictions and schema constraints. Batch reads use the transaction connection so they see earlier writes. The isolated PostgreSQL regression covers ownership and rollback. This does not establish equivalent protection for every other API surface.
- **Memory recall:** [`semantic_recall`](../packages/server/src/agent_memory/repo.rs) uses case-insensitive SQL `LIKE` matching. Embedding generation and embedding columns do not make this endpoint a vector recall API.
- **Entity search:** semantic and hybrid handlers in [REST](../packages/server/src/api/rest.rs) are separate from agent-memory recall and use PostgreSQL vector queries. Confirm extension and schema availability for the selected endpoint.
- **File storage:** startup constructs [`LocalFsBackend`](../packages/server/src/main.rs). Configuration fields alone do not establish support for an S3-compatible backend.
- **Audit and realtime:** inspect the selected mutation route, [triple-store implementation](../packages/server/src/store/) and [subscriptions documentation](subscriptions.md) before assuming that every write has the same audit-root and cross-replica notification behavior.

## Verification

Run builds and tests locally or on an authorized server. GitHub Actions is disabled. The README rewrite checked these statements against source and validated documentation structure; it did not rerun the application suite, launch a database, or establish production-readiness claims.

## Admin console

The embedded console lives at `/admin/`, including assets and deep links. It uses same-origin APIs and runtime email/password sign-in; there is no bundled development bearer token. Only an account with the admin role enters the console. Sessions live in tab session storage and are cleared on sign-out or a 401 response. For separate development servers, set `DDB_DEV_PROXY` to the backend URL.

Schema relationships, authentication users, storage and system status come from server APIs. Storage supports upload, download and deletion. Server logs remain available through process output, and backups through PostgreSQL/deployment tooling. A webhook editor is not yet present; use `/api/webhooks`. The console does not manufacture backup history, metrics or webhook deliveries.
