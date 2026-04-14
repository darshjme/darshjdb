# DarshanQL SQL Dialects

> Status: v0.3.2 draft. SQLite execution wiring lands alongside
> Agent 1's SqliteStore; this document describes the planner-side
> dialect abstraction already in main.

## Why dialects?

v0.3.1 shipped the DarshanQL planner
(`packages/server/src/query/mod.rs`) with PostgreSQL-specific SQL
baked into every code path: `to_jsonb(...)` wraps, `::uuid` casts,
`@>` JSONB containment, `#>> '{}'` JSON-text extraction,
`to_tsvector` / `plainto_tsquery`, and `pgvector`'s `<=>` cosine
operator. That is perfect for the production Postgres backend but
leaves the SQLite store with no way to run the same logical plan.

v0.3.2 introduces a `SqlDialect` trait
(`packages/server/src/query/dialect.rs`) that funnels every
dialect-specific SQL fragment through a small interface. The
planner is now generic over the dialect; Postgres is a concrete
`PgDialect` impl and SQLite is a concrete `SqliteDialect` impl.

## The `SqlDialect` trait

See `packages/server/src/query/dialect.rs` for the canonical
definition. The methods currently exposed:

| Method                                | Postgres spelling                                     | SQLite spelling                          |
| ------------------------------------- | ----------------------------------------------------- | ---------------------------------------- |
| `placeholder(idx)`                    | `$1`                                                  | `?1`                                     |
| `jsonb_param(idx, Text)`              | `to_jsonb($1::text)`                                  | `json_quote(?1)`                         |
| `jsonb_param(idx, Json)`              | `$1::jsonb`                                           | `?1`                                     |
| `compare_triple_value(col, op, p)`    | `col op p`                                            | `col op p`                               |
| `jsonb_contains(col, p)`              | `col @> p`                                            | `instr(col, p) > 0`                      |
| `text_ilike(col, p)`                  | `col #>> '{}' ILIKE p`                                | `col LIKE p`                             |
| `uuid_cast(p)`                        | `p::uuid`                                             | `p`                                      |
| `uuid_array_cast(p)`                  | `p::uuid[]`                                           | `p`                                      |
| `in_uuid_list(col, placeholders)`     | `col IN (p1, p2, …)`                                  | `col IN (p1, p2, …)`                     |
| `fulltext_match(col, p)`              | `to_tsvector('english', col #>> '{}') @@ plainto_tsquery('english', p)` | `col LIKE '%' \|\| p \|\| '%'` |
| `vector_literal(vec)`                 | `'[0.1,0.2,…]'::vector`                               | sentinel (unsupported)                   |
| `cosine_distance(col, lit)`           | `col.embedding <=> lit`                               | sentinel (unsupported)                   |
| `supports_vector()`                   | `true`                                                | `false`                                  |
| `now_expr()`                          | `NOW()`                                               | `datetime('now')`                        |
| `recursive_cte_keyword()`             | `WITH RECURSIVE`                                      | `WITH RECURSIVE`                         |

### What the SQLite dialect approximates

Two features have no drop-in SQLite equivalent and are approximated:

- **JSON containment (`@>`)** — approximated as `instr(col, p) > 0`
  on the JSON text. This is a lossy substring check that matches
  the planner's current `Contains` usage (scalar and small-fragment
  containment). A portable IR in v0.4 replaces this with a proper
  JSON semantic check.

- **Full-text search (`to_tsvector` / `plainto_tsquery`)** —
  approximated as a `LIKE '%term%'` substring match. SQLite's
  proper solution is FTS5, which requires a separate virtual
  table; wiring FTS5 through the triple store is tracked as a
  v0.3.x follow-up.

### What the SQLite dialect refuses

Vector similarity search has no native SQLite equivalent. The
planner gates vector emission on `SqlDialect::supports_vector()`:

- `plan_query_with_dialect`: semantic (`$semantic`) queries on
  SQLite silently skip the `embeddings` join and return the base
  entity rows. A warning is logged.
- `plan_hybrid_query_with_dialect`: hybrid (`$hybrid`) queries on
  SQLite return `DarshJError::InvalidQuery` with a message pointing
  at `$search` as the text-only alternative.

## Connection routing (v0.3.2 wire-up)

`main.rs` wiring is **not** part of this sprint — it lands in the
post-sprint merge that combines this branch with Agent 1's
SqliteStore. The intended shape is:

```rust
// packages/server/src/main.rs (post-merge)
let database_url = config.database.url.as_str();

let (store, dialect): (Arc<dyn Store>, Arc<dyn SqlDialect>) =
    if database_url.starts_with("sqlite:") {
        let sqlite_store = SqliteStore::connect(database_url).await?;
        (Arc::new(sqlite_store), Arc::new(SqliteDialect))
    } else if database_url.starts_with("postgres://")
        || database_url.starts_with("postgresql://")
    {
        let pg_store = PgStore::connect(database_url).await?;
        (Arc::new(pg_store), Arc::new(PgDialect))
    } else {
        anyhow::bail!(
            "unsupported DATABASE_URL prefix; expected sqlite: or postgres:"
        );
    };
```

The store and dialect are threaded together because they must
agree: a `PgDialect` plan is not executable on a SQLite connection
and vice versa. In practice the server holds a single pair for its
lifetime and passes it to every request handler.

### Plan cache considerations

`PlanCache` keys by AST shape, not dialect. That is fine because
each server instance uses exactly one dialect; mixing dialects in
the same cache would produce false hits with the wrong SQL. The
post-merge wiring should instantiate one `PlanCache` per dialect
(or just one, since the dialect is fixed per process).

## Testing

`packages/server/src/query/dialect.rs` contains 22 unit tests that
snapshot-assert every dialect method for both `PgDialect` and
`SqliteDialect`.

`packages/server/src/query/mod.rs` contains 13 planner-level parity
tests that feed the same `QueryAST` into both dialects and assert
the emitted SQL. Notable cases covered:

- WHERE operators (Eq, Neq, Gt/Gte/Lt/Lte, Contains, Like)
- string vs numeric parameter kinds
- Full-text search
- Semantic vector (PG only)
- Hybrid (SQLite returns `InvalidQuery`)
- ORDER BY correlated sub-selects
- Nested UUID-batch plans
- `plan_query()` default wrapper compatibility
- Plan cache round-trip with both dialects

Run them with:

```sh
cargo test -p ddb-server --lib query::dialect
cargo test -p ddb-server --lib query::tests::parity
```

## Roadmap

- **v0.3.2** — this document; planner refactor complete. Wiring
  and runtime routing land with Agent 1's SqliteStore merge.
- **v0.3.3** — SQLite FTS5 virtual-table backend for
  `fulltext_match`; replaces the LIKE fallback.
- **v0.4** — portable IR that lowers to both dialects from a
  single tree instead of string-concatenation. Removes the
  `@>` approximation via a proper JSON containment primitive.
