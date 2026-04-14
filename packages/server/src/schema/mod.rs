//! SurrealDB-style schema modes for DarshJDB.
//!
//! Provides three schema enforcement modes per table:
//!
//! - **SCHEMAFULL**: strict types; every field must be defined in the schema.
//!   Unknown fields are rejected. Type mismatches are errors.
//! - **SCHEMALESS**: any JSON accepted — the current default behaviour.
//!   No validation is performed beyond basic JSON well-formedness.
//! - **MIXED**: defined fields are type-checked and enforced; additional
//!   undeclared fields pass through without validation.
//!
//! # DDL-style definitions
//!
//! ```text
//! DEFINE TABLE users SCHEMAFULL
//! DEFINE FIELD name ON users TYPE string ASSERT $value != ""
//! DEFINE FIELD age  ON users TYPE int    DEFAULT 0
//! DEFINE INDEX idx_email ON users FIELDS email UNIQUE
//! ```
//!
//! Schemas are persisted in a PostgreSQL `_schemas` table and cached
//! in-memory via [`SchemaRegistry`].

pub mod migration;
// Slice 28/30 — Phase 9 SurrealDB parity: strict-mode schema enforcement
// gated by `DdbConfig.schema.schema_mode == "strict"`.
pub mod strict;
pub mod validator;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ── Schema mode ────────────────────────────────────────────────────

/// The enforcement level for a table's schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[derive(Default)]
pub enum SchemaMode {
    /// Strict: all fields must be defined; unknown fields are rejected.
    Schemafull,
    /// Flexible: any JSON accepted with no validation (default).
    #[default]
    Schemaless,
    /// Hybrid: defined fields are enforced; extras are allowed.
    Mixed,
}

impl fmt::Display for SchemaMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schemafull => f.write_str("SCHEMAFULL"),
            Self::Schemaless => f.write_str("SCHEMALESS"),
            Self::Mixed => f.write_str("MIXED"),
        }
    }
}

impl SchemaMode {
    /// Parse a mode string (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "SCHEMAFULL" | "STRICT" => Some(Self::Schemafull),
            "SCHEMALESS" | "FLEXIBLE" => Some(Self::Schemaless),
            "MIXED" | "HYBRID" => Some(Self::Mixed),
            _ => None,
        }
    }
}

// ── Field type ─────────────────────────────────────────────────────

/// Supported field types for schema definitions.
///
/// These map to the existing [`ValueType`] discriminators in the triple
/// store but provide a higher-level abstraction for DDL definitions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    /// UTF-8 text.
    String,
    /// 64-bit signed integer.
    Int,
    /// 64-bit IEEE 754 float.
    Float,
    /// Boolean.
    Bool,
    /// RFC 3339 timestamp.
    Datetime,
    /// UUID (stored as string, validated as UUID).
    Uuid,
    /// Arbitrary JSON object or array.
    Json,
    /// Reference to another entity (UUID foreign key).
    Record(Option<String>),
    /// Array of a specific type.
    Array(Box<FieldType>),
    /// One of several types.
    Union(Vec<FieldType>),
    /// Any type (no constraint).
    Any,
}

impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String => f.write_str("string"),
            Self::Int => f.write_str("int"),
            Self::Float => f.write_str("float"),
            Self::Bool => f.write_str("bool"),
            Self::Datetime => f.write_str("datetime"),
            Self::Uuid => f.write_str("uuid"),
            Self::Json => f.write_str("json"),
            Self::Record(Some(table)) => write!(f, "record({table})"),
            Self::Record(None) => f.write_str("record"),
            Self::Array(inner) => write!(f, "array<{inner}>"),
            Self::Union(types) => {
                let parts: Vec<String> = types.iter().map(|t| t.to_string()).collect();
                write!(f, "{}", parts.join(" | "))
            }
            Self::Any => f.write_str("any"),
        }
    }
}

impl FieldType {
    /// Parse a type string (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let lower = s.to_lowercase();

        // Handle union types (e.g. "string | int").
        if lower.contains('|') {
            let parts: Vec<FieldType> = s
                .split('|')
                .filter_map(|p| FieldType::parse(p.trim()))
                .collect();
            if parts.len() >= 2 {
                return Some(Self::Union(parts));
            }
        }

        // Handle array types (e.g. "array<string>").
        if lower.starts_with("array<") && lower.ends_with('>') {
            let inner = &s[6..s.len() - 1];
            return FieldType::parse(inner).map(|t| Self::Array(Box::new(t)));
        }

        // Handle record types (e.g. "record(users)").
        if lower.starts_with("record(") && lower.ends_with(')') {
            let table = &s[7..s.len() - 1];
            return Some(Self::Record(Some(table.to_string())));
        }

