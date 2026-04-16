# Phase 2 Integration Summary — DarshJDB v0.3.2

**Date**: 2026-04-15
**Tip**: `8f7f96c` — `chore(release): bump workspace to 0.3.2`
**Tag**: `v0.3.2`
**GitHub release**: https://github.com/darshjme/darshjdb/releases/tag/v0.3.2
**Backup branch**: `backup/pre-v0.3.2-integration` (snapshot of `main` @ `65eb1b1`
before Phase 2 started)

## Scope

Land the three v0.3.2 sprint branches (sqlite-store, darshanql-dialect,
mlua-runtime) on top of the v0.3.1.1 hotfix, write the integration commit
that wires Store + SqlDialect through main.rs and the embedded function
runtime, ship `v0.3.2`.

## Step 1 — Rebase onto v0.3.1.1

Each sprint branch was rooted at v0.3.1 (`32e9b18`); main was at v0.3.1.1
(`65eb1b1`). Rebased each branch with `git rebase --onto 65eb1b1 32e9b18
feat/v0.3.2-<name>`.

| Branch                          | Commits | Conflicts | Resolution |
| ------------------------------- | ------- | --------- | ---------- |
| `feat/v0.3.2-darshanql-dialect` | 12      | none      | clean      |
| `feat/v0.3.2-sqlite-store`      | 10      | 1 file    | took theirs (v0.3.2 full impl); preserved StoreTx symmetry with PgStoreTx |
| `feat/v0.3.2-mlua-runtime`      | 12      | none      | clean      |

The sqlite-store conflict was in `packages/server/src/store/sqlite.rs`:
the v0.3.1.1 hotfix's `StoreTx` symmetry edit to the 134-line stub vs
the v0.3.2 full 776-line `rusqlite` implementation. Resolution was to
take the v0.3.2 impl wholesale (it already carries symmetric
`commit/rollback` markers matching `PgStoreTx`). All three rebased
branches were verified green via `cargo check` in their feature gates
before merging.

## Step 2 — Merge into main

Order: dialect → sqlite-store → mlua. Each merge was `--no-ff` to keep
the sprint history visible; each was followed by a `cargo check` matrix
and the feature-specific test suite.

| Order | Merge commit | Branch                          | Tests run                              |
| ----- | ------------ | ------------------------------- | -------------------------------------- |
| 1     | `cdb9c86`    | `feat/v0.3.2-darshanql-dialect` | `cargo test -p ddb-server --lib query::` → 186 passed |
| 2     | `5ab5f4d`    | `feat/v0.3.2-sqlite-store`      | `cargo test -p ddb-server --features sqlite-store --lib store::sqlite` → 12 passed |
| 3     | `3c665dc`    | `feat/v0.3.2-mlua-runtime`      | `cargo test -p ddb-server --features mlua-runtime --lib functions::mlua::` → 23 passed |

Each branch dropped its own `SUMMARY.md` at the repo root, causing
add/add conflicts on the second and third merges. Resolved by relocating
each to `.planning/phases/v0.3.2/SUMMARY-<branch>.md` in dedicated
follow-up commits (`f98afb3`, `8e7ba3d`, `05bd10e`). The `05bd10e`
commit also stripped the `# TODO(v0.3.2-sprint-merge):` markers from
`packages/server/Cargo.toml` and corrected the `sqlite-store` Cargo
feature comment from "stub" to "full backend".

## Step 3 — Integration commit

`c8b2012` — `feat(v0.3.2): wire Store + SqlDialect through main.rs +
mlua DDB host API`. One cohesive commit that lands three concerns
because they form one surface:

### 3a — Top-level `Arc<dyn Store>` + `Arc<dyn SqlDialect>` in `main.rs`

Constructed once at boot from the existing PgTripleStore + PgPool path:

```rust
let store_dyn: Arc<dyn ddb_server::store::Store + Send + Sync> = Arc::new(
    ddb_server::store::pg::PgStore::new((*triple_store_arc).clone()),
);
let dialect_dyn: Arc<dyn ddb_server::query::dialect::SqlDialect + Send + Sync> =
    Arc::new(ddb_server::query::dialect::PgDialect);
```

