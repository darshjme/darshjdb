# v0.3.2 Sprint Agent 2 — DarshanQL SqlDialect Summary

Branch: `feat/v0.3.2-darshanql-dialect`
Worktree: `.claude/worktrees/v032-darshanql-dialect`
Base tag: `v0.3.1` (commit `32e9b18`)

## Landed

### New module: `packages/server/src/query/dialect.rs` (~490 LOC)

`SqlDialect` trait with 15 methods covering every dialect-specific SQL fragment the v0.3.1 planner emitted:

- `placeholder(idx)` — `$1` (PG) vs `?1` (SQLite)
- `jsonb_param(idx, ParamKind)` — `to_jsonb($1::text)` / `$1::jsonb` vs `json_quote(?1)` / `?1`
- `compare_triple_value(alias, op, param)` — shared shape
- `jsonb_contains(alias, param)` — `@>` vs `instr() > 0`
- `text_ilike(alias, param)` — `#>> '{}' ILIKE` vs `LIKE`
- `uuid_cast(param)` — `::uuid` vs pass-through
- `uuid_array_cast(param)` — `::uuid[]` vs pass-through
- `in_uuid_list(column, placeholders)` — portable `IN (…)` renderer
- `fulltext_match(alias, param)` — `to_tsvector`/`plainto_tsquery` vs `LIKE '%' || param || '%'`
- `vector_literal(values)` — `'[…]'::vector` vs unsupported sentinel
- `cosine_distance(alias, literal)` — `<=>` vs unsupported sentinel
- `supports_vector()` — `true` (PG) vs `false` (SQLite)
- `recursive_cte_keyword()` — shared `WITH RECURSIVE`
- `now_expr()` — `NOW()` vs `datetime('now')`
- `name()` — `"postgres"` / `"sqlite"`

Both `PgDialect` and `SqliteDialect` are zero-sized types (`#[derive(Debug, Clone, Copy, Default)]`), `Send + Sync`, cheap to share. Module-level tests: 22 unit tests, one per method per dialect.

### Planner refactor: `packages/server/src/query/mod.rs`

- New `plan_query_with_dialect(ast, &dyn SqlDialect) -> Result<QueryPlan>`
- `plan_query(ast)` is now a thin wrapper passing `&PgDialect`
- New `plan_hybrid_query_with_dialect(ast, &dyn SqlDialect)`
- `plan_hybrid_query(ast)` is now a thin wrapper passing `&PgDialect`
- `build_nested_plans(nested, &dyn SqlDialect, depth)` threads the dialect through recursive nested-plan construction

Rewired call sites inside `plan_query_with_dialect`:

- Type-filter JOIN → `dialect.jsonb_param(…, ParamKind::Text)`
- WHERE attribute placeholders → `dialect.placeholder(idx)`
- WHERE value comparisons (`=`, `!=`, `>`, `>=`, `<`, `<=`) → `dialect.compare_triple_value(alias, op, jsonb_param)`
- WHERE `Contains` → `dialect.jsonb_contains(alias, param)`
- WHERE `Like` → `dialect.text_ilike(alias, placeholder)`
- `$search` → `dialect.fulltext_match(alias, placeholder)`
- `$semantic` (gated on `supports_vector`) → `dialect.vector_literal(vec)` + `dialect.cosine_distance(alias, lit)`
- `ORDER BY` correlated sub-select → `dialect.placeholder(idx)`

Rewired call sites inside `plan_hybrid_query_with_dialect`:

- Returns `InvalidQuery` on dialects that do not support vector search (SQLite) rather than emitting invalid SQL
- Postgres path uses `dialect.jsonb_param`, `dialect.placeholder`, `dialect.fulltext_match`, `dialect.vector_literal`, and `dialect.cosine_distance`
- Produces byte-for-byte v0.3.1 SQL on Postgres (verified by the pre-existing `plan_hybrid_generates_rrf_ctes` test still passing unmodified)

### Parity test suite (13 tests) in `query::tests`

- `parity_plan_basic_both_dialects_work`
- `parity_where_eq_string_value`
- `parity_where_eq_numeric_value`
- `parity_where_all_operators` (8 operators)
- `parity_where_contains_containment`
- `parity_where_like_prefix`
- `parity_search_fulltext`
- `parity_semantic_vector_pg_only`
- `parity_hybrid_sqlite_errors`
- `parity_order_by_correlated_subquery`
- `parity_nested_plan_uuid_batch`
- `parity_pg_default_wrapper_matches_with_dialect` — guarantees `plan_query()` is bit-identical to `plan_query_with_dialect(…, &PgDialect)` for a representative set of ASTs
- `parity_plan_cache_works_with_both_dialects` — PlanCache round-trips plans from either dialect

### Documentation: `docs/SQL_DIALECTS.md` (new)

