//! CRUD data handlers: list, create, get, patch, delete.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::rest::AppState;
use crate::auth::Operation;
use crate::query;
use crate::sync::broadcaster::ChangeEvent;
use crate::triple_store::{PgTripleStore, TripleInput, TripleStore};

use super::helpers::{
    check_permission, extract_auth_context, infer_value_type,
    negotiate_response, negotiate_response_status, validate_entity_name,
};

// ---------------------------------------------------------------------------
// Data list
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct DataListParams {
    limit: Option<u32>,
    #[serde(rename = "cursor")]
    #[allow(dead_code)]
    _cursor: Option<String>,
    #[serde(flatten)]
    #[allow(dead_code)]
    _filters: HashMap<String, String>,
}

/// `GET /api/data/:entity` -- List entities of a type with pagination.
pub async fn data_list(
    State(state): State<AppState>,
    Path(entity): Path<String>,
    Query(params): Query<DataListParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers, &state)?;
    let limit = params.limit.unwrap_or(50).min(1000);

    validate_entity_name(&entity)?;

    let perm_result = check_permission(&auth_ctx, &entity, Operation::Read, &state.permissions)?;

    let query_json = serde_json::json!({
        "type": entity,
        "$limit": limit
    });
    let mut ast = query::parse_darshan_ql(&query_json)
        .map_err(|e| ApiError::internal(format!("Failed to build list query: {e}")))?;

    if let Some(where_sql) = perm_result.build_where_clause(auth_ctx.user_id) {
        ast.where_clauses.push(query::WhereClause {
            attribute: "__permission_filter".to_string(),
            op: query::WhereOp::Eq,
            value: serde_json::Value::String(where_sql),
        });
    }

    let plan = query::plan_query(&ast)
        .map_err(|e| ApiError::internal(format!("Failed to plan list query: {e}")))?;
    let results = query::execute_query(&state.pool, &plan)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to execute list query: {e}")))?;

    let has_more = results.len() as u32 >= limit;
    let response = serde_json::json!({
        "data": results,
        "cursor": Value::Null,
        "has_more": has_more
    });

    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Data create
// ---------------------------------------------------------------------------

/// `POST /api/data/:entity` -- Create a new entity.
pub async fn data_create(
    State(state): State<AppState>,
    Path(entity): Path<String>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers, &state)?;

    validate_entity_name(&entity)?;

    let _perm_result = check_permission(&auth_ctx, &entity, Operation::Create, &state.permissions)?;

    if !body.is_object() {
        return Err(ApiError::bad_request("Request body must be a JSON object"));
    }

    let id = Uuid::new_v4();
    let obj = body
        .as_object()
        .ok_or_else(|| ApiError::bad_request("Request body must be a JSON object"))?;

    // Schema validation (SCHEMAFULL / MIXED mode).
    let obj = if let Some(ref registry) = state.schema_registry {
        if let Some(schema) = registry.get(&entity) {
            let doc: std::collections::HashMap<String, Value> = obj
                .iter()
                .filter(|(k, _)| !k.starts_with('$'))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let result = crate::schema::validator::SchemaValidator::validate_insert(&schema, &doc);
            if !result.is_valid() {
                return Err(ApiError::bad_request(format!(
                    "Schema validation failed: {}",
                    result.error_message()
                )));
            }
            let mut validated = result.document;
            for (k, v) in obj.iter() {
                if k.starts_with('$') {
                    validated.insert(k.clone(), v.clone());
                }
            }
            validated
                .into_iter()
                .collect::<serde_json::Map<String, Value>>()
        } else {
            obj.clone()
        }
    } else {
        obj.clone()
    };
    let obj = &obj;

    let ttl_seconds: Option<i64> = obj.get("$ttl").and_then(|v| v.as_i64());

    let mut triples = vec![TripleInput {
        entity_id: id,
        attribute: ":db/type".to_string(),
        value: Value::String(entity.clone()),
        value_type: 0,
        ttl_seconds,
    }];
    for (key, value) in obj {
        if key.starts_with('$') {
            continue;
        }
        let value_type = infer_value_type(value);
        triples.push(TripleInput {
            entity_id: id,
            attribute: format!("{entity}/{key}"),
            value: value.clone(),
            value_type,
            ttl_seconds,
        });
    }

    let tx_id = state
        .triple_store
        .set_triples(&triples)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create entity: {e}")))?;

    if let Some(ref rule_engine) = state.rule_engine {
        let implied = rule_engine
            .evaluate(&triples)
            .await
            .map_err(|e| ApiError::internal(format!("Rule engine error: {e}")))?;
        if !implied.is_empty() {
            let _ = state
                .triple_store
                .set_triples(&implied)
                .await
                .map_err(|e| ApiError::internal(format!("Failed to write implied triples: {e}")))?;
        }
    }

    state.query_cache.invalidate_by_entity_type(&entity);

    let attributes: Vec<String> = triples.into_iter().map(|t| t.attribute).collect();
    let _ = state.change_tx.send(ChangeEvent {
        tx_id,
        entity_ids: vec![id.to_string()],
        attributes,
        entity_type: Some(entity.clone()),
        actor_id: None,
    });

    let mut response = serde_json::json!({
        "id": id,
        "entity": entity,
        "tx_id": tx_id,
        "data": body
    });
    if let Some(ttl) = ttl_seconds {
        let exp = chrono::Utc::now() + chrono::Duration::seconds(ttl);
        if let Some(obj) = response.as_object_mut() {
            obj.insert("_ttl".into(), serde_json::json!(ttl));
            obj.insert("_expires_at".into(), serde_json::json!(exp.to_rfc3339()));
        }
    }

    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

