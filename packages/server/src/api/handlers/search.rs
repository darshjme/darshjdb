//! Embeddings and semantic search handlers.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::rest::AppState;

use super::helpers::{format_pgvector_literal, negotiate_response, negotiate_response_status};

// ---------------------------------------------------------------------------
// Store embedding
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Deserialize)]
pub struct EmbeddingStoreRequest {
    entity_id: Uuid,
    attribute: String,
    embedding: Vec<f32>,
    #[serde(default = "default_embedding_model")]
    model: String,
}

#[allow(dead_code)]
fn default_embedding_model() -> String {
    "text-embedding-ada-002".to_string()
}

#[allow(dead_code)]
/// `POST /api/embeddings` -- Store an embedding vector for an entity+attribute pair.
pub async fn embeddings_store(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<EmbeddingStoreRequest>,
) -> Result<Response, ApiError> {
    if body.embedding.is_empty() {
        return Err(ApiError::bad_request("embedding vector must not be empty"));
    }
    if body.attribute.is_empty() {
        return Err(ApiError::bad_request("attribute must not be empty"));
    }

    let vec_literal = format_pgvector_literal(&body.embedding);

    let result = sqlx::query_scalar::<_, i64>(
        "INSERT INTO embeddings (entity_id, attribute, embedding, model) \
         VALUES ($1, $2, $3::vector, $4) \
         RETURNING id",
    )
    .bind(body.entity_id)
    .bind(&body.attribute)
    .bind(&vec_literal)
    .bind(&body.model)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("Failed to store embedding: {e}")))?;

    let response = serde_json::json!({
        "id": result,
        "entity_id": body.entity_id,
        "attribute": body.attribute,
        "model": body.model,
        "dimensions": body.embedding.len(),
    });
    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

// ---------------------------------------------------------------------------
// Get embeddings
// ---------------------------------------------------------------------------

#[allow(dead_code)]
/// `GET /api/embeddings/:entity_id` -- Get all embeddings for an entity.
pub async fn embeddings_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(entity_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let rows = sqlx::query_as::<_, (i64, String, String, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, attribute, model, created_at \
         FROM embeddings \
         WHERE entity_id = $1 \
         ORDER BY created_at DESC",
    )
    .bind(entity_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("Failed to fetch embeddings: {e}")))?;

    let embeddings: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, attribute, model, created_at)| {
            serde_json::json!({
                "id": id,
                "entity_id": entity_id,
                "attribute": attribute,
                "model": model,
                "created_at": created_at.to_rfc3339(),
            })
        })
        .collect();

    let response = serde_json::json!({
        "data": embeddings,
        "meta": { "count": embeddings.len() }
    });
    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Semantic search
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Deserialize)]
pub struct SemanticSearchRequest {
    entity_type: String,
    vector: Vec<f32>,
    #[serde(default = "default_search_limit")]
    limit: u32,
    #[serde(default)]
    attribute: Option<String>,
}

#[allow(dead_code)]
fn default_search_limit() -> u32 {
    10
}

#[allow(dead_code)]
/// `POST /api/search/semantic` -- Search by vector similarity.
pub async fn search_semantic(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<SemanticSearchRequest>,
) -> Result<Response, ApiError> {
    if body.vector.is_empty() {
        return Err(ApiError::bad_request("vector must not be empty"));
    }
    if body.entity_type.is_empty() {
        return Err(ApiError::bad_request("entity_type must not be empty"));
    }

    let vec_literal = format_pgvector_literal(&body.vector);

    let (sql, has_attr_param) = if body.attribute.is_some() {
        (
            format!(
                "SELECT e.entity_id, e.attribute, \
                        (e.embedding <=> '{vec}'::vector) AS distance \
                 FROM embeddings e \
                 INNER JOIN triples t_type ON t_type.entity_id = e.entity_id \
                   AND t_type.attribute = ':db/type' \
                   AND t_type.value = $1::jsonb \
                   AND NOT t_type.retracted \
                 WHERE e.attribute = $2 \
                 ORDER BY e.embedding <=> '{vec}'::vector \
                 LIMIT $3",
                vec = vec_literal,
            ),
            true,
        )
    } else {
        (
            format!(
                "SELECT e.entity_id, e.attribute, \
                        (e.embedding <=> '{vec}'::vector) AS distance \
                 FROM embeddings e \
                 INNER JOIN triples t_type ON t_type.entity_id = e.entity_id \
                   AND t_type.attribute = ':db/type' \
                   AND t_type.value = $1::jsonb \
                   AND NOT t_type.retracted \
                 ORDER BY e.embedding <=> '{vec}'::vector \
                 LIMIT $2",
                vec = vec_literal,
            ),
            false,
        )
    };

    let rows: Vec<(Uuid, String, f64)> = if has_attr_param {
        sqlx::query_as::<_, (Uuid, String, f64)>(&sql)
            .bind(serde_json::Value::String(body.entity_type.clone()))
            .bind(body.attribute.as_deref().unwrap_or(""))
            .bind(body.limit as i32)
            .fetch_all(&state.pool)
            .await
    } else {
        sqlx::query_as::<_, (Uuid, String, f64)>(&sql)
            .bind(serde_json::Value::String(body.entity_type.clone()))
            .bind(body.limit as i32)
            .fetch_all(&state.pool)
            .await
    }
    .map_err(|e| ApiError::internal(format!("Semantic search failed: {e}")))?;

    let results: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(entity_id, attribute, distance)| {
            serde_json::json!({
                "entity_id": entity_id,
                "attribute": attribute,
                "distance": distance,
                "similarity": 1.0 - distance,
            })
        })
        .collect();

    let response = serde_json::json!({
        "data": results,
        "meta": {
            "count": results.len(),
            "entity_type": body.entity_type,
        }
    });
    Ok(negotiate_response(&headers, &response))
}
