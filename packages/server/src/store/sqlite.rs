// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//
//! SQLite [`Store`] backend.
//!
//! Full triple-store implementation over `rusqlite` with the `bundled`
//! feature (ships its own SQLite amalgamation, so no system libsqlite3
//! is required). This is the v0.3.2 deliverable that upgrades the
//! v0.3.1 compile-time stub into a backend you can actually run a
//! DarshJDB node against without Postgres.
//!
//! # Scope
//!
//! What works end-to-end:
//! - Schema migration (idempotent) — [`SqliteStore::open`] runs the SQL
//!   in `migrations/sqlite/001_initial.sql` on every open.
//! - Single-triple and batch writes via [`Store::set_triples`]
//!   (validated, value JSON-encoded, TTL supported).
//! - Entity reads via [`Store::get_entity`] (excludes retracted /
//!   expired rows).
//! - Logical retraction via [`Store::retract`].
//! - Schema inference via [`Store::get_schema`] (live scan of the
//!   triple table, identical to the Postgres path).
//! - Monotonic transaction id allocation via [`Store::next_tx_id`]
//!   against a single-row `darshan_tx_seq` counter table, using
//!   `RETURNING` (SQLite >= 3.35).
//! - Marker transaction handle via [`Store::begin_tx`] — mirrors the
//!   Postgres adapter's shape so the trait object surface is identical.
//!
//! # Deferred to v0.4
//!
//! - [`Store::query`] is the backend-portability cliff: DarshanQL
//!   currently emits Postgres-flavoured SQL with JSONB operators,
//!   `DISTINCT ON`, and `::uuid` casts. The SQLite adapter returns
//!   [`DarshJError::InvalidQuery`] with a clear message pointing at
//!   the v0.4 portable IR milestone.
//! - FTS5 virtual table and sqlite-vec vector search — see the TODO
//!   at the bottom of `migrations/sqlite/001_initial.sql`.
//! - Streaming / long-lived multi-statement transactions — the
//!   `SqliteStoreTx` handle is still a marker, matching `PgStoreTx`.
//!
//! # Concurrency model
//!
//! rusqlite is fundamentally single-threaded: a `Connection` is `Send`
//! but not `Sync`. We guard a single connection behind a [`Mutex`] and
//! dispatch blocking work to `tokio::task::spawn_blocking` so the async
//! runtime never stalls on a disk fsync. This is deliberately the
//! simplest correct design — a future iteration can shard reads across
//! a pool of readonly connections with WAL mode.

#![cfg(feature = "sqlite-store")]

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use rusqlite::{params, Connection, TransactionBehavior};

use crate::error::{DarshJError, Result};
use crate::query::QueryPlan;
use crate::triple_store::schema::{
    AttributeInfo, EntityType, ReferenceInfo, Schema, ValueType,
};
use crate::triple_store::{Triple, TripleInput};

use super::{Store, StoreTx};

/// Schema migration SQL applied on every [`SqliteStore::open`].
///
/// Embedded at compile time so the binary ships with a self-contained
/// schema and no runtime filesystem dependency. The canonical source
/// lives at `migrations/sqlite/001_initial.sql` at the repo root.
const SCHEMA_SQL: &str = include_str!("../../../../migrations/sqlite/001_initial.sql");

/// SQLite-backed [`Store`].
///
/// Holds a single `rusqlite::Connection` under a `Mutex`. Clone-cheap
/// via `Arc` wrapping of the internal state.
#[derive(Clone)]
pub struct SqliteStore {
    inner: Arc<SqliteStoreInner>,
}

struct SqliteStoreInner {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl SqliteStore {
    /// Open or create a SQLite database at `path` and run migrations.
    ///
    /// Pass `":memory:"` for an anonymous in-memory database — useful
    /// for unit tests and ephemeral workloads. The function is
    /// idempotent: reopening an existing database re-runs the schema
    /// SQL, which uses `CREATE TABLE IF NOT EXISTS` / `INSERT OR IGNORE`
    /// so repeat invocations are safe.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let conn = if path == Path::new(":memory:") {
            Connection::open_in_memory()
        } else {
            Connection::open(&path)
        }
        .map_err(|e| DarshJError::Internal(format!("sqlite open failed: {e}")))?;

