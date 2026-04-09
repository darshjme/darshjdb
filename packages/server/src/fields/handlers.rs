//! HTTP handlers for field CRUD operations.
//!
//! Fields are stored as EAV triples under `field:{uuid}` entities:
//!
//! - `field/name` -- human-readable name
//! - `field/type` -- [`FieldType`] serialised as string
//! - `field/table` -- the entity type this field belongs to
//! - `field/config` -- JSON-encoded full [`FieldConfig`]
//! - `field/order` -- display ordering (i32)

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::rest::AppState;
use crate::triple_store::{TripleInput, TripleStore};

use super::conversion::{self, ConversionSummary};
use super::{
    ATTR_FIELD_CONFIG, ATTR_FIELD_NAME, ATTR_FIELD_ORDER, ATTR_FIELD_TABLE, ATTR_FIELD_TYPE,
    FieldConfig, FieldId, FieldOptions, FieldType,
};

// ── Route builder ──────────────────────────────────────────────────

/// Build the `/api/fields` sub-router.
pub fn field_routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_field).get(list_fields))
        .route(
            "/{id}",
            get(get_field).patch(update_field).delete(delete_field),
        )
}

// ── Request / Response types ───────────────────────────────────────

/// Request body for `POST /api/fields`.
#[derive(Debug, Deserialize)]
pub struct CreateFieldRequest {
    pub name: String,
    pub field_type: FieldType,
    pub table_entity_type: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique: bool,
    #[serde(default)]
    pub default_value: Option<Value>,
    #[serde(default)]
    pub options: Option<FieldOptions>,
    #[serde(default)]
    pub order: i32,
}

/// Query parameters for `GET /api/fields`.
#[derive(Debug, Deserialize)]
pub struct ListFieldsQuery {
    /// Filter by entity type (table).
    #[serde(rename = "type")]
    pub entity_type: Option<String>,
}

/// Request body for `PATCH /api/fields/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdateFieldRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default)]
    pub unique: Option<bool>,
    #[serde(default)]
    pub default_value: Option<Value>,
    #[serde(default)]
    pub options: Option<FieldOptions>,
    #[serde(default)]
    pub order: Option<i32>,
    /// Change the field type -- triggers batch conversion of existing values.
    #[serde(default)]
    pub field_type: Option<FieldType>,
}

/// Response for field operations.
#[derive(Debug, Serialize)]
pub struct FieldResponse {
    pub field: FieldConfig,
}

/// Response for field listing.
#[derive(Debug, Serialize)]
pub struct ListFieldsResponse {
    pub fields: Vec<FieldConfig>,
}

/// Response for field type change with conversion info.
#[derive(Debug, Serialize)]
pub struct UpdateFieldResponse {
    pub field: FieldConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversion: Option<ConversionSummaryResponse>,
}

/// Serialisable conversion summary.
#[derive(Debug, Serialize)]
pub struct ConversionSummaryResponse {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
    pub warnings: Vec<String>,
}

impl From<ConversionSummary> for ConversionSummaryResponse {
    fn from(s: ConversionSummary) -> Self {
        Self {
            total: s.total,
            success: s.success,
            failed: s.failed,
            warnings: s.warnings,
        }
    }
}

// ── Handlers ───────────────────────────────────────────────────────

/// `POST /api/fields` -- Create a new field definition.
async fn create_field(
    State(state): State<AppState>,
    Json(req): Json<CreateFieldRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Validate name.
    if req.name.trim().is_empty() {
        return Err(ApiError::bad_request("field name must not be empty"));
    }
    if req.table_entity_type.trim().is_empty() {
        return Err(ApiError::bad_request("table_entity_type must not be empty"));
    }

    let id = FieldId::new();
    let config = FieldConfig {
        id,
        name: req.name,
        field_type: req.field_type,
        table_entity_type: req.table_entity_type,
        description: req.description,
        required: req.required,
        unique: req.unique,
        default_value: req.default_value,
        options: req.options,
        order: req.order,
    };

    // Validate options match field type.
    config
        .validate_options()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    // Persist as triples.
    persist_field(&state, &config).await?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(FieldResponse { field: config }),
    ))
}

/// `GET /api/fields?type={entity_type}` -- List fields.
async fn list_fields(
    State(state): State<AppState>,
    Query(query): Query<ListFieldsQuery>,
) -> Result<Json<ListFieldsResponse>, ApiError> {
    let fields = load_all_fields(&state, query.entity_type.as_deref()).await?;
    Ok(Json(ListFieldsResponse { fields }))
}