These flow into the function runtime construction so the embedded mlua
backend can hold them without reaching for the concrete pool.

### 3b — `MluaContext` + wired `ddb.*` host API

Added an `MluaContext { store, dialect }` struct in `functions/mlua.rs`
and a new `MluaRuntime::new_with_context(dir, max_conc, Some(ctx))`
constructor. `install_ddb_api` now takes `Option<&MluaContext>`:

- **With context** (production server boot): `ddb.query`,
  `ddb.triples.get`, `ddb.triples.put` are wired live as
  `create_async_function` closures that capture `Arc` clones of the
  store + dialect.
  - `ddb.query(json_ast)` → `parse_darshan_ql` →
    `plan_query_with_dialect(ast, &*dialect)` → `store.query(plan).await`
    → Lua table.
  - `ddb.triples.get(uuid_string)` → `store.get_entity(uuid).await` → Lua array.
  - `ddb.triples.put(uuid_string, attribute, value)` →
    `store.next_tx_id().await` + `store.set_triples(tx_id, &[input]).await`.
- **Without context** (test default): same shims raise
  `NotYetImplemented` so existing test assertions stay green.
- `ddb.kv.{get,set}` stays `NotYetImplemented` regardless, with an
  updated message pointing to v0.3.2.1 (cache boundary not exposed).

