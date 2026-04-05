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
            SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted
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

        // Validate every input before touching the database.
        for t in triples {
            t.validate()?;
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
