# v0.3.2 Sprint — Agent 1 (SqliteStore) Summary

Branch: `feat/v0.3.2-sqlite-store` (rooted at `v0.3.1` / commit `32e9b18`).

## Landed

All Store trait methods implemented and unit-tested.

- `SqliteStore::open` — schema migration (idempotent, WAL, json1 CHECK) — `open_in_memory_and_migrate`
- `backend_name` — returns "sqlite"
- `set_triples(tx_id, &[TripleInput])` — validated batch INSERT in IMMEDIATE tx, TTL computed — `set_triples_and_get_entity_roundtrip`, `bulk_ingest_batch`
- `get_entity(entity_id)` — filters retracted + expired TTL — `set_triples_and_get_entity_roundtrip`, `ttl_triples_hidden_when_expired`
- `retract(entity_id, attribute)` — logical delete — `retract_hides_triples`
- `get_schema()` — live entity-type inference with cardinality, required flag, references — `get_schema_infers_entity_types`
- `next_tx_id()` — monotonic via `UPDATE darshan_tx_seq ... RETURNING` (SQLite 3.35+) — `open_in_memory_and_migrate`
- `begin_tx()` — stateless marker handle (parity with PgStoreTx), probes connection — `begin_tx_marker_roundtrip`
- Input validation path — empty attribute rejected — `invalid_triple_rejected_before_write`
- `query(plan)` — intentional InvalidQuery refusal (v0.4 portable IR) — `query_returns_invalid_query`

Tests: 9 passed / 0 failed via `cargo test -p ddb-server --features sqlite-store --lib -- store::sqlite`.

## Deferred

- `query()` over DarshanQL plans — returns InvalidQuery until DarshanQL grows a portable IR (v0.4). Refuses honestly rather than silently returning wrong results.
- FTS5 — layout documented in `migrations/sqlite/001_initial.sql` trailing TODO; needs sync triggers, v0.4.
- sqlite-vec vector search — v0.4.
- Multi-statement StoreTx — kept marker-only, matching PgStoreTx. Needs a richer owned-connection primitive shared with the Postgres adapter.
- Read connection pool / WAL reader sharding — performance follow-up, not correctness.
- Cross-feature integration tests (cache, agent-memory, reactive tracker) — Postgres-only until main.rs wiring post-merge (out of scope).

## Blocked

Nothing. Trait surface in `packages/server/src/store/mod.rs` was stable enough to implement against directly.

Note for orchestrator: this agent did NOT modify `packages/server/Cargo.toml`. The v0.3.1 stub already declared `rusqlite = { version = "0.31", optional = true, features = ["bundled"] }` and `sqlite-store = ["dep:rusqlite"]`. Bundled rusqlite ships with SQLITE_ENABLE_JSON1, giving us `json_valid()` / `json_extract()` / `RETURNING` for free. Zero new dependencies. Zero touch on shared Cargo.toml files → zero 3-way merge risk from Agent 1.

## Files touched

- `migrations/sqlite/001_initial.sql` (new, 84 lines)
- `packages/server/src/store/sqlite.rs` (stub replaced, +707 / -57)
- `docs/STORAGE_BACKENDS.md` (appended v0.3.2 section, +63)
- `SUMMARY.md` (this file)

Territory respected: no edits to `packages/server/src/query/**` (Agent 2), `packages/server/src/functions/**` (Agent 3), the workspace `Cargo.toml`, `packages/server/Cargo.toml`, or `packages/server/src/main.rs`.

## Commit graph

```
fa2d46d docs(sqlite-store): record v0.3.2 shipped status
733179b feat(sqlite-store): implement Store trait over rusqlite
d93368c feat(sqlite-store): schema migration — triples + darshan_tx_seq
```

Final commit (this SUMMARY) is committed separately with message `docs(sprint): v0.3.2 SqliteStore agent summary`.

## Verification

```bash
cargo check -p ddb-server
cargo check -p ddb-server --features sqlite-store
cargo test  -p ddb-server --features sqlite-store --lib -- store::sqlite
```

Expected test output: 9 passed; 0 failed; 0 ignored.

