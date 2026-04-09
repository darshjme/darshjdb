//! Schema versioning and auto-migration for DarshJDB.
//!
//! When a table schema is updated (e.g. a field is added, removed, or
//! changed), the migration engine produces a diff and optionally applies
//! compensating transformations to existing data.
//!
//! # Version tracking
//!
//! Each schema in the `_schemas` table carries a monotonically increasing
//! `version` column. The `_schema_migrations` table logs every change
//! with before/after snapshots so migrations can be audited and rolled back.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use super::{FieldDefinition, FieldType, SchemaMode, TableSchema};

// ── Migration action ───────────────────────────────────────────────

/// A single migration step produced by diffing two schema versions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SchemaMigrationAction {
    /// Table schema mode changed (e.g. SCHEMALESS → SCHEMAFULL).
    ChangeMode {
        table: String,
        from: SchemaMode,
        to: SchemaMode,
    },
    /// A new field was added.
    AddField {
        table: String,
        field: FieldDefinition,
    },
    /// A field was removed.
    RemoveField { table: String, field_name: String },
    /// A field's type changed.
    AlterFieldType {
        table: String,
        field_name: String,
        from: Option<FieldType>,
        to: Option<FieldType>,
    },
    /// A field's default value changed.
    AlterFieldDefault {
        table: String,
        field_name: String,
        from: Option<serde_json::Value>,
        to: Option<serde_json::Value>,
    },
    /// A field's assert expression changed.
    AlterFieldAssert {
        table: String,
        field_name: String,
        from: Option<String>,
        to: Option<String>,
    },
    /// A field's required flag changed.
    AlterFieldRequired {
        table: String,
        field_name: String,
        from: bool,
        to: bool,
    },
    /// An index was added.
    AddIndex {
        table: String,
        index_name: String,
        fields: Vec<String>,
        unique: bool,
    },
    /// An index was removed.
    RemoveIndex { table: String, index_name: String },
}

// ── Migration record ───────────────────────────────────────────────

/// A persisted migration record.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MigrationRecord {
    /// Auto-generated migration id.
    pub id: i64,
    /// Table this migration applies to.
    pub table_name: String,
    /// Schema version before the migration.
    pub from_version: i64,
    /// Schema version after the migration.
    pub to_version: i64,
    /// JSON-encoded list of migration actions.
    pub actions: serde_json::Value,
    /// When the migration was applied.
    pub applied_at: DateTime<Utc>,
    /// Whether the migration has been rolled back.
    pub rolled_back: bool,
}

// ── Migration engine ───────────────────────────────────────────────

/// Produces migration diffs and persists migration history.
pub struct SchemaMigrationEngine {
    pool: PgPool,
}

impl SchemaMigrationEngine {
    /// Create a new migration engine, ensuring the `_schema_migrations`
    /// table exists.
    pub async fn new(pool: PgPool) -> crate::error::Result<Self> {
        let engine = Self { pool };
        engine.ensure_migration_table().await?;
        Ok(engine)
    }

