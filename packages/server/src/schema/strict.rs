//! Slice 28/30 — Strict schema enforcement (Phase 9 SurrealDB parity).
//!
//! Author: Darshankumar Joshi.
//!
//! The strict schema enforcer reads `schema_definitions` (PK:
//! `(collection, attribute)`) and validates incoming documents before
//! they are written to the triple store. Enforcement is **only** active
//! when `DdbConfig.schema.schema_mode == "strict"`. In the default
//! "flexible" mode every call is a no-op so the write path keeps the
//! SurrealDB-style `SchemaRegistry` behaviour untouched.
//!
//! The table is intentionally tiny and opinionated so operators can flip
//! a single config knob to harden an entire collection without learning
//! DarshQL's DDL.
//!
//! ```text
//! POST /api/admin/schema/users
//! {
//!   "definitions": [
//!     { "attribute": "email", "value_type": "string", "required": true, "unique_index": true },
//!     { "attribute": "age",   "value_type": "number", "required": false, "default_val": 0 }
//!   ]
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use chrono::DateTime;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DarshJError, Result};

/// A single `schema_definitions` row — one attribute of one collection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct StrictFieldDef {
    /// Collection / entity-type name (e.g. `"users"`).
    pub collection: String,
    /// Attribute / field name (e.g. `"email"`).
    pub attribute: String,
    /// Declared value type. Must be one of the strings accepted by
    /// [`StrictValueType::parse`]. Stored as free-form TEXT so callers
    /// can express `link:users` without an enum extension.
    pub value_type: String,
    /// Field must be present on CREATE.
    pub required: bool,
    /// Field must be unique across the whole collection.
    pub unique_index: bool,
    /// Default value injected when the attribute is absent from a CREATE.
    pub default_val: Option<Value>,
    /// Optional validator expression (regex for strings, JSON schema
    /// pointer for objects — opaque to this enforcer; evaluated by the
    /// future validator plugin). Persisted but not yet interpreted.
    pub validator: Option<String>,
}

/// Recognised value-type tags accepted in the `value_type` column.
///
/// Kept as a parser so future additions (e.g. `geometry:point`) can be
/// added without touching the storage layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrictValueType {
    String,
    Number,
    Boolean,
    Datetime,
    Uuid,
    Array,
    Object,
    Geometry,
    Vector,
    /// `link:<collection>` — foreign key to another collection.
    Link(String),
}

impl StrictValueType {
    /// Parse a `value_type` string. Returns `None` for unrecognised input.
    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.trim().to_lowercase();
        if let Some(rest) = s.strip_prefix("link:") {
            let target = rest.trim();
            if target.is_empty() {
                return None;
            }
            return Some(Self::Link(target.to_string()));
        }
        match s.as_str() {
            "string" | "text" => Some(Self::String),
            "number" | "int" | "integer" | "float" | "double" => Some(Self::Number),
            "boolean" | "bool" => Some(Self::Boolean),
            "datetime" | "timestamp" => Some(Self::Datetime),
            "uuid" => Some(Self::Uuid),
            "array" => Some(Self::Array),
            "object" | "json" => Some(Self::Object),
            "geometry" => Some(Self::Geometry),
            "vector" => Some(Self::Vector),
            _ => None,
        }
    }

    /// Return `true` if `value` conforms to this type.
    ///
    /// Link types only assert that the value is a valid UUID string —
    /// referential integrity (that the linked row actually exists) is
    /// the caller's responsibility because the triple store defers
    /// foreign-key checks to the graph engine.
    pub fn matches(&self, value: &Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::Datetime => value
                .as_str()
                .map(|s| DateTime::parse_from_rfc3339(s).is_ok())
                .unwrap_or(false),
            Self::Uuid => value
                .as_str()
                .map(|s| Uuid::parse_str(s).is_ok())
                .unwrap_or(false),
            Self::Array => value.is_array(),
            Self::Object => value.is_object(),
            // Geometry and Vector are encoded as JSON objects/arrays —
            // deeper validation is left to the pgvector / PostGIS layers.
            Self::Geometry => value.is_object() || value.is_array(),
            Self::Vector => value.is_array(),
            Self::Link(_) => value
                .as_str()
                .map(|s| Uuid::parse_str(s).is_ok())
                .unwrap_or(false),
        }
    }
}

