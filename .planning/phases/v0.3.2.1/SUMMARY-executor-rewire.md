# SUMMARY.md — v0.3.2.1 executor-rewire

Branch: feat/v0.3.2.1-executor-rewire
Worktree: /Users/darshjme/repos/darshandb/.claude/worktrees/v0321-executor-rewire
Base: tag v0.3.2 (8f7f96c)
Date: 2026-04-15

## Commits landed

```
00049c4  docs(changelog): v0.3.2.1 — executor rewire + SqliteStore::query
95803e0  test(sqlite-store): end-to-end query roundtrip integration test
d3f504a  refactor(darshql): thread ExecutorContext through executor.rs
3e40a22  feat(dialect): add executor capability gates (DDL, graph, hybrid)
3cf9c2d  feat(sqlite-store): implement Store::query via rusqlite + dialect plan
```

Five atomic commits, ordered low-risk to high-risk:

1. **3cf9c2d — SqliteStore::query real implementation.** Replaces the v0.3.2 stub that always returned InvalidQuery. Binds serde_json::Value params through a small ToSql adapter, expands `__UUID_LIST__` for nested plans, executes via rusqlite on a blocking task, materialises rows in the same QueryResultRow JSON shape as PgStore::query. Refuses vector-sentinel SQL up front with a clear message.
2. **3e40a22 — Dialect capability gates.** Adds `supports_ddl`, `supports_graph_traversal`, `supports_hybrid_search` to SqlDialect with default-true. SqliteDialect overrides all three to false. PgDialect inherits the existing v0.3.1 surface unchanged.
3. **d3f504a — ExecutorContext refactor.** Threads `{pool, Arc<dyn Store>, Arc<dyn SqlDialect>}` through every function in darshql/executor.rs. The HTTP entry point keeps the existing `execute(&PgPool, …)` signature and forwards to a new `execute_with_context` shim. Tier 2 statement types (DEFINE TABLE, DEFINE FIELD, RELATE, SELECT field with `->edge`, `count(->edge)`) gate on `ctx.dialect.supports_*()`.
4. **95803e0 — Integration test.** New tests/sqlite_e2e_query.rs covers six SELECT shapes against an in-memory SqliteStore through plan_query_with_dialect + Store::query.
5. **00049c4 — CHANGELOG + DEFERRED.** v0.3.2.1 section in CHANGELOG.md and a per-item DEFERRED.md at the worktree root.

## Files touched

```
M  CHANGELOG.md
A  DEFERRED.md
A  SUMMARY.md                                          (this file)
M  packages/server/src/query/darshql/executor.rs
M  packages/server/src/query/dialect.rs
M  packages/server/src/store/sqlite.rs
A  packages/server/tests/sqlite_e2e_query.rs
```

Six source files; no other crates touched. main.rs, Cargo.toml, packages/server/src/api/rest.rs, packages/server/src/functions/**, packages/cache/**, packages/cache-server/** are all untouched.

## Tests added

| File                          | New tests | Total in file |
|-------------------------------|-----------|---------------|
| query::dialect::tests         | +3        | 26            |
| store::sqlite::tests          | +2 (-1)   | 14            |
| tests/sqlite_e2e_query.rs     | +6        | 6             |
| **net new**                   | **+10**   |               |

(The single removed test in store::sqlite was the obsolete query_returns_invalid_query that asserted the old "always refuses" behaviour; replaced by the two new round-trip tests plus a query_rejects_pgvector_sentinel regression for the gate path.)

Lib-test counts for the touched modules:
- before: 186 (cargo test -p ddb-server --lib query::)
- after:  189 (added 3 dialect capability tests)
- before (sqlite): 12 (cargo test -p ddb-server --features sqlite-store --lib store::sqlite)
- after:  14

## Verification matrix

All commands run from the worktree root.

- cargo check -p ddb-server — green
- cargo check -p ddb-server --features sqlite-store — green
- cargo check -p ddb-server --features mlua-runtime — green
- cargo check -p ddb-server --features "sqlite-store mlua-runtime" — green
- cargo check --workspace — green
- cargo test -p ddb-server --lib query:: — 189 passed, 1 ignored (pre-existing v0.2.0 baseline)
- cargo test -p ddb-server --lib query::darshql — 17 passed
- cargo test -p ddb-server --features sqlite-store --lib store::sqlite — 14 passed
- cargo test -p ddb-server --features sqlite-store --lib query::dialect — 26 passed
- cargo test -p ddb-server --features sqlite-store --test sqlite_e2e_query — 6 passed
- cargo clippy -p ddb-server --all-targets -- -D warnings — green
- cargo clippy -p ddb-server --all-targets --features sqlite-store -- -D warnings — green

The two test files referenced by the sprint brief that do not exist (tests/sqlite_e2e.rs for the standalone triple-level surface) are documented in DEFERRED.md. Every case those files would cover is already pinned by the in-module store::sqlite::tests set.

## Deferrals (with rationale)

See DEFERRED.md at the worktree root for the full list. Headlines:

1. **Portable executor body for SELECT / CREATE / INSERT / RETRACT.** The ExecutorContext exposes Arc<dyn Store> but the body still reaches for ctx.pool directly. Reason: darshql/executor.rs consumes a SurrealQL-shaped AST that is independent of the JSON-shaped query::QueryAST driven by plan_query_with_dialect. There is no planner today that turns the SurrealQL AST into a QueryPlan, so there is nothing for Store::query to receive. The capability gates ensure unsupported paths fail loudly on non-Pg dialects until the v0.3.3 planner unification lands.
2. **PgStore↔SqliteStore parity test.** Requires Postgres test infrastructure; the embedded-pg fixture is gated and not part of the default CI matrix. Tracked for v0.3.2.2.
3. **tests/sqlite_e2e.rs** triple-level coverage split — every case already lives in store::sqlite::tests. Cosmetic.

## Cross-agent collision notes

The sibling feat/v0.3.2.1-mlua-kv agent owns packages/server/src/functions/**, the DdbCache crates, and the cache-server crate. This branch touched **none** of those:

- packages/server/src/store/sqlite.rs — mine
- packages/server/src/query/dialect.rs — mine
- packages/server/src/query/darshql/executor.rs — mine
- packages/server/tests/sqlite_e2e_query.rs — mine, new file
- CHANGELOG.md, DEFERRED.md, SUMMARY.md — mine, root-level

Zero file overlap with the mlua-kv agent's territory. No collision risk on merge.

## What does NOT ship in v0.3.2.1

- Any change to packages/server/src/main.rs (zero edits)
- Any change to packages/server/Cargo.toml (zero edits)
- Any change to api/rest.rs (zero edits — backwards-compat shim in darshql::execute keeps the call site untouched)
- Tier 1 portable rewires for the SurrealQL executor (blocked on the planner gap, see DEFERRED.md)

## Ready to merge

The branch is ready for the orchestrator's main-merge gate. No push, no tag — handoff is via this SUMMARY.md and the five commits above.
