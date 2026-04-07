//! Axum HTTP handlers for the collaboration and sharing system.
//!
//! Endpoints:
//!
//! | Method   | Path                        | Description              |
//! |----------|-----------------------------|--------------------------|
//! | `POST`   | `/api/share`                | Create a share link      |
//! | `GET`    | `/api/share/{token}`        | Access shared resource   |
//! | `DELETE` | `/api/share/{id}`           | Revoke share link        |
//! | `POST`   | `/api/collaborators`        | Invite collaborator      |
//! | `GET`    | `/api/collaborators`        | List collaborators       |
//! | `PATCH`  | `/api/collaborators/{id}`   | Update collaborator role |
//! | `DELETE` | `/api/collaborators/{id}`   | Remove collaborator      |
//! | `POST`   | `/api/workspaces`           | Create workspace         |
//! | `GET`    | `/api/workspaces`           | List user's workspaces   |
//! | `PATCH`  | `/api/workspaces/{id}`      | Update workspace         |

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::collaborator::{self, CollaboratorRole, InviteStatus};
use super::share::{self, ResourceType, SharePermission};
use super::workspace;
use crate::api::error::ApiError;
use crate::api::rest::AppState;
use crate::auth::AuthContext;

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the collaboration router. All routes require authentication
/// (enforced by the parent router's auth middleware layer).
pub fn collaboration_router() -> Router<AppState> {
    Router::new()
        // Share links
        .route("/share", post(create_share))
        .route("/share/{token}", get(access_share))
        .route("/share/{id}", delete(revoke_share))
        // Collaborators
        .route("/collaborators", post(invite_collaborator).get(list_collaborators))
        .route(
            "/collaborators/{id}",
            patch(update_collaborator_role).delete(remove_collaborator),
        )
        // Workspaces
        .route("/workspaces", post(create_workspace).get(list_workspaces))
        .route("/workspaces/{id}", patch(update_workspace))
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateShareRequest {
    resource_type: ResourceType,
    resource_id: Uuid,
    permission: SharePermission,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    max_uses: Option<u32>,
}

#[derive(Deserialize)]
struct AccessShareQuery {
    #[serde(default)]
    password: Option<String>,
}

#[derive(Deserialize)]
struct InviteCollaboratorRequest {
    email: String,
    resource_type: ResourceType,
    resource_id: Uuid,
    role: CollaboratorRole,
}

#[derive(Deserialize)]
struct ListCollaboratorsQuery {
    resource_type: ResourceType,
    resource_id: Uuid,
}

#[derive(Deserialize)]
struct UpdateCollaboratorRoleRequest {
    role: CollaboratorRole,
}

#[derive(Deserialize)]
struct CreateWorkspaceRequest {
    name: String,
    #[serde(default)]
    settings: Option<Value>,
}

#[derive(Deserialize)]
struct UpdateWorkspaceRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    settings: Option<Value>,
}

// ---------------------------------------------------------------------------
// Share handlers
// ---------------------------------------------------------------------------

/// `POST /api/share` — Create a new share link.
async fn create_share(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateShareRequest>,
) -> Result<Response, ApiError> {
    // Hash password if provided.
    let password_hash = match body.password {
        Some(ref pw) if !pw.is_empty() => {
            let hash = crate::auth::PasswordProvider::hash_password(pw)
                .map_err(|e| ApiError::internal(format!("password hashing failed: {e}")))?;
            Some(hash)
        }
        _ => None,
    };

    let link = share::create_share(
        &state.triple_store,
        body.resource_type,
        body.resource_id,
        body.permission,
        password_hash,
        body.expires_at,
        body.max_uses,
        ctx.user_id,
    )
    .await
    .map_err(|e| ApiError::internal(format!("failed to create share: {e}")))?;

    Ok((StatusCode::CREATED, Json(json!(link))).into_response())
}

/// `GET /api/share/{token}` — Access a shared resource via token.
async fn access_share(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Query(query): Query<AccessShareQuery>,
) -> Result<Response, ApiError> {
    let config = share::resolve_share(&state.triple_store, &token)
        .await
        .map_err(|e| ApiError::internal(format!("share resolution failed: {e}")))?;

    let config = match config {
        Some(c) => c,
        None => return Err(ApiError::not_found("Share link not found, expired, or revoked")),
    };

    // Verify password if the share is password-protected.
    if let Some(ref hash) = config.password_hash {
        let provided = query.password.as_deref().unwrap_or("");
        let valid = crate::auth::PasswordProvider::verify_password(provided, hash)
            .unwrap_or(false);
        if !valid {
            return Err(ApiError::new(
                crate::api::error::ErrorCode::Unauthenticated,
                "Invalid share password",
            ));
        }
    }

    // Increment usage counter.
    share::increment_use_count(&state.triple_store, config.id, config.use_count)
        .await
        .map_err(|e| ApiError::internal(format!("failed to update use count: {e}")))?;

    Ok(Json(json!({
        "resource_type": config.resource_type,
        "resource_id": config.resource_id,
        "permission": config.permission,
    }))
    .into_response())
}

