//! REST API handlers for API key management.
//!
//! Endpoints for creating, listing, revoking, and rotating API keys.
//! All endpoints require authentication (JWT or existing API key with Admin scope).
//! Users can only manage their own keys unless they have the `admin` role.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    ApiKeyScope, create_api_key, get_api_key, list_api_keys, revoke_api_key, rotate_api_key,
};
use crate::auth::AuthContext;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    /// Human-readable name for this key.
    pub name: String,
    /// Scopes to grant.
    #[serde(default = "default_scopes")]
    pub scopes: Vec<ApiKeyScope>,
    /// Optional per-key rate limit (requests per minute).
    pub rate_limit: Option<u32>,
    /// Optional expiry timestamp.
    pub expires_at: Option<DateTime<Utc>>,
}

fn default_scopes() -> Vec<ApiKeyScope> {
    vec![ApiKeyScope::Read]
}

#[derive(Debug, Serialize)]
pub struct CreateApiKeyResponse {
    /// The key's unique ID (for future management operations).
    pub id: Uuid,
    /// The raw API key -- shown exactly once.
    pub key: String,
    /// Display prefix (e.g. `ddb_key_a1b2c3d4`).
    pub key_prefix: String,
    /// Human-readable name.
    pub name: String,
    /// Granted scopes.
    pub scopes: Vec<ApiKeyScope>,
    /// When the key was created.
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct RotateApiKeyResponse {
    /// The new key's unique ID.
    pub id: Uuid,
    /// The new raw API key -- shown exactly once.
    pub key: String,
    /// Display prefix of the new key.
    pub key_prefix: String,
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// State required by API key handlers.
#[derive(Clone)]
pub struct ApiKeyState {
    pub pool: PgPool,
}

// ---------------------------------------------------------------------------
// Route builder
// ---------------------------------------------------------------------------

/// Build the API key sub-router. Mount at `/api/keys`.
pub fn api_key_routes() -> Router<ApiKeyState> {
    Router::new()
        .route("/", post(create_key).get(list_keys))
        .route("/{id}", delete(revoke_key))
        .route("/{id}/rotate", post(rotate_key))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/keys -- Create a new API key.
///
/// Returns the raw key exactly once. It cannot be retrieved again.
async fn create_key(
    State(state): State<ApiKeyState>,
    auth: Option<axum::Extension<AuthContext>>,
    Json(body): Json<CreateApiKeyRequest>,
) -> impl IntoResponse {
    let auth = match auth {
        Some(axum::Extension(ctx)) => ctx,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "authentication required"})),
            )
                .into_response();
        }
    };

    if body.name.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "name is required"})),
        )
            .into_response();
    }

    match create_api_key(
        &state.pool,
        &body.name,
        body.scopes.clone(),
        body.rate_limit,
        body.expires_at,
        auth.user_id,
    )
    .await
    {
        Ok((id, raw_key)) => {
            let prefix: String = raw_key.chars().take(16).collect();
            let resp = CreateApiKeyResponse {
                id,
                key: raw_key,
                key_prefix: prefix,
                name: body.name,
                scopes: body.scopes,
                created_at: Utc::now().to_rfc3339(),
            };
            (
                StatusCode::CREATED,
                Json(serde_json::to_value(resp).unwrap()),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("failed to create API key: {e}")})),
        )
            .into_response(),
    }
}

/// GET /api/keys -- List API keys (prefix + name only, never full key).
async fn list_keys(
    State(state): State<ApiKeyState>,
    auth: Option<axum::Extension<AuthContext>>,
) -> impl IntoResponse {
    let auth = match auth {
        Some(axum::Extension(ctx)) => ctx,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "authentication required"})),
            )
                .into_response();
        }
    };

    let user_filter = if auth.roles.contains(&"admin".to_string()) {
        None
    } else {
        Some(auth.user_id)
    };

    match list_api_keys(&state.pool, user_filter).await {
        Ok(keys) => {
            let sanitized: Vec<serde_json::Value> = keys
                .into_iter()
                .map(|k| serde_json::to_value(k).unwrap_or_default())
                .collect();
            (StatusCode::OK, Json(serde_json::json!({"keys": sanitized}))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("failed to list keys: {e}")})),
        )
            .into_response(),
    }
}

/// DELETE /api/keys/{id} -- Revoke an API key.
async fn revoke_key(
    State(state): State<ApiKeyState>,
    auth: Option<axum::Extension<AuthContext>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let auth = match auth {
        Some(axum::Extension(ctx)) => ctx,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "authentication required"})),
            )
                .into_response();
        }
    };

    // Ownership check.
    match get_api_key(&state.pool, id).await {
        Ok(Some(key)) => {
            if key.created_by != auth.user_id && !auth.roles.contains(&"admin".to_string()) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"error": "not your API key"})),
                )
                    .into_response();
            }
        }
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "API key not found"})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("{e}")})),
            )
                .into_response();
        }
    }

    match revoke_api_key(&state.pool, id).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({"revoked": true}))).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "API key not found or already revoked"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

/// POST /api/keys/{id}/rotate -- Rotate an API key (revoke old, issue new).
///
/// Returns the new raw key exactly once.
async fn rotate_key(
    State(state): State<ApiKeyState>,
    auth: Option<axum::Extension<AuthContext>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let auth = match auth {
        Some(axum::Extension(ctx)) => ctx,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "authentication required"})),
            )
                .into_response();
        }
    };

    // Ownership check.
    match get_api_key(&state.pool, id).await {
        Ok(Some(key)) => {
            if key.created_by != auth.user_id && !auth.roles.contains(&"admin".to_string()) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"error": "not your API key"})),
                )
                    .into_response();
            }
        }
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "API key not found"})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("{e}")})),
            )
                .into_response();
        }
    }

    match rotate_api_key(&state.pool, id).await {
        Ok(Some((new_id, new_key))) => {
            let prefix: String = new_key.chars().take(16).collect();
            let resp = RotateApiKeyResponse {
                id: new_id,
                key: new_key,
                key_prefix: prefix,
            };
            (StatusCode::OK, Json(serde_json::to_value(resp).unwrap())).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "API key not found or already revoked"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}