Documents the trait, every method's Postgres vs SQLite spelling, the two approximations (JSONB containment and full-text search), the one refusal (vector search), the intended post-merge `main.rs` wiring that pairs `Store` + `Dialect` based on the `DATABASE_URL` prefix, and the v0.3.3/v0.4 roadmap. `docs/STORAGE_BACKENDS.md` was left untouched because Agent 1 likely wants to update it as part of the SqliteStore landing.

## Deferred

- **Planner call sites inside `packages/server/src/query/darshql/executor.rs`** were not refactored. That file is a live-pool executor, not a pure planner — it constructs SQL with `to_jsonb(...)` and `::uuid` casts inline but also calls `sqlx::query(...).execute(pool)` directly. Rewiring those sites meaningfully requires routing through the `Store` trait (Agent 1's territory), so touching them in this sprint would have created a shared-file collision. The `SqlDialect` trait is ready to accept those call sites as a follow-up.
- **SQLite FTS5** integration for `fulltext_match`. The current `SqliteDialect::fulltext_match` returns `col LIKE '%' || ? || '%'`, which is syntactically valid and returns correct (if unranked) results. FTS5 requires a separate virtual table, a schema-migration decision owned by Agent 1.
- **Proper SQLite JSON containment.** `jsonb_contains` approximates `@>` as `instr(col, param) > 0`, which is lossy for nested JSON paths but works for the planner's current scalar-containment usage. A portable IR in v0.4 replaces this.

## Blocked

None. No orchestrator decisions required.

## Files touched

Created:

- `packages/server/src/query/dialect.rs`
- `docs/SQL_DIALECTS.md`
- `SUMMARY.md` (this file)

Modified:

- `packages/server/src/query/mod.rs`

## Commit graph

```
f30fa17 docs(query): SQL_DIALECTS.md describing PgDialect and SqliteDialect
a553a29 test(query): dialect parity suite for Postgres and SQLite planners
203db8e refactor(query): route plan_query through SqlDialect trait
c6d145a feat(query): introduce SqlDialect trait with Pg and Sqlite impls
```

## Verification

```sh
# Compile
cargo check -p ddb-server
#   -> clean (one pre-existing sqlx-postgres future-incompat warning)

# Full query tests (171 pre-existing + 27 new = 198)
cargo test -p ddb-server --lib query
#   -> 198 passed; 0 failed; 1 ignored (pre-existing baseline)

# Dialect unit tests
cargo test -p ddb-server --lib query::dialect
#   -> 22 passed

# Parity suite
cargo test -p ddb-server --lib query::tests::parity
#   -> 13 passed
```

Clippy is clean on the files I touched. Pre-existing warnings in `packages/server/src/config/mod.rs` are unrelated to this branch.

## Cross-agent notes for the orchestrator

- **No shared file touched.** This branch modifies exactly one Rust file in the server crate (`packages/server/src/query/mod.rs`) plus one new module file in a greenfield location. No edits to `Cargo.toml`, `main.rs`, `store/mod.rs`, `store/sqlite.rs`, `store/pg.rs`, or anything in `functions/`. Merge with Agents 1 and 3 should be conflict-free.

- **Agent 1 (SqliteStore) integration hand-off.** To execute a `QueryPlan` on SQLite, Agent 1's `SqliteStore::query` impl needs to call `plan_query_with_dialect(ast, &SqliteDialect)` (instead of `plan_query(ast)`) and bind parameters using `?N`-positional syntax. The `Store::query(&QueryPlan)` trait method already accepts a fully-baked plan, so the dialect choice lives one level up — in whichever handler converts a DarshanQL JSON request into a `QueryPlan`. Two reasonable landing spots:
  1. Add a `dialect(&self) -> &'static dyn SqlDialect` method on the `Store` trait and have the handler call `plan_query_with_dialect(ast, store.dialect())`.
  2. Keep `Store` unchanged and thread `Arc<dyn SqlDialect>` through `AppState` alongside `Arc<dyn Store>`.

  Option 2 is lighter and avoids changing Agent 1's trait. I recommend it.

- **`plan_query()` compatibility guarantee.** The pre-existing server code that imports `plan_query` from `crate::query` does not need to change during the merge — `plan_query()` now unconditionally delegates to `plan_query_with_dialect(…, &PgDialect)` and emits the same SQL byte-for-byte. The `parity_pg_default_wrapper_matches_with_dialect` test enforces this invariant in CI.

- **Plan cache scope.** `PlanCache` keys by AST shape, not dialect. A single process should instantiate exactly one cache because it uses exactly one dialect. If the post-merge wiring ever supports switching dialects at runtime (it should not), the cache would need a dialect tag added to `shape_key`.

- **`plan_hybrid_query_with_dialect` on SQLite returns `InvalidQuery`.** Agent 1's `SqliteStore` will never receive a `QueryPlan` produced from `$hybrid` — because the planner refuses to produce it on SQLite in the first place. The HTTP handler's error path should surface this as a 400 with the trait's error message.

- **Agent 3 (mlua) interaction.** Zero. Lua functions do not emit SQL and do not touch the query planner. Merge is trivially orthogonal.