/// `DELETE /api/share/{id}` — Revoke a share link.
async fn revoke_share(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    // Verify ownership: only the creator can revoke.
    let config = share::resolve_share_by_id(&state.triple_store, share::ShareId(id))
        .await
        .map_err(|e| ApiError::internal(format!("share lookup failed: {e}")))?;

    match config {
        Some(c) if c.created_by != ctx.user_id => {
            return Err(ApiError::permission_denied(
                "Only the share creator can revoke it",
            ));
        }
        None => return Err(ApiError::not_found("Share link not found")),
        _ => {}
    }

    share::revoke_share(&state.triple_store, share::ShareId(id))
        .await
        .map_err(|e| ApiError::internal(format!("failed to revoke share: {e}")))?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Collaborator handlers
// ---------------------------------------------------------------------------

/// `POST /api/collaborators` — Invite a collaborator.
async fn invite_collaborator(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<InviteCollaboratorRequest>,
) -> Result<Response, ApiError> {
    // Check that the inviting user has Owner or Admin role.
    let caller_role = collaborator::get_user_role(
        &state.triple_store,
        ctx.user_id,
        body.resource_type,
        body.resource_id,
    )
    .await
    .map_err(|e| ApiError::internal(format!("role lookup failed: {e}")))?;

    let caller_role = match caller_role {
        Some(r) if r.can_manage() => r,
        Some(_) => {
            return Err(ApiError::permission_denied(
                "Only Owners and Admins can invite collaborators",
            ));
        }
        None => {
            // Also check workspace-level permissions if this resource
            // belongs to a workspace. For now, deny if no explicit role.
            return Err(ApiError::permission_denied(
                "You do not have access to this resource",
            ));
        }
    };

    // Admins cannot invite as Admin or Owner.
    if caller_role == CollaboratorRole::Admin && body.role >= CollaboratorRole::Admin {
        return Err(ApiError::permission_denied(
            "Admins can only invite Editors, Commenters, or Viewers",
        ));
    }

    let collab = collaborator::invite_collaborator(
        &state.triple_store,
        &body.email,
        body.resource_type,
        body.resource_id,
        body.role,
        ctx.user_id,
    )
    .await
    .map_err(|e| ApiError::internal(format!("invite failed: {e}")))?;

    Ok((StatusCode::CREATED, Json(json!(collab))).into_response())
}

/// `GET /api/collaborators?resource_type=table&resource_id={uuid}` — List collaborators.
async fn list_collaborators(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(query): Query<ListCollaboratorsQuery>,
) -> Result<Response, ApiError> {
    let collabs = collaborator::list_collaborators(
        &state.triple_store,
        query.resource_type,
        query.resource_id,
    )
    .await
    .map_err(|e| ApiError::internal(format!("list failed: {e}")))?;

    // Only show collaborators if the caller is one of them.
    let is_member = collabs
        .iter()
        .any(|c| c.user_id == Some(ctx.user_id) && c.status == InviteStatus::Accepted);

    if !is_member {
        return Err(ApiError::permission_denied(
            "You are not a collaborator on this resource",
        ));
    }

    Ok(Json(json!(collabs)).into_response())
}

/// `PATCH /api/collaborators/{id}` — Update a collaborator's role.
async fn update_collaborator_role(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(collaborator_id): Path<Uuid>,
    Json(body): Json<UpdateCollaboratorRoleRequest>,
) -> Result<Response, ApiError> {
    // Load the target collaborator.
    let target = collaborator::load_collaborator(&state.triple_store, collaborator_id)
        .await
        .map_err(|e| ApiError::internal(format!("lookup failed: {e}")))?
        .ok_or_else(|| ApiError::not_found("Collaborator not found"))?;

    // Check caller's role on the same resource.
    let caller_role = collaborator::get_user_role(
        &state.triple_store,
        ctx.user_id,
        target.resource_type,
        target.resource_id,
    )
    .await
    .map_err(|e| ApiError::internal(format!("role lookup failed: {e}")))?
    .ok_or_else(|| ApiError::permission_denied("You are not a collaborator on this resource"))?;

    // Check hierarchy: caller must be able to modify the target's current role.
    if !caller_role.can_modify(target.role) {
        return Err(ApiError::permission_denied(
            "Insufficient permissions to modify this collaborator",
        ));
    }

    // Also check that caller can assign the new role.
    if !caller_role.can_modify(body.role) && body.role != CollaboratorRole::Viewer {
        return Err(ApiError::permission_denied(
            "Insufficient permissions to assign this role",
        ));
    }

    collaborator::update_role(&state.triple_store, collaborator_id, body.role)
        .await
        .map_err(|e| ApiError::internal(format!("role update failed: {e}")))?;

    Ok(Json(json!({"status": "updated", "new_role": body.role})).into_response())
}

/// `DELETE /api/collaborators/{id}` — Remove a collaborator.
async fn remove_collaborator(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(collaborator_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let target = collaborator::load_collaborator(&state.triple_store, collaborator_id)
        .await
        .map_err(|e| ApiError::internal(format!("lookup failed: {e}")))?
        .ok_or_else(|| ApiError::not_found("Collaborator not found"))?;

    // A user can always remove themselves.
    let is_self = target.user_id == Some(ctx.user_id);

    if !is_self {
        let caller_role = collaborator::get_user_role(
            &state.triple_store,
            ctx.user_id,
            target.resource_type,
            target.resource_id,
        )
        .await
        .map_err(|e| ApiError::internal(format!("role lookup failed: {e}")))?
        .ok_or_else(|| {
            ApiError::permission_denied("You are not a collaborator on this resource")
        })?;

        if !caller_role.can_modify(target.role) {
            return Err(ApiError::permission_denied(
                "Insufficient permissions to remove this collaborator",
            ));
        }
    }

    // Prevent removing the owner.
    if target.role == CollaboratorRole::Owner {
        return Err(ApiError::permission_denied(
            "Cannot remove the resource owner",
        ));
    }

    collaborator::remove_collaborator(&state.triple_store, collaborator_id)
        .await
        .map_err(|e| ApiError::internal(format!("removal failed: {e}")))?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Workspace handlers
// ---------------------------------------------------------------------------

/// `POST /api/workspaces` — Create a new workspace.
async fn create_workspace(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateWorkspaceRequest>,
) -> Result<Response, ApiError> {
    let ws = workspace::create_workspace(
        &state.triple_store,
        &body.name,
        ctx.user_id,
        body.settings,
    )
    .await
    .map_err(|e| ApiError::internal(format!("workspace creation failed: {e}")))?;

    Ok((StatusCode::CREATED, Json(json!(ws))).into_response())
}

/// `GET /api/workspaces` — List workspaces the authenticated user belongs to.
async fn list_workspaces(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Response, ApiError> {
    let workspaces = workspace::list_user_workspaces(&state.triple_store, ctx.user_id)
        .await
        .map_err(|e| ApiError::internal(format!("workspace listing failed: {e}")))?;

    Ok(Json(json!(workspaces)).into_response())
}

/// `PATCH /api/workspaces/{id}` — Update workspace name or settings.
async fn update_workspace(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<UpdateWorkspaceRequest>,
) -> Result<Response, ApiError> {
    // Check that the caller is an Owner or Admin of the workspace.
    let role = workspace::get_member_role(&state.triple_store, workspace_id, ctx.user_id)
        .await
        .map_err(|e| ApiError::internal(format!("role lookup failed: {e}")))?;

    match role {
        Some(r) if r.can_manage() => {}
        Some(_) => {
            return Err(ApiError::permission_denied(
                "Only Owners and Admins can update workspace settings",
            ));
        }
        None => {
            return Err(ApiError::permission_denied(
                "You are not a member of this workspace",
            ));
        }
    }

    workspace::update_workspace(
        &state.triple_store,
        workspace_id,
        body.name.as_deref(),
        body.settings,
    )
    .await
    .map_err(|e| ApiError::internal(format!("workspace update failed: {e}")))?;

    // Return the updated workspace.
    let ws = workspace::get_workspace(&state.triple_store, workspace_id)
        .await
        .map_err(|e| ApiError::internal(format!("workspace lookup failed: {e}")))?
        .ok_or_else(|| ApiError::not_found("Workspace not found after update"))?;

    Ok(Json(json!(ws)).into_response())
}
