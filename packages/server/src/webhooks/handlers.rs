//! HTTP handlers for webhook management endpoints.
//!
//! All endpoints require authentication. Webhook ownership is enforced:
//! users can only manage their own webhooks unless they have the `admin` role.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    RetryPolicy, WebhookConfig, WebhookSender, delete_webhook, get_webhook, list_deliveries,
    list_webhooks, register_webhook, update_webhook,
};
use crate::auth::AuthContext;
use crate::events::EventKind;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateWebhookRequest {
    pub url: String,
    #[serde(default)]
    pub events: Vec<EventKind>,
    #[serde(default)]
    pub entity_types: Option<Vec<String>>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub retry_policy: Option<RetryPolicy>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWebhookRequest {
    pub url: Option<String>,
    pub events: Option<Vec<EventKind>>,
    pub active: Option<bool>,
    pub entity_types: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct DeliveryQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Serialize)]
pub struct CreateWebhookResponse {
    pub id: Uuid,
    pub secret: String,
    pub url: String,
    pub active: bool,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Shared state expected by handlers
// ---------------------------------------------------------------------------

/// State required by webhook handlers, extracted from AppState.
#[derive(Clone)]
pub struct WebhookState {
    pub pool: sqlx::PgPool,
    pub sender: Arc<WebhookSender>,
}

// ---------------------------------------------------------------------------
// Route builder
// ---------------------------------------------------------------------------

/// Build the webhook sub-router. Mount at `/api/webhooks`.
pub fn webhook_routes() -> Router<WebhookState> {
    Router::new()
        .route("/", post(create_webhook).get(list_webhooks_handler))
        .route(
            "/{id}",
            get(get_webhook_handler)
                .patch(patch_webhook)
                .delete(delete_webhook_handler),
        )
        .route("/{id}/deliveries", get(list_deliveries_handler))
        .route("/{id}/test", post(test_webhook))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/webhooks -- Create a new webhook.
///
/// Returns the webhook ID and the signing secret (shown only once).
async fn create_webhook(
    State(state): State<WebhookState>,
    auth: Option<axum::Extension<AuthContext>>,
    Json(body): Json<CreateWebhookRequest>,
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

    // Generate a random signing secret.
    use rand::RngCore;
    let mut secret_bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut secret_bytes);
    let secret = hex::encode(secret_bytes);

    let config = WebhookConfig {
        id: Uuid::new_v4(),
        url: body.url.clone(),
        secret: secret.clone(),
        events: body.events,
        entity_types: body.entity_types,
        headers: body.headers,
        active: true,
        created_by: auth.user_id,
        retry_policy: body.retry_policy.unwrap_or_default(),
        created_at: Utc::now(),
        consecutive_failures: 0,
    };

    match register_webhook(&state.pool, &config).await {
        Ok(()) => {
            let resp = CreateWebhookResponse {
                id: config.id,
                secret,
                url: config.url,
                active: true,
                created_at: config.created_at.to_rfc3339(),
            };
            (
                StatusCode::CREATED,
                Json(serde_json::to_value(resp).unwrap()),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("failed to create webhook: {e}")})),
        )
            .into_response(),
    }
}

/// GET /api/webhooks -- List all webhooks owned by the caller.
async fn list_webhooks_handler(
    State(state): State<WebhookState>,
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

    match list_webhooks(&state.pool, user_filter).await {
        Ok(webhooks) => {
            let sanitized: Vec<serde_json::Value> = webhooks
                .into_iter()
                .map(|wh| serde_json::to_value(wh).unwrap_or_default())
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({"webhooks": sanitized})),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("failed to list webhooks: {e}")})),
        )
            .into_response(),
    }
}

/// GET /api/webhooks/{id} -- Get a webhook with recent delivery status.
async fn get_webhook_handler(
    State(state): State<WebhookState>,
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

    match get_webhook(&state.pool, id).await {
        Ok(Some(wh)) => {
            if wh.created_by != auth.user_id && !auth.roles.contains(&"admin".to_string()) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"error": "not your webhook"})),
                )
                    .into_response();
            }

            // Fetch recent deliveries.
            let deliveries = list_deliveries(&state.pool, id, 10)
                .await
                .unwrap_or_default();

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "webhook": serde_json::to_value(&wh).unwrap_or_default(),
                    "recent_deliveries": serde_json::to_value(&deliveries).unwrap_or_default(),
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "webhook not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

/// PATCH /api/webhooks/{id} -- Update a webhook.
async fn patch_webhook(
    State(state): State<WebhookState>,
    auth: Option<axum::Extension<AuthContext>>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateWebhookRequest>,
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
    match get_webhook(&state.pool, id).await {
        Ok(Some(wh)) => {
            if wh.created_by != auth.user_id && !auth.roles.contains(&"admin".to_string()) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"error": "not your webhook"})),
                )
                    .into_response();
            }
        }
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "webhook not found"})),
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

    match update_webhook(
        &state.pool,
        id,
        body.url.as_deref(),
        body.events.as_deref(),
        body.active,
        body.entity_types.as_deref(),
    )
    .await
    {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({"updated": true}))).into_response(),
        Ok(false) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "no fields to update"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

/// DELETE /api/webhooks/{id} -- Delete a webhook.
async fn delete_webhook_handler(
    State(state): State<WebhookState>,
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

    match get_webhook(&state.pool, id).await {
        Ok(Some(wh)) => {
            if wh.created_by != auth.user_id && !auth.roles.contains(&"admin".to_string()) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"error": "not your webhook"})),
                )
                    .into_response();
            }
        }
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "webhook not found"})),
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

    match delete_webhook(&state.pool, id).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({"deleted": true}))).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "webhook not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

/// GET /api/webhooks/{id}/deliveries -- List delivery attempts.
async fn list_deliveries_handler(
    State(state): State<WebhookState>,
    auth: Option<axum::Extension<AuthContext>>,
    Path(id): Path<Uuid>,
    Query(query): Query<DeliveryQuery>,
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
    match get_webhook(&state.pool, id).await {
        Ok(Some(wh)) => {
            if wh.created_by != auth.user_id && !auth.roles.contains(&"admin".to_string()) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"error": "not your webhook"})),
                )
                    .into_response();
            }
        }
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "webhook not found"})),
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

    let limit = query.limit.clamp(1, 200);
    match list_deliveries(&state.pool, id, limit).await {
        Ok(deliveries) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "deliveries": serde_json::to_value(&deliveries).unwrap_or_default()
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

/// POST /api/webhooks/{id}/test -- Send a test payload to a webhook.
async fn test_webhook(
    State(state): State<WebhookState>,
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

    let webhook = match get_webhook(&state.pool, id).await {
        Ok(Some(wh)) => {
            if wh.created_by != auth.user_id && !auth.roles.contains(&"admin".to_string()) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"error": "not your webhook"})),
                )
                    .into_response();
            }
            wh
        }
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "webhook not found"})),
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
    };

    let delivery = state.sender.send_test(&webhook).await;
    (
        StatusCode::OK,
        Json(serde_json::to_value(&delivery).unwrap_or_default()),
    )
        .into_response()
}
