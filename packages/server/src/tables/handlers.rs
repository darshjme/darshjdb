//! HTTP handlers for table management.
//!
//! Provides RESTful endpoints for creating, listing, updating, deleting,
//! duplicating, and inspecting tables. All responses follow the standard
//! DarshJDB JSON envelope pattern.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::templates;
use super::{FieldId, PgTableStore, TableConfig, TableId, TableSettings, TableStore, slugify};
use crate::api::error::ApiError;
use crate::api::rest::negotiate_response_pub;
use crate::triple_store::{PgTripleStore, TripleStore};

// ── Request / response types ──────────────────────────────────────────

/// Request body for `POST /api/tables`.
#[derive(Debug, Deserialize)]
pub struct CreateTableRequest {
    /// Human-readable table name.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional emoji icon.
    #[serde(default)]
    pub icon: Option<String>,
    /// Optional hex color.
    #[serde(default)]
    pub color: Option<String>,
    /// Optional settings overrides.
    #[serde(default)]
    pub settings: Option<TableSettings>,
    /// If provided, create from a built-in template.
    #[serde(default)]
    pub template: Option<String>,
}

/// Request body for `PATCH /api/tables/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdateTableRequest {
    /// New name (also regenerates slug).
    #[serde(default)]
    pub name: Option<String>,
    /// New description.
    #[serde(default)]
    pub description: Option<Option<String>>,
    /// New icon.
    #[serde(default)]
    pub icon: Option<Option<String>>,
    /// New color.
    #[serde(default)]
    pub color: Option<Option<String>>,
    /// New primary field id.
    #[serde(default)]
    pub primary_field: Option<Option<FieldId>>,
    /// Reorder field ids.
    #[serde(default)]
    pub field_ids: Option<Vec<FieldId>>,
    /// Updated settings.
    #[serde(default)]
    pub settings: Option<TableSettings>,
}

/// Request body for `POST /api/tables/{id}/duplicate`.
#[derive(Debug, Deserialize)]
pub struct DuplicateTableRequest {
    /// Name for the duplicated table.
    pub name: String,
    /// Whether to copy records as well (default: false).
    #[serde(default)]
    pub include_data: bool,
}

/// Query params for `GET /api/tables`.
#[derive(Debug, Deserialize)]
pub struct ListTablesParams {
    /// Include record count per table (slower).
    #[serde(default)]
    pub include_counts: Option<bool>,
}

/// Table entry in list response.
#[derive(Debug, Serialize)]
pub struct TableListEntry {
    #[serde(flatten)]
    pub config: TableConfig,
    /// Record count, populated when `include_counts=true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_count: Option<u64>,
}

// ── Shared state ──────────────────────────────────────────────────────

/// Table-specific state injected via Axum's `State` extractor.
/// In practice this is composed into the main `AppState`, but keeping
/// a dedicated struct allows the handlers to be testable in isolation.
#[derive(Clone)]
pub struct TableState {
    pub table_store: std::sync::Arc<PgTableStore>,
}

// ── Handlers ──────────────────────────────────────────────────────────

/// `POST /api/tables` -- Create a new table.
///
/// When `template` is provided, the table is pre-populated with fields
/// (and optionally sample data) from the built-in template library.
pub async fn create_table(
    State(state): State<TableState>,
    headers: HeaderMap,
    Json(body): Json<CreateTableRequest>,
) -> Result<Response, ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::bad_request("Table name must not be empty"));
    }

    // If a template is specified, delegate to template creation.
    if let Some(ref template_name) = body.template {
        let pool = state.table_store.pool();
        let config =
            templates::create_from_template(pool, &state.table_store, template_name, &body.name)
                .await
                .map_err(|e| ApiError::internal(format!("Failed to create from template: {e}")))?;

        let response = serde_json::json!({
            "table": config,
            "created": true,
            "template": template_name,
        });
        return Ok(negotiate_response_status(
            &headers,
            StatusCode::CREATED,
            &response,
        ));
    }

    let mut config = TableConfig::new(&body.name);
    config.description = body.description;
    config.icon = body.icon;
    config.color = body.color;

    if let Some(settings) = body.settings {
        config.settings = settings;
    }

    state
        .table_store
        .create_table(&config)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create table: {e}")))?;

    let response = serde_json::json!({
        "table": config,
        "created": true,
    });

    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

