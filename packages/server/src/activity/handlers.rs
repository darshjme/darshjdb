//! HTTP handlers for comments, activity log, and notifications.
//!
//! # Endpoints
//!
//! ## Comments
//! - `POST   /api/data/{entity}/{id}/comments`  — Add comment to a record
//! - `GET    /api/data/{entity}/{id}/comments`   — List threaded comments
//! - `PATCH  /api/comments/{id}`                 — Edit a comment
//! - `DELETE /api/comments/{id}`                 — Soft-delete a comment
//!
//! ## Activity
//! - `GET /api/data/{entity}/{id}/activity`      — Activity log for a record
//! - `GET /api/activity?user={id}`               — Activity by a user
//!
//! ## Notifications
//! - `GET   /api/notifications`                  — Current user's notifications
//! - `PATCH /api/notifications/{id}/read`        — Mark one notification read
//! - `PATCH /api/notifications/read-all`         — Mark all read
//! - `GET   /api/notifications/count`            — Unread count (for badge)

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::rest::AppState;

use super::activity::{self, Action, RecordActivityInput};
use super::comments::{self, CreateCommentInput, UpdateCommentInput};
use super::notifications;

// ── Auth helper (re-use the pattern from rest.rs) ──────────────────

/// Extract the authenticated user's ID from the request headers.
///
/// Delegates to the session manager for JWT validation. Returns an
/// `ApiError` if the token is missing or invalid.
fn extract_user_id(headers: &HeaderMap, state: &AppState) -> Result<Uuid, ApiError> {
    let token = extract_bearer_token(headers)?;
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let ua = headers
        .get(http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let dfp = headers
        .get("x-device-fingerprint")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let auth_ctx = state
        .session_manager
        .validate_token(&token, ip, ua, dfp)
        .map_err(|e| ApiError::unauthenticated(format!("Invalid token: {e}")))?;
    Ok(auth_ctx.user_id)
}

fn extract_bearer_token(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers
        .get(http::header::AUTHORIZATION)
        .ok_or_else(|| ApiError::unauthenticated("Missing Authorization header"))?
        .to_str()
        .map_err(|_| ApiError::unauthenticated("Invalid Authorization header encoding"))?;

    if !value.starts_with("Bearer ") {
        return Err(ApiError::unauthenticated(
            "Authorization header must use Bearer scheme",
        ));
    }

    let token = value[7..].trim().to_string();
    if token.is_empty() {
        return Err(ApiError::unauthenticated("Bearer token is empty"));
    }

    Ok(token)
}

// ===========================================================================
// Comment handlers
// ===========================================================================

/// `POST /api/data/{entity}/{id}/comments` — Add a comment to a record.
pub async fn comment_create(
    State(state): State<AppState>,
    Path((_entity, entity_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
    axum::Json(input): axum::Json<CreateCommentInput>,
) -> Result<Response, ApiError> {
    let user_id = extract_user_id(&headers, &state)?;

    let comment = comments::create_comment(&state.pool, entity_id, user_id, &input)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create comment: {e}")))?;

    // Record activity.
    let _ = activity::record_activity(
        &state.pool,
        RecordActivityInput {
            entity_type: "comment".into(),
            entity_id,
            action: Action::Commented,
            user_id,
            changes: vec![],
            metadata: Some(json!({ "comment_id": comment.id })),
        },
    )
    .await;

    // Auto-generate notifications for mentions.
    if !input.mentions.is_empty() {
        let _ = notifications::notify_mentions(
            &state.pool,
            user_id,
            comment.id,
            entity_id,
            &input.mentions,
            &input.content,
        )
        .await;
    }

    // Auto-generate reply notification.
    if let Some(parent_id) = input.reply_to {
        if let Ok(parent) = comments::get_comment_by_id(&state.pool, parent_id).await {
            let _ = notifications::notify_reply(
                &state.pool,
                user_id,
                comment.id,
                parent.user_id,
                &input.content,
            )
            .await;
        }
    }

    Ok((StatusCode::CREATED, axum::Json(json!({ "comment": comment }))).into_response())
}

/// `GET /api/data/{entity}/{id}/comments` — List threaded comments.
pub async fn comment_list(
    State(state): State<AppState>,
    Path((_entity, entity_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _user_id = extract_user_id(&headers, &state)?;

    let threads = comments::list_comments(&state.pool, entity_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list comments: {e}")))?;

    Ok((StatusCode::OK, axum::Json(json!({ "comments": threads }))).into_response())
}

/// `PATCH /api/comments/{id}` — Edit a comment.
pub async fn comment_update(
    State(state): State<AppState>,
    Path(comment_id): Path<Uuid>,
    headers: HeaderMap,
    axum::Json(input): axum::Json<UpdateCommentInput>,
) -> Result<Response, ApiError> {
    let user_id = extract_user_id(&headers, &state)?;

    let comment = comments::update_comment(&state.pool, comment_id, user_id, &input)
        .await
        .map_err(|e| match &e {
            crate::error::DarshJError::EntityNotFound(_) => {
                ApiError::not_found("Comment not found")
            }
            crate::error::DarshJError::Internal(msg) if msg.contains("only the comment author") => {
                ApiError::permission_denied("Only the comment author can edit")
            }
            _ => ApiError::internal(format!("Failed to update comment: {e}")),
        })?;

    Ok((StatusCode::OK, axum::Json(json!({ "comment": comment }))).into_response())
}

/// `DELETE /api/comments/{id}` — Soft-delete a comment.
pub async fn comment_delete(
    State(state): State<AppState>,
    Path(comment_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user_id = extract_user_id(&headers, &state)?;

    comments::delete_comment(&state.pool, comment_id, user_id)
        .await
        .map_err(|e| match &e {
            crate::error::DarshJError::EntityNotFound(_) => {
                ApiError::not_found("Comment not found")
            }
            crate::error::DarshJError::Internal(msg) if msg.contains("only the comment author") => {
                ApiError::permission_denied("Only the comment author can delete")
            }
            _ => ApiError::internal(format!("Failed to delete comment: {e}")),
        })?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

// ===========================================================================
// Activity handlers
// ===========================================================================

/// Query parameters for the activity endpoints.
#[derive(Debug, Deserialize)]
pub struct ActivityParams {
    /// Filter by user id.
    pub user: Option<Uuid>,
    /// Maximum number of entries to return (default 50, max 500).
    pub limit: Option<u32>,
}

/// `GET /api/data/{entity}/{id}/activity` — Activity log for a record.
pub async fn activity_for_record(
    State(state): State<AppState>,
    Path((_entity, entity_id)): Path<(String, Uuid)>,
    Query(params): Query<ActivityParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _user_id = extract_user_id(&headers, &state)?;
    let limit = params.limit.unwrap_or(50).min(500);

    let entries = activity::get_activity(&state.pool, entity_id, limit)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to fetch activity: {e}")))?;

    Ok((StatusCode::OK, axum::Json(json!({ "activity": entries }))).into_response())
}

/// `GET /api/activity?user={id}` — Get a user's activity or table-level activity.
pub async fn activity_query(
    State(state): State<AppState>,
    Query(params): Query<ActivityParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth_user_id = extract_user_id(&headers, &state)?;
    let limit = params.limit.unwrap_or(50).min(500);

    let user_id = params.user.unwrap_or(auth_user_id);
    let entries = activity::get_user_activity(&state.pool, user_id, limit)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to fetch user activity: {e}")))?;

    Ok((StatusCode::OK, axum::Json(json!({ "activity": entries }))).into_response())
}

// ===========================================================================
// Notification handlers
// ===========================================================================

/// Query parameters for the notifications endpoint.
#[derive(Debug, Deserialize)]
pub struct NotificationParams {
    /// If true, only return unread notifications.
    pub unread_only: Option<bool>,
}

/// `GET /api/notifications` — Get current user's notifications.
pub async fn notifications_list(
    State(state): State<AppState>,
    Query(params): Query<NotificationParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user_id = extract_user_id(&headers, &state)?;
    let unread_only = params.unread_only.unwrap_or(false);

    let notifs = notifications::get_notifications(&state.pool, user_id, unread_only)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to fetch notifications: {e}")))?;

    Ok((StatusCode::OK, axum::Json(json!({ "notifications": notifs }))).into_response())
}

/// `PATCH /api/notifications/{id}/read` — Mark a notification as read.
pub async fn notification_mark_read(
    State(state): State<AppState>,
    Path(notification_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _user_id = extract_user_id(&headers, &state)?;

    notifications::mark_read(&state.pool, notification_id)
        .await
        .map_err(|e| match &e {
            crate::error::DarshJError::EntityNotFound(_) => {
                ApiError::not_found("Notification not found or already read")
            }
            _ => ApiError::internal(format!("Failed to mark notification read: {e}")),
        })?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `PATCH /api/notifications/read-all` — Mark all notifications as read.
pub async fn notification_mark_all_read(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user_id = extract_user_id(&headers, &state)?;

    let count = notifications::mark_all_read(&state.pool, user_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to mark all read: {e}")))?;

    Ok((StatusCode::OK, axum::Json(json!({ "marked_read": count }))).into_response())
}

/// `GET /api/notifications/count` — Unread notification count (for badge).
pub async fn notification_unread_count(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user_id = extract_user_id(&headers, &state)?;

    let count = notifications::unread_count(&state.pool, user_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to count notifications: {e}")))?;

    Ok((StatusCode::OK, axum::Json(json!({ "unread_count": count }))).into_response())
}