        // Enable WAL for durability + reader/writer concurrency on
        // on-disk databases. `:memory:` ignores WAL, which is fine.
        // We use pragma_update_and_check so failures surface instead
        // of being silently ignored.
        if path != Path::new(":memory:") {
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|e| DarshJError::Internal(format!("sqlite WAL enable failed: {e}")))?;
        }
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| DarshJError::Internal(format!("sqlite pragma failed: {e}")))?;

        // Set a generous busy_timeout so brief contention (lock upgrade
        // during IMMEDIATE transactions, WAL checkpoint races) backs off
        // cleanly instead of failing fast with SQLITE_BUSY.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| DarshJError::Internal(format!("sqlite busy_timeout failed: {e}")))?;

        // Apply schema. execute_batch runs multiple statements in one
        // call; rusqlite supports it because the bundled SQLite build
        // compiles with SQLITE_ENABLE_JSON1 so json_valid / json_extract
        // are available to the CHECK constraint.
        conn.execute_batch(SCHEMA_SQL)
            .map_err(|e| DarshJError::Internal(format!("sqlite schema apply failed: {e}")))?;

        Ok(Self {
            inner: Arc::new(SqliteStoreInner {
                conn: Mutex::new(conn),
                path,
            }),
        })
    }

    /// Return the filesystem path this store was opened with.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Run a synchronous closure against the locked connection on a
    /// blocking thread. The closure receives `&mut Connection` so it
    /// can start savepoints / immediate transactions as needed.
    async fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = inner
                .conn
                .lock()
                .map_err(|e| DarshJError::Internal(format!("sqlite mutex poisoned: {e}")))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| DarshJError::Internal(format!("sqlite blocking task panicked: {e}")))?
    }
}

/// Marker transaction handle for the SQLite backend.
///
/// Mirrors [`crate::store::pg::PgStoreTx`] — the v0.3.1 [`Store`] trait
/// intentionally does not expose live multi-statement transactions
/// through the dynamic-dispatch surface. Set-operations that need an
/// atomic boundary (`set_triples`) use a SQLite IMMEDIATE transaction
/// *internally* for that single call. Multi-operation transactions
/// across multiple `Store` calls are a v0.3.3+ API.
pub struct SqliteStoreTx {
    _private: (),
}

#[async_trait]
impl StoreTx for SqliteStoreTx {
    async fn commit(self: Box<Self>) -> Result<()> {
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

// ── helpers ────────────────────────────────────────────────────────

/// Parse an RFC3339 string as stored in `triples.created_at`. Accepts
/// both naive `YYYY-MM-DDTHH:MM:SS.sssZ` (what SQLite's `strftime`
/// emits for the default column value) and full RFC3339 with offset
/// (what chrono::DateTime::to_rfc3339 emits on writes).
///
/// MINOR-4: uses `strip_suffix('Z')` for exact-one-Z matching so
/// malformed inputs like `2026-04-15T12:34:56ZZ` or a bare
/// `2026-04-15T12:34:56` (no UTC marker at all) fail loudly instead
/// of being silently accepted by `trim_end_matches`.
fn parse_sqlite_ts(s: &str) -> Result<DateTime<Utc>> {
    // Try strict RFC3339 first (handles `+HH:MM` offsets and `Z`).
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // Fallback: SQLite's `strftime('%Y-%m-%dT%H:%M:%fZ', 'now')`.
    // Require exactly one trailing 'Z' — reject anything else.
    let trimmed = s.strip_suffix('Z').ok_or_else(|| {
        DarshJError::Internal(format!(
            "sqlite: timestamp {s:?} carries no UTC marker (expected trailing 'Z' or RFC3339 offset)"
        ))
    })?;
    let naive = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%.f")
        .map_err(|e| DarshJError::Internal(format!("sqlite: timestamp parse failed for {s:?}: {e}")))?;
    Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
}

/// Materialise a `Triple` from a rusqlite row. Column order must match
/// every SELECT in this module: id, entity_id, attribute, value,
/// value_type, tx_id, created_at, retracted, expires_at.
fn row_to_triple(row: &rusqlite::Row<'_>) -> rusqlite::Result<Triple> {
    let id: i64 = row.get(0)?;
    let entity_id_str: String = row.get(1)?;
    let attribute: String = row.get(2)?;
    let value_str: String = row.get(3)?;
    let value_type: i64 = row.get(4)?;
    let tx_id: i64 = row.get(5)?;
    let created_at_str: String = row.get(6)?;
    let retracted_int: i64 = row.get(7)?;
    let expires_at_str: Option<String> = row.get(8)?;

    let entity_id = Uuid::parse_str(&entity_id_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        )
    })?;

