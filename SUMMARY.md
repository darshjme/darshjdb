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
