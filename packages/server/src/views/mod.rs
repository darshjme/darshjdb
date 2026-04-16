//! Multi-view system for DarshJDB.
//!
//! Views define named, reusable lenses over an entity type — each with its
//! own filters, sorts, field ordering, and display configuration. Inspired
//! by Teable's Grid / Form / Kanban / Gallery / Calendar paradigm, but
//! stored as EAV triples in the existing triple store so views participate
//! in the same transaction, audit, and reactive infrastructure as data.
//!
//! # Storage layout
//!
//! Each view is an entity with prefix `view:{uuid}` and attributes:
//! - `view/name`        — human-readable name (String)
//! - `view/kind`        — variant tag (Grid | Form | Kanban | Gallery | Calendar)
//! - `view/table`       — the entity type this view queries
//! - `view/config`      — JSON blob with filters, sorts, field_order, etc.
//! - `view/created_by`  — UUID of the creating user
//! - `view/created_at`  — ISO-8601 timestamp
//! - `view/updated_at`  — ISO-8601 timestamp

pub mod handlers;
pub mod query;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::triple_store::{PgTripleStore, Triple, TripleInput, TripleStore};

// ── Core types ─────────────────────────────────────────────────────

/// Unique identifier for a view.
pub type ViewId = Uuid;

/// The kind of view determines which UI layout and which config fields
/// are relevant (e.g. `kanban_field` only matters for Kanban views).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    Grid,
    Form,
    Kanban,
    Gallery,
    Calendar,
}

impl ViewKind {
    /// Parse from a stored string value.
    pub fn from_str_value(s: &str) -> Option<Self> {
        match s {
            "grid" => Some(Self::Grid),
            "form" => Some(Self::Form),
            "kanban" => Some(Self::Kanban),
            "gallery" => Some(Self::Gallery),
            "calendar" => Some(Self::Calendar),
            _ => None,
        }
    }

    /// Serialise to a storage-friendly string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::Form => "form",
            Self::Kanban => "kanban",
            Self::Gallery => "gallery",
            Self::Calendar => "calendar",
        }
    }
}

impl std::fmt::Display for ViewKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Filter / Sort clauses ──────────────────────────────────────────

/// Comparison operator for a filter clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    Contains,
    IsEmpty,
    IsNotEmpty,
}

/// A single filter predicate applied by the view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterClause {
    /// Attribute name to filter on.
    pub field: String,
    /// Comparison operator.
    pub op: FilterOp,
    /// Value to compare against. `null` is valid for IsEmpty / IsNotEmpty.
    #[serde(default)]
    pub value: serde_json::Value,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDir {
    Asc,
    Desc,
}

/// A sort instruction within a view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortClause {
    /// Attribute name to sort by.
    pub field: String,
    /// Sort direction.
    pub direction: SortDir,
}

// ── View config ────────────────────────────────────────────────────