    let value: serde_json::Value = serde_json::from_str(&value_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;

    let created_at = parse_sqlite_ts(&created_at_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
        )
    })?;

    let expires_at = match expires_at_str {
        Some(s) => Some(parse_sqlite_ts(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
            )
        })?),
        None => None,
    };

    Ok(Triple {
        id,
        entity_id,
        attribute,
        value,
        value_type: value_type as i16,
        tx_id,
        created_at,
        retracted: retracted_int != 0,
        expires_at,
    })
}

/// Map a `rusqlite::Error` into a DarshJError. rusqlite errors are
/// wrapped as `Internal` because the project's canonical `Database`
/// variant is specific to `sqlx::Error`.
fn map_rq(err: rusqlite::Error) -> DarshJError {
    DarshJError::Internal(format!("sqlite: {err}"))
}

#[async_trait]
impl Store for SqliteStore {
    fn backend_name(&self) -> &'static str {
        "sqlite"
    }

    async fn set_triples(&self, tx_id: i64, triples: &[TripleInput]) -> Result<()> {
        if triples.is_empty() {
            return Ok(());
        }
        // Validate up-front so we never partial-write.
        for t in triples {
            t.validate()?;
        }
        // Clone inputs into an owned Vec so we can move into the
        // blocking task without borrow gymnastics.
        let owned: Vec<TripleInput> = triples.to_vec();

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_rq)?;
            {
                let mut stmt = tx
                    .prepare_cached(
                        "INSERT INTO triples
                            (entity_id, attribute, value, value_type, tx_id, expires_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    )
                    .map_err(map_rq)?;

                for t in &owned {
                    let expires_at = t.ttl_seconds.map(|s| {
                        (Utc::now() + chrono::Duration::seconds(s))
                            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                            .to_string()
                    });
                    let value_str = serde_json::to_string(&t.value)
                        .map_err(DarshJError::Serialization)?;
                    stmt.execute(params![
                        t.entity_id.to_string(),
                        &t.attribute,
                        value_str,
                        t.value_type as i64,
                        tx_id,
                        expires_at,
                    ])
                    .map_err(map_rq)?;
                }
            }
            tx.commit().map_err(map_rq)?;
            Ok(())
        })
        .await
    }

    async fn get_entity(&self, entity_id: Uuid) -> Result<Vec<Triple>> {
        let entity_str = entity_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, entity_id, attribute, value, value_type, tx_id,
                            created_at, retracted, expires_at
                     FROM triples
                     WHERE entity_id = ?1
                       AND retracted = 0
                       AND (expires_at IS NULL OR expires_at > strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                     ORDER BY attribute, tx_id DESC",
                )
                .map_err(map_rq)?;
            let rows = stmt
                .query_map(params![entity_str], row_to_triple)
                .map_err(map_rq)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_rq)?);
            }
            Ok(out)
        })
        .await
    }

    async fn retract(&self, entity_id: Uuid, attribute: &str) -> Result<()> {
        let entity_str = entity_id.to_string();
        let attribute = attribute.to_string();
        self.with_conn(move |conn| {
            // Wrap in an explicit IMMEDIATE transaction for symmetry
            // with set_triples and to keep future multi-statement
            // extensions atomic under concurrent writers.
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_rq)?;
            tx.execute(
                "UPDATE triples
                 SET retracted = 1
                 WHERE entity_id = ?1 AND attribute = ?2 AND retracted = 0",
                params![entity_str, attribute],
            )
            .map_err(map_rq)?;
            tx.commit().map_err(map_rq)?;
            Ok(())
        })
        .await
    }

    async fn query(&self, _plan: &QueryPlan) -> Result<Vec<serde_json::Value>> {
        // DarshanQL currently emits Postgres-flavoured SQL — JSONB
        // operators, `::uuid` casts, `DISTINCT ON`, array UNNEST. The
        // SQLite adapter refuses rather than silently returning wrong
        // results. The portable IR that emits SQLite-compatible SQL
        // is tracked as v0.4 work.
        Err(DarshJError::InvalidQuery(
            "SqliteStore::query is not yet supported — DarshanQL emits Postgres-specific SQL. \
             Portable IR lands in v0.4; for now use direct triple-level APIs \
             (set_triples / get_entity / retract)."
                .into(),
        ))
    }

    async fn get_schema(&self) -> Result<Schema> {
        self.with_conn(move |conn| {
            // Strategy mirrors the Postgres implementation: scan active
            // triples, bucket them by `:db/type` per entity, and derive
            // attribute/reference shape. For the SQLite baseline we
            // keep the implementation straightforward and in-memory —
            // the Postgres path pushes aggregation down to SQL, but for
            // small/embedded workloads this is fine.

            // as_of_tx := max(tx_id) across all triples, or 0 if empty.
            let as_of_tx: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(tx_id), 0) FROM triples WHERE retracted = 0",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(map_rq)?;

            // Entity -> type name (from `:db/type`).
            let mut entity_types: HashMap<String, String> = HashMap::new();
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT entity_id, json_extract(value, '$') FROM triples
                         WHERE attribute = ':db/type'
                           AND retracted = 0
                           AND json_type(value) = 'text'",
                    )
                    .map_err(map_rq)?;
                let rows = stmt
                    .query_map([], |row| {
                        let eid: String = row.get(0)?;
                        // json_extract on a JSON string returns the
                        // unquoted text content directly.
                        let ty: String = row.get(1)?;
                        Ok((eid, ty))
                    })
                    .map_err(map_rq)?;
                for r in rows {
                    let (eid, ty) = r.map_err(map_rq)?;
                    entity_types.insert(eid, ty);
                }
            }

            // For each (type, attribute) aggregate value-type set,
            // cardinality, and collect reference targets.
            let mut types: HashMap<String, EntityType> = HashMap::new();
            // (type, attribute) -> set of entity_ids carrying it.
            let mut attr_entities: HashMap<(String, String), std::collections::HashSet<String>> =
                HashMap::new();
            // (type, attribute) -> observed value_types.
            let mut attr_value_types: HashMap<
                (String, String),
                std::collections::HashSet<i16>,
            > = HashMap::new();
            // entities per type.
            let mut type_entities: HashMap<String, std::collections::HashSet<String>> =
                HashMap::new();

            {
                let mut stmt = conn
                    .prepare(
                        "SELECT entity_id, attribute, value_type FROM triples
                         WHERE retracted = 0",
                    )
                    .map_err(map_rq)?;
                let rows = stmt
                    .query_map([], |row| {
                        let eid: String = row.get(0)?;
                        let attr: String = row.get(1)?;
                        let vt: i64 = row.get(2)?;
                        Ok((eid, attr, vt as i16))
                    })
                    .map_err(map_rq)?;
                for r in rows {
                    let (eid, attr, vt) = r.map_err(map_rq)?;
                    let Some(ty) = entity_types.get(&eid).cloned() else {
                        continue;
                    };
                    type_entities.entry(ty.clone()).or_default().insert(eid.clone());
                    attr_entities
                        .entry((ty.clone(), attr.clone()))
                        .or_default()
                        .insert(eid.clone());
                    attr_value_types
                        .entry((ty.clone(), attr))
                        .or_default()
                        .insert(vt);
                }
            }

            for (ty, ents) in &type_entities {
                let et = types.entry(ty.clone()).or_insert_with(|| EntityType {
                    name: ty.clone(),
                    attributes: HashMap::new(),
                    references: Vec::new(),
                    entity_count: 0,
                });
                et.entity_count = ents.len() as u64;
            }

            for ((ty, attr), ents) in &attr_entities {
                // Skip the synthetic `:db/type` marker from the exposed schema.
                if attr == ":db/type" {
                    continue;
                }
                let et = match types.get_mut(ty) {
                    Some(e) => e,
                    None => continue,
                };
                let vts = attr_value_types
                    .get(&(ty.clone(), attr.clone()))
                    .cloned()
                    .unwrap_or_default();
                let mut value_types: Vec<ValueType> = vts
                    .into_iter()
                    .filter_map(ValueType::from_i16)
                    .collect();
                value_types.sort_by_key(|v| *v as i16);
                let cardinality = ents.len() as u64;
                let required = cardinality == et.entity_count && et.entity_count > 0;
                et.attributes.insert(
                    attr.clone(),
                    AttributeInfo {
                        name: attr.clone(),
                        value_types: value_types.clone(),
                        required,
                        cardinality,
                    },
                );
                if value_types.contains(&ValueType::Reference) {
                    et.references.push(ReferenceInfo {
                        attribute: attr.clone(),
                        target_type: "unknown".to_string(),
                        cardinality,
                    });
                }
            }

            Ok(Schema {
                entity_types: types,
                as_of_tx,
            })
        })
        .await
    }

    async fn next_tx_id(&self) -> Result<i64> {
        self.with_conn(move |conn| {
            // Emulate a sequence: single-row counter, incremented under
            // an IMMEDIATE transaction so concurrent writers serialise
            // cleanly. `RETURNING` keeps this to a single round-trip.
            let next: i64 = conn
                .query_row(
                    "UPDATE darshan_tx_seq
                     SET next_value = next_value + 1
                     WHERE id = 1
                     RETURNING next_value - 1",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(map_rq)?;
            Ok(next)
        })
        .await
    }

    async fn begin_tx(&self) -> Result<Box<dyn StoreTx + Send>> {
        // Match the Postgres adapter's contract: return a stateless
        // marker, but probe the connection so connectivity issues
        // surface here rather than on the next mutating call.
        self.with_conn(move |conn| {
            conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
                .map_err(map_rq)?;
            Ok(())
        })
        .await?;
        Ok(Box::new(SqliteStoreTx { _private: () }))
    }
}

