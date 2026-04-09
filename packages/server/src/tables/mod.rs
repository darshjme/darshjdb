//! Explicit table management for DarshJDB.
//!
//! While the triple store can infer entity types from `:db/type` triples,
//! this module provides explicit table definitions with metadata, field
//! ordering, table-level settings, and templates. Table configs are
//! themselves stored as EAV triples (entity = `table:{uuid}`), making
//! the system fully self-describing.

pub mod handlers;
pub mod migration;
pub mod templates;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::triple_store::{PgTripleStore, TripleInput, TripleStore};

// ── Core types ────────────────────────────────────────────────────────

/// Strongly-typed table identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TableId(pub Uuid);

impl TableId {
    /// Generate a new random table id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// The entity id used to store this table's config in the triple store.
    pub fn entity_id(&self) -> Uuid {
        self.0
    }
}

impl Default for TableId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Strongly-typed field identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FieldId(pub Uuid);

impl FieldId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for FieldId {
    fn default() -> Self {
        Self::new()
    }
}

/// Table-level settings controlling behavior and limits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TableSettings {
    /// Whether duplicate records (identical field values) are allowed.
    pub allow_duplicates: bool,
    /// Whether to track full triple history for undo/audit.
    pub enable_history: bool,
    /// Hard limit on the number of records in this table.
    pub max_records: Option<u32>,
    /// Whether inline comments on records are enabled.
    pub enable_comments: bool,
}

impl Default for TableSettings {
    fn default() -> Self {
        Self {
            allow_duplicates: true,
            enable_history: true,
            max_records: None,
            enable_comments: false,
        }
    }
}

/// Full configuration for an explicit table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TableConfig {
    /// Unique table identifier.
    pub id: TableId,
    /// Human-readable name (e.g. "Project Tracker").
    pub name: String,
    /// URL-safe slug derived from the name (e.g. "project-tracker").
    pub slug: String,
    /// Optional description of what this table stores.
    pub description: Option<String>,
    /// Optional emoji icon for UI display.
    pub icon: Option<String>,
    /// Optional hex color for UI theming (e.g. "#4A90D9").
    pub color: Option<String>,
    /// Which field serves as the record "title" / primary display.
    pub primary_field: Option<FieldId>,
    /// Ordered list of field ids — defines column ordering in views.
    pub field_ids: Vec<FieldId>,
    /// Default view to show when opening this table.
    pub default_view_id: Option<Uuid>,
    /// Table-level behavioral settings.
    pub settings: TableSettings,
    /// When this table config was created.
    pub created_at: DateTime<Utc>,
    /// When this table config was last modified.
    pub updated_at: DateTime<Utc>,
}

impl TableConfig {
    /// Create a new table config with the given name.
    /// Generates a slug automatically and uses default settings.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let slug = slugify(&name);
        let now = Utc::now();
        Self {
            id: TableId::new(),
            name,
            slug,
            description: None,
            icon: None,
            color: None,
            primary_field: None,
            field_ids: Vec::new(),
            default_view_id: None,
            settings: TableSettings::default(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// Table statistics returned by the stats endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableStats {
    /// Number of records (entities of this table's type).
    pub record_count: u64,
    /// Number of fields defined on this table.
    pub field_count: u64,
    /// Total number of triples belonging to records of this table.
    pub triple_count: u64,
    /// Last modification timestamp across all records.
    pub last_modified: Option<DateTime<Utc>>,
}

// ── Slug helper ───────────────────────────────────────────────────────

/// Convert a human-readable name into a URL-safe slug.
///
/// Rules: lowercase, non-alphanumeric characters become hyphens,
/// consecutive hyphens collapsed, leading/trailing hyphens stripped.
pub fn slugify(input: &str) -> String {
    let mut slug = String::with_capacity(input.len());
    let mut prev_hyphen = true; // prevent leading hyphen

    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_hyphen = false;
        } else if !prev_hyphen {
            slug.push('-');
            prev_hyphen = true;
        }
    }

    // Strip trailing hyphen.
    if slug.ends_with('-') {
        slug.pop();
    }
    slug
}

// ── TableStore trait ──────────────────────────────────────────────────

