//! Data mutation handlers: DarshJQL query and batch mutate.

use std::collections::HashMap;
use std::time::Instant;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::rest::AppState;
use crate::auth::Operation;
use crate::cache;
use crate::query::{self, QueryResultRow};
use crate::sync::broadcaster::ChangeEvent;
use crate::triple_store::{PgTripleStore, TripleInput};

use super::helpers::{
    check_permission, extract_auth_context, extract_bearer_token, infer_value_type,
    negotiate_response, validate_entity_name,
};

// ---------------------------------------------------------------------------
// Query (DarshJQL)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct QueryRequest {
    query: Value,
    #[serde(rename = "args")]
    #[allow(dead_code)]
    _args: Option<HashMap<String, Value>>,
}

/// `POST /api/query` -- Execute a DarshJQL query over HTTP.
pub async fn query_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<QueryRequest>,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers, &state)?;

    let start = Instant::now();

    let mut ast = query::parse_darshan_ql(&body.query)
        .map_err(|e| ApiError::bad_request(format!("Invalid query: {e}")))?;

    let perm_result = check_permission(
        &auth_ctx,
        &ast.entity_type,
        Operation::Read,
        &state.permissions,
    )?;

    let permission_where = perm_result.build_where_clause(auth_ctx.user_id);
    if let Some(ref where_sql) = permission_where {
        ast.where_clauses.push(query::WhereClause {
            attribute: "__permission_filter".to_string(),
            op: query::WhereOp::Eq,
            value: serde_json::Value::String(where_sql.clone()),
        });
    }

    let cache_key_input = serde_json::json!({
        "q": body.query,
        "uid": auth_ctx.user_id,
        "perm": permission_where,
    });
    let query_hash = cache::hash_query(&cache_key_input);
    let entity_type = ast.entity_type.clone();

    if let Some(cached_response) = state.query_cache.get(query_hash) {
        let response = serde_json::json!({
            "data": cached_response,
            "meta": {
                "count": cached_response.as_array().map(|a| a.len()).unwrap_or(0),
                "duration_ms": start.elapsed().as_secs_f64() * 1000.0,
                "filtered": !perm_result.where_clauses.is_empty(),
                "cached": true
            }
        });
        return Ok(negotiate_response(&headers, &response));
    }

    let plan = query::plan_query(&ast)
        .map_err(|e| ApiError::bad_request(format!("Query planning failed: {e}")))?;

    let results: Vec<QueryResultRow> = query::execute_query(&state.pool, &plan)
        .await
        .map_err(|e| ApiError::internal(format!("Query execution failed: {e}")))?;

    let count = results.len();

    state.pool_stats.record(start.elapsed());

    let results_value = serde_json::to_value(&results).unwrap_or_default();
    state
        .query_cache
        .set(query_hash, results_value, 0, entity_type);

    let response = serde_json::json!({
        "data": results,
        "meta": {
            "count": count,
            "duration_ms": start.elapsed().as_secs_f64() * 1000.0,
            "filtered": !perm_result.where_clauses.is_empty(),
            "cached": false
        }
    });

    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Mutate
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct MutateRequest {
    mutations: Vec<Mutation>,
}