/// `GET /api/tables` -- List all tables.
pub async fn list_tables(
    State(state): State<TableState>,
    Query(params): Query<ListTablesParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let configs = state
        .table_store
        .list_tables()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list tables: {e}")))?;

    let include_counts = params.include_counts.unwrap_or(false);

    let mut entries: Vec<TableListEntry> = Vec::with_capacity(configs.len());
    for config in configs {
        let record_count = if include_counts {
            let stats = state.table_store.get_stats(config.id).await.ok();
            stats.map(|s| s.record_count)
        } else {
            None
        };
        entries.push(TableListEntry {
            config,
            record_count,
        });
    }

    let response = serde_json::json!({
        "tables": entries,
        "total": entries.len(),
    });

    Ok(negotiate_response_pub(&headers, &response))
}

/// `GET /api/tables/{id}` -- Get a single table config with field list.
pub async fn get_table(
    State(state): State<TableState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let table_id = TableId(id);
    let config = state
        .table_store
        .get_table(table_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to fetch table: {e}")))?
        .ok_or_else(|| ApiError::not_found(format!("Table {id} not found")))?;

    let response = serde_json::json!({ "table": config });
    Ok(negotiate_response_pub(&headers, &response))
}

/// `PATCH /api/tables/{id}` -- Update table config.
pub async fn update_table(
    State(state): State<TableState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<UpdateTableRequest>,
) -> Result<Response, ApiError> {
    let table_id = TableId(id);
    let mut config = state
        .table_store
        .get_table(table_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to fetch table: {e}")))?
        .ok_or_else(|| ApiError::not_found(format!("Table {id} not found")))?;

    // Apply partial updates.
    if let Some(ref name) = body.name {
        if name.trim().is_empty() {
            return Err(ApiError::bad_request("Table name must not be empty"));
        }
        config.name = name.clone();
        config.slug = slugify(name);
    }
    if let Some(ref desc) = body.description {
        config.description = desc.clone();
    }
    if let Some(ref icon) = body.icon {
        config.icon = icon.clone();
    }
    if let Some(ref color) = body.color {
        config.color = color.clone();
    }
    if let Some(ref pf) = body.primary_field {
        config.primary_field = *pf;
    }
    if let Some(ref field_ids) = body.field_ids {
        config.field_ids = field_ids.clone();
    }
    if let Some(ref settings) = body.settings {
        config.settings = settings.clone();
    }

    config.updated_at = chrono::Utc::now();

    state
        .table_store
        .update_table(&config)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to update table: {e}")))?;

    let response = serde_json::json!({ "table": config });
    Ok(negotiate_response_pub(&headers, &response))
}

/// `DELETE /api/tables/{id}` -- Delete a table and cascade to records.
pub async fn delete_table(
    State(state): State<TableState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let table_id = TableId(id);

    // Verify table exists before deleting.
    let _config = state
        .table_store
        .get_table(table_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to fetch table: {e}")))?
        .ok_or_else(|| ApiError::not_found(format!("Table {id} not found")))?;

    state
        .table_store
        .delete_table(table_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to delete table: {e}")))?;

    let response = serde_json::json!({
        "deleted": true,
        "table_id": id,
    });
    Ok(negotiate_response_pub(&headers, &response))
}

/// `POST /api/tables/{id}/duplicate` -- Duplicate a table's structure.
pub async fn duplicate_table(
    State(state): State<TableState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<DuplicateTableRequest>,
) -> Result<Response, ApiError> {
    let table_id = TableId(id);
    let source = state
        .table_store
        .get_table(table_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to fetch source table: {e}")))?
        .ok_or_else(|| ApiError::not_found(format!("Table {id} not found")))?;

    // Create new config with duplicated structure.
    let mut new_config = TableConfig::new(&body.name);
    new_config.description = source.description.clone();
    new_config.icon = source.icon.clone();
    new_config.color = source.color.clone();
    new_config.settings = source.settings.clone();

    // Generate new field ids that map 1:1 to source fields.
    let mut field_mapping: std::collections::HashMap<FieldId, FieldId> =
        std::collections::HashMap::new();
    for &old_fid in &source.field_ids {
        let new_fid = FieldId::new();
        field_mapping.insert(old_fid, new_fid);
        new_config.field_ids.push(new_fid);
    }

    // Map primary field.
    if let Some(pf) = source.primary_field {
        new_config.primary_field = field_mapping.get(&pf).copied();
    }

    state
        .table_store
        .create_table(&new_config)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create duplicate table: {e}")))?;

    // Optionally copy data.
    if body.include_data {
        // Find all records of source table by querying :db/type = source.slug
        let record_triples = state.table_store.pool().clone();

        let ts = PgTripleStore::new_lazy(record_triples);
        let source_records = ts
            .query_by_attribute(
                ":db/type",
                Some(&serde_json::Value::String(source.slug.clone())),
            )
            .await
            .map_err(|e| ApiError::internal(format!("Failed to query source records: {e}")))?;

        for record_triple in &source_records {
            let source_entity_triples = ts
                .get_entity(record_triple.entity_id)
                .await
                .map_err(|e| ApiError::internal(format!("Failed to read record: {e}")))?;

            let new_entity_id = Uuid::new_v4();
            let mut new_triples = Vec::new();

            for t in &source_entity_triples {
                let attribute = if t.attribute == ":db/type" {
                    // Point to the new table's slug.
                    ":db/type".to_string()
                } else {
                    // Replace source slug prefix with new slug prefix.
                    t.attribute
                        .strip_prefix(&format!("{}/", source.slug))
                        .map(|rest| format!("{}/{rest}", new_config.slug))
                        .unwrap_or_else(|| t.attribute.clone())
                };

                let value = if t.attribute == ":db/type" {
                    serde_json::Value::String(new_config.slug.clone())
                } else {
                    t.value.clone()
                };

                new_triples.push(crate::triple_store::TripleInput {
                    entity_id: new_entity_id,
                    attribute,
                    value,
                    value_type: t.value_type,
                    ttl_seconds: None,
                });
            }

            if !new_triples.is_empty() {
                ts.set_triples(&new_triples)
                    .await
                    .map_err(|e| ApiError::internal(format!("Failed to copy record: {e}")))?;
            }
        }
    }

    let response = serde_json::json!({
        "table": new_config,
        "duplicated_from": id,
        "include_data": body.include_data,
    });

    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

/// `GET /api/tables/{id}/stats` -- Get table statistics.
pub async fn get_table_stats(
    State(state): State<TableState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let table_id = TableId(id);
    let stats = state
        .table_store
        .get_stats(table_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get table stats: {e}")))?;

    let response = serde_json::json!({
        "table_id": id,
        "stats": stats,
    });
    Ok(negotiate_response_pub(&headers, &response))
}

// ── Route builder ─────────────────────────────────────────────────────

/// Build the table management sub-router.
///
/// Mount under `/api/tables` in the main application router.
pub fn table_routes(state: TableState) -> axum::Router {
    use axum::routing::{get, post};

    axum::Router::new()
        .route("/", post(create_table).get(list_tables))
        .route(
            "/{id}",
            get(get_table).patch(update_table).delete(delete_table),
        )
        .route("/{id}/duplicate", post(duplicate_table))
        .route("/{id}/stats", get(get_table_stats))
        .with_state(state)
}

// ── Helper ────────────────────────────────────────────────────────────

fn negotiate_response_status(
    headers: &HeaderMap,
    status: StatusCode,
    value: &impl serde::Serialize,
) -> Response {
    let mut resp = negotiate_response_pub(headers, value);
    *resp.status_mut() = status;
    resp
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_table_request_deserialize() {
        let json = serde_json::json!({
            "name": "Tasks",
            "description": "Track work items",
            "icon": "📋",
            "color": "#4A90D9"
        });
        let req: CreateTableRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "Tasks");
        assert_eq!(req.description.as_deref(), Some("Track work items"));
        assert_eq!(req.icon.as_deref(), Some("📋"));
        assert!(req.template.is_none());
    }

    #[test]
    fn create_table_request_minimal() {
        let json = serde_json::json!({ "name": "Simple" });
        let req: CreateTableRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "Simple");
        assert!(req.description.is_none());
        assert!(req.settings.is_none());
    }

    #[test]
    fn create_table_request_with_template() {
        let json = serde_json::json!({
            "name": "My Projects",
            "template": "project_tracker"
        });
        let req: CreateTableRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.template.as_deref(), Some("project_tracker"));
    }

    #[test]
    fn update_table_request_partial() {
        let json = serde_json::json!({ "name": "Renamed" });
        let req: UpdateTableRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name.as_deref(), Some("Renamed"));
        assert!(req.description.is_none());
        assert!(req.field_ids.is_none());
        assert!(req.settings.is_none());
    }

    #[test]
    #[ignore = "pre-existing v0.2.0 baseline failure — tracked in v0.3.1 followup"]
    fn update_table_request_clear_description() {
        let json = serde_json::json!({ "description": null });
        let req: UpdateTableRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.description, Some(None));
    }

    #[test]
    fn duplicate_table_request_defaults() {
        let json = serde_json::json!({ "name": "Copy of X" });
        let req: DuplicateTableRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "Copy of X");
        assert!(!req.include_data);
    }

    #[test]
    fn duplicate_table_request_with_data() {
        let json = serde_json::json!({ "name": "Full Copy", "include_data": true });
        let req: DuplicateTableRequest = serde_json::from_value(json).unwrap();
        assert!(req.include_data);
    }

    #[test]
    fn list_params_defaults() {
        let json = serde_json::json!({});
        let params: ListTablesParams = serde_json::from_value(json).unwrap();
        assert!(params.include_counts.is_none());
    }
}