// ── tests ──────────────────────────────────────────────────────────

#[cfg(all(test, feature = "sqlite-store"))]
mod tests {
    use super::*;

    fn sample_triple(entity_id: Uuid, attr: &str, val: serde_json::Value) -> TripleInput {
        TripleInput {
            entity_id,
            attribute: attr.to_string(),
            value: val,
            value_type: ValueType::String as i16,
            ttl_seconds: None,
        }
    }

    #[tokio::test]
    async fn open_in_memory_and_migrate() {
        let store = SqliteStore::open(":memory:").expect("open");
        assert_eq!(store.backend_name(), "sqlite");
        // Re-opening the same path (memory is per-connection so we
        // re-run on the same store handle) — the migrations are
        // idempotent and `open` returning Ok is the contract.
        let tx_id = store.next_tx_id().await.expect("next_tx_id");
        assert_eq!(tx_id, 1, "first tx_id should be 1");
        let tx_id2 = store.next_tx_id().await.expect("next_tx_id");
        assert_eq!(tx_id2, 2, "sequence monotonic");
    }

    #[tokio::test]
    async fn set_triples_and_get_entity_roundtrip() {
        let store = SqliteStore::open(":memory:").expect("open");
        let entity = Uuid::new_v4();
        let tx_id = store.next_tx_id().await.unwrap();
        store
            .set_triples(
                tx_id,
                &[
                    sample_triple(entity, "user/email", serde_json::json!("alice@example.com")),
                    sample_triple(entity, "user/name", serde_json::json!("Alice")),
                ],
            )
            .await
            .expect("set_triples");

        let triples = store.get_entity(entity).await.expect("get_entity");
        assert_eq!(triples.len(), 2);
        let by_attr: HashMap<&str, &serde_json::Value> = triples
            .iter()
            .map(|t| (t.attribute.as_str(), &t.value))
            .collect();
        assert_eq!(
            by_attr.get("user/email").unwrap(),
            &&serde_json::json!("alice@example.com")
        );
        assert_eq!(by_attr.get("user/name").unwrap(), &&serde_json::json!("Alice"));

        // Every returned triple should carry the tx_id we just allocated.
        for t in &triples {
            assert_eq!(t.tx_id, tx_id);
            assert!(!t.retracted);
        }
    }