/// A single validation failure reported back to the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrictValidationError {
    pub field: String,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl StrictValidationError {
    pub fn new(field: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            code: code.into(),
            message: None,
        }
    }

    pub fn with_message(mut self, msg: impl Into<String>) -> Self {
        self.message = Some(msg.into());
        self
    }
}

/// Aggregated validation outcome for a single document.
#[derive(Debug, Clone, Default)]
pub struct StrictValidationReport {
    pub errors: Vec<StrictValidationError>,
    /// Coerced document with defaults injected. Only meaningful when
    /// `errors.is_empty()`.
    pub document: HashMap<String, Value>,
}

impl StrictValidationReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Render the canonical error payload described in the slice:
    /// `{"errors":[{"field":"email","code":"REQUIRED"},...]}`.
    pub fn error_payload(&self) -> Value {
        json!({ "errors": self.errors })
    }
}

/// Strict schema enforcer — loads `schema_definitions` once at startup
/// and keeps them in a lock-free [`DashMap`] keyed by collection name.
///
/// Runtime mutations (GET/POST `/api/admin/schema/:collection`) write
/// through to Postgres and refresh the cache atomically.
pub struct StrictSchemaEnforcer {
    pool: PgPool,
    /// `collection -> { attribute -> def }`.
    by_collection: DashMap<String, HashMap<String, StrictFieldDef>>,
    /// Whether enforcement is active at all. Set from
    /// `DdbConfig.schema.schema_mode == "strict"` at AppState construction.
    strict_mode: bool,
}

impl StrictSchemaEnforcer {
    /// Construct an enforcer, ensure the backing table exists, and
    /// pre-load every definition into memory.
    pub async fn new(pool: PgPool, strict_mode: bool) -> Result<Arc<Self>> {
        let enforcer = Arc::new(Self {
            pool,
            by_collection: DashMap::new(),
            strict_mode,
        });
        enforcer.ensure_schema().await?;
        enforcer.reload().await?;
        Ok(enforcer)
    }

    /// Whether strict enforcement is active for this process.
    pub fn is_strict(&self) -> bool {
        self.strict_mode
    }