        match lower.as_str() {
            "string" | "text" => Some(Self::String),
            "int" | "integer" | "i64" => Some(Self::Int),
            "float" | "f64" | "number" | "decimal" => Some(Self::Float),
            "bool" | "boolean" => Some(Self::Bool),
            "datetime" | "timestamp" => Some(Self::Datetime),
            "uuid" => Some(Self::Uuid),
            "json" | "object" => Some(Self::Json),
            "record" | "reference" => Some(Self::Record(None)),
            "any" => Some(Self::Any),
            _ => None,
        }
    }

    /// Return the triple-store value_type discriminator for this field type.
    pub fn to_value_type_i16(&self) -> i16 {
        match self {
            Self::String => 0,
            Self::Int => 1,
            Self::Float => 2,
            Self::Bool => 3,
            Self::Datetime => 4,
            Self::Uuid | Self::Record(_) => 5,
            Self::Json | Self::Array(_) => 6,
            Self::Union(_) | Self::Any => 6, // polymorphic → JSON
        }
    }
}

// ── Field definition ───────────────────────────────────────────────

/// A single field within a table schema.
///
/// Corresponds to: `DEFINE FIELD name ON table TYPE type [DEFAULT val] [ASSERT expr]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDefinition {
    /// Field name (e.g. `"name"`, `"email"`, `"age"`).
    pub name: String,
    /// Expected type. `None` means any type is accepted.
    pub field_type: Option<FieldType>,
    /// Default value injected when the field is absent from the document.
    pub default_value: Option<serde_json::Value>,
    /// Assertion expression evaluated against `$value`.
    /// The expression must evaluate to `true` for the value to be accepted.
    ///
    /// Supported operators:
    /// - `$value != ""` — non-empty string
    /// - `$value >= 0` — numeric lower bound
    /// - `$value <= 150` — numeric upper bound
    /// - `$value =~ "^[a-z]+$"` — regex match
    /// - `$value IN ["a", "b", "c"]` — enum membership
    pub assert_expr: Option<String>,
    /// Whether the field is required (must be present in every document).
    pub required: bool,
    /// Whether the field must contain a unique value across all entities.
    pub unique: bool,
    /// Whether the field is read-only after initial creation.
    pub readonly: bool,
}

impl FieldDefinition {
    /// Create a minimal field definition with just a name and type.
    pub fn new(name: impl Into<String>, field_type: FieldType) -> Self {
        Self {
            name: name.into(),
            field_type: Some(field_type),
            default_value: None,
            assert_expr: None,
            required: false,
            unique: false,
            readonly: false,
        }
    }

    /// Builder: set the default value.
    pub fn with_default(mut self, value: serde_json::Value) -> Self {
        self.default_value = Some(value);
        self
    }

    /// Builder: set the assert expression.
    pub fn with_assert(mut self, expr: impl Into<String>) -> Self {
        self.assert_expr = Some(expr.into());
        self
    }

    /// Builder: mark as required.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Builder: mark as unique.
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Builder: mark as read-only.
    pub fn readonly(mut self) -> Self {
        self.readonly = true;
        self
    }
}

// ── Index definition ───────────────────────────────────────────────

/// An index on one or more fields of a table.
///
/// Corresponds to: `DEFINE INDEX idx_name ON table FIELDS field1[, field2] [UNIQUE]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexDefinition {
    /// Index name (e.g. `"idx_email"`).
    pub name: String,
    /// Ordered list of fields in the index.
    pub fields: Vec<String>,
    /// Whether the index enforces uniqueness.
    pub unique: bool,
}

// ── Permission definition ──────────────────────────────────────────

/// Row-level permissions for a table.
///
/// Each operation maps to a boolean expression or `true`/`false`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TablePermissions {
    /// Expression for SELECT (read) operations.
    pub select: Option<String>,
    /// Expression for CREATE operations.
    pub create: Option<String>,
    /// Expression for UPDATE operations.
    pub update: Option<String>,
    /// Expression for DELETE operations.
    pub delete: Option<String>,
}

// ── Table schema ───────────────────────────────────────────────────