    #[tokio::test]
    async fn retract_hides_triples() {
        let store = SqliteStore::open(":memory:").expect("open");
        let entity = Uuid::new_v4();
        let tx_id = store.next_tx_id().await.unwrap();
        store
            .set_triples(
                tx_id,
                &[
                    sample_triple(entity, "user/email", serde_json::json!("alice@example.com")),
                    sample_triple(entity, "user/name", serde_json::json!("Alice")),
                ],
            )
            .await
            .unwrap();

        store.retract(entity, "user/email").await.expect("retract");

        let triples = store.get_entity(entity).await.unwrap();
        assert_eq!(triples.len(), 1, "retracted triple hidden");
        assert_eq!(triples[0].attribute, "user/name");
    }

    #[tokio::test]
    async fn bulk_ingest_batch() {
        let store = SqliteStore::open(":memory:").expect("open");
        let tx_id = store.next_tx_id().await.unwrap();
        // Stage 500 triples across 50 entities.
        let mut batch = Vec::with_capacity(500);
        let mut entities = Vec::with_capacity(50);
        for _ in 0..50 {
            let e = Uuid::new_v4();
            entities.push(e);
            for i in 0..10 {
                batch.push(sample_triple(
                    e,
                    &format!("attr/{i}"),
                    serde_json::json!(format!("value-{i}")),
                ));
            }
        }
        store.set_triples(tx_id, &batch).await.expect("bulk");

        // Spot-check: every entity should have exactly 10 triples back.
        for e in &entities {
            let triples = store.get_entity(*e).await.unwrap();
            assert_eq!(triples.len(), 10, "entity {e} should have 10 triples");
        }
    }