    /// Create `schema_definitions` if it does not yet exist. Idempotent.
    async fn ensure_schema(&self) -> Result<()> {
        sqlx::raw_sql(
            r#"
            CREATE TABLE IF NOT EXISTS schema_definitions (
                collection   TEXT    NOT NULL,
                attribute    TEXT    NOT NULL,
                value_type   TEXT    NOT NULL,
                required     BOOLEAN NOT NULL DEFAULT false,
                unique_index BOOLEAN NOT NULL DEFAULT false,
                default_val  JSONB,
                validator    TEXT,
                PRIMARY KEY (collection, attribute)
            );

            CREATE INDEX IF NOT EXISTS idx_schema_definitions_collection
                ON schema_definitions (collection);
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(DarshJError::Database)?;
        Ok(())
    }

    /// Refresh the in-memory cache from Postgres.
    pub async fn reload(&self) -> Result<()> {
        let rows: Vec<StrictFieldDef> = sqlx::query_as::<_, StrictFieldDef>(
            r#"
            SELECT collection, attribute, value_type, required, unique_index,
                   default_val, validator
            FROM schema_definitions
            ORDER BY collection, attribute
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DarshJError::Database)?;

        self.by_collection.clear();
        for row in rows {
            self.by_collection
                .entry(row.collection.clone())
                .or_default()
                .insert(row.attribute.clone(), row);
        }
        Ok(())
    }

    /// Return the cached definitions for a collection, if any.
    pub fn get(&self, collection: &str) -> Option<HashMap<String, StrictFieldDef>> {
        self.by_collection
            .get(collection)
            .map(|r| r.value().clone())
    }

    /// List every known collection name (alphabetical).
    pub fn collections(&self) -> Vec<String> {
        let mut names: Vec<String> = self.by_collection.iter().map(|r| r.key().clone()).collect();
        names.sort();
        names
    }

    /// Upsert a batch of definitions for a collection. Any attribute
    /// omitted from `defs` is **retained** — callers must explicitly
    /// delete obsolete rows via [`Self::delete_attribute`] to avoid
    /// clobbering definitions added out-of-band.
    pub async fn upsert(&self, collection: &str, defs: &[StrictFieldDef]) -> Result<()> {
        if collection.trim().is_empty() {
            return Err(DarshJError::InvalidAttribute(
                "collection name must not be empty".into(),
            ));
        }
        // Validate value_type strings up-front so we fail the entire
        // batch instead of leaving the table in a half-valid state.
        for d in defs {
            if StrictValueType::parse(&d.value_type).is_none() {
                return Err(DarshJError::InvalidAttribute(format!(
                    "unknown value_type '{}' for {}/{}",
                    d.value_type, collection, d.attribute
                )));
            }
        }

        let mut tx = self.pool.begin().await.map_err(DarshJError::Database)?;
        for d in defs {
            sqlx::query(
                r#"
                INSERT INTO schema_definitions
                    (collection, attribute, value_type, required, unique_index, default_val, validator)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (collection, attribute) DO UPDATE SET
                    value_type   = EXCLUDED.value_type,
                    required     = EXCLUDED.required,
                    unique_index = EXCLUDED.unique_index,
                    default_val  = EXCLUDED.default_val,
                    validator    = EXCLUDED.validator
                "#,
            )
            .bind(collection)
            .bind(&d.attribute)
            .bind(&d.value_type)
            .bind(d.required)
            .bind(d.unique_index)
            .bind(&d.default_val)
            .bind(&d.validator)
            .execute(&mut *tx)
            .await
            .map_err(DarshJError::Database)?;
        }
        tx.commit().await.map_err(DarshJError::Database)?;

        self.reload().await?;
        Ok(())
    }

    /// Delete every definition for a collection (used by tests / admin UI).
    pub async fn delete_collection(&self, collection: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM schema_definitions WHERE collection = $1")
            .bind(collection)
            .execute(&self.pool)
            .await
            .map_err(DarshJError::Database)?;
        self.reload().await?;
        Ok(result.rows_affected())
    }

    /// Delete a single attribute definition.
    pub async fn delete_attribute(&self, collection: &str, attribute: &str) -> Result<bool> {
        let result =
            sqlx::query("DELETE FROM schema_definitions WHERE collection = $1 AND attribute = $2")
                .bind(collection)
                .bind(attribute)
                .execute(&self.pool)
                .await
                .map_err(DarshJError::Database)?;
        self.reload().await?;
        Ok(result.rows_affected() > 0)
    }

    /// Validate a CREATE document against the strict schema for
    /// `collection`. Returns an `Ok(report)` whose `errors` slice is
    /// empty on success; the caller should inspect [`StrictValidationReport::is_valid`].
    ///
    /// When strict mode is **off**, or no definitions exist for the
    /// collection, this returns `Ok(empty-report-with-original-doc)` so
    /// callers can unconditionally route documents through the enforcer.
    pub fn validate_create(
        &self,
        collection: &str,
        document: &HashMap<String, Value>,
    ) -> StrictValidationReport {
        if !self.strict_mode {
            return StrictValidationReport {
                errors: Vec::new(),
                document: document.clone(),
            };
        }
        let defs = match self.by_collection.get(collection) {
            Some(d) => d.value().clone(),
            None => {
                // No schema configured → strict mode is best-effort: we
                // allow writes but record zero errors so tests can
                // distinguish "nothing to enforce" from "all good".
                return StrictValidationReport {
                    errors: Vec::new(),
                    document: document.clone(),
                };
            }
        };

        let mut errors = Vec::new();
        let mut coerced: HashMap<String, Value> = document.clone();

        // 1. Required fields must be present (or defaulted).
        // 2. Each supplied field must match its declared value_type.
        for (attr, def) in &defs {
            let value = coerced.get(attr).cloned();
            match value {
                None => {
                    if let Some(default) = &def.default_val {
                        coerced.insert(attr.clone(), default.clone());
                    } else if def.required {
                        errors.push(StrictValidationError::new(attr, "REQUIRED"));
                    }
                }
                Some(Value::Null) if def.required => {
                    errors.push(StrictValidationError::new(attr, "REQUIRED"));
                }
                Some(v) => {
                    if let Some(parsed_type) = StrictValueType::parse(&def.value_type) {
                        if !parsed_type.matches(&v) {
                            errors.push(
                                StrictValidationError::new(attr, "TYPE_MISMATCH").with_message(
                                    format!("expected {}, got {}", def.value_type, value_kind(&v)),
                                ),
                            );
                        }
                    } else {
                        // Unknown type in the schema row itself — flag
                        // it loudly so the admin fixes the definition.
                        errors.push(
                            StrictValidationError::new(attr, "SCHEMA_CORRUPT")
                                .with_message(format!("unknown value_type '{}'", def.value_type)),
                        );
                    }
                }
            }
        }

        StrictValidationReport {
            errors,
            document: coerced,
        }
    }

    /// Validate a PATCH / UPDATE payload. Required-field checks are
    /// relaxed (fields omitted from the patch are assumed unchanged)
    /// but type checks still apply to every supplied attribute.
    pub fn validate_patch(
        &self,
        collection: &str,
        patch: &HashMap<String, Value>,
    ) -> StrictValidationReport {
        if !self.strict_mode {
            return StrictValidationReport {
                errors: Vec::new(),
                document: patch.clone(),
            };
        }
        let defs = match self.by_collection.get(collection) {
            Some(d) => d.value().clone(),
            None => {
                return StrictValidationReport {
                    errors: Vec::new(),
                    document: patch.clone(),
                };
            }
        };

        let mut errors = Vec::new();
        for (attr, v) in patch {
            if let Some(def) = defs.get(attr)
                && let Some(parsed_type) = StrictValueType::parse(&def.value_type)
                && !parsed_type.matches(v)
                && !v.is_null()
            {
                errors.push(
                    StrictValidationError::new(attr, "TYPE_MISMATCH").with_message(format!(
                        "expected {}, got {}",
                        def.value_type,
                        value_kind(v)
                    )),
                );
            }
        }

        StrictValidationReport {
            errors,
            document: patch.clone(),
        }
    }
}

/// Convenience string representation of a JSON value's kind for
/// diagnostic messages (not exposed in the error code, which stays
/// stable for clients).
fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_enforcer_offline(defs: Vec<StrictFieldDef>, strict: bool) -> StrictSchemaEnforcer {
        // Build an enforcer without touching Postgres. The `pool` is a
        // lazy connector that is never used by the pure-logic tests.
        let pool = PgPool::connect_lazy("postgres://localhost/unused").expect("lazy pool");
        let enforcer = StrictSchemaEnforcer {
            pool,
            by_collection: DashMap::new(),
            strict_mode: strict,
        };
        for d in defs {
            enforcer
                .by_collection
                .entry(d.collection.clone())
                .or_default()
                .insert(d.attribute.clone(), d);
        }
        enforcer
    }