// ---------------------------------------------------------------------------
// Data get
// ---------------------------------------------------------------------------

/// `GET /api/data/:entity/:id` -- Fetch a single entity by ID.
pub async fn data_get(
    State(state): State<AppState>,
    Path((entity, id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers, &state)?;

    validate_entity_name(&entity)?;

    let perm_result = check_permission(&auth_ctx, &entity, Operation::Read, &state.permissions)?;

    let triples = state
        .triple_store
        .get_entity(id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to fetch entity: {e}")))?;

    if triples.is_empty() {
        return Err(ApiError::not_found(format!(
            "{entity} with id {id} not found"
        )));
    }

    let mut attrs = serde_json::Map::new();
    for t in &triples {
        let key = t
            .attribute
            .strip_prefix(&format!("{entity}/"))
            .unwrap_or(&t.attribute)
            .to_string();
        attrs.entry(key).or_insert_with(|| t.value.clone());
    }

    if !perm_result.where_clauses.is_empty() {
        let owner_id = attrs
            .get("owner_id")
            .or_else(|| attrs.get("id"))
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        let entity_owner = if entity == "users" {
            Some(id)
        } else {
            owner_id
        };

        if let Some(owner) = entity_owner
            && owner != auth_ctx.user_id
        {
            return Err(ApiError::permission_denied(format!(
                "Access denied: you do not own this {entity}"
            )));
        }
    }

    if !perm_result.restricted_fields.is_empty() {
        for field in &perm_result.restricted_fields {
            attrs.remove(field);
        }
    }
    if !perm_result.allowed_fields.is_empty() {
        let allowed: std::collections::HashSet<&str> = perm_result
            .allowed_fields
            .iter()
            .map(|s| s.as_str())
            .collect();
        attrs.retain(|k, _| allowed.contains(k.as_str()) || k.starts_with(":db/"));
    }

    let mut response = serde_json::json!({
        "id": id,
        "entity": entity,
        "data": attrs
    });
    if let Some(exp) = triples.iter().filter_map(|t| t.expires_at).min() {
        let remaining = (exp - chrono::Utc::now()).num_seconds().max(0);
        if let Some(obj) = response.as_object_mut() {
            obj.insert("_ttl".into(), serde_json::json!(remaining));
            obj.insert("_expires_at".into(), serde_json::json!(exp.to_rfc3339()));
        }
    }

    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Data patch
// ---------------------------------------------------------------------------

/// `PATCH /api/data/:entity/:id` -- Partially update an entity.
pub async fn data_patch(
    State(state): State<AppState>,
    Path((entity, id)): Path<(String, Uuid)>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers, &state)?;

    validate_entity_name(&entity)?;

    let perm_result = check_permission(&auth_ctx, &entity, Operation::Update, &state.permissions)?;

    if !perm_result.where_clauses.is_empty() {
        let existing = state
            .triple_store
            .get_entity(id)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to fetch entity: {e}")))?;

        let owner_id = existing
            .iter()
            .find(|t| t.attribute.ends_with("/owner_id"))
            .and_then(|t| t.value.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        let entity_owner = if entity == "users" {
            Some(id)
        } else {
            owner_id
        };

        if let Some(owner) = entity_owner
            && owner != auth_ctx.user_id
        {
            return Err(ApiError::permission_denied(format!(
                "Access denied: you do not own this {entity}"
            )));
        }
    }

    if !body.is_object() {
        return Err(ApiError::bad_request("Request body must be a JSON object"));
    }

    let obj = body
        .as_object()
        .ok_or_else(|| ApiError::bad_request("Request body must be a JSON object"))?;

    // Schema validation for updates.
    let obj = if let Some(ref registry) = state.schema_registry {
        if let Some(schema) = registry.get(&entity) {
            let doc: std::collections::HashMap<String, Value> = obj
                .iter()
                .filter(|(k, _)| !k.starts_with('$'))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let result = crate::schema::validator::SchemaValidator::validate_update(&schema, &doc);
            if !result.is_valid() {
                return Err(ApiError::bad_request(format!(
                    "Schema validation failed: {}",
                    result.error_message()
                )));
            }
            let mut validated = result.document;
            for (k, v) in obj.iter() {
                if k.starts_with('$') {
                    validated.insert(k.clone(), v.clone());
                }
            }
            validated
                .into_iter()
                .collect::<serde_json::Map<String, Value>>()
        } else {
            obj.clone()
        }
    } else {
        obj.clone()
    };
    let obj = &obj;

    let ttl_override: Option<i64> = obj.get("$ttl").and_then(|v| v.as_i64());

    let mut triples = Vec::new();

    for (key, value) in obj {
        if key.starts_with('$') {
            continue;
        }
        let value_type = infer_value_type(value);
        triples.push(TripleInput {
            entity_id: id,
            attribute: format!("{entity}/{key}"),
            value: value.clone(),
            value_type,
            ttl_seconds: None,
        });
    }

    let mut db_tx = state
        .triple_store
        .begin_tx()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to begin transaction: {e}")))?;

    for (key, _) in obj {
        let attr = format!("{entity}/{key}");
        PgTripleStore::retract_in_tx(&mut db_tx, id, &attr)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to retract attribute: {e}")))?;
    }

    let tx_id = if !triples.is_empty() {
        let tid = PgTripleStore::next_tx_id_in_tx(&mut db_tx)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to allocate tx_id: {e}")))?;
        PgTripleStore::set_triples_in_tx(&mut db_tx, &triples, tid)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to update entity: {e}")))?;

        if let Some(ref rule_engine) = state.rule_engine {
            let _ = rule_engine
                .evaluate_and_write_in_tx(&mut db_tx, &triples, tid)
                .await
                .map_err(|e| ApiError::internal(format!("Rule engine error: {e}")))?;
        }

        tid
    } else {
        0
    };

    db_tx
        .commit()
        .await
        .map_err(|e| ApiError::internal(format!("Transaction commit failed: {e}")))?;

    if let Some(ttl) = ttl_override {
        state
            .triple_store
            .set_entity_ttl(id, ttl)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to set TTL: {e}")))?;
    }

    state.query_cache.invalidate_by_entity_type(&entity);

    if tx_id > 0 {
        let attributes: Vec<String> = triples.into_iter().map(|t| t.attribute).collect();
        let _ = state.change_tx.send(ChangeEvent {
            tx_id,
            entity_ids: vec![id.to_string()],
            attributes,
            entity_type: Some(entity.clone()),
            actor_id: None,
        });
    }

    let ttl_info = if let Some(exp) = state.triple_store.get_entity_ttl(id).await.unwrap_or(None) {
        let remaining = (exp - chrono::Utc::now()).num_seconds().max(0);
        serde_json::json!({ "_ttl": remaining, "_expires_at": exp.to_rfc3339() })
    } else {
        serde_json::json!({})
    };

    let mut response = serde_json::json!({
        "id": id,
        "entity": entity,
        "tx_id": tx_id,
        "data": body
    });
    if let Some(obj) = response.as_object_mut()
        && let Some(ttl_obj) = ttl_info.as_object()
    {
        for (k, v) in ttl_obj {
            obj.insert(k.clone(), v.clone());
        }
    }

    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Data delete
// ---------------------------------------------------------------------------

/// `DELETE /api/data/:entity/:id` -- Delete an entity.
pub async fn data_delete(
    State(state): State<AppState>,
    Path((entity, id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers, &state)?;

    validate_entity_name(&entity)?;

    let perm_result = check_permission(&auth_ctx, &entity, Operation::Delete, &state.permissions)?;

    let existing = state
        .triple_store
        .get_entity(id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to fetch entity for deletion: {e}")))?;

    if existing.is_empty() {
        return Err(ApiError::not_found(format!(
            "{entity} with id {id} not found"
        )));
    }

    if !perm_result.where_clauses.is_empty() {
        let owner_id = existing
            .iter()
            .find(|t| t.attribute.ends_with("/owner_id"))
            .and_then(|t| t.value.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        let entity_owner = if entity == "users" {
            Some(id)
        } else {
            owner_id
        };

        if let Some(owner) = entity_owner
            && owner != auth_ctx.user_id
        {
            return Err(ApiError::permission_denied(format!(
                "Access denied: you do not own this {entity}"
            )));
        }
    }

    let mut db_tx = state
        .triple_store
        .begin_tx()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to begin transaction: {e}")))?;

    let del_tx_id = PgTripleStore::next_tx_id_in_tx(&mut db_tx)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to allocate tx_id: {e}")))?;

    for triple in &existing {
        PgTripleStore::retract_in_tx(&mut db_tx, id, &triple.attribute)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to retract triple: {e}")))?;
    }

    db_tx
        .commit()
        .await
        .map_err(|e| ApiError::internal(format!("Transaction commit failed: {e}")))?;

    state.query_cache.invalidate_by_entity_type(&entity);

    let deleted_attributes: Vec<String> = existing.into_iter().map(|t| t.attribute).collect();

    let _ = state.change_tx.send(ChangeEvent {
        tx_id: del_tx_id,
        entity_ids: vec![id.to_string()],
        attributes: deleted_attributes,
        entity_type: Some(entity),
        actor_id: None,
    });

    Ok(StatusCode::NO_CONTENT.into_response())
}