/// Complete schema definition for a single table.
///
/// Corresponds to: `DEFINE TABLE name [SCHEMAFULL|SCHEMALESS|MIXED]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableSchema {
    /// Table name (e.g. `"users"`, `"posts"`).
    pub name: String,
    /// Schema enforcement mode.
    pub mode: SchemaMode,
    /// Field definitions keyed by field name.
    pub fields: HashMap<String, FieldDefinition>,
    /// Index definitions keyed by index name.
    pub indexes: HashMap<String, IndexDefinition>,
    /// Row-level permissions.
    pub permissions: TablePermissions,
    /// Schema version (monotonically increasing).
    pub version: i64,
    /// When this schema was created or last modified.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TableSchema {
    /// Create a new schemaless table (no field constraints).
    pub fn schemaless(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mode: SchemaMode::Schemaless,
            fields: HashMap::new(),
            indexes: HashMap::new(),
            permissions: TablePermissions::default(),
            version: 1,
            updated_at: chrono::Utc::now(),
        }
    }

    /// Create a new schemafull table.
    pub fn schemafull(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mode: SchemaMode::Schemafull,
            fields: HashMap::new(),
            indexes: HashMap::new(),
            permissions: TablePermissions::default(),
            version: 1,
            updated_at: chrono::Utc::now(),
        }
    }

    /// Create a new mixed-mode table.
    pub fn mixed(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mode: SchemaMode::Mixed,
            fields: HashMap::new(),
            indexes: HashMap::new(),
            permissions: TablePermissions::default(),
            version: 1,
            updated_at: chrono::Utc::now(),
        }
    }

    /// Add a field definition to this table schema.
    pub fn define_field(mut self, field: FieldDefinition) -> Self {
        self.fields.insert(field.name.clone(), field);
        self
    }

    /// Add an index definition to this table schema.
    pub fn define_index(mut self, index: IndexDefinition) -> Self {
        self.indexes.insert(index.name.clone(), index);
        self
    }

    /// Set table permissions.
    pub fn with_permissions(mut self, permissions: TablePermissions) -> Self {
        self.permissions = permissions;
        self
    }
}

// ── Schema registry ────────────────────────────────────────────────

/// In-memory cache of table schemas backed by PostgreSQL.
///
/// The registry loads schemas from the `_schemas` table on startup and
/// keeps them in a [`DashMap`] for lock-free concurrent reads. Mutations
/// (DEFINE TABLE / DEFINE FIELD) write-through to Postgres and update
/// the in-memory cache atomically.
pub struct SchemaRegistry {
    /// Cached table schemas keyed by table name.
    tables: dashmap::DashMap<String, TableSchema>,
    /// Postgres connection pool for persistence.
    pool: sqlx::PgPool,
}

impl SchemaRegistry {
    /// Create a new registry, ensuring the `_schemas` table exists,
    /// then loading all persisted schemas into memory.
    pub async fn new(pool: sqlx::PgPool) -> crate::error::Result<Self> {
        let registry = Self {
            tables: dashmap::DashMap::new(),
            pool,
        };
        registry.ensure_schemas_table().await?;
        registry.load_all().await?;
        Ok(registry)
    }