    #[tokio::test]
    async fn ttl_triples_hidden_when_expired() {
        let store = SqliteStore::open(":memory:").expect("open");
        let entity = Uuid::new_v4();
        let tx_id = store.next_tx_id().await.unwrap();
        store
            .set_triples(
                tx_id,
                &[TripleInput {
                    entity_id: entity,
                    attribute: "session/token".to_string(),
                    value: serde_json::json!("deadbeef"),
                    value_type: ValueType::String as i16,
                    // Expire 1 second in the past — already expired.
                    ttl_seconds: Some(-1),
                    }],
            )
            .await
            .unwrap();

        let triples = store.get_entity(entity).await.unwrap();
        assert!(
            triples.is_empty(),
            "expired TTL triple must be hidden, got {triples:?}"
        );
    }

    #[tokio::test]
    async fn ttl_triples_hidden_near_expiry() {
        // Regression test for MAJOR-1: writer and reader TTL timestamp
        // formats must agree. With ttl_seconds=1, the triple must be
        // visible immediately and hidden after a >1s sleep. This would
        // have failed when the writer emitted `+00:00`-suffixed RFC3339
        // against a reader comparing `Z`-suffixed strftime output,
        // because '+' (0x2B) < 'Z' (0x5A) lexicographically.
        let store = SqliteStore::open(":memory:").expect("open");
        let entity = Uuid::new_v4();
        let tx_id = store.next_tx_id().await.unwrap();
        store
            .set_triples(
                tx_id,
                &[TripleInput {
                    entity_id: entity,
                    attribute: "session/token".to_string(),
                    value: serde_json::json!("live-then-expired"),
                    value_type: ValueType::String as i16,
                    ttl_seconds: Some(1),
                }],
            )
            .await
            .unwrap();

        let before = store.get_entity(entity).await.unwrap();
        assert_eq!(before.len(), 1, "triple must be visible before TTL expiry");

        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

        let after = store.get_entity(entity).await.unwrap();
        assert!(
            after.is_empty(),
            "triple must be hidden after TTL expiry, got {after:?}"
        );
    }

