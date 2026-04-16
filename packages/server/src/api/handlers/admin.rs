//! Admin handlers: schema introspection, functions listing, sessions, cache, bulk-load.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::rest::AppState;
use crate::triple_store::{TripleInput, TripleStore};

use super::helpers::{
    extract_bearer_token, infer_value_type, negotiate_response, require_admin_role,
    validate_entity_name,
};

// ---------------------------------------------------------------------------
// Schema introspection
// ---------------------------------------------------------------------------

/// `GET /api/admin/schema` -- Return the current inferred schema.
pub async fn admin_schema(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;
    require_admin_role(&headers)?;

    let schema = state
        .triple_store
        .get_schema()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to infer schema: {e}")))?;

    let response = serde_json::json!(schema);

    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Functions listing
// ---------------------------------------------------------------------------

/// `GET /api/admin/functions` -- List registered server-side functions.
pub async fn admin_functions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;
    require_admin_role(&headers)?;

    let functions = match state.function_registry.as_ref() {
        Some(registry) => {
            let defs = registry.list().await;
            defs.into_iter()
                .map(|f| {
                    serde_json::json!({
                        "name": f.name,
                        "export_name": f.export_name,
                        "file_path": f.file_path.display().to_string(),
                        "kind": format!("{:?}", f.kind),
                        "description": f.description,
                        "args_schema": f.args_schema.as_ref().map(|s| serde_json::to_value(s).unwrap_or_default()),
                    })
                })
                .collect::<Vec<_>>()
        }
        None => Vec::new(),
    };

    let response = serde_json::json!({
        "functions": functions,
        "count": functions.len(),
    });

    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Sessions listing
// ---------------------------------------------------------------------------

/// `GET /api/admin/sessions` -- List active sessions across all users.
#[allow(clippy::type_complexity)]
pub async fn admin_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;
    require_admin_role(&headers)?;

    let rows: Vec<(
        uuid::Uuid,
        uuid::Uuid,
        String,
        String,
        String,
        chrono::DateTime<chrono::Utc>,
        bool,
    )> = sqlx::query_as(
        "SELECT session_id, user_id, device_fingerprint, ip, user_agent, created_at, revoked \
         FROM sessions WHERE revoked = false AND refresh_expires_at > NOW() \
         ORDER BY created_at DESC LIMIT 500",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("Failed to query sessions: {e}")))?;

    let sessions: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(sid, uid, dfp, ip, ua, created, revoked)| {
            serde_json::json!({
                "session_id": sid,
                "user_id": uid,
                "device_fingerprint": dfp,
                "ip": ip,
                "user_agent": ua,
                "created_at": created.to_rfc3339(),
                "revoked": revoked,
            })
        })
        .collect();

    let count = sessions.len();
    let response = serde_json::json!({
        "sessions": sessions,
        "count": count,
    });

    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Cache stats
// ---------------------------------------------------------------------------

/// `GET /api/admin/cache` -- Return hot-cache statistics.
pub async fn admin_cache(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;
    require_admin_role(&headers)?;

    let stats = state.query_cache.stats();
    let response = serde_json::json!({
        "cache": stats,
    });

    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Bulk load
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct BulkLoadRequest {
    entities: Vec<BulkLoadEntity>,
}

#[derive(Deserialize)]
pub struct BulkLoadEntity {
    #[serde(rename = "type")]
    entity_type: String,
    id: Option<Uuid>,
    data: HashMap<String, Value>,
}

/// `POST /api/admin/bulk-load` -- High-throughput data import.
pub async fn admin_bulk_load(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<BulkLoadRequest>,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;
    require_admin_role(&headers)?;

    if body.entities.is_empty() {
        return Err(ApiError::bad_request("At least one entity is required"));
    }

    let mut triples: Vec<TripleInput> = Vec::new();

    for entity in &body.entities {
        validate_entity_name(&entity.entity_type)
            .map_err(|e| ApiError::bad_request(format!("Invalid entity type: {}", e.message)))?;

        let entity_id = entity.id.unwrap_or_else(Uuid::new_v4);

        triples.push(TripleInput {
            entity_id,
            attribute: ":db/type".to_string(),
            value: Value::String(entity.entity_type.clone()),
            value_type: 0,
            ttl_seconds: None,
        });

        for (key, value) in &entity.data {
            let value_type = infer_value_type(value);
            triples.push(TripleInput {
                entity_id,
                attribute: format!("{}/{}", entity.entity_type, key),
                value: value.clone(),
                value_type,
                ttl_seconds: None,
            });
        }
    }

    let result = state
        .triple_store
        .bulk_load(triples)
        .await
        .map_err(|e| ApiError::internal(format!("Bulk load failed: {e}")))?;

    let response = serde_json::json!({
        "ok": true,
        "entities": body.entities.len(),
        "triples_loaded": result.triples_loaded,
        "tx_id": result.tx_id,
        "duration_ms": result.duration_ms,
        "rate_per_sec": result.rate_per_sec,
    });

    Ok(negotiate_response(&headers, &response))
}