The test helper `invoke_global` was updated to use `call_async` so
async host functions can yield without tripping the "yield from outside
a coroutine" guard. Three new tests gated on `(mlua-runtime +
sqlite-store)` exercise the wired path against a real `:memory:`
SqliteStore:

- `ddb_triples_put_and_get_roundtrip_via_sqlite` — proves
  `ddb.triples.put` then `ddb.triples.get` roundtrips strings and
  numbers through the store.
- `ddb_query_with_context_returns_invalid_query_from_sqlite` — proves
  the parse → plan → store.query chain reaches the backend with the
  dialect attached (asserts the documented `InvalidQuery` message that
  SqliteStore::query returns until the executor rewire lands in v0.3.2.1).
- `ddb_kv_stays_stubbed_with_context` — pins the v0.3.2.1 deferral
  message so it can't drift.

### 3c — `DDB_FUNCTION_RUNTIME=mlua` dispatch in `main.rs`

Refactored the single-cfg v8 branch into a three-way decision tree
that handles `mlua`, `v8`, and the subprocess default with feature
gates and clear fallback warnings. When `mlua` is selected and
`--features mlua-runtime` is compiled in, an `MluaContext` is built
from `store_dyn.clone()` + `dialect_dyn.clone()` and threaded into
`MluaRuntime::new_with_context`.

### 3d — `sqlite:` URL guard in `main.rs`

Front-door rejection of `sqlite:` URLs with a clear message: the
SqliteStore library backend is wired into the Store trait and the
function runtime, but the HTTP server's auth/anchor/search/agent_memory/
chunked_uploads bootstraps remain Postgres-only and a sqlite-only HTTP
boot lands in v0.3.3. Misconfig surfaces immediately instead of as a
cryptic `pg_advisory_lock` panic.

### 3e — Tiny clippy fix in `store/sqlite.rs`

`.err().expect(...)` → `.expect_err(...)` so `clippy --all-targets
--features sqlite-store -D warnings` passes.

## Step 4 — Format sweep

`af3d802` — `chore(v0.3.2): cargo fmt sweep across merged sprint branches`.
The three sprint branches were each developed against slightly
different rustfmt baselines; merging them surfaced cosmetic drift in 9
files spanning the dialect, store, query, cluster, and config modules.
Pure formatting, no semantic changes.

## Step 5 — CHANGELOG + version bump

- `4bd0096` — `docs(changelog): v0.3.2 — SQLite backend + mlua runtime
  + dialect abstraction`. Full v0.3.2 section above the v0.3.1 entry,
  documenting Added, Changed, Security, Cargo features, Known
  limitations / deferred items, and Acknowledgements (gsd-army audit
  protocol contributors).
- `8f7f96c` — `chore(release): bump workspace to 0.3.2`. Workspace +
  every member crate now reports 0.3.2.

## Step 6 — Final verification matrix

All four feature combos verified green on `cargo check`, `cargo clippy
--all-targets -D warnings`, and `cargo test --lib`:

| Feature combo                     | check | clippy | lib tests |
| --------------------------------- | ----- | ------ | --------- |
| default                           | OK    | OK     | 1428 passed, 0 failed |
| `sqlite-store`                    | OK    | OK     | 1440 passed, 0 failed |
| `mlua-runtime`                    | OK    | OK     | 1451 passed, 0 failed |
| `sqlite-store mlua-runtime`       | OK    | OK     | 1466 passed, 0 failed |

15-16 ignored tests are pre-existing baselines (e.g. the v0.2.0
`project_fields_hides_and_reorders` baseline failure, the
`cpu_bound_loop_is_bounded` mlua test that needs `set_interrupt`
landing in v0.3.3).

## Step 7 — Tag + push + release

```
git tag -a v0.3.2 -m "DarshJDB v0.3.2 — SQLite backend + mlua runtime + dialect abstraction"
git push origin main      # 65eb1b1..8f7f96c main -> main
git push origin v0.3.2    # [new tag] v0.3.2 -> v0.3.2
gh release create v0.3.2 ...
```

Release URL: https://github.com/darshjme/darshjdb/releases/tag/v0.3.2

## Deferred to v0.3.2.1 / v0.3.3

Each item is documented in the v0.3.2 CHANGELOG section under
"Known limitations / deferred to v0.3.2.1".

| Item | Why deferred | Tracking |
| ---- | ------------ | -------- |
| `darshql/executor.rs` rewire onto `Store::query` | 959 lines, 12 statement types, 20+ pg-specific helpers including graph traversal and DEFINE TABLE — too large for the integration commit budget. The simpler `parse_darshan_ql → plan_query → execute_query` JSON-AST path IS fully wired through Store via PgStore::query, which is what the mlua `ddb.query` binding uses. | v0.3.2.1 |
| `SqliteStore::query` returning real rows | Depends on the executor rewire; the SQLite SQL emission path covers triple-level CRUD (which IS wired) but not the full DarshanQL surface. | v0.3.2.1 |
| `ddb.kv.{get,set}` unstub | The DdbCache (slice 10) is keyed on the HTTP request boundary and is not exposed to the function runtime. Wiring it requires a tenant-scoped cache handle. | v0.3.2.1 |
| `sqlite:` URL HTTP boot | The HTTP server's auth/anchor/search/agent_memory/chunked_uploads bootstraps remain Postgres-only; a sqlite-only HTTP boot path needs SQL portability work across all of them. main.rs rejects sqlite: URLs at the front door so misconfig surfaces immediately. | v0.3.3 |
| Mid-instruction CPU interruption for Lua | Needs mlua 0.10 `set_interrupt` hook. Today the wall-clock timeout cancels at the next yield boundary, which is sufficient for cooperative user code (the `lua_call_respects_wall_clock_cap` test passes) but a `while true do end` tight loop is bounded only by the OS scheduler. | v0.3.3 |

## Final tip

```
8f7f96c chore(release): bump workspace to 0.3.2
4bd0096 docs(changelog): v0.3.2 — SQLite backend + mlua runtime + dialect abstraction
af3d802 chore(v0.3.2): cargo fmt sweep across merged sprint branches
c8b2012 feat(v0.3.2): wire Store + SqlDialect through main.rs + mlua DDB host API
05bd10e chore(planning): relocate mlua SUMMARY.md + strip sprint-merge TODOs from Cargo.toml
3c665dc Merge feat/v0.3.2-mlua-runtime — embedded mlua + hardened sandbox
8e7ba3d chore(planning): relocate sqlite-store SUMMARY.md under .planning/phases/v0.3.2/
5ab5f4d Merge feat/v0.3.2-sqlite-store — real rusqlite backend
f98afb3 chore(planning): relocate dialect SUMMARY.md under .planning/phases/v0.3.2/
cdb9c86 Merge feat/v0.3.2-darshanql-dialect — SqlDialect trait + Pg/Sqlite impls
65eb1b1 chore(store): symmetrize StoreTx commit/rollback across Pg and Sqlite (v0.3.1.1)
```