/// Async interface for table config CRUD, backed by the triple store.
pub trait TableStore: Send + Sync {
    /// Create a new table config, persisting it as EAV triples.
    fn create_table(
        &self,
        config: &TableConfig,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Retrieve a table config by id.
    fn get_table(
        &self,
        id: TableId,
    ) -> impl std::future::Future<Output = Result<Option<TableConfig>>> + Send;

    /// List all table configs.
    fn list_tables(&self) -> impl std::future::Future<Output = Result<Vec<TableConfig>>> + Send;

    /// Update a table config (full replace).
    fn update_table(
        &self,
        config: &TableConfig,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Delete a table config and optionally cascade to records.
    fn delete_table(&self, id: TableId) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Get statistics for a table.
    fn get_stats(
        &self,
        id: TableId,
    ) -> impl std::future::Future<Output = Result<TableStats>> + Send;
}

// ── Postgres implementation ───────────────────────────────────────────

/// Production table store backed by PgTripleStore.
///
/// Table configs are stored as entities with attribute prefix `table/`.
/// Each table has a `:db/type` of `"__table"` and its serialized config
/// is stored as a JSON blob in `table/config`.
pub struct PgTableStore {
    pool: PgPool,
    triple_store: PgTripleStore,
}

impl PgTableStore {
    /// Create a new table store wrapping the given pool and triple store.
    pub fn new(pool: PgPool, triple_store: PgTripleStore) -> Self {
        Self { pool, triple_store }
    }

    /// Return a reference to the underlying pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

impl TableStore for PgTableStore {
    async fn create_table(&self, config: &TableConfig) -> Result<()> {
        let entity_id = config.id.entity_id();
        let config_json = serde_json::to_value(config)?;

        let triples = vec![
            TripleInput {
                entity_id,
                attribute: ":db/type".to_string(),
                value: serde_json::Value::String("__table".to_string()),
                value_type: 0, // String
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "table/name".to_string(),
                value: serde_json::Value::String(config.name.clone()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "table/slug".to_string(),
                value: serde_json::Value::String(config.slug.clone()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "table/config".to_string(),
                value: config_json,
                value_type: 6, // Json
                ttl_seconds: None,
            },
        ];

        self.triple_store.set_triples(&triples).await?;
        Ok(())
    }

    async fn get_table(&self, id: TableId) -> Result<Option<TableConfig>> {
        let entity_id = id.entity_id();
        let triples = self.triple_store.get_entity(entity_id).await?;

        if triples.is_empty() {
            return Ok(None);
        }

        // Find the table/config triple which holds the full serialized config.
        let config_triple = triples.iter().find(|t| t.attribute == "table/config");

        match config_triple {
            Some(t) => {
                let config: TableConfig = serde_json::from_value(t.value.clone())?;
                Ok(Some(config))
            }
            None => Ok(None),
        }
    }

    async fn list_tables(&self) -> Result<Vec<TableConfig>> {
        // Find all entities with :db/type = "__table"
        let triples = self
            .triple_store
            .query_by_attribute(
                ":db/type",
                Some(&serde_json::Value::String("__table".to_string())),
            )
            .await?;

        let mut configs = Vec::new();
        for t in &triples {
            let entity_triples = self.triple_store.get_entity(t.entity_id).await?;
            if let Some(config_triple) = entity_triples
                .iter()
                .find(|et| et.attribute == "table/config")
                && let Ok(config) =
                    serde_json::from_value::<TableConfig>(config_triple.value.clone())
            {
                configs.push(config);
            }
        }

        // Sort by name for deterministic output.
        configs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(configs)
    }

    async fn update_table(&self, config: &TableConfig) -> Result<()> {
        let entity_id = config.id.entity_id();

        // Retract old values, then write new ones.
        self.triple_store.retract(entity_id, "table/name").await?;
        self.triple_store.retract(entity_id, "table/slug").await?;
        self.triple_store.retract(entity_id, "table/config").await?;

        let config_json = serde_json::to_value(config)?;
        let triples = vec![
            TripleInput {
                entity_id,
                attribute: "table/name".to_string(),
                value: serde_json::Value::String(config.name.clone()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "table/slug".to_string(),
                value: serde_json::Value::String(config.slug.clone()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "table/config".to_string(),
                value: config_json,
                value_type: 6,
                ttl_seconds: None,
            },
        ];

        self.triple_store.set_triples(&triples).await?;
        Ok(())
    }

    async fn delete_table(&self, id: TableId) -> Result<()> {
        let entity_id = id.entity_id();

        // Retract all table metadata triples.
        self.triple_store.retract(entity_id, ":db/type").await?;
        self.triple_store.retract(entity_id, "table/name").await?;
        self.triple_store.retract(entity_id, "table/slug").await?;
        self.triple_store.retract(entity_id, "table/config").await?;

        // Also retract all records that belong to this table.
        // Records have :db/type = table slug, so find and retract them.
        if let Ok(Some(config)) = self.get_table(id).await {
            let record_triples = self
                .triple_store
                .query_by_attribute(
                    ":db/type",
                    Some(&serde_json::Value::String(config.slug.clone())),
                )
                .await?;

            for t in &record_triples {
                // Retract all triples for each record entity.
                let entity_triples = self.triple_store.get_entity(t.entity_id).await?;
                for et in &entity_triples {
                    self.triple_store
                        .retract(et.entity_id, &et.attribute)
                        .await?;
                }
            }
        }

        Ok(())
    }

    async fn get_stats(&self, id: TableId) -> Result<TableStats> {
        let config = self
            .get_table(id)
            .await?
            .ok_or_else(|| DarshJError::EntityNotFound(id.entity_id()))?;

        // Count records by querying for entities with :db/type = slug.
        let record_triples = self
            .triple_store
            .query_by_attribute(
                ":db/type",
                Some(&serde_json::Value::String(config.slug.clone())),
            )
            .await?;

        let record_count = record_triples.len() as u64;

        // Count total triples across all record entities.
        let mut triple_count: u64 = 0;
        let mut last_modified: Option<DateTime<Utc>> = None;

        for t in &record_triples {
            let entity_triples = self.triple_store.get_entity(t.entity_id).await?;
            triple_count += entity_triples.len() as u64;

            for et in &entity_triples {
                match last_modified {
                    None => last_modified = Some(et.created_at),
                    Some(existing) if et.created_at > existing => {
                        last_modified = Some(et.created_at);
                    }
                    _ => {}
                }
            }
        }

        Ok(TableStats {
            record_count,
            field_count: config.field_ids.len() as u64,
            triple_count,
            last_modified,
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_id_generates_unique() {
        let a = TableId::new();
        let b = TableId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn table_id_default() {
        let a = TableId::default();
        let b = TableId::default();
        assert_ne!(a, b);
    }

    #[test]
    fn table_id_entity_id_matches() {
        let id = TableId::new();
        assert_eq!(id.entity_id(), id.0);
    }

    #[test]
    fn table_id_display() {
        let uuid = Uuid::nil();
        let id = TableId(uuid);
        assert_eq!(format!("{id}"), uuid.to_string());
    }

    #[test]
    fn field_id_generates_unique() {
        let a = FieldId::new();
        let b = FieldId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn table_settings_default() {
        let s = TableSettings::default();
        assert!(s.allow_duplicates);
        assert!(s.enable_history);
        assert_eq!(s.max_records, None);
        assert!(!s.enable_comments);
    }

    #[test]
    fn table_config_new() {
        let config = TableConfig::new("My Test Table");
        assert_eq!(config.name, "My Test Table");
        assert_eq!(config.slug, "my-test-table");
        assert!(config.description.is_none());
        assert!(config.icon.is_none());
        assert!(config.color.is_none());
        assert!(config.primary_field.is_none());
        assert!(config.field_ids.is_empty());
        assert!(config.default_view_id.is_none());
        assert_eq!(config.settings, TableSettings::default());
    }

    #[test]
    fn table_config_serialization_roundtrip() {
        let mut config = TableConfig::new("Contacts");
        config.description = Some("All company contacts".into());
        config.icon = Some("📇".into());
        config.color = Some("#FF5722".into());
        let f1 = FieldId::new();
        let f2 = FieldId::new();
        config.field_ids = vec![f1, f2];
        config.primary_field = Some(f1);
        config.settings.enable_comments = true;
        config.settings.max_records = Some(10_000);

        let json = serde_json::to_value(&config).unwrap();
        let back: TableConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn slugify_special_chars() {
        assert_eq!(slugify("My Table! (v2)"), "my-table-v2");
    }

    #[test]
    fn slugify_consecutive_specials() {
        assert_eq!(slugify("a---b   c"), "a-b-c");
    }

    #[test]
    fn slugify_leading_trailing() {
        assert_eq!(slugify("  hello  "), "hello");
    }

    #[test]
    fn slugify_numbers() {
        assert_eq!(slugify("Phase 3 Plan"), "phase-3-plan");
    }

    #[test]
    fn slugify_empty() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn slugify_only_special() {
        assert_eq!(slugify("!@#$%"), "");
    }

    #[test]
    fn table_stats_serialization() {
        let stats = TableStats {
            record_count: 42,
            field_count: 7,
            triple_count: 294,
            last_modified: Some(Utc::now()),
        };
        let json = serde_json::to_string(&stats).unwrap();
        let back: TableStats = serde_json::from_str(&json).unwrap();
        assert_eq!(stats.record_count, back.record_count);
        assert_eq!(stats.field_count, back.field_count);
        assert_eq!(stats.triple_count, back.triple_count);
    }

    #[test]
    fn table_settings_serialization() {
        let s = TableSettings {
            allow_duplicates: false,
            enable_history: true,
            max_records: Some(5000),
            enable_comments: true,
        };
        let json = serde_json::to_value(&s).unwrap();
        let back: TableSettings = serde_json::from_value(json).unwrap();
        assert_eq!(s, back);
    }
}