#[derive(Deserialize)]
pub struct Mutation {
    op: MutationOp,
    entity: String,
    id: Option<Uuid>,
    data: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MutationOp {
    Insert,
    Update,
    Delete,
    Upsert,
}

/// `POST /api/mutate` -- Submit a transaction of mutations over HTTP.
pub async fn mutate(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<MutateRequest>,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;
    let mutate_start = Instant::now();

    if body.mutations.is_empty() {
        return Err(ApiError::bad_request("At least one mutation is required"));
    }

    for (i, m) in body.mutations.iter().enumerate() {
        validate_entity_name(&m.entity)
            .map_err(|e| ApiError::bad_request(format!("Mutation {i}: {}", e.message)))?;
        match m.op {
            MutationOp::Update | MutationOp::Delete => {
                if m.id.is_none() {
                    return Err(ApiError::bad_request(format!(
                        "Mutation {i}: id is required for update/delete"
                    )));
                }
            }
            MutationOp::Insert | MutationOp::Upsert => {
                if m.data.is_none() {
                    return Err(ApiError::bad_request(format!(
                        "Mutation {i}: data is required for insert/upsert"
                    )));
                }
            }
        }
    }

    // Schema validation for batch mutations.
    if let Some(ref registry) = state.schema_registry {
        for (i, m) in body.mutations.iter().enumerate() {
            if let Some(data) = &m.data {
                if let Some(obj) = data.as_object() {
                    if let Some(schema) = registry.get(&m.entity) {
                        let doc: std::collections::HashMap<String, Value> = obj
                            .iter()
                            .filter(|(k, _)| !k.starts_with('$'))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        let is_update = matches!(m.op, MutationOp::Update | MutationOp::Upsert);
                        let result = if is_update {
                            crate::schema::validator::SchemaValidator::validate_update(&schema, &doc)
                        } else {
                            crate::schema::validator::SchemaValidator::validate_insert(&schema, &doc)
                        };
                        if !result.is_valid() {
                            return Err(ApiError::bad_request(format!(
                                "Mutation {i}: schema validation failed: {}",
                                result.error_message()
                            )));
                        }
                    }
                }
            }
        }
    }

    let mut db_tx = state
        .triple_store
        .begin_tx()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to begin transaction: {e}")))?;

    let tx_id = PgTripleStore::next_tx_id_in_tx(&mut db_tx)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to allocate tx_id: {e}")))?;

    let mut all_triples: Vec<TripleInput> = Vec::new();
    let mut entity_ids: Vec<Uuid> = Vec::new();

    for m in &body.mutations {
        match m.op {
            MutationOp::Insert => {
                let entity_id = m.id.unwrap_or_else(Uuid::new_v4);
                entity_ids.push(entity_id);

                all_triples.push(TripleInput {
                    entity_id,
                    attribute: ":db/type".to_string(),
                    value: Value::String(m.entity.clone()),
                    value_type: 0,
                    ttl_seconds: None,
                });

                if let Some(data) = &m.data
                    && let Some(obj) = data.as_object()
                {
                    for (key, value) in obj {
                        let value_type = infer_value_type(value);
                        all_triples.push(TripleInput {
                            entity_id,
                            attribute: format!("{}/{}", m.entity, key),
                            value: value.clone(),
                            value_type,
                            ttl_seconds: None,
                        });
                    }
                }
            }
            MutationOp::Update | MutationOp::Upsert => {
                let entity_id = m.id.unwrap_or_else(Uuid::new_v4);
                entity_ids.push(entity_id);

                if let Some(data) = &m.data
                    && let Some(obj) = data.as_object()
                {
                    for (key, _) in obj {
                        let attr = format!("{}/{}", m.entity, key);
                        PgTripleStore::retract_in_tx(&mut db_tx, entity_id, &attr)
                            .await
                            .map_err(|e| {
                                ApiError::internal(format!("Failed to retract attribute: {e}"))
                            })?;
                    }
                    for (key, value) in obj {
                        let value_type = infer_value_type(value);
                        all_triples.push(TripleInput {
                            entity_id,
                            attribute: format!("{}/{}", m.entity, key),
                            value: value.clone(),
                            value_type,
                            ttl_seconds: None,
                        });
                    }
                }
            }
            MutationOp::Delete => {
                let entity_id = m.id.expect("validated above");
                entity_ids.push(entity_id);

                let existing = PgTripleStore::get_entity_in_tx(&mut db_tx, entity_id)
                    .await
                    .map_err(|e| {
                        ApiError::internal(format!("Failed to fetch entity for deletion: {e}"))
                    })?;
                for triple in &existing {
                    PgTripleStore::retract_in_tx(&mut db_tx, entity_id, &triple.attribute)
                        .await
                        .map_err(|e| {
                            ApiError::internal(format!("Failed to retract triple: {e}"))
                        })?;
                }
            }
        }
    }

    if !all_triples.is_empty() {
        PgTripleStore::set_triples_in_tx(&mut db_tx, &all_triples, tx_id)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to write triples: {e}")))?;
    }

    let mut implied_triples: Vec<TripleInput> = Vec::new();
    if !all_triples.is_empty()
        && let Some(ref rule_engine) = state.rule_engine
    {
        implied_triples = rule_engine
            .evaluate_and_write_in_tx(&mut db_tx, &all_triples, tx_id)
            .await
            .map_err(|e| ApiError::internal(format!("Rule engine error: {e}")))?;
    }

    db_tx
        .commit()
        .await
        .map_err(|e| ApiError::internal(format!("Transaction commit failed: {e}")))?;

    state.pool_stats.record(mutate_start.elapsed());

    let mut touched_attributes: Vec<String> = all_triples
        .into_iter()
        .chain(implied_triples)
        .map(|t| t.attribute)
        .collect();
    touched_attributes.sort();
    touched_attributes.dedup();

    let mut entity_types: Vec<String> = body.mutations.iter().map(|m| m.entity.clone()).collect();
    entity_types.sort();
    entity_types.dedup();

    for et in &entity_types {
        state.query_cache.invalidate_by_entity_type(et);
    }

    if tx_id > 0 {
        let _ = state.change_tx.send(ChangeEvent {
            tx_id,
            entity_ids: entity_ids.iter().map(|id| id.to_string()).collect(),
            attributes: touched_attributes,
            entity_type: entity_types.into_iter().next(),
            actor_id: None,
        });
    }

    let response = serde_json::json!({
        "tx_id": tx_id,
        "affected": body.mutations.len(),
        "entity_ids": entity_ids,
    });

    Ok(negotiate_response(&headers, &response))
}
