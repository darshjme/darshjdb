//! Core triple store: the foundational storage layer for DarshanDB.
//!
//! Every piece of data is stored as an (entity, attribute, value) triple
//! tagged with a transaction id and a value-type discriminator. This
//! module defines the [`Triple`] type, the [`TripleStore`] trait, and
//! a production Postgres implementation ([`PgTripleStore`]).

pub mod schema;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

use crate::error::{DarshanError, Result};
use schema::{AttributeInfo, EntityType, ReferenceInfo, Schema, ValueType};

// ── Bulk load result ───────────────────────────────────────────────

/// Outcome of a [`PgTripleStore::bulk_load`] operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkLoadResult {
    /// Number of triples written.
    pub triples_loaded: usize,
    /// Transaction id assigned to the batch.
    pub tx_id: i64,
    /// Wall-clock duration of the UNNEST bulk insert in milliseconds.
    pub duration_ms: u64,
    /// Sustained throughput (triples per second).
    pub rate_per_sec: f64,
}

// ── Triple ──────────────────────────────────────────────────────────

/// A single fact in the triple store.
///
/// Triples are append-only; "deletion" is expressed by setting
/// [`Triple::retracted`] to `true` in a later transaction.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Triple {
    /// Auto-generated primary key.
    pub id: i64,
    /// The entity this fact belongs to.
    pub entity_id: Uuid,
    /// Attribute name (e.g. `"user/email"`).
    pub attribute: String,
    /// The value, encoded as JSON.
    pub value: serde_json::Value,
    /// Discriminator tag — see [`ValueType`].
    pub value_type: i16,
    /// Monotonically increasing transaction identifier.
    pub tx_id: i64,
    /// When this triple was written.
    pub created_at: DateTime<Utc>,
    /// Whether the triple has been logically retracted.
    pub retracted: bool,
    /// Optional expiry timestamp for TTL support. When set, the triple
    /// will be automatically retracted after this time.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Input for writing a new triple (before assignment of id / tx / timestamp).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TripleInput {
    /// Target entity.
    pub entity_id: Uuid,
    /// Attribute name.
    pub attribute: String,
    /// Value payload.
    pub value: serde_json::Value,
    /// Value type tag.
    pub value_type: i16,
    /// Optional TTL in seconds. When set, `expires_at` will be computed
    /// as `NOW() + interval` on insert.
    #[serde(default)]
    pub ttl_seconds: Option<i64>,
}

impl TripleInput {
    /// Validate this input, returning a descriptive error if anything is wrong.
    ///
    /// Checks:
    /// - `attribute` is non-empty and does not exceed 512 bytes
    /// - `value_type` is a known [`ValueType`] discriminator
    pub fn validate(&self) -> Result<()> {
        if self.attribute.is_empty() {
            return Err(DarshanError::InvalidAttribute(
                "attribute name must not be empty".into(),
            ));
        }
        if self.attribute.len() > 512 {
            return Err(DarshanError::InvalidAttribute(format!(
                "attribute name exceeds 512 bytes: {} bytes",
                self.attribute.len()
            )));
        }
        if ValueType::from_i16(self.value_type).is_none() {
            return Err(DarshanError::TypeMismatch {
                expected: format!("value_type 0..={}", ValueType::max_discriminator()),
                actual: format!("{}", self.value_type),
            });
        }
        Ok(())
    }
}

// ── Trait ────────────────────────────────────────────────────────────

/// Async interface over the triple store, allowing alternative backends
/// (e.g. in-memory for tests) without touching business logic.
///
/// All methods return [`Result`] and are object-safe with `Send` futures
/// so the trait can be used behind `Arc<dyn TripleStore>`.
pub trait TripleStore: Send + Sync {
    /// Retrieve all active (non-retracted) triples for an entity.
    fn get_entity(
        &self,
        entity_id: Uuid,
    ) -> impl std::future::Future<Output = Result<Vec<Triple>>> + Send;

    /// Retrieve all active triples for an entity with a specific attribute.
    fn get_attribute(
        &self,
        entity_id: Uuid,
        attribute: &str,
    ) -> impl std::future::Future<Output = Result<Vec<Triple>>> + Send;

    /// Atomically write a batch of triples under a single transaction id.
    fn set_triples(
        &self,
        triples: &[TripleInput],
    ) -> impl std::future::Future<Output = Result<i64>> + Send;

