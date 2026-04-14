# Storage backends — DarshJDB v0.3.1

DarshJDB was born as a Postgres-first server. Every line of the
triple store, the DarshJQL planner, and the migration engine assumes
PostgreSQL 16 with TimescaleDB and pgvector preloaded. That choice
has paid for itself in throughput and feature density, but it also
means the server has historically had **zero backend portability**.

The v0.3.1 architecture wave opens a path out by introducing a
backend-agnostic [`Store`] trait and validating it against two real
backends at the type level. This document tracks the journey.

[`Store`]: ../packages/server/src/store/mod.rs

---

## Current (v0.3.1) — what works

### Default build

- **Backend**: PostgreSQL 16.
- **Extensions required**: TimescaleDB, pgvector, pg_cron (all bundled
  in `timescale/timescaledb-ha:pg16-latest`).
- **Postgres-specific features the query engine depends on**:
  - `UUID` column type with `::uuid` casting
  - `JSONB` + `@>`, `?`, `->`, `->>` operators
  - `text[]` and other array types in `UNNEST`
  - GIN indexes on JSONB
  - `pg_notify` / `LISTEN` for cross-replica invalidation
  - Sequences (`nextval('darshan_tx_seq')`)
  - `ON CONFLICT ... DO UPDATE`
  - pgvector's `vector` column type + HNSW / IVFFlat index
  - TimescaleDB hypertables for history & anchor tables

### Store trait abstraction

The trait lives at `packages/server/src/store/mod.rs`. It is
intentionally narrower than the internal `TripleStore` trait and
uses `async_trait` so it is object-safe:

```rust
#[async_trait]
pub trait Store: Send + Sync {
    fn backend_name(&self) -> &'static str;
    async fn set_triples(&self, tx_id: i64, triples: &[TripleInput]) -> Result<()>;
    async fn get_entity(&self, entity_id: Uuid) -> Result<Vec<Triple>>;
    async fn retract(&self, entity_id: Uuid, attribute: &str) -> Result<()>;
    async fn query(&self, plan: &QueryPlan) -> Result<Vec<serde_json::Value>>;
    async fn get_schema(&self) -> Result<Schema>;
    async fn next_tx_id(&self) -> Result<i64>;
    async fn begin_tx(&self) -> Result<Box<dyn StoreTx + Send>>;
}
```

Two adapters are shipped in v0.3.1:

| Adapter | Path | Status |
|---|---|---|
| `PgStore` | `store/pg.rs` | **Production.** Thin wrapper over `PgTripleStore` — delegates every method. Not rewritten. |
| `SqliteStore` | `store/sqlite.rs` | **Compile-time stub.** Behind `sqlite-store` feature. Every method returns `NotYetImplemented`. Its existence forces the trait boundary to be real. |

Default build: `cargo check -p ddb-server` — Postgres only.
Trait validation build: `cargo check -p ddb-server --features sqlite-store`.

---

## Portability status — honest assessment

DarshanQL emits PostgreSQL-specific SQL. A plan built by
`packages/server/src/query/mod.rs::plan_query` references JSONB
operators, UUID casting, and `text[]` arrays directly in the SQL
string it constructs. That string is what `execute_query` hands to
sqlx. A SQLite or DuckDB backend cannot simply "also accept" that
SQL — the grammar is different.

Therefore `SqliteStore::query` cannot work until DarshanQL is
refactored to emit a portable query IR with backend-specific
lowering stages. That refactor is the **v0.4 milestone**.

What **can** work earlier (v0.3.2):

- `SqliteStore::set_triples` — the triple schema is trivial enough
  to port (`entity_id BLOB`, `attribute TEXT`, `value TEXT`,
  `value_type INT`, `tx_id INT`, `created_at TEXT`, `retracted INT`,
  `expires_at TEXT`). UNNEST becomes a prepared `INSERT` with a
  multi-row `VALUES` clause.
- `SqliteStore::get_entity` — single `SELECT ... WHERE entity_id = ?`.
- `SqliteStore::retract` — single `UPDATE`.
- `SqliteStore::next_tx_id` — SQLite's `INTEGER PRIMARY KEY AUTOINCREMENT`
  on a dedicated `darshan_tx_seq` table, or a rowid-based sequence.
- `SqliteStore::get_schema` — schema inference from triple data (the
  trick is grouping, which SQLite handles fine).

The v0.3.2 milestone targets exactly this subset.

---

## Future (v0.3.2+) — backend roadmap

| Backend | Target version | Use case |
|---|---|---|
| **Postgres** (default) | v0.3.1 (shipped) | Production, HA, analytics |
| **SQLite** | v0.3.2 | Single-binary zero-dep mode, edge deployments, offline dev |
| **In-memory** | v0.3.2 | Unit tests, integration tests without Docker |
| **DuckDB** | v0.3.3 | OLAP workloads, columnar scans, embedded analytics |
| **SurrealDB** | v0.4.0 | True distributed (Raft-backed) multi-region deployments |
| **FoundationDB** | v0.4.0 | Global-scale, strict serializability, massive key space |

Cross-backend DarshanQL (portable IR + lowering) is the v0.4 gate
that unblocks query on non-Postgres backends.

---

## Why not just use SurrealDB?

SurrealDB is an excellent database and we considered depending on it
directly. We chose to write the triple store from scratch because:

1. **Storage engine agnosticism**. SurrealDB is tied to its own KV
   engine choices (RocksDB, TiKV, FoundationDB). DarshJDB's triple
   store is an *abstraction over* any KV or relational engine.
2. **Tiered memory for agents**. Our `packages/agent-memory` crate
   needs hooks deep inside the triple layer for embedding updates,
   episodic compaction, and boosting — surface-level DB APIs don't
   expose the primitives.
3. **Reactive tracker**. `pg_notify`-driven live queries are
   per-triple and per-plan. Implementing this over SurrealDB's
   change feeds would be slower than going direct.

SurrealDB remains on the roadmap as a possible **backend** — not a
dependency — once the `Store` trait's query path is portable.

---

## Testing the trait boundary

```bash
# Default: just Postgres.
cargo check -p ddb-server

# Prove the trait is real by compiling against SQLite too.
cargo check -p ddb-server --features sqlite-store

# Combined with other feature flags.
cargo check -p ddb-server --features sqlite-store,embedded-db
```

If you add a method to `Store` and forget to implement it in *either*
adapter, the second command fails loudly. That is the point.