/// Full configuration for a stored view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewConfig {
    /// Unique view id.
    pub id: ViewId,
    /// Human-readable name.
    pub name: String,
    /// View variant.
    pub kind: ViewKind,
    /// Entity type this view queries (e.g. `"Task"`, `"Contact"`).
    pub table_entity_type: String,
    /// Built-in filter predicates.
    #[serde(default)]
    pub filters: Vec<FilterClause>,
    /// Default sort order.
    #[serde(default)]
    pub sorts: Vec<SortClause>,
    /// Ordered list of visible field names.
    #[serde(default)]
    pub field_order: Vec<String>,
    /// Fields excluded from display.
    #[serde(default)]
    pub hidden_fields: Vec<String>,
    /// Group-by field (Grid view grouping).
    #[serde(default)]
    pub group_by: Option<String>,
    /// Field used as the Kanban column key.
    #[serde(default)]
    pub kanban_field: Option<String>,
    /// Date/datetime field used for Calendar layout.
    #[serde(default)]
    pub calendar_field: Option<String>,
    /// Field whose value determines row/card color.
    #[serde(default)]
    pub color_field: Option<String>,
    /// Pixel height of each row (Grid view).
    #[serde(default)]
    pub row_height: Option<u32>,
    /// User who created this view.
    pub created_by: Uuid,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last modification timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Subset of fields that can be updated via `PATCH`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ViewUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<ViewKind>,
    #[serde(default)]
    pub filters: Option<Vec<FilterClause>>,
    #[serde(default)]
    pub sorts: Option<Vec<SortClause>>,
    #[serde(default)]
    pub field_order: Option<Vec<String>>,
    #[serde(default)]
    pub hidden_fields: Option<Vec<String>>,
    #[serde(default)]
    pub group_by: Option<Option<String>>,
    #[serde(default)]
    pub kanban_field: Option<Option<String>>,
    #[serde(default)]
    pub calendar_field: Option<Option<String>>,
    #[serde(default)]
    pub color_field: Option<Option<String>>,
    #[serde(default)]
    pub row_height: Option<Option<u32>>,
}

/// Payload for creating a new view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateViewRequest {
    pub name: String,
    pub kind: ViewKind,
    pub table_entity_type: String,
    #[serde(default)]
    pub filters: Vec<FilterClause>,
    #[serde(default)]
    pub sorts: Vec<SortClause>,
    #[serde(default)]
    pub field_order: Vec<String>,
    #[serde(default)]
    pub hidden_fields: Vec<String>,
    #[serde(default)]
    pub group_by: Option<String>,
    #[serde(default)]
    pub kanban_field: Option<String>,
    #[serde(default)]
    pub calendar_field: Option<String>,
    #[serde(default)]
    pub color_field: Option<String>,
    #[serde(default)]
    pub row_height: Option<u32>,
}

// ── ViewStore trait ────────────────────────────────────────────────

/// Async CRUD interface for view persistence.
///
/// Views are stored as triples so they benefit from the same audit,
/// reactivity, and point-in-time read capabilities as regular data.
pub trait ViewStore: Send + Sync {
    fn create_view(
        &self,
        req: CreateViewRequest,
        created_by: Uuid,
    ) -> impl std::future::Future<Output = Result<ViewConfig>> + Send;

    fn get_view(&self, id: ViewId) -> impl std::future::Future<Output = Result<ViewConfig>> + Send;

    fn list_views(
        &self,
        table_entity_type: &str,
    ) -> impl std::future::Future<Output = Result<Vec<ViewConfig>>> + Send;

    fn update_view(
        &self,
        id: ViewId,
        update: ViewUpdate,
    ) -> impl std::future::Future<Output = Result<ViewConfig>> + Send;

    fn delete_view(&self, id: ViewId) -> impl std::future::Future<Output = Result<()>> + Send;
}

// ── PgViewStore ────────────────────────────────────────────────────

/// View store backed by the existing [`PgTripleStore`].
///
/// Each view occupies a single entity `view:{uuid}` with attributes
/// `view/name`, `view/kind`, `view/table`, `view/config`, `view/created_by`,
/// `view/created_at`, and `view/updated_at`.
#[derive(Clone)]
pub struct PgViewStore {
    store: PgTripleStore,
}

impl PgViewStore {
    pub fn new(store: PgTripleStore) -> Self {
        Self { store }
    }