    /// Retract (soft-delete) all active triples matching the entity + attribute.
    fn retract(
        &self,
        entity_id: Uuid,
        attribute: &str,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Find triples by attribute name, optionally filtering on value.
    fn query_by_attribute(
        &self,
        attribute: &str,
        value: Option<&serde_json::Value>,
    ) -> impl std::future::Future<Output = Result<Vec<Triple>>> + Send;

    /// Infer the current schema by scanning triple data.
    fn get_schema(&self) -> impl std::future::Future<Output = Result<Schema>> + Send;

    /// Point-in-time read: return the entity's triples as they were at `tx_id`.
    fn get_entity_at(
        &self,
        entity_id: Uuid,
        tx_id: i64,
    ) -> impl std::future::Future<Output = Result<Vec<Triple>>> + Send;
}

// ── Postgres implementation ─────────────────────────────────────────

/// Production triple store backed by PostgreSQL via `sqlx`.
#[derive(Clone)]
pub struct PgTripleStore {
    pool: PgPool,
}

impl PgTripleStore {
    /// Create a new store and ensure the schema (table + indexes) exists.
    pub async fn new(pool: PgPool) -> Result<Self> {
        let store = Self { pool };
        store.ensure_schema().await?;
        Ok(store)
    }

    /// Return a reference to the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Create a store without running schema migrations.
    /// Useful for tests or when the schema is known to exist already.
    pub fn new_lazy(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create the `triples` table and all supporting indexes if they
    /// do not already exist. This is idempotent.
    async fn ensure_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS triples (
                id          BIGSERIAL PRIMARY KEY,
                entity_id   UUID        NOT NULL,
                attribute   TEXT        NOT NULL,
                value       JSONB       NOT NULL,
                value_type  SMALLINT    NOT NULL DEFAULT 0,
                tx_id       BIGINT      NOT NULL,
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                retracted   BOOLEAN     NOT NULL DEFAULT false,
                expires_at  TIMESTAMPTZ
            );

            -- Composite index for entity lookups filtered by attribute.
            CREATE INDEX IF NOT EXISTS idx_triples_entity_attr
                ON triples (entity_id, attribute)
                WHERE NOT retracted;

            -- GIN index for value-based queries (contains, equality on JSONB).
            CREATE INDEX IF NOT EXISTS idx_triples_attr_value
                ON triples USING gin (attribute, value)
                WHERE NOT retracted;

            -- Transaction ordering.
            CREATE INDEX IF NOT EXISTS idx_triples_tx_id
                ON triples (tx_id);

            -- Covering index for point-in-time reads.
            CREATE INDEX IF NOT EXISTS idx_triples_entity_tx
                ON triples (entity_id, tx_id);

            -- Attribute scan for schema inference.
            CREATE INDEX IF NOT EXISTS idx_triples_attribute
                ON triples (attribute)
                WHERE NOT retracted;

            -- TTL expiry scan index: find expired triples efficiently.
            CREATE INDEX IF NOT EXISTS idx_triples_expires
                ON triples (expires_at)
                WHERE expires_at IS NOT NULL AND NOT retracted;

            -- Sequence for transaction ids.
            CREATE SEQUENCE IF NOT EXISTS darshan_tx_seq
                START WITH 1 INCREMENT BY 1;

            -- Add expires_at column to existing tables (idempotent migration).
            ALTER TABLE triples ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ;
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Allocate the next transaction id from the database sequence.
    async fn next_tx_id(&self) -> Result<i64> {
        let row: (i64,) = sqlx::query_as("SELECT nextval('darshan_tx_seq')")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    /// Begin a new database transaction.
    pub async fn begin_tx(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>> {
        Ok(self.pool.begin().await?)
    }

    /// Allocate the next transaction id within an existing transaction.
    pub async fn next_tx_id_in_tx(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<i64> {
        let row: (i64,) = sqlx::query_as("SELECT nextval('darshan_tx_seq')")
            .fetch_one(&mut **tx)
            .await?;
        Ok(row.0)
    }

    /// Write a batch of triples within an existing transaction.
    ///
    /// Unlike [`TripleStore::set_triples`], this does NOT commit — the
    /// caller owns the transaction and decides when to commit/rollback.
    pub async fn set_triples_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        triples: &[TripleInput],
        tx_id: i64,
    ) -> Result<()> {
        for t in triples {
            t.validate()?;
        }
        for t in triples {
            let expires_at = t
                .ttl_seconds
                .map(|secs| Utc::now() + chrono::Duration::seconds(secs));
            sqlx::query(
                r#"
                INSERT INTO triples (entity_id, attribute, value, value_type, tx_id, expires_at)
                VALUES ($1, $2, $3, $4, $5, $6)
                "#,
            )
            .bind(t.entity_id)
            .bind(&t.attribute)
            .bind(&t.value)
            .bind(t.value_type)
            .bind(tx_id)
            .bind(expires_at)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }

    /// Retract (soft-delete) triples within an existing transaction.
    pub async fn retract_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        entity_id: Uuid,
        attribute: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE triples
            SET retracted = true
            WHERE entity_id = $1 AND attribute = $2 AND NOT retracted
            "#,
        )
        .bind(entity_id)
        .bind(attribute)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Fetch active triples for an entity within an existing transaction.
    pub async fn get_entity_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        entity_id: Uuid,
    ) -> Result<Vec<Triple>> {
        let triples = sqlx::query_as::<_, Triple>(
            r#"
            SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted, expires_at
            FROM triples
            WHERE entity_id = $1 AND NOT retracted
            ORDER BY attribute, tx_id DESC
            "#,
        )
        .bind(entity_id)
        .fetch_all(&mut **tx)
        .await?;
        Ok(triples)
    }

    /// Load triples using PostgreSQL UNNEST-based bulk insert for maximum
    /// throughput. 10-50x faster than individual INSERT statements because
    /// the entire batch is sent as a single query with array parameters,
    /// eliminating per-row round-trip overhead and allowing Postgres to
    /// optimise its WAL writes.
    ///
    /// The method:
    /// 1. Validates every input up-front (fails fast, no partial writes).
    /// 2. Allocates a single transaction id for the whole batch.
    /// 3. Decomposes the `Vec<TripleInput>` into columnar arrays.
    /// 4. Executes one `INSERT INTO triples ... SELECT ... FROM UNNEST(...)`.
    /// 5. Returns a [`BulkLoadResult`] with count, tx_id, timing, and rate.
    pub async fn bulk_load(&self, triples: Vec<TripleInput>) -> Result<BulkLoadResult> {
        if triples.is_empty() {
            return Err(DarshanError::InvalidQuery(
                "cannot bulk-load an empty triple batch".into(),
            ));
        }

        // Validate all inputs before touching the database.
        for t in &triples {
            t.validate()?;
        }

        let count = triples.len();
        let start = Instant::now();

        // Allocate a single tx_id for the entire bulk load.
        let tx_id = self.next_tx_id().await?;

        // Decompose into columnar arrays for UNNEST.
        let mut entity_ids: Vec<Uuid> = Vec::with_capacity(count);
        let mut attributes: Vec<String> = Vec::with_capacity(count);
        let mut values: Vec<serde_json::Value> = Vec::with_capacity(count);
        let mut value_types: Vec<i16> = Vec::with_capacity(count);
        let mut expires_at_vec: Vec<Option<DateTime<Utc>>> = Vec::with_capacity(count);

        for t in triples {
            let exp = t
                .ttl_seconds
                .map(|secs| Utc::now() + chrono::Duration::seconds(secs));
            entity_ids.push(t.entity_id);
            attributes.push(t.attribute);
            values.push(t.value);
            value_types.push(t.value_type);
            expires_at_vec.push(exp);
        }

        // Single-query bulk insert using UNNEST — Postgres processes all
        // rows in one shot, dramatically reducing parse/plan/WAL overhead.
        sqlx::query(
            r#"
            INSERT INTO triples (entity_id, attribute, value, value_type, tx_id, expires_at)
            SELECT * FROM UNNEST($1::uuid[], $2::text[], $3::jsonb[], $4::smallint[], $5::bigint[], $6::timestamptz[])
            "#,
        )
        .bind(&entity_ids)
        .bind(&attributes)
        .bind(&values)
        .bind(&value_types)
        .bind(&vec![tx_id; count])
        .bind(&expires_at_vec)
        .execute(&self.pool)
        .await?;

        let elapsed = start.elapsed();
        let duration_ms = elapsed.as_millis() as u64;
        let rate_per_sec = if duration_ms > 0 {
            (count as f64) / (duration_ms as f64 / 1000.0)
        } else {
            count as f64 // sub-millisecond => report count as rate
        };

        Ok(BulkLoadResult {
            triples_loaded: count,
            tx_id,
            duration_ms,
            rate_per_sec,
        })
    }
}

impl TripleStore for PgTripleStore {
    async fn get_entity(&self, entity_id: Uuid) -> Result<Vec<Triple>> {
        let triples = sqlx::query_as::<_, Triple>(
            r#"
            SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted, expires_at
            FROM triples
            WHERE entity_id = $1 AND NOT retracted
            ORDER BY attribute, tx_id DESC
            "#,
        )
        .bind(entity_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(triples)
    }

    async fn get_attribute(&self, entity_id: Uuid, attribute: &str) -> Result<Vec<Triple>> {
        let triples = sqlx::query_as::<_, Triple>(
            r#"
            SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted, expires_at
            FROM triples
            WHERE entity_id = $1 AND attribute = $2 AND NOT retracted
            ORDER BY tx_id DESC
            "#,
        )
        .bind(entity_id)
        .bind(attribute)
        .fetch_all(&self.pool)
        .await?;

        Ok(triples)
    }

    async fn set_triples(&self, triples: &[TripleInput]) -> Result<i64> {
        if triples.is_empty() {
            return Err(DarshanError::InvalidQuery(
                "cannot write an empty triple batch".into(),
            ));
        }

        // Validate every input before touching the database.
        for t in triples {
            t.validate()?;
        }

        let tx_id = self.next_tx_id().await?;
        let mut tx = self.pool.begin().await?;

        for t in triples {
            let expires_at = t
                .ttl_seconds
                .map(|secs| Utc::now() + chrono::Duration::seconds(secs));
            sqlx::query(
                r#"
                INSERT INTO triples (entity_id, attribute, value, value_type, tx_id, expires_at)
                VALUES ($1, $2, $3, $4, $5, $6)
                "#,
            )
            .bind(t.entity_id)
            .bind(&t.attribute)
            .bind(&t.value)
            .bind(t.value_type)
            .bind(tx_id)
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(tx_id)
    }

    async fn retract(&self, entity_id: Uuid, attribute: &str) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE triples
            SET retracted = true
            WHERE entity_id = $1 AND attribute = $2 AND NOT retracted
            "#,
        )
        .bind(entity_id)
        .bind(attribute)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn query_by_attribute(
        &self,
        attribute: &str,
        value: Option<&serde_json::Value>,
    ) -> Result<Vec<Triple>> {
        let triples = match value {
            Some(v) => {
                sqlx::query_as::<_, Triple>(
                    r#"
                    SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted, expires_at
                    FROM triples
                    WHERE attribute = $1 AND value = $2 AND NOT retracted
                    ORDER BY tx_id DESC
                    "#,
                )
                .bind(attribute)
                .bind(v)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, Triple>(
                    r#"
                    SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted, expires_at
                    FROM triples
                    WHERE attribute = $1 AND NOT retracted
                    ORDER BY tx_id DESC
                    "#,
                )
                .bind(attribute)
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(triples)
    }

    async fn get_schema(&self) -> Result<Schema> {
        // Get the current max tx_id for the snapshot marker.
        let max_tx: (Option<i64>,) =
            sqlx::query_as("SELECT MAX(tx_id) FROM triples WHERE NOT retracted")
                .fetch_one(&self.pool)
                .await?;
        let as_of_tx = max_tx.0.unwrap_or(0);

        // Discover all (entity_id, attribute, value_type) tuples grouped
        // by the entity's `:db/type` attribute (if present).
        let rows: Vec<(Uuid, String, i16, Option<serde_json::Value>)> = sqlx::query_as(
            r#"
            WITH typed_entities AS (
                SELECT entity_id, value #>> '{}' AS entity_type
                FROM triples
                WHERE attribute = ':db/type' AND NOT retracted
            )
            SELECT t.entity_id, t.attribute, t.value_type,
                   te.entity_type::jsonb AS entity_type
            FROM triples t
            LEFT JOIN typed_entities te ON te.entity_id = t.entity_id
            WHERE NOT t.retracted
            ORDER BY te.entity_type, t.attribute
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut types_map: HashMap<String, EntityTypeBuilder> = HashMap::new();

        for (entity_id, attribute, value_type, entity_type_json) in &rows {
            let type_name = entity_type_json
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or("_untyped")
                .to_string();

            let builder = types_map
                .entry(type_name.clone())
                .or_insert_with(|| EntityTypeBuilder::new(type_name));
            builder.observe(*entity_id, attribute, *value_type);
        }

        let entity_types = types_map
            .into_iter()
            .map(|(name, b)| (name, b.build()))
            .collect();

        Ok(Schema {
            entity_types,
            as_of_tx,
        })
    }

    async fn get_entity_at(&self, entity_id: Uuid, tx_id: i64) -> Result<Vec<Triple>> {
        // For each attribute, get the latest triple at or before `tx_id`.
        let triples = sqlx::query_as::<_, Triple>(
            r#"
            SELECT DISTINCT ON (attribute)
                id, entity_id, attribute, value, value_type, tx_id, created_at, retracted, expires_at
            FROM triples
            WHERE entity_id = $1 AND tx_id <= $2
            ORDER BY attribute, tx_id DESC
            "#,
        )
        .bind(entity_id)
        .bind(tx_id)
        .fetch_all(&self.pool)
        .await?;

        // Filter out triples that were retracted as of that tx.
        let active: Vec<Triple> = triples.into_iter().filter(|t| !t.retracted).collect();
        Ok(active)
    }
}

// ── TTL / Expiry methods ──────────────────────────────────────────

impl PgTripleStore {
    /// Retract all triples whose `expires_at` has passed. Returns the
    /// list of distinct entity IDs that were expired so callers can emit
    /// change events for reactive subscriptions.
    pub async fn expire_triples(&self) -> Result<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            r#"
            UPDATE triples
            SET retracted = true
            WHERE expires_at IS NOT NULL
              AND expires_at < now()
              AND NOT retracted
            RETURNING DISTINCT entity_id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Set or update the TTL for all active triples belonging to an entity.
    /// A `ttl_seconds` value of `-1` removes the TTL (persists forever).
    pub async fn set_entity_ttl(&self, entity_id: Uuid, ttl_seconds: i64) -> Result<u64> {
        let expires_at: Option<DateTime<Utc>> = if ttl_seconds < 0 {
            None
        } else {
            Some(Utc::now() + chrono::Duration::seconds(ttl_seconds))
        };

        let result = sqlx::query(
            r#"
            UPDATE triples
            SET expires_at = $2
            WHERE entity_id = $1 AND NOT retracted
            "#,
        )
        .bind(entity_id)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Get the earliest `expires_at` for an entity (its effective TTL).
    pub async fn get_entity_ttl(&self, entity_id: Uuid) -> Result<Option<DateTime<Utc>>> {
        let row: (Option<DateTime<Utc>>,) = sqlx::query_as(
            r#"
            SELECT MIN(expires_at)
            FROM triples
            WHERE entity_id = $1 AND NOT retracted AND expires_at IS NOT NULL
            "#,
        )
        .bind(entity_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.0)
    }
}

// ── Entity Pool ────────────────────────────────────────────────────
//
// Maps external UUIDs to compact internal i64 IDs. This is the core
// of dictionary encoding from Ontotext GraphDB — all index lookups
// become integer comparisons instead of 16-byte UUID comparisons.
//
// Hot entries are cached in a lock-free DashMap so repeated lookups
// never touch Postgres.

/// Maps external UUIDs to internal integer IDs for fast index lookups.
#[derive(Clone)]
pub struct EntityPool {
    pool: PgPool,
    /// UUID -> internal_id cache (hot path, lock-free).
    fwd: Arc<DashMap<Uuid, i64>>,
    /// internal_id -> UUID reverse cache.
    rev: Arc<DashMap<i64, Uuid>>,
}

impl EntityPool {
    /// Create a new entity pool backed by the given Postgres connection.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            fwd: Arc::new(DashMap::new()),
            rev: Arc::new(DashMap::new()),
        }
    }

    /// Ensure the entity_pool table exists (idempotent).
    pub async fn ensure_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS entity_pool (
                internal_id BIGSERIAL PRIMARY KEY,
                external_id UUID NOT NULL UNIQUE
            );
            CREATE INDEX IF NOT EXISTS idx_entity_pool_external
                ON entity_pool (external_id);
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Return the internal id for a UUID, creating a new mapping if needed.
    ///
    /// Uses INSERT ... ON CONFLICT DO NOTHING + a follow-up SELECT to
    /// handle concurrent inserts without serialization errors.
    pub async fn get_or_create(&self, uuid: Uuid) -> Result<i64> {
        // Fast path: check cache first.
        if let Some(id) = self.fwd.get(&uuid) {
            return Ok(*id);
        }

        // Attempt insert; ignore conflict if another connection raced us.
        sqlx::query("INSERT INTO entity_pool (external_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(uuid)
            .execute(&self.pool)
            .await?;

        // Always SELECT — either we just inserted or the row existed.
        let row: (i64,) =
            sqlx::query_as("SELECT internal_id FROM entity_pool WHERE external_id = $1")
                .bind(uuid)
                .fetch_one(&self.pool)
                .await?;

        let internal_id = row.0;
        self.fwd.insert(uuid, internal_id);
        self.rev.insert(internal_id, uuid);
        Ok(internal_id)
    }

    /// Resolve an internal id back to its external UUID.
    pub async fn resolve(&self, internal_id: i64) -> Result<Uuid> {
        // Fast path: check reverse cache.
        if let Some(uuid) = self.rev.get(&internal_id) {
            return Ok(*uuid);
        }

        let row: (Uuid,) =
            sqlx::query_as("SELECT external_id FROM entity_pool WHERE internal_id = $1")
                .bind(internal_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| match e {
                    sqlx::Error::RowNotFound => DarshanError::Internal(format!(
                        "entity pool: no mapping for internal_id {internal_id}"
                    )),
                    other => DarshanError::Database(other),
                })?;

        let uuid = row.0;
        self.fwd.insert(uuid, internal_id);
        self.rev.insert(internal_id, uuid);
        Ok(uuid)
    }

    /// Batch-resolve a slice of UUIDs to internal ids, creating mappings
    /// for any that don't yet exist. Returns ids in the same order as input.
    pub async fn batch_get_or_create(&self, uuids: &[Uuid]) -> Result<Vec<i64>> {
        if uuids.is_empty() {
            return Ok(Vec::new());
        }

        // Separate cached hits from misses.
        let mut results = vec![0i64; uuids.len()];
        let mut miss_indices = Vec::new();
        let mut miss_uuids = Vec::new();

        for (i, uuid) in uuids.iter().enumerate() {
            if let Some(id) = self.fwd.get(uuid) {
                results[i] = *id;
            } else {
                miss_indices.push(i);
                miss_uuids.push(*uuid);
            }
        }

        if miss_uuids.is_empty() {
            return Ok(results);
        }

        // Bulk insert misses (ignore conflicts).
        let mut tx = self.pool.begin().await?;
        for uuid in &miss_uuids {
            sqlx::query("INSERT INTO entity_pool (external_id) VALUES ($1) ON CONFLICT DO NOTHING")
                .bind(uuid)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        // Bulk fetch all the ids we need.
        let rows: Vec<(Uuid, i64)> = sqlx::query_as(
            "SELECT external_id, internal_id FROM entity_pool WHERE external_id = ANY($1)",
        )
        .bind(&miss_uuids)
        .fetch_all(&self.pool)
        .await?;

        let fetched: HashMap<Uuid, i64> = rows.into_iter().collect();

        for (idx, uuid) in miss_indices.iter().zip(miss_uuids.iter()) {
            let internal_id = *fetched.get(uuid).ok_or_else(|| {
                DarshanError::Internal(format!(
                    "entity pool: batch insert succeeded but SELECT missed UUID {uuid}"
                ))
            })?;
            results[*idx] = internal_id;
            self.fwd.insert(*uuid, internal_id);
            self.rev.insert(internal_id, *uuid);
        }

        Ok(results)
    }

    /// Batch-resolve internal ids back to UUIDs. Returns UUIDs in the
    /// same order as input.
    pub async fn batch_resolve(&self, ids: &[i64]) -> Result<Vec<Uuid>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = vec![Uuid::nil(); ids.len()];
        let mut miss_indices = Vec::new();
        let mut miss_ids = Vec::new();

        for (i, id) in ids.iter().enumerate() {
            if let Some(uuid) = self.rev.get(id) {
                results[i] = *uuid;
            } else {
                miss_indices.push(i);
                miss_ids.push(*id);
            }
        }

        if miss_ids.is_empty() {
            return Ok(results);
        }

        let rows: Vec<(i64, Uuid)> = sqlx::query_as(
            "SELECT internal_id, external_id FROM entity_pool WHERE internal_id = ANY($1)",
        )
        .bind(&miss_ids)
        .fetch_all(&self.pool)
        .await?;

        let fetched: HashMap<i64, Uuid> = rows.into_iter().collect();

        for (idx, id) in miss_indices.iter().zip(miss_ids.iter()) {
            let uuid = *fetched.get(id).ok_or_else(|| {
                DarshanError::Internal(format!("entity pool: no mapping for internal_id {id}"))
            })?;
            results[*idx] = uuid;
            self.fwd.insert(uuid, *id);
            self.rev.insert(*id, uuid);
        }

        Ok(results)
    }
}

// ── Attribute Pool ─────────────────────────────────────────────────
//
// Maps attribute name strings (e.g. "user/email") to compact i32 IDs.
// Same dictionary-encoding technique but for the attribute dimension.

/// Maps attribute name strings to internal integer IDs.
#[derive(Clone)]
pub struct AttributePool {
    pool: PgPool,
    /// name -> internal_id cache.
    fwd: Arc<DashMap<String, i32>>,
    /// internal_id -> name reverse cache.
    rev: Arc<DashMap<i32, String>>,
}

impl AttributePool {
    /// Create a new attribute pool backed by the given Postgres connection.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            fwd: Arc::new(DashMap::new()),
            rev: Arc::new(DashMap::new()),
        }
    }

    /// Ensure the attribute_pool table exists (idempotent).
    pub async fn ensure_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS attribute_pool (
                internal_id SERIAL PRIMARY KEY,
                name TEXT NOT NULL UNIQUE
            );
            CREATE INDEX IF NOT EXISTS idx_attribute_pool_name
                ON attribute_pool (name);
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Return the internal id for an attribute name, creating a new mapping if needed.
    pub async fn get_or_create(&self, name: &str) -> Result<i32> {
        // Fast path: check cache.
        if let Some(id) = self.fwd.get(name) {
            return Ok(*id);
        }

        sqlx::query("INSERT INTO attribute_pool (name) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(name)
            .execute(&self.pool)
            .await?;

        let row: (i32,) = sqlx::query_as("SELECT internal_id FROM attribute_pool WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await?;

        let internal_id = row.0;
        self.fwd.insert(name.to_string(), internal_id);
        self.rev.insert(internal_id, name.to_string());
        Ok(internal_id)
    }

    /// Resolve an internal id back to its attribute name string.
    pub async fn resolve(&self, internal_id: i32) -> Result<String> {
        // Fast path: check reverse cache.
        if let Some(name) = self.rev.get(&internal_id) {
            return Ok(name.clone());
        }

        let row: (String,) =
            sqlx::query_as("SELECT name FROM attribute_pool WHERE internal_id = $1")
                .bind(internal_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| match e {
                    sqlx::Error::RowNotFound => DarshanError::Internal(format!(
                        "attribute pool: no mapping for internal_id {internal_id}"
                    )),
                    other => DarshanError::Database(other),
                })?;

        let name = row.0;
        self.fwd.insert(name.clone(), internal_id);
        self.rev.insert(internal_id, name.clone());
        Ok(name)
    }
}

// ── Pool accessors on PgTripleStore ────────────────────────────────

impl PgTripleStore {
    /// Create an [`EntityPool`] sharing this store's connection pool.
    pub fn entity_pool(&self) -> EntityPool {
        EntityPool::new(self.pool.clone())
    }

    /// Create an [`AttributePool`] sharing this store's connection pool.
    pub fn attribute_pool(&self) -> AttributePool {
        AttributePool::new(self.pool.clone())
    }
}

// ── Schema inference helpers ────────────────────────────────────────

/// Accumulates observations about a single entity type during schema scan.
struct EntityTypeBuilder {
    name: String,
    /// attribute -> set of (entity_id, value_type) observations
    attrs: HashMap<String, Vec<(Uuid, i16)>>,
    entities: std::collections::HashSet<Uuid>,
}

impl EntityTypeBuilder {
    fn new(name: String) -> Self {
        Self {
            name,
            attrs: HashMap::new(),
            entities: std::collections::HashSet::new(),
        }
    }

    fn observe(&mut self, entity_id: Uuid, attribute: &str, value_type: i16) {
        self.entities.insert(entity_id);
        self.attrs
            .entry(attribute.to_string())
            .or_default()
            .push((entity_id, value_type));
    }

    fn build(self) -> EntityType {
        let entity_count = self.entities.len() as u64;
        let mut attributes = HashMap::new();
        let mut references = Vec::new();

        for (attr, observations) in &self.attrs {
            let distinct_entities: std::collections::HashSet<Uuid> =
                observations.iter().map(|(eid, _)| *eid).collect();
            let cardinality = distinct_entities.len() as u64;

            let mut type_set: Vec<i16> = observations.iter().map(|(_, vt)| *vt).collect();
            type_set.sort();
            type_set.dedup();

            let value_types: Vec<ValueType> = type_set
                .iter()
                .filter_map(|vt| ValueType::from_i16(*vt))
                .collect();

            let required = cardinality == entity_count && entity_count > 0;

            // If any observation is a Reference type, record it.
            if value_types.contains(&ValueType::Reference) {
                references.push(ReferenceInfo {
                    attribute: attr.clone(),
                    target_type: "_unknown".to_string(), // resolved by higher layers
                    cardinality,
                });
            }

            attributes.insert(
                attr.clone(),
                AttributeInfo {
                    name: attr.clone(),
                    value_types,
                    required,
                    cardinality,
                },
            );
        }

        EntityType {
            name: self.name,
            attributes,
            references,
            entity_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    // ── Triple serialization ────────────────────────────────────────

    #[test]
    fn triple_json_roundtrip() {
        let t = Triple {
            id: 1,
            entity_id: Uuid::nil(),
            attribute: "user/name".into(),
            value: json!("Alice"),
            value_type: ValueType::String as i16,
            tx_id: 42,
            created_at: chrono::Utc::now(),
            retracted: false,
            expires_at: None,
        };

        let serialized = serde_json::to_string(&t).unwrap();
        let deserialized: Triple = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.id, t.id);
        assert_eq!(deserialized.entity_id, t.entity_id);
        assert_eq!(deserialized.attribute, t.attribute);
        assert_eq!(deserialized.value, t.value);
        assert_eq!(deserialized.value_type, t.value_type);
        assert_eq!(deserialized.tx_id, t.tx_id);
        assert_eq!(deserialized.retracted, t.retracted);
    }

    #[test]
    fn triple_clone_is_independent() {
        let t = Triple {
            id: 1,
            entity_id: Uuid::new_v4(),
            attribute: "x".into(),
            value: json!(123),
            value_type: 1,
            tx_id: 1,
            created_at: chrono::Utc::now(),
            retracted: false,
            expires_at: None,
        };
        let mut cloned = t.clone();
        cloned.retracted = true;
        assert!(!t.retracted);
        assert!(cloned.retracted);
    }

    // ── TripleInput validation ──────────────────────────────────────

    #[test]
    fn triple_input_valid() {
        let input = TripleInput {
            entity_id: Uuid::new_v4(),
            attribute: "user/email".into(),
            value: json!("a@b.com"),
            value_type: ValueType::String as i16,
            ttl_seconds: None,
        };
        assert!(input.validate().is_ok());
    }

    #[test]
    fn triple_input_empty_attribute_rejected() {
        let input = TripleInput {
            entity_id: Uuid::new_v4(),
            attribute: "".into(),
            value: json!("x"),
            value_type: 0,
            ttl_seconds: None,
        };
        let err = input.validate().unwrap_err();
        assert!(
            matches!(err, DarshanError::InvalidAttribute(ref msg) if msg.contains("empty")),
            "expected InvalidAttribute(empty), got: {err}"
        );
    }

    #[test]
    fn triple_input_overlong_attribute_rejected() {
        let input = TripleInput {
            entity_id: Uuid::new_v4(),
            attribute: "a".repeat(513),
            value: json!("x"),
            value_type: 0,
            ttl_seconds: None,
        };
        let err = input.validate().unwrap_err();
        assert!(
            matches!(err, DarshanError::InvalidAttribute(ref msg) if msg.contains("512")),
            "expected InvalidAttribute(512), got: {err}"
        );
    }

    #[test]
    fn triple_input_invalid_value_type_rejected() {
        let input = TripleInput {
            entity_id: Uuid::new_v4(),
            attribute: "valid".into(),
            value: json!(null),
            value_type: 99,
            ttl_seconds: None,
        };
        let err = input.validate().unwrap_err();
        assert!(
            matches!(err, DarshanError::TypeMismatch { .. }),
            "expected TypeMismatch, got: {err}"
        );
    }

    #[test]
    fn triple_input_negative_value_type_rejected() {
        let input = TripleInput {
            entity_id: Uuid::new_v4(),
            attribute: "valid".into(),
            value: json!(true),
            value_type: -1,
            ttl_seconds: None,
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn triple_input_all_valid_types_accepted() {
        for vt in 0..=ValueType::max_discriminator() {
            let input = TripleInput {
                entity_id: Uuid::new_v4(),
                attribute: "a".into(),
                value: json!(null),
                value_type: vt,
                ttl_seconds: None,
            };
            assert!(input.validate().is_ok(), "value_type {vt} should be valid");
        }
    }

    #[test]
    fn triple_input_boundary_attribute_length() {
        // Exactly 512 bytes should be accepted
        let input = TripleInput {
            entity_id: Uuid::new_v4(),
            attribute: "a".repeat(512),
            value: json!(null),
            value_type: 0,
            ttl_seconds: None,
        };
        assert!(input.validate().is_ok());
    }

    // ── EntityTypeBuilder ───────────────────────────────────────────

    #[test]
    fn builder_single_entity_single_attr() {
        let mut b = EntityTypeBuilder::new("User".into());
        let eid = Uuid::new_v4();
        b.observe(eid, "name", ValueType::String as i16);

        let et = b.build();
        assert_eq!(et.name, "User");
        assert_eq!(et.entity_count, 1);
        assert_eq!(et.attributes.len(), 1);
        let attr = &et.attributes["name"];
        assert_eq!(attr.value_types, vec![ValueType::String]);
        assert!(attr.required); // 1 entity, 1 observation => required
    }

    #[test]
    fn builder_required_detection() {
        let mut b = EntityTypeBuilder::new("User".into());
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();
        // Both entities have "name"
        b.observe(e1, "name", ValueType::String as i16);
        b.observe(e2, "name", ValueType::String as i16);
        // Only e1 has "bio"
        b.observe(e1, "bio", ValueType::String as i16);

        let et = b.build();
        assert!(et.attributes["name"].required, "name seen on all entities");
        assert!(
            !et.attributes["bio"].required,
            "bio not seen on all entities"
        );
    }

    #[test]
    fn builder_polymorphic_value_types() {
        let mut b = EntityTypeBuilder::new("Doc".into());
        let eid = Uuid::new_v4();
        b.observe(eid, "data", ValueType::String as i16);
        b.observe(eid, "data", ValueType::Json as i16);

        let et = b.build();
        let vts = &et.attributes["data"].value_types;
        assert!(vts.contains(&ValueType::String));
        assert!(vts.contains(&ValueType::Json));
    }

    #[test]
    fn builder_unknown_value_type_filtered() {
        let mut b = EntityTypeBuilder::new("X".into());
        let eid = Uuid::new_v4();
        b.observe(eid, "weird", 99); // unknown discriminator

        let et = b.build();
        let vts = &et.attributes["weird"].value_types;
        assert!(vts.is_empty(), "unknown value_type should be filtered out");
    }

    #[test]
    fn builder_reference_detection() {
        let mut b = EntityTypeBuilder::new("Post".into());
        let eid = Uuid::new_v4();
        b.observe(eid, "author_id", ValueType::Reference as i16);

        let et = b.build();
        assert_eq!(et.references.len(), 1);
        assert_eq!(et.references[0].attribute, "author_id");
        assert_eq!(et.references[0].target_type, "_unknown");
    }

    #[test]
    fn builder_no_entities_no_required() {
        let b = EntityTypeBuilder::new("Empty".into());
        let et = b.build();
        assert_eq!(et.entity_count, 0);
        assert!(et.attributes.is_empty());
    }

    #[test]
    fn builder_deduplicates_value_types() {
        let mut b = EntityTypeBuilder::new("T".into());
        let eid = Uuid::new_v4();
        // Same entity, same attr, same type observed 3 times
        b.observe(eid, "x", ValueType::Integer as i16);
        b.observe(eid, "x", ValueType::Integer as i16);
        b.observe(eid, "x", ValueType::Integer as i16);

        let et = b.build();
        assert_eq!(
            et.attributes["x"].value_types.len(),
            1,
            "duplicate value_types should be deduped"
        );
    }

    #[test]
    fn builder_cardinality_counts_distinct_entities() {
        let mut b = EntityTypeBuilder::new("T".into());
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();
        // e1 observed twice on same attr
        b.observe(e1, "x", 0);
        b.observe(e1, "x", 0);
        b.observe(e2, "x", 0);

        let et = b.build();
        assert_eq!(
            et.attributes["x"].cardinality, 2,
            "cardinality should count distinct entities"
        );
    }
}