    /// Create the migration history table.
    async fn ensure_migration_table(&self) -> crate::error::Result<()> {
        sqlx::raw_sql(
            r#"
            CREATE TABLE IF NOT EXISTS _schema_migrations (
                id           BIGSERIAL    PRIMARY KEY,
                table_name   TEXT         NOT NULL,
                from_version BIGINT       NOT NULL,
                to_version   BIGINT       NOT NULL,
                actions      JSONB        NOT NULL DEFAULT '[]',
                applied_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
                rolled_back  BOOLEAN      NOT NULL DEFAULT false
            );

            CREATE INDEX IF NOT EXISTS idx_schema_migrations_table
                ON _schema_migrations (table_name, to_version);
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Diff two schema versions and return the migration actions.
    pub fn diff(old: &TableSchema, new: &TableSchema) -> Vec<SchemaMigrationAction> {
        let mut actions = Vec::new();

        // Mode change.
        if old.mode != new.mode {
            actions.push(SchemaMigrationAction::ChangeMode {
                table: new.name.clone(),
                from: old.mode,
                to: new.mode,
            });
        }

        // Added and altered fields.
        let mut field_names: Vec<&String> = new.fields.keys().collect();
        field_names.sort();
        for name in field_names {
            let new_field = &new.fields[name];
            match old.fields.get(name) {
                None => {
                    actions.push(SchemaMigrationAction::AddField {
                        table: new.name.clone(),
                        field: new_field.clone(),
                    });
                }
                Some(old_field) => {
                    // Check each property individually.
                    if old_field.field_type != new_field.field_type {
                        actions.push(SchemaMigrationAction::AlterFieldType {
                            table: new.name.clone(),
                            field_name: name.clone(),
                            from: old_field.field_type.clone(),
                            to: new_field.field_type.clone(),
                        });
                    }
                    if old_field.default_value != new_field.default_value {
                        actions.push(SchemaMigrationAction::AlterFieldDefault {
                            table: new.name.clone(),
                            field_name: name.clone(),
                            from: old_field.default_value.clone(),
                            to: new_field.default_value.clone(),
                        });
                    }
                    if old_field.assert_expr != new_field.assert_expr {
                        actions.push(SchemaMigrationAction::AlterFieldAssert {
                            table: new.name.clone(),
                            field_name: name.clone(),
                            from: old_field.assert_expr.clone(),
                            to: new_field.assert_expr.clone(),
                        });
                    }
                    if old_field.required != new_field.required {
                        actions.push(SchemaMigrationAction::AlterFieldRequired {
                            table: new.name.clone(),
                            field_name: name.clone(),
                            from: old_field.required,
                            to: new_field.required,
                        });
                    }
                }
            }
        }

        // Removed fields.
        let mut removed: Vec<&String> = old
            .fields
            .keys()
            .filter(|n| !new.fields.contains_key(*n))
            .collect();
        removed.sort();
        for name in removed {
            actions.push(SchemaMigrationAction::RemoveField {
                table: new.name.clone(),
                field_name: name.clone(),
            });
        }

        // Added indexes.
        let mut new_idx_names: Vec<&String> = new
            .indexes
            .keys()
            .filter(|n| !old.indexes.contains_key(*n))
            .collect();
        new_idx_names.sort();
        for name in new_idx_names {
            let idx = &new.indexes[name];
            actions.push(SchemaMigrationAction::AddIndex {
                table: new.name.clone(),
                index_name: name.clone(),
                fields: idx.fields.clone(),
                unique: idx.unique,
            });
        }

        // Removed indexes.
        let mut removed_idx: Vec<&String> = old
            .indexes
            .keys()
            .filter(|n| !new.indexes.contains_key(*n))
            .collect();
        removed_idx.sort();
        for name in removed_idx {
            actions.push(SchemaMigrationAction::RemoveIndex {
                table: new.name.clone(),
                index_name: name.clone(),
            });
        }

        actions
    }

    /// Record a migration in the history table.
    pub async fn record_migration(
        &self,
        table: &str,
        from_version: i64,
        to_version: i64,
        actions: &[SchemaMigrationAction],
    ) -> crate::error::Result<i64> {
        let actions_json = serde_json::to_value(actions)?;
        let row: (i64,) = sqlx::query_as(
            r#"
            INSERT INTO _schema_migrations (table_name, from_version, to_version, actions)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
        )
        .bind(table)
        .bind(from_version)
        .bind(to_version)
        .bind(&actions_json)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    /// Fetch migration history for a table, ordered by version.
    pub async fn get_history(&self, table: &str) -> crate::error::Result<Vec<MigrationRecord>> {
        let rows = sqlx::query_as::<_, MigrationRecord>(
            r#"
            SELECT id, table_name, from_version, to_version, actions, applied_at, rolled_back
            FROM _schema_migrations
            WHERE table_name = $1
            ORDER BY to_version ASC
            "#,
        )
        .bind(table)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Apply a schema change: diff → record migration → persist new schema.
    ///
    /// This is the high-level entry point that coordinates the migration
    /// engine with the schema registry. Call this instead of manually
    /// running diff + record + define_table.
    pub async fn apply_migration(
        &self,
        registry: &super::SchemaRegistry,
        new_schema: TableSchema,
    ) -> crate::error::Result<Vec<SchemaMigrationAction>> {
        let old_schema = registry
            .get(&new_schema.name)
            .unwrap_or_else(|| TableSchema::schemaless(&new_schema.name));

        let actions = Self::diff(&old_schema, &new_schema);

        if !actions.is_empty() {
            let from_version = old_schema.version;
            let to_version = new_schema.version;
            self.record_migration(&new_schema.name, from_version, to_version, &actions)
                .await?;

            tracing::info!(
                table = %new_schema.name,
                from_version,
                to_version,
                action_count = actions.len(),
                "Schema migration applied"
            );
        }

        registry.define_table(new_schema).await?;
        Ok(actions)
    }

    /// Backfill default values for a newly added field across all
    /// existing entities of a given type.
    ///
    /// This is used when a required field with a default is added to
    /// a SCHEMAFULL table — existing entities need the default value
    /// injected as new triples.
    pub async fn backfill_defaults(
        &self,
        table: &str,
        field: &FieldDefinition,
    ) -> crate::error::Result<u64> {
        let default_value = match &field.default_value {
            Some(v) => v,
            None => return Ok(0),
        };

        let attribute = format!("{table}/{}", field.name);
        let value_type = field
            .field_type
            .as_ref()
            .map(|ft| ft.to_value_type_i16())
            .unwrap_or(6); // JSON fallback

        // Find all entities of this type that do NOT already have this attribute.
        let result = sqlx::query(
            r#"
            INSERT INTO triples (entity_id, attribute, value, value_type, tx_id)
            SELECT DISTINCT t.entity_id, $1, $2, $3, nextval('darshan_tx_seq')
            FROM triples t
            WHERE t.attribute = ':db/type'
              AND t.value = $4
              AND NOT t.retracted
              AND NOT EXISTS (
                  SELECT 1 FROM triples t2
                  WHERE t2.entity_id = t.entity_id
                    AND t2.attribute = $1
                    AND NOT t2.retracted
              )
            "#,
        )
        .bind(&attribute)
        .bind(default_value)
        .bind(value_type)
        .bind(serde_json::json!(table))
        .execute(&self.pool)
        .await?;

        let count = result.rows_affected();
        if count > 0 {
            tracing::info!(
                table,
                field = %field.name,
                count,
                "Backfilled default values for new field"
            );
        }

        Ok(count)
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::IndexDefinition;

    #[test]
    fn diff_no_changes() {
        let schema = TableSchema::schemafull("users")
            .define_field(FieldDefinition::new("name", FieldType::String));
        let actions = SchemaMigrationEngine::diff(&schema, &schema.clone());
        assert!(actions.is_empty());
    }

    #[test]
    fn diff_mode_change() {
        let old = TableSchema::schemaless("users");
        let new = TableSchema::schemafull("users");
        let actions = SchemaMigrationEngine::diff(&old, &new);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            SchemaMigrationAction::ChangeMode {
                from: SchemaMode::Schemaless,
                to: SchemaMode::Schemafull,
                ..
            }
        ));
    }

    #[test]
    fn diff_add_field() {
        let old = TableSchema::schemafull("users");
        let new = TableSchema::schemafull("users")
            .define_field(FieldDefinition::new("email", FieldType::String).required());
        let actions = SchemaMigrationEngine::diff(&old, &new);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            SchemaMigrationAction::AddField { field, .. } if field.name == "email"
        ));
    }

    #[test]
    fn diff_remove_field() {
        let old = TableSchema::schemafull("users")
            .define_field(FieldDefinition::new("old_field", FieldType::String));
        let new = TableSchema::schemafull("users");
        let actions = SchemaMigrationEngine::diff(&old, &new);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            SchemaMigrationAction::RemoveField { field_name, .. } if field_name == "old_field"
        ));
    }

    #[test]
    fn diff_alter_field_type() {
        let old = TableSchema::schemafull("users")
            .define_field(FieldDefinition::new("score", FieldType::Int));
        let new = TableSchema::schemafull("users")
            .define_field(FieldDefinition::new("score", FieldType::Float));
        let actions = SchemaMigrationEngine::diff(&old, &new);
        assert!(actions.iter().any(|a| matches!(
            a,
            SchemaMigrationAction::AlterFieldType { field_name, .. } if field_name == "score"
        )));
    }

    #[test]
    fn diff_alter_required_flag() {
        let old = TableSchema::schemafull("users")
            .define_field(FieldDefinition::new("bio", FieldType::String));
        let new = TableSchema::schemafull("users")
            .define_field(FieldDefinition::new("bio", FieldType::String).required());
        let actions = SchemaMigrationEngine::diff(&old, &new);
        assert!(actions.iter().any(|a| matches!(
            a,
            SchemaMigrationAction::AlterFieldRequired {
                from: false,
                to: true,
                ..
            }
        )));
    }

    #[test]
    fn diff_add_index() {
        let old = TableSchema::schemafull("users");
        let new = TableSchema::schemafull("users").define_index(IndexDefinition {
            name: "idx_email".into(),
            fields: vec!["email".into()],
            unique: true,
        });
        let actions = SchemaMigrationEngine::diff(&old, &new);
        assert!(actions.iter().any(|a| matches!(
            a,
            SchemaMigrationAction::AddIndex { index_name, unique: true, .. } if index_name == "idx_email"
        )));
    }

    #[test]
    fn diff_remove_index() {
        let old = TableSchema::schemafull("users").define_index(IndexDefinition {
            name: "idx_old".into(),
            fields: vec!["old".into()],
            unique: false,
        });
        let new = TableSchema::schemafull("users");
        let actions = SchemaMigrationEngine::diff(&old, &new);
        assert!(actions.iter().any(|a| matches!(
            a,
            SchemaMigrationAction::RemoveIndex { index_name, .. } if index_name == "idx_old"
        )));
    }

    #[test]
    fn diff_complex_migration() {
        let old = TableSchema::schemaless("posts")
            .define_field(FieldDefinition::new("title", FieldType::String).required())
            .define_field(FieldDefinition::new("body", FieldType::String))
            .define_index(IndexDefinition {
                name: "idx_old".into(),
                fields: vec!["title".into()],
                unique: false,
            });

        let new = TableSchema::schemafull("posts")
            .define_field(FieldDefinition::new("title", FieldType::String).required())
            .define_field(
                FieldDefinition::new("views", FieldType::Int).with_default(serde_json::json!(0)),
            )
            .define_index(IndexDefinition {
                name: "idx_title".into(),
                fields: vec!["title".into()],
                unique: true,
            });

        let actions = SchemaMigrationEngine::diff(&old, &new);

        // Should have: ChangeMode, AddField(views), RemoveField(body),
        //              AddIndex(idx_title), RemoveIndex(idx_old)
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, SchemaMigrationAction::ChangeMode { .. }))
        );
        assert!(actions.iter().any(|a| matches!(
            a,
            SchemaMigrationAction::AddField { field, .. } if field.name == "views"
        )));
        assert!(actions.iter().any(|a| matches!(
            a,
            SchemaMigrationAction::RemoveField { field_name, .. } if field_name == "body"
        )));
        assert!(actions.iter().any(|a| matches!(
            a,
            SchemaMigrationAction::AddIndex { index_name, .. } if index_name == "idx_title"
        )));
        assert!(actions.iter().any(|a| matches!(
            a,
            SchemaMigrationAction::RemoveIndex { index_name, .. } if index_name == "idx_old"
        )));
    }

    #[test]
    fn diff_deterministic_order() {
        let old = TableSchema::schemafull("t")
            .define_field(FieldDefinition::new("z", FieldType::String))
            .define_field(FieldDefinition::new("a", FieldType::String));
        let new = TableSchema::schemafull("t")
            .define_field(FieldDefinition::new("m", FieldType::Int))
            .define_field(FieldDefinition::new("b", FieldType::Bool));

        let first = SchemaMigrationEngine::diff(&old, &new);
        for _ in 0..20 {
            let again = SchemaMigrationEngine::diff(&old, &new);
            assert_eq!(first, again, "diff must be deterministic");
        }
    }
}