    /// Build the entity id for a view.
    ///
    /// We derive a deterministic UUID from the view id by hashing with a
    /// fixed namespace prefix, then setting UUID v4 variant bits. This
    /// avoids needing the `uuid/v5` feature while remaining collision-free.
    fn entity_id(id: ViewId) -> Uuid {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(format!("darshjdb:view:{id}").as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash[..16]);
        // Set version (4) and variant (RFC 4122) bits so it's a valid UUID.
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes)
    }

    /// Reconstruct a `ViewConfig` from a set of triples for a single view entity.
    fn reconstruct(entity_id: Uuid, triples: &[Triple]) -> Result<ViewConfig> {
        let attr = |name: &str| -> Option<&serde_json::Value> {
            triples
                .iter()
                .find(|t| t.entity_id == entity_id && t.attribute == name && !t.retracted)
                .map(|t| &t.value)
        };

        let id_str = attr("view/id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DarshJError::Internal("view missing view/id".into()))?;
        let id = Uuid::parse_str(id_str)
            .map_err(|e| DarshJError::Internal(format!("invalid view/id: {e}")))?;

        let name = attr("view/name")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled")
            .to_string();

        let kind_str = attr("view/kind").and_then(|v| v.as_str()).unwrap_or("grid");
        let kind = ViewKind::from_str_value(kind_str)
            .ok_or_else(|| DarshJError::Internal(format!("unknown view kind: {kind_str}")))?;

        let table_entity_type = attr("view/table")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Config blob stores filters, sorts, field_order, hidden_fields, and
        // display-specific settings.
        let config_val = attr("view/config")
            .cloned()
            .unwrap_or(serde_json::json!({}));
        let config_obj = config_val.as_object();

        let filters: Vec<FilterClause> = config_obj
            .and_then(|c| c.get("filters"))
            .map(|v| serde_json::from_value(v.clone()).unwrap_or_default())
            .unwrap_or_default();

        let sorts: Vec<SortClause> = config_obj
            .and_then(|c| c.get("sorts"))
            .map(|v| serde_json::from_value(v.clone()).unwrap_or_default())
            .unwrap_or_default();

        let field_order: Vec<String> = config_obj
            .and_then(|c| c.get("field_order"))
            .map(|v| serde_json::from_value(v.clone()).unwrap_or_default())
            .unwrap_or_default();

        let hidden_fields: Vec<String> = config_obj
            .and_then(|c| c.get("hidden_fields"))
            .map(|v| serde_json::from_value(v.clone()).unwrap_or_default())
            .unwrap_or_default();

        let group_by = config_obj
            .and_then(|c| c.get("group_by"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let kanban_field = config_obj
            .and_then(|c| c.get("kanban_field"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let calendar_field = config_obj
            .and_then(|c| c.get("calendar_field"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let color_field = config_obj
            .and_then(|c| c.get("color_field"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let row_height = config_obj
            .and_then(|c| c.get("row_height"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);

        let created_by_str = attr("view/created_by")
            .and_then(|v| v.as_str())
            .unwrap_or("00000000-0000-0000-0000-000000000000");
        let created_by = Uuid::parse_str(created_by_str).unwrap_or(Uuid::nil());

        let created_at = attr("view/created_at")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .unwrap_or_else(Utc::now);

        let updated_at = attr("view/updated_at")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .unwrap_or_else(Utc::now);

        Ok(ViewConfig {
            id,
            name,
            kind,
            table_entity_type,
            filters,
            sorts,
            field_order,
            hidden_fields,
            group_by,
            kanban_field,
            calendar_field,
            color_field,
            row_height,
            created_by,
            created_at,
            updated_at,
        })
    }

    /// Build the triple inputs for persisting a full view config.
    fn build_triples(entity_id: Uuid, view: &ViewConfig) -> Vec<TripleInput> {
        let config_blob = serde_json::json!({
            "filters": view.filters,
            "sorts": view.sorts,
            "field_order": view.field_order,
            "hidden_fields": view.hidden_fields,
            "group_by": view.group_by,
            "kanban_field": view.kanban_field,
            "calendar_field": view.calendar_field,
            "color_field": view.color_field,
            "row_height": view.row_height,
        });

        vec![
            TripleInput {
                entity_id,
                attribute: "view/id".into(),
                value: serde_json::Value::String(view.id.to_string()),
                value_type: 0, // string
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "view/name".into(),
                value: serde_json::Value::String(view.name.clone()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "view/kind".into(),
                value: serde_json::Value::String(view.kind.as_str().to_string()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "view/table".into(),
                value: serde_json::Value::String(view.table_entity_type.clone()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "view/config".into(),
                value: config_blob,
                value_type: 5, // json
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "view/created_by".into(),
                value: serde_json::Value::String(view.created_by.to_string()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "view/created_at".into(),
                value: serde_json::Value::String(view.created_at.to_rfc3339()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id,
                attribute: "view/updated_at".into(),
                value: serde_json::Value::String(view.updated_at.to_rfc3339()),
                value_type: 0,
                ttl_seconds: None,
            },
        ]
    }

    /// Retract all attributes of a view entity so we can overwrite cleanly.
    async fn retract_all(&self, entity_id: Uuid) -> Result<()> {
        let attrs = [
            "view/id",
            "view/name",
            "view/kind",
            "view/table",
            "view/config",
            "view/created_by",
            "view/created_at",
            "view/updated_at",
        ];
        for attr in attrs {
            self.store.retract(entity_id, attr).await?;
        }
        Ok(())
    }
}

impl ViewStore for PgViewStore {
    async fn create_view(&self, req: CreateViewRequest, created_by: Uuid) -> Result<ViewConfig> {
        let now = Utc::now();
        let id = Uuid::new_v4();

        if req.name.trim().is_empty() {
            return Err(DarshJError::InvalidQuery(
                "view name must not be empty".into(),
            ));
        }
        if req.table_entity_type.trim().is_empty() {
            return Err(DarshJError::InvalidQuery(
                "table_entity_type must not be empty".into(),
            ));
        }

        let view = ViewConfig {
            id,
            name: req.name,
            kind: req.kind,
            table_entity_type: req.table_entity_type,
            filters: req.filters,
            sorts: req.sorts,
            field_order: req.field_order,
            hidden_fields: req.hidden_fields,
            group_by: req.group_by,
            kanban_field: req.kanban_field,
            calendar_field: req.calendar_field,
            color_field: req.color_field,
            row_height: req.row_height,
            created_by,
            created_at: now,
            updated_at: now,
        };

        let entity_id = Self::entity_id(id);
        let triples = Self::build_triples(entity_id, &view);
        self.store.set_triples(&triples).await?;

        Ok(view)
    }

    async fn get_view(&self, id: ViewId) -> Result<ViewConfig> {
        let entity_id = Self::entity_id(id);
        let triples = self.store.get_entity(entity_id).await?;
        if triples.is_empty() {
            return Err(DarshJError::EntityNotFound(id));
        }
        Self::reconstruct(entity_id, &triples)
    }

    async fn list_views(&self, table_entity_type: &str) -> Result<Vec<ViewConfig>> {
        let table_val = serde_json::Value::String(table_entity_type.to_string());
        let triples = self
            .store
            .query_by_attribute("view/table", Some(&table_val))
            .await?;

        let mut views = Vec::new();
        for triple in &triples {
            let entity_triples = self.store.get_entity(triple.entity_id).await?;
            match Self::reconstruct(triple.entity_id, &entity_triples) {
                Ok(v) => views.push(v),
                Err(e) => {
                    tracing::warn!(
                        entity_id = %triple.entity_id,
                        error = %e,
                        "skipping malformed view entity"
                    );
                }
            }
        }

        // Sort by created_at ascending for stable ordering.
        views.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(views)
    }

    async fn update_view(&self, id: ViewId, update: ViewUpdate) -> Result<ViewConfig> {
        let mut view = self.get_view(id).await?;

        if let Some(name) = update.name {
            if name.trim().is_empty() {
                return Err(DarshJError::InvalidQuery(
                    "view name must not be empty".into(),
                ));
            }
            view.name = name;
        }
        if let Some(kind) = update.kind {
            view.kind = kind;
        }
        if let Some(filters) = update.filters {
            view.filters = filters;
        }
        if let Some(sorts) = update.sorts {
            view.sorts = sorts;
        }
        if let Some(field_order) = update.field_order {
            view.field_order = field_order;
        }
        if let Some(hidden_fields) = update.hidden_fields {
            view.hidden_fields = hidden_fields;
        }
        if let Some(group_by) = update.group_by {
            view.group_by = group_by;
        }
        if let Some(kanban_field) = update.kanban_field {
            view.kanban_field = kanban_field;
        }
        if let Some(calendar_field) = update.calendar_field {
            view.calendar_field = calendar_field;
        }
        if let Some(color_field) = update.color_field {
            view.color_field = color_field;
        }
        if let Some(row_height) = update.row_height {
            view.row_height = row_height;
        }

        view.updated_at = Utc::now();

        let entity_id = Self::entity_id(id);
        // Retract old triples then write new ones atomically.
        self.retract_all(entity_id).await?;
        let triples = Self::build_triples(entity_id, &view);
        self.store.set_triples(&triples).await?;

        Ok(view)
    }

    async fn delete_view(&self, id: ViewId) -> Result<()> {
        let entity_id = Self::entity_id(id);
        // Verify the view exists first.
        let triples = self.store.get_entity(entity_id).await?;
        if triples.is_empty() {
            return Err(DarshJError::EntityNotFound(id));
        }
        self.retract_all(entity_id).await
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_kind_roundtrip() {
        for kind in [
            ViewKind::Grid,
            ViewKind::Form,
            ViewKind::Kanban,
            ViewKind::Gallery,
            ViewKind::Calendar,
        ] {
            let s = kind.as_str();
            let parsed = ViewKind::from_str_value(s).expect("should parse");
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn view_kind_display() {
        assert_eq!(ViewKind::Grid.to_string(), "grid");
        assert_eq!(ViewKind::Kanban.to_string(), "kanban");
    }

    #[test]
    fn view_kind_serde_roundtrip() {
        let kind = ViewKind::Calendar;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"calendar\"");
        let parsed: ViewKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }

    #[test]
    fn filter_op_serde() {
        let clause = FilterClause {
            field: "status".into(),
            op: FilterOp::Eq,
            value: serde_json::json!("active"),
        };
        let json = serde_json::to_value(&clause).unwrap();
        assert_eq!(json["op"], "eq");
        let parsed: FilterClause = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.op, FilterOp::Eq);
    }

    #[test]
    fn sort_clause_serde() {
        let clause = SortClause {
            field: "created_at".into(),
            direction: SortDir::Desc,
        };
        let json = serde_json::to_value(&clause).unwrap();
        assert_eq!(json["direction"], "desc");
    }

    #[test]
    fn create_view_request_serde() {
        let json = serde_json::json!({
            "name": "Active Tasks",
            "kind": "kanban",
            "table_entity_type": "Task",
            "kanban_field": "status",
            "filters": [{"field": "archived", "op": "eq", "value": false}],
            "sorts": [{"field": "priority", "direction": "asc"}]
        });
        let req: CreateViewRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.kind, ViewKind::Kanban);
        assert_eq!(req.kanban_field, Some("status".into()));
        assert_eq!(req.filters.len(), 1);
        assert_eq!(req.sorts.len(), 1);
    }

    #[test]
    fn view_update_partial() {
        let json = serde_json::json!({
            "name": "Renamed View",
            "filters": [{"field": "done", "op": "eq", "value": true}]
        });
        let update: ViewUpdate = serde_json::from_value(json).unwrap();
        assert_eq!(update.name.as_deref(), Some("Renamed View"));
        assert!(update.filters.is_some());
        assert!(update.kind.is_none());
        assert!(update.sorts.is_none());
    }

    #[test]
    fn entity_id_deterministic() {
        let id = Uuid::parse_str("a1b2c3d4-e5f6-7890-abcd-ef1234567890").unwrap();
        let eid1 = PgViewStore::entity_id(id);
        let eid2 = PgViewStore::entity_id(id);
        assert_eq!(eid1, eid2);
    }

    #[test]
    fn reconstruct_from_triples() {
        let view_id = Uuid::new_v4();
        let entity_id = PgViewStore::entity_id(view_id);
        let now = Utc::now();

        let make_triple = |attr: &str, value: serde_json::Value| Triple {
            id: 0,
            entity_id,
            attribute: attr.to_string(),
            value,
            value_type: 0,
            tx_id: 1,
            created_at: now,
            retracted: false,
            expires_at: None,
        };

        let triples = vec![
            make_triple("view/id", serde_json::json!(view_id.to_string())),
            make_triple("view/name", serde_json::json!("Test View")),
            make_triple("view/kind", serde_json::json!("grid")),
            make_triple("view/table", serde_json::json!("Contact")),
            make_triple(
                "view/config",
                serde_json::json!({
                    "filters": [],
                    "sorts": [{"field": "name", "direction": "asc"}],
                    "field_order": ["name", "email"],
                    "hidden_fields": ["internal_id"],
                }),
            ),
            make_triple(
                "view/created_by",
                serde_json::json!(Uuid::nil().to_string()),
            ),
            make_triple("view/created_at", serde_json::json!(now.to_rfc3339())),
            make_triple("view/updated_at", serde_json::json!(now.to_rfc3339())),
        ];

        let view = PgViewStore::reconstruct(entity_id, &triples).unwrap();
        assert_eq!(view.id, view_id);
        assert_eq!(view.name, "Test View");
        assert_eq!(view.kind, ViewKind::Grid);
        assert_eq!(view.table_entity_type, "Contact");
        assert_eq!(view.sorts.len(), 1);
        assert_eq!(view.field_order, vec!["name", "email"]);
        assert_eq!(view.hidden_fields, vec!["internal_id"]);
    }

    #[test]
    fn build_triples_roundtrip() {
        let view_id = Uuid::new_v4();
        let entity_id = PgViewStore::entity_id(view_id);
        let now = Utc::now();

        let view = ViewConfig {
            id: view_id,
            name: "Kanban Board".into(),
            kind: ViewKind::Kanban,
            table_entity_type: "Task".into(),
            filters: vec![FilterClause {
                field: "status".into(),
                op: FilterOp::Neq,
                value: serde_json::json!("archived"),
            }],
            sorts: vec![],
            field_order: vec!["title".into(), "status".into()],
            hidden_fields: vec![],
            group_by: None,
            kanban_field: Some("status".into()),
            calendar_field: None,
            color_field: Some("priority".into()),
            row_height: None,
            created_by: Uuid::nil(),
            created_at: now,
            updated_at: now,
        };

        let triples_input = PgViewStore::build_triples(entity_id, &view);
        assert_eq!(triples_input.len(), 8);

        // Convert TripleInput -> Triple for reconstruction test.
        let triples: Vec<Triple> = triples_input
            .into_iter()
            .enumerate()
            .map(|(i, t)| Triple {
                id: i as i64,
                entity_id: t.entity_id,
                attribute: t.attribute,
                value: t.value,
                value_type: t.value_type,
                tx_id: 1,
                created_at: now,
                retracted: false,
                expires_at: None,
            })
            .collect();

        let reconstructed = PgViewStore::reconstruct(entity_id, &triples).unwrap();
        assert_eq!(reconstructed.id, view_id);
        assert_eq!(reconstructed.name, "Kanban Board");
        assert_eq!(reconstructed.kind, ViewKind::Kanban);
        assert_eq!(reconstructed.kanban_field, Some("status".into()));
        assert_eq!(reconstructed.color_field, Some("priority".into()));
        assert_eq!(reconstructed.filters.len(), 1);
    }
}
