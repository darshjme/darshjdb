//! Core triple store: the foundational storage layer for DarshanDB.
//!
//! Every piece of data is stored as an (entity, attribute, value) triple
//! tagged with a transaction id and a value-type discriminator. This
//! module defines the [`Triple`] type, the [`TripleStore`] trait, and
//! a production Postgres implementation ([`PgTripleStore`]).

pub mod schema;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::{DarshanError, Result};
use schema::{AttributeInfo, EntityType, ReferenceInfo, Schema, ValueType};

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
                retracted   BOOLEAN     NOT NULL DEFAULT false
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

            -- Sequence for transaction ids.
            CREATE SEQUENCE IF NOT EXISTS darshan_tx_seq
                START WITH 1 INCREMENT BY 1;
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
}

impl TripleStore for PgTripleStore {
    async fn get_entity(&self, entity_id: Uuid) -> Result<Vec<Triple>> {
        let triples = sqlx::query_as::<_, Triple>(
            r#"
            SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted
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
            SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted
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

        let tx_id = self.next_tx_id().await?;
        let mut tx = self.pool.begin().await?;

        for t in triples {
            sqlx::query(
                r#"
                INSERT INTO triples (entity_id, attribute, value, value_type, tx_id)
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(t.entity_id)
            .bind(&t.attribute)
            .bind(&t.value)
            .bind(t.value_type)
            .bind(tx_id)
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
                    SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted
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
                    SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted
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
                id, entity_id, attribute, value, value_type, tx_id, created_at, retracted
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
