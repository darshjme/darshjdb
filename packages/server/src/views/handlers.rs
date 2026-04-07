//! Axum REST handlers for the view system.
//!
//! All handlers follow the existing DarshJDB pattern: `State<AppState>`,
//! `HeaderMap` for content negotiation, `Path`/`Query` extractors, and
//! `Result<Response, ApiError>` return types.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::rest::{negotiate_response_pub as negotiate_response, AppState};
use crate::auth::{AuthContext, Operation};
use crate::query::{self, QueryResultRow};
use crate::views::query::{apply_view_to_query, project_fields};
use crate::views::{CreateViewRequest, PgViewStore, ViewStore, ViewUpdate};

// ── Helper ─────────────────────────────────────────────────────────

/// Extract the authenticated user from the request extensions.
///
/// The auth middleware has already validated the JWT and inserted an
/// `AuthContext` into the request extensions before we reach handlers.
fn extract_auth(headers: &HeaderMap, state: &AppState) -> Result<AuthContext, ApiError> {
    let token = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim());

    let token = match token {
        Some(t) if !t.is_empty() => t,
        _ => return Err(ApiError::unauthenticated("Missing Bearer token")),
    };

    // Dev mode shortcut.
    if state.dev_mode && token == "dev" {
        return Ok(AuthContext {
            user_id: Uuid::nil(),
            session_id: Uuid::nil(),
            roles: vec!["admin".into(), "user".into()],
            ip: "127.0.0.1".into(),
            user_agent: "dev-mode".into(),
            device_fingerprint: "dev".into(),
        });
    }

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

    state
        .session_manager
        .validate_token(token, ip, ua, dfp)
        .map_err(ApiError::from)
}

/// Build a `PgViewStore` from the shared application state.
fn view_store(state: &AppState) -> PgViewStore {
    PgViewStore::new(
        crate::triple_store::PgTripleStore::new_lazy(state.pool.clone()),
    )
}

// ── Handlers ───────────────────────────────────────────────────────

/// `POST /api/views` — Create a new view for an entity type.
pub async fn create_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<CreateViewRequest>,
) -> Result<Response, ApiError> {
    let auth = extract_auth(&headers, &state)?;
    let store = view_store(&state);

    let view = store
        .create_view(body, auth.user_id)
        .await
        .map_err(ApiError::from)?;

    let response = serde_json::json!({
        "data": view,
        "meta": { "created": true }
    });

    Ok(negotiate_response(&headers, &response))
}

/// Query parameters for `GET /api/views`.
#[derive(Deserialize)]
pub struct ListViewsParams {
    /// Entity type to list views for (required).
    #[serde(rename = "type")]
    pub entity_type: Option<String>,
}

/// `GET /api/views` — List all views for an entity type.
pub async fn list_views(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<ListViewsParams>,
) -> Result<Response, ApiError> {
    let _auth = extract_auth(&headers, &state)?;
    let store = view_store(&state);

    let entity_type = params
        .entity_type
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("query parameter 'type' is required"))?;

    let views = store.list_views(entity_type).await.map_err(ApiError::from)?;

    let response = serde_json::json!({
        "data": views,
        "meta": { "count": views.len() }
    });

    Ok(negotiate_response(&headers, &response))
}

/// `GET /api/views/:id` — Get a single view configuration.
pub async fn get_view(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _auth = extract_auth(&headers, &state)?;
    let store = view_store(&state);

    let view = store.get_view(id).await.map_err(ApiError::from)?;

    Ok(negotiate_response(&headers, &view))
}

/// `PATCH /api/views/:id` — Partially update a view configuration.
pub async fn update_view(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<ViewUpdate>,
) -> Result<Response, ApiError> {
    let _auth = extract_auth(&headers, &state)?;
    let store = view_store(&state);

    let view = store.update_view(id, body).await.map_err(ApiError::from)?;

    let response = serde_json::json!({
        "data": view,
        "meta": { "updated": true }
    });

    Ok(negotiate_response(&headers, &response))
}

/// `DELETE /api/views/:id` — Delete a view.
pub async fn delete_view(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _auth = extract_auth(&headers, &state)?;
    let store = view_store(&state);

    store.delete_view(id).await.map_err(ApiError::from)?;

    let response = serde_json::json!({
        "data": null,
        "meta": { "deleted": true }
    });

    Ok(negotiate_response(&headers, &response))
}

// ── View query ─────────────────────────────────────────────────────

/// Request body for `POST /api/views/:id/query`.
#[derive(Deserialize)]
pub struct ViewQueryRequest {
    /// Optional additional DarshJQL query to layer on top of the view.
    /// If omitted, only the view's built-in filters/sorts are applied.
    #[serde(default)]
    pub query: Option<serde_json::Value>,
}

/// `POST /api/views/:id/query` — Execute a query through a view's lens.
///
/// The view's filters and sorts are merged with any user-supplied query.
/// Hidden fields are stripped from the result. Field ordering is applied.
pub async fn query_view(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<ViewQueryRequest>,
) -> Result<Response, ApiError> {
    let _auth = extract_auth(&headers, &state)?;
    let store = view_store(&state);
    let start = Instant::now();

    // Load the view configuration.
    let view = store.get_view(id).await.map_err(ApiError::from)?;

    // Build the base query AST — either from user input or a bare type query.
    let mut ast = if let Some(ref user_query) = body.query {
        query::parse_darshan_ql(user_query)
            .map_err(|e| ApiError::bad_request(format!("Invalid query: {e}")))?
    } else {
        query::QueryAST {
            entity_type: view.table_entity_type.clone(),
            where_clauses: vec![],
            order: vec![],
            limit: None,
            offset: None,
            search: None,
            semantic: None,
            hybrid: None,
            nested: vec![],
        }
    };

    // Ensure the query targets the same entity type as the view.
    if ast.entity_type != view.table_entity_type {
        return Err(ApiError::bad_request(format!(
            "Query entity type '{}' does not match view's table '{}'",
            ast.entity_type, view.table_entity_type,
        )));
    }

    // Apply view filters and sorts onto the AST.
    apply_view_to_query(&mut ast, &view);

    // Plan and execute.
    let plan = query::plan_query(&ast)
        .map_err(|e| ApiError::bad_request(format!("Query planning failed: {e}")))?;

    let mut results: Vec<QueryResultRow> = query::execute_query(&state.pool, &plan)
        .await
        .map_err(|e| ApiError::internal(format!("Query execution failed: {e}")))?;

    // Apply field projection (hide fields, reorder).
    project_fields(&mut results, &view);

    let count = results.len();
    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

    let response = serde_json::json!({
        "data": results,
        "meta": {
            "view_id": view.id,
            "view_name": view.name,
            "view_kind": view.kind,
            "count": count,
            "duration_ms": duration_ms,
        }
    });

    Ok(negotiate_response(&headers, &response))
}

// ── Route builder ──────────────────────────────────────────────────

/// Build the Axum router fragment for view endpoints.
///
/// Mount this inside the protected routes section of `build_router`.
pub fn view_routes() -> axum::Router<AppState> {
    use axum::routing::{get, post};

    axum::Router::new()
        .route("/views", post(create_view).get(list_views))
        .route(
            "/views/{id}",
            get(get_view).patch(update_view).delete(delete_view),
        )
        .route("/views/{id}/query", post(query_view))
}