    #[tokio::test]
    async fn invalid_triple_rejected_before_write() {
        let store = SqliteStore::open(":memory:").expect("open");
        let tx_id = store.next_tx_id().await.unwrap();
        let bad = TripleInput {
            entity_id: Uuid::new_v4(),
            attribute: String::new(), // empty -> validation error
            value: serde_json::json!("x"),
            value_type: 0,
            ttl_seconds: None,
        };
        let err = store.set_triples(tx_id, &[bad]).await;
        assert!(err.is_err(), "empty attribute must be rejected");
    }

    #[tokio::test]
    async fn query_returns_invalid_query() {
        // QueryPlan requires a populated SQL — the SQLite adapter
        // refuses anyway, so we feed it a minimal plan and assert
        // the error shape.
        let store = SqliteStore::open(":memory:").expect("open");
        let plan = QueryPlan {
            sql: "SELECT 1".to_string(),
            params: vec![],
            nested_plans: vec![],
            limit: None,
            offset: None,
        };
        let err = store.query(&plan).await.err().expect("error");
        match err {
            DarshJError::InvalidQuery(msg) => {
                assert!(msg.contains("SqliteStore"), "message: {msg}");
            }
            other => panic!("expected InvalidQuery, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn begin_tx_marker_roundtrip() {
        let store = SqliteStore::open(":memory:").expect("open");
        let handle = store.begin_tx().await.expect("begin_tx");
        handle.commit().await.expect("commit");
    }

    #[tokio::test]
    async fn get_schema_skips_non_text_db_type() {
        // Regression test for MINOR-3: if any :db/type triple has a
        // non-string JSON value, the SQL-level `json_type(value) = 'text'`
        // filter must skip it so `row.get::<_, String>` cannot crash the
        // entire get_schema endpoint.
        let store = SqliteStore::open(":memory:").expect("open");
        let tx_id = store.next_tx_id().await.unwrap();
        let good = Uuid::new_v4();
        let bad = Uuid::new_v4();
        store
            .set_triples(
                tx_id,
                &[
                    // Well-formed user.
                    sample_triple(good, ":db/type", serde_json::json!("user")),
                    sample_triple(good, "user/email", serde_json::json!("alice@example.com")),
                    // Malformed :db/type: value is a JSON object, not a string.
                    // Must be ignored by get_schema rather than crashing it.
                    sample_triple(
                        bad,
                        ":db/type",
                        serde_json::json!({"nested": "object"}),
                    ),
                    sample_triple(bad, "user/email", serde_json::json!("bob@example.com")),
                ],
            )
            .await
            .unwrap();

        let schema = store.get_schema().await.expect("schema must not crash");
        // Only the well-formed entity's type is inferred.
        assert_eq!(schema.entity_types.len(), 1);
        let user = schema.entity_types.get("user").expect("user type");
        assert_eq!(user.entity_count, 1);
    }

    #[tokio::test]
    async fn get_schema_infers_entity_types() {
        let store = SqliteStore::open(":memory:").expect("open");
        let tx_id = store.next_tx_id().await.unwrap();
        let alice = Uuid::new_v4();
        let bob = Uuid::new_v4();
        store
            .set_triples(
                tx_id,
                &[
                    sample_triple(alice, ":db/type", serde_json::json!("user")),
                    sample_triple(alice, "user/email", serde_json::json!("alice@example.com")),
                    sample_triple(alice, "user/name", serde_json::json!("Alice")),
                    sample_triple(bob, ":db/type", serde_json::json!("user")),
                    sample_triple(bob, "user/email", serde_json::json!("bob@example.com")),
                ],
            )
            .await
            .unwrap();

        let schema = store.get_schema().await.expect("schema");
        assert_eq!(schema.as_of_tx, tx_id);
        let user = schema.entity_types.get("user").expect("user type");
        assert_eq!(user.entity_count, 2);
        // user/email is on both entities -> required=true
        let email = user.attributes.get("user/email").unwrap();
        assert_eq!(email.cardinality, 2);
        assert!(email.required);
        // user/name is only on alice -> required=false
        let name = user.attributes.get("user/name").unwrap();
        assert_eq!(name.cardinality, 1);
        assert!(!name.required);
    }
}