    fn users_schema() -> Vec<StrictFieldDef> {
        vec![
            StrictFieldDef {
                collection: "users".into(),
                attribute: "email".into(),
                value_type: "string".into(),
                required: true,
                unique_index: true,
                default_val: None,
                validator: None,
            },
            StrictFieldDef {
                collection: "users".into(),
                attribute: "age".into(),
                value_type: "number".into(),
                required: false,
                unique_index: false,
                default_val: Some(json!(0)),
                validator: None,
            },
            StrictFieldDef {
                collection: "users".into(),
                attribute: "profile_id".into(),
                value_type: "link:profiles".into(),
                required: false,
                unique_index: false,
                default_val: None,
                validator: None,
            },
        ]
    }

    #[tokio::test]
    async fn strict_off_is_always_pass_through() {
        let enforcer = make_enforcer_offline(users_schema(), false);
        let doc = HashMap::from([("anything".to_string(), json!(42))]);
        let report = enforcer.validate_create("users", &doc);
        assert!(report.is_valid());
        assert_eq!(report.document, doc);
    }

    #[tokio::test]
    async fn missing_required_is_flagged_with_canonical_code() {
        let enforcer = make_enforcer_offline(users_schema(), true);
        let doc = HashMap::new();
        let report = enforcer.validate_create("users", &doc);
        assert!(!report.is_valid());
        let errors = &report.errors;
        assert_eq!(errors.len(), 1, "only email is required");
        assert_eq!(errors[0].field, "email");
        assert_eq!(errors[0].code, "REQUIRED");
    }