    /// Create the `_schemas` table and indexes if they do not exist.
    async fn ensure_schemas_table(&self) -> crate::error::Result<()> {
        sqlx::raw_sql(
            r#"
            CREATE TABLE IF NOT EXISTS _schemas (
                name        TEXT        PRIMARY KEY,
                mode        TEXT        NOT NULL DEFAULT 'SCHEMALESS',
                fields      JSONB       NOT NULL DEFAULT '{}',
                indexes     JSONB       NOT NULL DEFAULT '{}',
                permissions JSONB       NOT NULL DEFAULT '{}',
                version     BIGINT      NOT NULL DEFAULT 1,
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
            );

            CREATE INDEX IF NOT EXISTS idx_schemas_updated
                ON _schemas (updated_at);
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Load all schemas from PostgreSQL into the in-memory cache.
    async fn load_all(&self) -> crate::error::Result<()> {
        let rows = sqlx::query_as::<_, SchemaRow>(
            "SELECT name, mode, fields, indexes, permissions, version, updated_at FROM _schemas",
        )
        .fetch_all(&self.pool)
        .await?;

        for row in rows {
            if let Ok(schema) = row.into_table_schema() {
                self.tables.insert(schema.name.clone(), schema);
            }
        }
        Ok(())
    }

    /// Look up the schema for a table. Returns `None` for unknown tables
    /// (which means schemaless by default).
    pub fn get(&self, table: &str) -> Option<TableSchema> {
        self.tables.get(table).map(|r| r.value().clone())
    }

    /// Return the effective schema mode for a table.
    /// Unknown tables are treated as SCHEMALESS.
    pub fn mode(&self, table: &str) -> SchemaMode {
        self.tables
            .get(table)
            .map(|r| r.value().mode)
            .unwrap_or(SchemaMode::Schemaless)
    }

    /// Define (create or replace) a table schema.
    pub async fn define_table(&self, schema: TableSchema) -> crate::error::Result<()> {
        let fields_json = serde_json::to_value(&schema.fields)?;
        let indexes_json = serde_json::to_value(&schema.indexes)?;
        let permissions_json = serde_json::to_value(&schema.permissions)?;
        let mode_str = schema.mode.to_string();

        sqlx::query(
            r#"
            INSERT INTO _schemas (name, mode, fields, indexes, permissions, version, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, now())
            ON CONFLICT (name) DO UPDATE SET
                mode        = EXCLUDED.mode,
                fields      = EXCLUDED.fields,
                indexes     = EXCLUDED.indexes,
                permissions = EXCLUDED.permissions,
                version     = _schemas.version + 1,
                updated_at  = now()
            "#,
        )
        .bind(&schema.name)
        .bind(&mode_str)
        .bind(&fields_json)
        .bind(&indexes_json)
        .bind(&permissions_json)
        .bind(schema.version)
        .execute(&self.pool)
        .await?;

        // Update the in-memory cache.
        self.tables.insert(schema.name.clone(), schema);
        Ok(())
    }

    /// Define a field on an existing table. If the table does not exist
    /// in the registry, creates it with MIXED mode.
    pub async fn define_field(
        &self,
        table: &str,
        field: FieldDefinition,
    ) -> crate::error::Result<()> {
        let mut schema = self
            .tables
            .get(table)
            .map(|r| r.value().clone())
            .unwrap_or_else(|| TableSchema::mixed(table));

        schema.fields.insert(field.name.clone(), field);
        schema.version += 1;
        schema.updated_at = chrono::Utc::now();

        self.define_table(schema).await
    }

    /// Define an index on an existing table.
    pub async fn define_index(
        &self,
        table: &str,
        index: IndexDefinition,
    ) -> crate::error::Result<()> {
        let mut schema = self
            .tables
            .get(table)
            .map(|r| r.value().clone())
            .unwrap_or_else(|| TableSchema::schemaless(table));

        schema.indexes.insert(index.name.clone(), index);
        schema.version += 1;
        schema.updated_at = chrono::Utc::now();

        self.define_table(schema).await
    }

    /// Remove a table schema entirely. Reverts the table to schemaless.
    pub async fn remove_table(&self, table: &str) -> crate::error::Result<()> {
        sqlx::query("DELETE FROM _schemas WHERE name = $1")
            .bind(table)
            .execute(&self.pool)
            .await?;
        self.tables.remove(table);
        Ok(())
    }

    /// Remove a field definition from a table.
    pub async fn remove_field(&self, table: &str, field_name: &str) -> crate::error::Result<()> {
        if let Some(mut entry) = self.tables.get_mut(table) {
            let schema = entry.value_mut();
            schema.fields.remove(field_name);
            schema.version += 1;
            schema.updated_at = chrono::Utc::now();

            let fields_json = serde_json::to_value(&schema.fields)?;
            sqlx::query(
                r#"
                UPDATE _schemas
                SET fields = $1, version = version + 1, updated_at = now()
                WHERE name = $2
                "#,
            )
            .bind(&fields_json)
            .bind(table)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// List all registered table schemas.
    pub fn list_tables(&self) -> Vec<TableSchema> {
        self.tables.iter().map(|r| r.value().clone()).collect()
    }
}

// ── PostgreSQL row mapping ─────────────────────────────────────────

/// Raw row from the `_schemas` table.
#[derive(sqlx::FromRow)]
struct SchemaRow {
    name: String,
    mode: String,
    fields: serde_json::Value,
    indexes: serde_json::Value,
    permissions: serde_json::Value,
    version: i64,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl SchemaRow {
    /// Convert a database row into a [`TableSchema`].
    fn into_table_schema(self) -> crate::error::Result<TableSchema> {
        let mode = SchemaMode::parse(&self.mode).unwrap_or_default();
        let fields: HashMap<String, FieldDefinition> = serde_json::from_value(self.fields)?;
        let indexes: HashMap<String, IndexDefinition> = serde_json::from_value(self.indexes)?;
        let permissions: TablePermissions = serde_json::from_value(self.permissions)?;

        Ok(TableSchema {
            name: self.name,
            mode,
            fields,
            indexes,
            permissions,
            version: self.version,
            updated_at: self.updated_at,
        })
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_mode_parse_round_trip() {
        for (input, expected) in [
            ("SCHEMAFULL", SchemaMode::Schemafull),
            ("schemafull", SchemaMode::Schemafull),
            ("STRICT", SchemaMode::Schemafull),
            ("SCHEMALESS", SchemaMode::Schemaless),
            ("FLEXIBLE", SchemaMode::Schemaless),
            ("MIXED", SchemaMode::Mixed),
            ("HYBRID", SchemaMode::Mixed),
        ] {
            assert_eq!(SchemaMode::parse(input), Some(expected), "parse({input})");
        }
        assert_eq!(SchemaMode::parse("invalid"), None);
    }

    #[test]
    fn schema_mode_display() {
        assert_eq!(SchemaMode::Schemafull.to_string(), "SCHEMAFULL");
        assert_eq!(SchemaMode::Schemaless.to_string(), "SCHEMALESS");
        assert_eq!(SchemaMode::Mixed.to_string(), "MIXED");
    }

    #[test]
    fn field_type_parse_basic() {
        assert_eq!(FieldType::parse("string"), Some(FieldType::String));
        assert_eq!(FieldType::parse("INT"), Some(FieldType::Int));
        assert_eq!(FieldType::parse("float"), Some(FieldType::Float));
        assert_eq!(FieldType::parse("bool"), Some(FieldType::Bool));
        assert_eq!(FieldType::parse("datetime"), Some(FieldType::Datetime));
        assert_eq!(FieldType::parse("uuid"), Some(FieldType::Uuid));
        assert_eq!(FieldType::parse("json"), Some(FieldType::Json));
        assert_eq!(FieldType::parse("any"), Some(FieldType::Any));
    }

    #[test]
    fn field_type_parse_record() {
        assert_eq!(FieldType::parse("record"), Some(FieldType::Record(None)));
        assert_eq!(
            FieldType::parse("record(users)"),
            Some(FieldType::Record(Some("users".into())))
        );
    }

    #[test]
    fn field_type_parse_array() {
        assert_eq!(
            FieldType::parse("array<string>"),
            Some(FieldType::Array(Box::new(FieldType::String)))
        );
        assert_eq!(
            FieldType::parse("array<int>"),
            Some(FieldType::Array(Box::new(FieldType::Int)))
        );
    }

    #[test]
    fn field_type_parse_union() {
        let ft = FieldType::parse("string | int").unwrap();
        assert_eq!(
            ft,
            FieldType::Union(vec![FieldType::String, FieldType::Int])
        );
    }

    #[test]
    fn field_type_display() {
        assert_eq!(FieldType::String.to_string(), "string");
        assert_eq!(FieldType::Int.to_string(), "int");
        assert_eq!(
            FieldType::Record(Some("users".into())).to_string(),
            "record(users)"
        );
        assert_eq!(
            FieldType::Array(Box::new(FieldType::String)).to_string(),
            "array<string>"
        );
    }

    #[test]
    fn field_definition_builder() {
        let field = FieldDefinition::new("email", FieldType::String)
            .required()
            .unique()
            .with_assert("$value != \"\"");

        assert_eq!(field.name, "email");
        assert!(field.required);
        assert!(field.unique);
        assert_eq!(field.assert_expr.as_deref(), Some("$value != \"\""));
    }

    #[test]
    fn table_schema_schemafull_builder() {
        let schema = TableSchema::schemafull("users")
            .define_field(FieldDefinition::new("name", FieldType::String).required())
            .define_field(
                FieldDefinition::new("age", FieldType::Int).with_default(serde_json::json!(0)),
            )
            .define_index(IndexDefinition {
                name: "idx_email".into(),
                fields: vec!["email".into()],
                unique: true,
            });

        assert_eq!(schema.name, "users");
        assert_eq!(schema.mode, SchemaMode::Schemafull);
        assert_eq!(schema.fields.len(), 2);
        assert_eq!(schema.indexes.len(), 1);
        assert!(schema.indexes["idx_email"].unique);
    }

    #[test]
    fn table_schema_schemaless_by_default() {
        let schema = TableSchema::schemaless("logs");
        assert_eq!(schema.mode, SchemaMode::Schemaless);
        assert!(schema.fields.is_empty());
    }

    #[test]
    fn field_type_to_value_type_discriminator() {
        assert_eq!(FieldType::String.to_value_type_i16(), 0);
        assert_eq!(FieldType::Int.to_value_type_i16(), 1);
        assert_eq!(FieldType::Float.to_value_type_i16(), 2);
        assert_eq!(FieldType::Bool.to_value_type_i16(), 3);
        assert_eq!(FieldType::Datetime.to_value_type_i16(), 4);
        assert_eq!(FieldType::Uuid.to_value_type_i16(), 5);
        assert_eq!(FieldType::Record(None).to_value_type_i16(), 5);
        assert_eq!(FieldType::Json.to_value_type_i16(), 6);
    }
}
