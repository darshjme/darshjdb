# DEFERRED.md — v0.3.2.1 executor-rewire branch

Items planned for this sprint that did not ship in
`feat/v0.3.2.1-executor-rewire`. Each entry is a one-line rationale
plus the v0.3.3 (or earlier) target where the work resumes.

## Architectural deferrals

- **Portable SELECT / CREATE / INSERT / RETRACT body for the
  DarshanQL SurrealQL-shaped executor.** The `ExecutorContext` is
  threaded through every function and exposes `Arc<dyn Store>`, but
  the executor body still calls `sqlx::query` against `ctx.pool`
  directly. Reason: `darshql/executor.rs` consumes a SurrealQL-shaped
  AST (`Statement::Select`) that is independent of the JSON-shaped
  `query::QueryAST` driven by `plan_query_with_dialect`. There is no
  planner today that turns the SurrealQL AST into a `QueryPlan`, so
  there is nothing for `Store::query` to receive. Resumes in v0.3.3
  once the two planner surfaces unify.

- **`INFO FOR` (`SHOW TABLES` / `SHOW COLUMNS`) on SQLite.** The
  storage layout (`:schema/*` triples) is portable; the executor's
  read SQL is not. Gated under `supports_ddl()` for now and refuses
  on SQLite. Tracked together with `DEFINE TABLE` / `DEFINE FIELD`
  in v0.3.3.

- **Hybrid full-text + vector search on SQLite.** Requires either
  `sqlite-vec` for native cosine distance or an in-process re-ranker.
  `supports_hybrid_search()` is gated; the v0.3.3 plan introduces an
  in-process vector fallback so the gate flips to true.

## Test-coverage deferrals

- **`tests/sqlite_e2e.rs`** for the standalone
  `set_triples + get_entity + retract` triple-level surface. The
  verification matrix in the sprint brief lists this file but every
  case it would cover is already pinned by the
  `store::sqlite::tests` unit tests inside `sqlite.rs` itself
  (`set_triples_and_get_entity_roundtrip`, `retract_hides_triples`,
  `bulk_ingest_batch`, `ttl_triples_hidden_*`,
  `concurrent_set_triples_do_not_deadlock`,
  `get_schema_*`). The integration-test split is cosmetic and
  scheduled for the next sprint cleanup pass.

- **PgStore round-trip parity test for the new `Store::query`
  contract.** A side-by-side test that runs the same `QueryAST`
  through both `PgStore::query` and `SqliteStore::query` and asserts
  `serde_json::to_value(...)` equality. Requires Postgres test
  infrastructure (`testcontainers` or the embedded-pg fixture) to
  spin up reproducibly in CI; the embedded-pg fixture exists but is
  gated behind `--features embedded-db`. Tracked for v0.3.2.2 along
  with the embedded-pg CI matrix work.

## Surface-level items left for v0.3.3

- `darshql::execute_with_context` is exported but no production
  caller uses it yet — the HTTP entry point still goes through the
  backwards-compatible `execute(&PgPool, …)` shim. The mlua runtime
  and any new portable host APIs should switch to the context-based
  signature at their next touch.

- Tier-1 portable rewires planned by the sprint brief (`SELECT`,
  `INSERT`, `UPDATE`, `DELETE` through `Store::set_triples` /
  `Store::retract`) are not landed because the underlying SurrealQL
  planner gap above blocks them. The capability gates ensure the
  unsupported paths fail loudly on non-Pg dialects until that lands.
</content>
</invoke>