Clippy on lib target produces zero warnings inside `packages/server/src/store/sqlite.rs` (pre-existing warnings in other files are outside Agent 1's territory).

Branch is not pushed — orchestrator owns the push and the 3-way merge with Agents 2 and 3.

## v0.3.2 Post-Review Fixes (2026-04-15)

Code review on the initial 4 commits surfaced 0 Critical / 2 Major / 4 Minor findings. A fix agent landed all of them on the same branch. Each fix is its own atomic commit; every commit has `cargo check --features sqlite-store` and `cargo test ... store::sqlite` green before proceeding.

- `fix(sqlite-store): unify TTL timestamp format with SQLite strftime reader` — **MAJOR-1**. The `set_triples` writer was emitting `chrono::to_rfc3339()` (`…+00:00`) while the `get_entity` reader compared against `strftime('%Y-%m-%dT%H:%M:%fZ', 'now')` (`…Z`). Those strings are lexicographically incomparable (`+` = 0x2B, `Z` = 0x5A), so any sub-second TTL comparison gave wrong results — the original `ttl_triples_hidden_when_expired` test only passed because it used `ttl_seconds: -1` (a year-stale date wins regardless of suffix). Writer now emits `%Y-%m-%dT%H:%M:%S%.3fZ`, matching the reader byte-for-byte. New `ttl_triples_hidden_near_expiry` regression test uses `ttl_seconds=1` + a 1200ms sleep to catch future drift under real sub-second timing.
- `fix(sqlite-store): use IMMEDIATE transactions + 5s busy_timeout` — **MAJOR-2** and **MINOR-1**. `conn.transaction()` defaults to `DEFERRED` in rusqlite; the prior SUMMARY's claim of "IMMEDIATE tx" was aspirational. `set_triples` now uses `transaction_with_behavior(TransactionBehavior::Immediate)`, `retract` is wrapped in its own IMMEDIATE tx for symmetry and future multi-statement safety, and `SqliteStore::open` sets `conn.busy_timeout(5s)` so brief lock contention (WAL checkpoint races, IMMEDIATE upgrades) backs off cleanly instead of failing fast with `SQLITE_BUSY`.
- `fix(sqlite-store): filter non-TEXT :db/type values in get_schema` — **MINOR-3**. `json_extract(value, '$')` returns the underlying JSON type; any `:db/type` triple whose value was a JSON object, number, or bool crashed `row.get::<_, String>()` with `InvalidColumnType` and took down the entire `get_schema` endpoint — a DoS vector from any caller who could write a triple. Fixed at the SQL level with `AND json_type(value) = 'text'` so malformed rows are skipped. New `get_schema_skips_non_text_db_type` test inserts a `:db/type` with an object value and asserts inference still succeeds for the well-formed entities.
- `fix(sqlite-store): parse_sqlite_ts rejects missing Z suffix explicitly` — **MINOR-4**. `trim_end_matches('Z')` stripped all trailing `Z` characters and silently accepted malformed input like `…56ZZ` or a bare `…56` (no UTC marker). Now uses `strip_suffix('Z')` for exact-one-`Z` matching, returns `DarshJError::Internal` with a descriptive message on missing/wrong suffix, and `row_to_triple` wraps that error through `rusqlite::Error::FromSqlConversionFailure` for the driver layer.
- `test(sqlite-store): concurrent set_triples smoke (R1.10)`. Nyquist audit flagged concurrent-access coverage as missing. New `concurrent_set_triples_do_not_deadlock` test spawns 8 tasks against one cloned `SqliteStore` on a `multi_thread` tokio runtime, each committing 20 triples; joins all; asserts 160 rows land and every entity has exactly 20 triples visible. Proves `Mutex<Connection>` + `spawn_blocking` doesn't deadlock and IMMEDIATE tx serializes cleanly under real contention.

### Test count

`cargo test -p ddb-server --features sqlite-store --lib -- store::sqlite` now runs **12 tests** (up from 9):

```
test store::sqlite::tests::begin_tx_marker_roundtrip ... ok
test store::sqlite::tests::bulk_ingest_batch ... ok
test store::sqlite::tests::concurrent_set_triples_do_not_deadlock ... ok
test store::sqlite::tests::get_schema_infers_entity_types ... ok
test store::sqlite::tests::get_schema_skips_non_text_db_type ... ok
test store::sqlite::tests::invalid_triple_rejected_before_write ... ok
test store::sqlite::tests::open_in_memory_and_migrate ... ok
test store::sqlite::tests::query_returns_invalid_query ... ok
test store::sqlite::tests::retract_hides_triples ... ok
test store::sqlite::tests::set_triples_and_get_entity_roundtrip ... ok
test store::sqlite::tests::ttl_triples_hidden_near_expiry ... ok
test store::sqlite::tests::ttl_triples_hidden_when_expired ... ok

test result: ok. 12 passed; 0 failed; 0 ignored
```

### Correction to original claims

The original "Landed" section claimed `set_triples` used an "IMMEDIATE tx". That was wrong as shipped — it defaulted to `DEFERRED` in rusqlite. As of the MAJOR-2 fix, the claim is now accurate: `set_triples` and `retract` both begin IMMEDIATE transactions, and the connection carries a 5-second `busy_timeout` so upgrade contention backs off instead of erroring out.

### Verification (post-fix)

```bash
cargo check   -p ddb-server
cargo check   -p ddb-server --features sqlite-store
cargo test    -p ddb-server --features sqlite-store --lib -- store::sqlite
cargo clippy  -p ddb-server --features sqlite-store --lib
```

All green. Clippy produces 4 warnings, all in `packages/server/src/config/mod.rs` (collapsible-if, pre-existing) — those belong to Phase 1 Stream A and are outside the SqliteStore fix agent's territory. Zero new warnings inside `packages/server/src/store/sqlite.rs`.