    #[tokio::test]
    async fn default_value_is_injected_when_optional_field_missing() {
        let enforcer = make_enforcer_offline(users_schema(), true);
        let doc = HashMap::from([("email".to_string(), json!("darsh@example.com"))]);
        let report = enforcer.validate_create("users", &doc);
        assert!(report.is_valid(), "errors: {:?}", report.errors);
        assert_eq!(report.document.get("age"), Some(&json!(0)));
    }

    #[tokio::test]
    async fn type_mismatch_is_flagged() {
        let enforcer = make_enforcer_offline(users_schema(), true);
        let doc = HashMap::from([
            ("email".to_string(), json!(123)),
            ("age".to_string(), json!("thirty")),
        ]);
        let report = enforcer.validate_create("users", &doc);
        assert_eq!(report.errors.len(), 2);
        for err in &report.errors {
            assert_eq!(err.code, "TYPE_MISMATCH");
        }
    }

    #[tokio::test]
    async fn link_type_requires_uuid_string() {
        let enforcer = make_enforcer_offline(users_schema(), true);
        let good = HashMap::from([
            ("email".to_string(), json!("ok@example.com")),
            (
                "profile_id".to_string(),
                json!("550e8400-e29b-41d4-a716-446655440000"),
            ),
        ]);
        assert!(enforcer.validate_create("users", &good).is_valid());

        let bad = HashMap::from([
            ("email".to_string(), json!("ok@example.com")),
            ("profile_id".to_string(), json!("not-a-uuid")),
        ]);
        let report = enforcer.validate_create("users", &bad);
        assert!(!report.is_valid());
        assert_eq!(report.errors[0].field, "profile_id");
    }

    #[tokio::test]
    async fn unknown_collection_passes_through_without_errors() {
        let enforcer = make_enforcer_offline(users_schema(), true);
        let doc = HashMap::from([("x".to_string(), json!(1))]);
        let report = enforcer.validate_create("posts", &doc);
        assert!(report.is_valid());
    }

    #[tokio::test]
    async fn patch_only_type_checks_supplied_fields() {
        let enforcer = make_enforcer_offline(users_schema(), true);
        // Email (required) is omitted — patch mode allows this.
        let patch = HashMap::from([("age".to_string(), json!(30))]);
        assert!(enforcer.validate_patch("users", &patch).is_valid());

        let bad_patch = HashMap::from([("age".to_string(), json!("thirty"))]);
        let report = enforcer.validate_patch("users", &bad_patch);
        assert!(!report.is_valid());
        assert_eq!(report.errors[0].code, "TYPE_MISMATCH");
    }

    #[tokio::test]
    async fn error_payload_matches_slice_contract() {
        let enforcer = make_enforcer_offline(users_schema(), true);
        let doc = HashMap::new();
        let report = enforcer.validate_create("users", &doc);
        let payload = report.error_payload();
        let errors = payload
            .get("errors")
            .and_then(|v| v.as_array())
            .expect("errors array");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["field"], "email");
        assert_eq!(errors[0]["code"], "REQUIRED");
    }

    #[test]
    fn value_type_parse_accepts_all_slice_tags() {
        for tag in [
            "string",
            "number",
            "boolean",
            "datetime",
            "uuid",
            "array",
            "object",
            "geometry",
            "vector",
            "link:users",
        ] {
            assert!(
                StrictValueType::parse(tag).is_some(),
                "tag {tag} must parse"
            );
        }
        assert!(StrictValueType::parse("bogus").is_none());
        assert!(
            StrictValueType::parse("link:").is_none(),
            "empty link target is invalid"
        );
    }
}