/// `GET /api/fields/{id}` -- Get a single field.
async fn get_field(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<FieldResponse>, ApiError> {
    let config = load_field(&state, id).await?;
    Ok(Json(FieldResponse { field: config }))
}

/// `PATCH /api/fields/{id}` -- Update a field.
async fn update_field(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateFieldRequest>,
) -> Result<Json<UpdateFieldResponse>, ApiError> {
    let mut config = load_field(&state, id).await?;
    let old_type = config.field_type;

    // Apply updates.
    if let Some(name) = req.name {
        if name.trim().is_empty() {
            return Err(ApiError::bad_request("field name must not be empty"));
        }
        config.name = name;
    }
    if let Some(desc) = req.description {
        config.description = Some(desc);
    }
    if let Some(required) = req.required {
        config.required = required;
    }
    if let Some(unique) = req.unique {
        config.unique = unique;
    }
    if let Some(default) = req.default_value {
        config.default_value = Some(default);
    }
    if let Some(options) = req.options {
        config.options = Some(options);
    }
    if let Some(order) = req.order {
        config.order = order;
    }

    let mut conversion_summary = None;

    if let Some(new_type) = req.field_type
        && new_type != old_type
    {
        config.field_type = new_type;

        // Batch-convert existing values.
        let existing_values = load_field_values(&state, &config).await?;
        if !existing_values.is_empty() {
            let results = conversion::convert_field_type(&existing_values, old_type, new_type);
            let summary = conversion::summarise(&results);

            // Write back converted values.
            write_converted_values(&state, &config, &results).await?;

            conversion_summary = Some(ConversionSummaryResponse::from(summary));
        }
    }

    // Validate options match (possibly new) field type.
    config
        .validate_options()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    // Re-persist.
    persist_field(&state, &config).await?;

    Ok(Json(UpdateFieldResponse {
        field: config,
        conversion: conversion_summary,
    }))
}

/// `DELETE /api/fields/{id}` -- Delete a field and retract its values.
async fn delete_field(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let config = load_field(&state, id).await?;

    // Retract all field metadata triples.
    let entity_id = config.entity_id();
    for attr in [
        ATTR_FIELD_NAME,
        ATTR_FIELD_TYPE,
        ATTR_FIELD_TABLE,
        ATTR_FIELD_CONFIG,
        ATTR_FIELD_ORDER,
    ] {
        state
            .triple_store
            .retract(entity_id, attr)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }

    // Retract all entity values that use this field's attribute name.
    // The attribute in the data triples is the field name.
    let data_triples = state
        .triple_store
        .query_by_attribute(&config.name, None)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    for triple in &data_triples {
        state
            .triple_store
            .retract(triple.entity_id, &triple.attribute)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ── Triple-store helpers ───────────────────────────────────────────

/// Persist a [`FieldConfig`] as EAV triples.
async fn persist_field(state: &AppState, config: &FieldConfig) -> Result<(), ApiError> {
    let entity_id = config.entity_id();

    // Retract old values first (idempotent for creates).
    for attr in [
        ATTR_FIELD_NAME,
        ATTR_FIELD_TYPE,
        ATTR_FIELD_TABLE,
        ATTR_FIELD_CONFIG,
        ATTR_FIELD_ORDER,
    ] {
        let _ = state.triple_store.retract(entity_id, attr).await;
    }

    let config_json =
        serde_json::to_value(config).map_err(|e| ApiError::internal(e.to_string()))?;

    let triples = vec![
        TripleInput {
            entity_id,
            attribute: ATTR_FIELD_NAME.into(),
            value: Value::String(config.name.clone()),
            value_type: 0, // String
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: ATTR_FIELD_TYPE.into(),
            value: Value::String(config.field_type.label().to_string()),
            value_type: 0,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: ATTR_FIELD_TABLE.into(),
            value: Value::String(config.table_entity_type.clone()),
            value_type: 0,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: ATTR_FIELD_CONFIG.into(),
            value: config_json,
            value_type: 6, // Json
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: ATTR_FIELD_ORDER.into(),
            value: serde_json::json!(config.order),
            value_type: 1, // Integer
            ttl_seconds: None,
        },
    ];

    state
        .triple_store
        .set_triples(&triples)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(())
}

/// Load a single field config from the triple store.
async fn load_field(state: &AppState, id: Uuid) -> Result<FieldConfig, ApiError> {
    let triples = state
        .triple_store
        .get_entity(id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    if triples.is_empty() {
        return Err(ApiError::not_found(format!("field {id} not found")));
    }

    // Find the config triple.
    let config_triple = triples
        .iter()
        .find(|t| t.attribute == ATTR_FIELD_CONFIG)
        .ok_or_else(|| ApiError::internal(format!("field {id} missing config triple")))?;

    let config: FieldConfig = serde_json::from_value(config_triple.value.clone())
        .map_err(|e| ApiError::internal(format!("corrupt field config: {e}")))?;

    Ok(config)
}

/// Load all fields, optionally filtered by entity type.
async fn load_all_fields(
    state: &AppState,
    entity_type: Option<&str>,
) -> Result<Vec<FieldConfig>, ApiError> {
    // Query all triples with attribute field/config.
    let config_triples = state
        .triple_store
        .query_by_attribute(ATTR_FIELD_CONFIG, None)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let mut fields: Vec<FieldConfig> = Vec::new();

    for triple in &config_triples {
        let config: FieldConfig = match serde_json::from_value(triple.value.clone()) {
            Ok(c) => c,
            Err(_) => continue, // skip corrupt entries
        };

        if let Some(et) = entity_type
            && config.table_entity_type != et
        {
            continue;
        }

        fields.push(config);
    }

    // Sort by order, then name.
    fields.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.name.cmp(&b.name)));

    Ok(fields)
}

/// Load all values for a field (by querying triples with the field's
/// attribute name within entities of the field's table type).
async fn load_field_values(state: &AppState, config: &FieldConfig) -> Result<Vec<Value>, ApiError> {
    let triples = state
        .triple_store
        .query_by_attribute(&config.name, None)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(triples.into_iter().map(|t| t.value).collect())
}

/// Write converted values back to the triple store.
async fn write_converted_values(
    state: &AppState,
    config: &FieldConfig,
    results: &[conversion::ConversionResult],
) -> Result<(), ApiError> {
    // Get the original triples so we know which entities to update.
    let triples = state
        .triple_store
        .query_by_attribute(&config.name, None)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let mut inputs = Vec::new();

    for (triple, result) in triples.iter().zip(results.iter()) {
        if let Some(ref new_value) = result.value {
            // Retract old value.
            state
                .triple_store
                .retract(triple.entity_id, &triple.attribute)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;

            // Write new value.
            inputs.push(TripleInput {
                entity_id: triple.entity_id,
                attribute: config.name.clone(),
                value: new_value.clone(),
                value_type: value_type_for_field(config.field_type),
                ttl_seconds: None,
            });
        }
        // If value is None (lossy conversion), the old value is left as-is.
    }

    if !inputs.is_empty() {
        state
            .triple_store
            .set_triples(&inputs)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }

    Ok(())
}

/// Map a field type to the appropriate triple store value type tag.
fn value_type_for_field(ft: FieldType) -> i16 {
    match ft {
        FieldType::Number | FieldType::Currency | FieldType::Percent | FieldType::Duration => {
            2 // Float
        }
        FieldType::Checkbox => 3, // Boolean
        FieldType::Date
        | FieldType::DateTime
        | FieldType::CreatedTime
        | FieldType::LastModifiedTime => {
            4 // Timestamp
        }
        FieldType::Link => 5,                                // Reference
        FieldType::Rating | FieldType::AutoNumber => 1,      // Integer
        FieldType::Attachment | FieldType::MultiSelect => 6, // Json
        _ => 0,                                              // String (default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_type_for_field_mapping() {
        assert_eq!(value_type_for_field(FieldType::SingleLineText), 0);
        assert_eq!(value_type_for_field(FieldType::LongText), 0);
        assert_eq!(value_type_for_field(FieldType::Number), 2);
        assert_eq!(value_type_for_field(FieldType::Checkbox), 3);
        assert_eq!(value_type_for_field(FieldType::Date), 4);
        assert_eq!(value_type_for_field(FieldType::DateTime), 4);
        assert_eq!(value_type_for_field(FieldType::Link), 5);
        assert_eq!(value_type_for_field(FieldType::Rating), 1);
        assert_eq!(value_type_for_field(FieldType::AutoNumber), 1);
        assert_eq!(value_type_for_field(FieldType::Attachment), 6);
        assert_eq!(value_type_for_field(FieldType::MultiSelect), 6);
        assert_eq!(value_type_for_field(FieldType::Currency), 2);
        assert_eq!(value_type_for_field(FieldType::Percent), 2);
        assert_eq!(value_type_for_field(FieldType::Duration), 2);
        assert_eq!(value_type_for_field(FieldType::Email), 0);
        assert_eq!(value_type_for_field(FieldType::Url), 0);
        assert_eq!(value_type_for_field(FieldType::Phone), 0);
    }

    #[test]
    fn create_field_request_deserializes() {
        let json = serde_json::json!({
            "name": "Status",
            "field_type": "single_select",
            "table_entity_type": "task",
            "required": true,
            "options": {
                "kind": "select",
                "choices": [
                    {"id": "1", "name": "Todo", "color": "#gray"}
                ]
            }
        });
        let req: CreateFieldRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "Status");
        assert_eq!(req.field_type, FieldType::SingleSelect);
        assert!(req.required);
    }

    #[test]
    fn update_field_request_all_optional() {
        let json = serde_json::json!({});
        let req: UpdateFieldRequest = serde_json::from_value(json).unwrap();
        assert!(req.name.is_none());
        assert!(req.field_type.is_none());
        assert!(req.options.is_none());
    }

    #[test]
    fn conversion_summary_response_from() {
        let summary = conversion::ConversionSummary {
            total: 10,
            success: 8,
            failed: 2,
            warnings: vec!["warn1".into(), "warn2".into()],
        };
        let resp = ConversionSummaryResponse::from(summary);
        assert_eq!(resp.total, 10);
        assert_eq!(resp.success, 8);
        assert_eq!(resp.failed, 2);
        assert_eq!(resp.warnings.len(), 2);
    }
}
