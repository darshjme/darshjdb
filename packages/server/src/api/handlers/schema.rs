//! Schema management handlers: DEFINE TABLE / FIELD / INDEX, migration history.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::rest::AppState;

use super::helpers::{negotiate_response, negotiate_response_status};

// ---------------------------------------------------------------------------
// List tables
// ---------------------------------------------------------------------------

/// `GET /api/schema/tables` -- List all defined table schemas.
pub async fn schema_list_tables(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    let tables = registry.list_tables();
    let response = serde_json::json!({
        "tables": tables,
        "count": tables.len(),
    });
    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Define table
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DefineTableRequest {
    name: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    fields: Option<HashMap<String, crate::schema::FieldDefinition>>,
    #[serde(default)]
    indexes: Option<HashMap<String, crate::schema::IndexDefinition>>,
}

/// `POST /api/schema/tables` -- Define (create or replace) a table schema.
pub async fn schema_define_table(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<DefineTableRequest>,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    let mode = body
        .mode
        .as_deref()
        .and_then(crate::schema::SchemaMode::parse)
        .unwrap_or(crate::schema::SchemaMode::Schemaless);

    let mut schema = match mode {
        crate::schema::SchemaMode::Schemafull => crate::schema::TableSchema::schemafull(&body.name),
        crate::schema::SchemaMode::Schemaless => crate::schema::TableSchema::schemaless(&body.name),
        crate::schema::SchemaMode::Mixed => crate::schema::TableSchema::mixed(&body.name),
    };

    if let Some(existing) = registry.get(&body.name) {
        schema.version = existing.version + 1;
    }

    if let Some(fields) = body.fields {
        schema.fields = fields;
    }
    if let Some(indexes) = body.indexes {
        schema.indexes = indexes;
    }

    registry
        .define_table(schema.clone())
        .await
        .map_err(|e| ApiError::internal(format!("Failed to define table: {e}")))?;

    let response = serde_json::json!({
        "status": "ok",
        "table": schema,
    });
    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

// ---------------------------------------------------------------------------
// Remove table
// ---------------------------------------------------------------------------

/// `DELETE /api/schema/tables/:table` -- Remove a table schema.
pub async fn schema_remove_table(
    State(state): State<AppState>,
    Path(table): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    registry
        .remove_table(&table)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to remove table schema: {e}")))?;

    let response = serde_json::json!({ "status": "ok", "table": table, "action": "removed" });
    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Define field
// ---------------------------------------------------------------------------

/// `POST /api/schema/tables/:table/fields` -- Define a field on a table.
pub async fn schema_define_field(
    State(state): State<AppState>,
    Path(table): Path<String>,
    headers: HeaderMap,
    axum::Json(field): axum::Json<crate::schema::FieldDefinition>,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    registry
        .define_field(&table, field.clone())
        .await
        .map_err(|e| ApiError::internal(format!("Failed to define field: {e}")))?;

    let response = serde_json::json!({
        "status": "ok",
        "table": table,
        "field": field,
    });
    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

// ---------------------------------------------------------------------------
// Remove field
// ---------------------------------------------------------------------------

/// `DELETE /api/schema/tables/:table/fields/:field` -- Remove a field.
pub async fn schema_remove_field(
    State(state): State<AppState>,
    Path((table, field)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    registry
        .remove_field(&table, &field)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to remove field: {e}")))?;

    let response = serde_json::json!({ "status": "ok", "table": table, "field": field, "action": "removed" });
    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Define index
// ---------------------------------------------------------------------------

/// `POST /api/schema/tables/:table/indexes` -- Define an index on a table.
pub async fn schema_define_index(
    State(state): State<AppState>,
    Path(table): Path<String>,
    headers: HeaderMap,
    axum::Json(index): axum::Json<crate::schema::IndexDefinition>,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    registry
        .define_index(&table, index.clone())
        .await
        .map_err(|e| ApiError::internal(format!("Failed to define index: {e}")))?;

    let response = serde_json::json!({
        "status": "ok",
        "table": table,
        "index": index,
    });
    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

// ---------------------------------------------------------------------------
// Migration history
// ---------------------------------------------------------------------------

/// `GET /api/schema/tables/:table/migrations` -- View migration history.
pub async fn schema_migration_history(
    State(state): State<AppState>,
    Path(table): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let pool = state.pool.clone();
    let engine = crate::schema::migration::SchemaMigrationEngine::new(pool)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to init migration engine: {e}")))?;

    let history = engine
        .get_history(&table)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to fetch migration history: {e}")))?;

    let response = serde_json::json!({
        "table": table,
        "migrations": history,
        "count": history.len(),
    });
    Ok(negotiate_response(&headers, &response))
}
