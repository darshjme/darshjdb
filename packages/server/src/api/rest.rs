//! Full REST API router for DarshanDB.
//!
//! Assembles all route groups (auth, data, functions, storage, SSE,
//! admin, docs) into a single [`axum::Router`] and provides the
//! handler implementations for each endpoint.
//!
//! # Content Negotiation
//!
//! All JSON-producing handlers inspect the `Accept` header. When the
//! client sends `Accept: application/msgpack`, responses are serialized
//! with MessagePack instead of JSON. Request bodies follow `Content-Type`.
//!
//! # Rate Limiting
//!
//! A [`RateLimitLayer`] injects `X-RateLimit-Limit`, `X-RateLimit-Remaining`,
//! and `X-RateLimit-Reset` headers into every response.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use super::error::{ApiError, ErrorCode};
use super::openapi;

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// Shared application state threaded through all handlers via Axum's
/// `State` extractor.
///
/// In production each field would hold a live connection pool, function
/// registry, storage backend, etc. The stubs here provide the type
/// signatures so the full route tree compiles and can be integration-tested
/// once the backing implementations land.
#[derive(Clone)]
pub struct AppState {
    /// Pre-computed OpenAPI 3.1 specification (JSON).
    pub openapi_spec: Arc<Value>,
    /// Broadcast channel for SSE subscription fan-out.
    pub sse_tx: broadcast::Sender<SsePayload>,
    /// Server boot instant for uptime reporting.
    pub started_at: Instant,
}

impl AppState {
    /// Create application state with default configuration.
    pub fn new() -> Self {
        let (sse_tx, _) = broadcast::channel(1024);
        Self {
            openapi_spec: Arc::new(openapi::generate_openapi_spec()),
            sse_tx,
            started_at: Instant::now(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Payload broadcast over the SSE channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsePayload {
    /// The query hash this event pertains to.
    pub query_hash: u64,
    /// The diff or full result, serialized as JSON.
    pub data: Value,
    /// Transaction ID that triggered this event.
    pub tx_id: i64,
}

// ---------------------------------------------------------------------------
// Content negotiation helpers
// ---------------------------------------------------------------------------

/// Returns `true` when the client prefers MessagePack over JSON.
fn wants_msgpack(headers: &HeaderMap) -> bool {
    headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("application/msgpack"))
        .unwrap_or(false)
}

/// Serialize `value` as JSON or MessagePack depending on the `Accept` header.
fn negotiate_response(headers: &HeaderMap, value: &impl Serialize) -> Response {
    if wants_msgpack(headers) {
        match rmp_serde::to_vec(value) {
            Ok(bytes) => {
                let mut resp = (StatusCode::OK, bytes).into_response();
                resp.headers_mut().insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/msgpack"),
                );
                resp
            }
            Err(e) => ApiError::internal(format!("msgpack encode: {e}")).into_response(),
        }
    } else {
        axum::Json(value).into_response()
    }
}

/// Serialize `value` with a specific status code, respecting content negotiation.
fn negotiate_response_status(
    headers: &HeaderMap,
    status: StatusCode,
    value: &impl Serialize,
) -> Response {
    if wants_msgpack(headers) {
        match rmp_serde::to_vec(value) {
            Ok(bytes) => {
                let mut resp = (status, bytes).into_response();
                resp.headers_mut().insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/msgpack"),
                );
                resp
            }
            Err(e) => ApiError::internal(format!("msgpack encode: {e}")).into_response(),
        }
    } else {
        (status, axum::Json(value)).into_response()
    }
}

// ---------------------------------------------------------------------------
// Rate-limit middleware
// ---------------------------------------------------------------------------

/// Middleware that injects rate-limit headers into every response.
///
/// The actual accounting is delegated to the auth module's [`RateLimiter`];
/// this layer only adds the standard headers.
async fn rate_limit_headers(req: Request<Body>, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    // Defaults; a real implementation reads from the rate limiter state.
    let limit = "1000";
    let remaining = "999";
    let reset = "60";

    if let Ok(v) = HeaderValue::from_str(limit) {
        headers.insert("X-RateLimit-Limit", v);
    }
    if let Ok(v) = HeaderValue::from_str(remaining) {
        headers.insert("X-RateLimit-Remaining", v);
    }
    if let Ok(v) = HeaderValue::from_str(reset) {
        headers.insert("X-RateLimit-Reset", v);
    }

    response
}

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

/// Build the complete DarshanDB REST API router.
///
/// Mount this under `/api` in your top-level Axum application:
///
/// ```rust,no_run
/// use darshandb_server::api::rest::{build_router, AppState};
///
/// let app = axum::Router::new()
///     .nest("/api", build_router(AppState::new()));
/// ```
pub fn build_router(state: AppState) -> Router {
    Router::new()
        // -- Auth ----------------------------------------------------------
        .route("/auth/signup", post(auth_signup))
        .route("/auth/signin", post(auth_signin))
        .route("/auth/magic-link", post(auth_magic_link))
        .route("/auth/verify", post(auth_verify))
        .route("/auth/oauth/{provider}", post(auth_oauth))
        .route("/auth/refresh", post(auth_refresh))
        .route("/auth/signout", post(auth_signout))
        .route("/auth/me", get(auth_me))
        // -- Data ----------------------------------------------------------
        .route("/query", post(query))
        .route("/mutate", post(mutate))
        .route("/data/{entity}", get(data_list).post(data_create))
        .route(
            "/data/{entity}/{id}",
            get(data_get).patch(data_patch).delete(data_delete),
        )
        // -- Functions -----------------------------------------------------
        .route("/fn/{name}", post(fn_invoke))
        // -- Storage -------------------------------------------------------
        .route("/storage/upload", post(storage_upload))
        .route("/storage/{*path}", get(storage_get).delete(storage_delete))
        // -- SSE -----------------------------------------------------------
        .route("/subscribe", get(subscribe))
        // -- Admin ---------------------------------------------------------
        .route("/admin/schema", get(admin_schema))
        .route("/admin/functions", get(admin_functions))
        .route("/admin/sessions", get(admin_sessions))
        // -- Docs ----------------------------------------------------------
        .route("/openapi.json", get(openapi_json))
        .route("/docs", get(docs))
        // -- Middleware ----------------------------------------------------
        .layer(middleware::from_fn(rate_limit_headers))
        .with_state(state)
}

// ===========================================================================
// Auth handlers
// ===========================================================================

/// `POST /api/auth/signup` — Create a new account with email and password.
#[derive(Deserialize)]
struct SignupRequest {
    email: String,
    password: String,
}

async fn auth_signup(
    State(_state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<SignupRequest>,
) -> Result<Response, ApiError> {
    // Input validation.
    if body.email.is_empty() || !body.email.contains('@') {
        return Err(ApiError::bad_request("Invalid email address"));
    }
    if body.password.len() < 8 {
        return Err(ApiError::bad_request(
            "Password must be at least 8 characters",
        ));
    }

    // TODO: wire to PasswordProvider + SessionManager once auth submodules land.
    let response = serde_json::json!({
        "access_token": format!("ddb_at_{}", Uuid::new_v4()),
        "refresh_token": format!("ddb_rt_{}", Uuid::new_v4()),
        "expires_in": 3600
    });

    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

/// `POST /api/auth/signin` — Authenticate with email and password.
#[derive(Deserialize)]
struct SigninRequest {
    email: String,
    password: String,
}

async fn auth_signin(
    State(_state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<SigninRequest>,
) -> Result<Response, ApiError> {
    if body.email.is_empty() {
        return Err(ApiError::bad_request("Email is required"));
    }
    if body.password.is_empty() {
        return Err(ApiError::bad_request("Password is required"));
    }

    // TODO: wire to PasswordProvider.
    let response = serde_json::json!({
        "access_token": format!("ddb_at_{}", Uuid::new_v4()),
        "refresh_token": format!("ddb_rt_{}", Uuid::new_v4()),
        "expires_in": 3600
    });

    Ok(negotiate_response(&headers, &response))
}

/// `POST /api/auth/magic-link` — Send a passwordless sign-in link.
#[derive(Deserialize)]
struct MagicLinkRequest {
    email: String,
}

async fn auth_magic_link(
    State(_state): State<AppState>,
    axum::Json(body): axum::Json<MagicLinkRequest>,
) -> Result<Response, ApiError> {
    if body.email.is_empty() || !body.email.contains('@') {
        return Err(ApiError::bad_request("Invalid email address"));
    }

    // Always return 200 to prevent email enumeration.
    // TODO: wire to MagicLinkProvider.
    Ok((
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "message": "If an account exists, a magic link has been sent."
        })),
    )
        .into_response())
}

/// `POST /api/auth/verify` — Verify a magic-link token or MFA code.
#[derive(Deserialize)]
struct VerifyRequest {
    token: String,
    mfa_code: Option<String>,
}

async fn auth_verify(
    State(_state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<VerifyRequest>,
) -> Result<Response, ApiError> {
    if body.token.is_empty() {
        return Err(ApiError::bad_request("Token is required"));
    }

    // TODO: wire to token verification + optional MFA check.
    let response = serde_json::json!({
        "access_token": format!("ddb_at_{}", Uuid::new_v4()),
        "refresh_token": format!("ddb_rt_{}", Uuid::new_v4()),
        "expires_in": 3600
    });

    Ok(negotiate_response(&headers, &response))
}

/// `POST /api/auth/oauth/:provider` — Exchange an OAuth2 authorization code.
#[derive(Deserialize)]
struct OAuthRequest {
    code: Option<String>,
    redirect_uri: Option<String>,
}

async fn auth_oauth(
    State(_state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<OAuthRequest>,
) -> Result<Response, ApiError> {
    let _valid_providers = ["google", "github", "apple"];
    if !_valid_providers.contains(&provider.as_str()) {
        return Err(ApiError::bad_request(format!(
            "Unsupported OAuth provider: {provider}"
        )));
    }

    let code = body
        .code
        .as_deref()
        .filter(|c| !c.is_empty())
        .ok_or_else(|| ApiError::bad_request("OAuth authorization code is required"))?;

    // TODO: wire to OAuth2Provider for the given provider kind.
    let _ = code;
    let response = serde_json::json!({
        "access_token": format!("ddb_at_{}", Uuid::new_v4()),
        "refresh_token": format!("ddb_rt_{}", Uuid::new_v4()),
        "expires_in": 3600
    });

    Ok(negotiate_response(&headers, &response))
}

/// `POST /api/auth/refresh` — Rotate a refresh token for a new token pair.
#[derive(Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

async fn auth_refresh(
    State(_state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<RefreshRequest>,
) -> Result<Response, ApiError> {
    if body.refresh_token.is_empty() {
        return Err(ApiError::bad_request("Refresh token is required"));
    }

    // TODO: wire to SessionManager::rotate.
    let response = serde_json::json!({
        "access_token": format!("ddb_at_{}", Uuid::new_v4()),
        "refresh_token": format!("ddb_rt_{}", Uuid::new_v4()),
        "expires_in": 3600
    });

    Ok(negotiate_response(&headers, &response))
}

/// `POST /api/auth/signout` — Revoke the current session.
async fn auth_signout(
    State(_state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // Require Bearer token.
    let _token = extract_bearer_token(&headers)?;

    // TODO: wire to SessionManager::revoke.
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/auth/me` — Return the authenticated user's profile.
async fn auth_me(State(_state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    // TODO: decode JWT and look up user record.
    let response = serde_json::json!({
        "user_id": Uuid::new_v4(),
        "email": "user@example.com",
        "roles": ["user"],
        "created_at": chrono::Utc::now().to_rfc3339()
    });

    Ok(negotiate_response(&headers, &response))
}

// ===========================================================================
// Data handlers
// ===========================================================================

/// `POST /api/query` — Execute a DarshanQL query over HTTP.
#[derive(Deserialize)]
struct QueryRequest {
    query: String,
    args: Option<HashMap<String, Value>>,
}

async fn query(
    State(_state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<QueryRequest>,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if body.query.is_empty() {
        return Err(ApiError::bad_request("Query string is required"));
    }

    let start = Instant::now();

    // TODO: wire to query engine with permission-injected context.
    let response = serde_json::json!({
        "data": [],
        "meta": {
            "count": 0,
            "duration_ms": start.elapsed().as_secs_f64() * 1000.0
        }
    });

    Ok(negotiate_response(&headers, &response))
}

/// `POST /api/mutate` — Submit a transaction of mutations over HTTP.
#[derive(Deserialize)]
struct MutateRequest {
    mutations: Vec<Mutation>,
}

/// A single mutation within a transaction.
#[derive(Deserialize)]
struct Mutation {
    op: MutationOp,
    entity: String,
    id: Option<Uuid>,
    data: Option<Value>,
}

/// Supported mutation operations.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum MutationOp {
    Insert,
    Update,
    Delete,
    Upsert,
}

async fn mutate(
    State(_state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<MutateRequest>,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if body.mutations.is_empty() {
        return Err(ApiError::bad_request("At least one mutation is required"));
    }

    // Validate each mutation.
    for (i, m) in body.mutations.iter().enumerate() {
        if m.entity.is_empty() {
            return Err(ApiError::bad_request(format!(
                "Mutation {i}: entity name is required"
            )));
        }
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

    // TODO: wire to triple store transaction engine.
    let response = serde_json::json!({
        "tx_id": 1,
        "affected": body.mutations.len()
    });

    Ok(negotiate_response(&headers, &response))
}

/// Query parameters for the data list endpoint.
#[derive(Deserialize)]
struct DataListParams {
    limit: Option<u32>,
    cursor: Option<String>,
    #[serde(flatten)]
    filters: HashMap<String, String>,
}

/// `GET /api/data/:entity` — List entities of a type with pagination.
async fn data_list(
    State(_state): State<AppState>,
    Path(entity): Path<String>,
    Query(params): Query<DataListParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;
    let _limit = params.limit.unwrap_or(50).min(1000);

    validate_entity_name(&entity)?;

    // TODO: wire to query engine with cursor pagination.
    let response = serde_json::json!({
        "data": [],
        "cursor": Value::Null,
        "has_more": false
    });

    Ok(negotiate_response(&headers, &response))
}

/// `POST /api/data/:entity` — Create a new entity.
async fn data_create(
    State(_state): State<AppState>,
    Path(entity): Path<String>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    validate_entity_name(&entity)?;

    if !body.is_object() {
        return Err(ApiError::bad_request("Request body must be a JSON object"));
    }

    let id = Uuid::new_v4();

    // TODO: wire to triple store insert.
    let response = serde_json::json!({
        "id": id,
        "entity": entity,
        "data": body
    });

    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

/// `GET /api/data/:entity/:id` — Fetch a single entity by ID.
async fn data_get(
    State(_state): State<AppState>,
    Path((entity, id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    validate_entity_name(&entity)?;

    // TODO: wire to triple store lookup.
    // For now, return not-found to show the error format works.
    Err(ApiError::not_found(format!(
        "{entity} with id {id} not found"
    )))
}

/// `PATCH /api/data/:entity/:id` — Partially update an entity.
async fn data_patch(
    State(_state): State<AppState>,
    Path((entity, id)): Path<(String, Uuid)>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    validate_entity_name(&entity)?;

    if !body.is_object() {
        return Err(ApiError::bad_request("Request body must be a JSON object"));
    }

    // TODO: wire to triple store update.
    let response = serde_json::json!({
        "id": id,
        "entity": entity,
        "data": body
    });

    Ok(negotiate_response(&headers, &response))
}

/// `DELETE /api/data/:entity/:id` — Delete an entity.
async fn data_delete(
    State(_state): State<AppState>,
    Path((entity, id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    validate_entity_name(&entity)?;

    // TODO: wire to triple store retract.
    let _ = id;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ===========================================================================
// Function handlers
// ===========================================================================

/// `POST /api/fn/:name` — Invoke a registered server-side function.
async fn fn_invoke(
    State(_state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    axum::Json(_args): axum::Json<Value>,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if name.is_empty() {
        return Err(ApiError::bad_request("Function name is required"));
    }

    // Validate function name format: alphanumeric, underscores, dots, hyphens.
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
    {
        return Err(ApiError::bad_request(
            "Function name contains invalid characters",
        ));
    }

    // TODO: wire to FunctionRegistry lookup + Runtime execution.
    let response = serde_json::json!({
        "result": Value::Null,
        "duration_ms": 0.0
    });

    Ok(negotiate_response(&headers, &response))
}

// ===========================================================================
// Storage handlers
// ===========================================================================

/// `POST /api/storage/upload` — Upload a file.
///
/// Accepts `multipart/form-data` with a `file` field and optional `path` field.
async fn storage_upload(
    State(_state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if body.is_empty() {
        return Err(ApiError::bad_request("Upload body is empty"));
    }

    // Size guard (50 MB default limit).
    const MAX_UPLOAD_SIZE: usize = 50 * 1024 * 1024;
    if body.len() > MAX_UPLOAD_SIZE {
        return Err(ApiError::new(
            ErrorCode::PayloadTooLarge,
            format!("File exceeds maximum size of {MAX_UPLOAD_SIZE} bytes"),
        ));
    }

    // TODO: wire to StorageEngine::put.
    let path = format!("uploads/{}", Uuid::new_v4());
    let response = serde_json::json!({
        "path": path,
        "size": body.len(),
        "content_type": headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream"),
        "signed_url": Value::Null
    });

    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

/// Query parameters for storage retrieval.
#[derive(Deserialize)]
struct StorageGetParams {
    /// Return a signed URL instead of the file content.
    signed: Option<bool>,
    /// Image transformation string (e.g. `w=200,h=200,fit=cover`).
    transform: Option<String>,
}

/// `GET /api/storage/*path` — Download a file or retrieve a signed URL.
async fn storage_get(
    State(_state): State<AppState>,
    Path(path): Path<String>,
    Query(params): Query<StorageGetParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if path.is_empty() {
        return Err(ApiError::bad_request("Storage path is required"));
    }

    // Prevent path traversal.
    if path.contains("..") {
        return Err(ApiError::bad_request("Path traversal is not allowed"));
    }

    if params.signed.unwrap_or(false) {
        // TODO: wire to StorageEngine::signed_url.
        let response = serde_json::json!({
            "signed_url": format!("/api/storage/{path}?token=signed_{}", Uuid::new_v4()),
            "expires_in": 3600
        });
        return Ok(negotiate_response(&headers, &response));
    }

    // TODO: wire to StorageEngine::get, apply transforms if requested.
    let _ = params.transform;
    Err(ApiError::not_found(format!("File not found: {path}")))
}

/// `DELETE /api/storage/*path` — Delete a stored file.
async fn storage_delete(
    State(_state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if path.is_empty() {
        return Err(ApiError::bad_request("Storage path is required"));
    }
    if path.contains("..") {
        return Err(ApiError::bad_request("Path traversal is not allowed"));
    }

    // TODO: wire to StorageEngine::delete.
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ===========================================================================
// SSE handler
// ===========================================================================

/// Query parameters for the SSE subscription endpoint.
#[derive(Deserialize)]
struct SubscribeParams {
    /// DarshanQL query to subscribe to.
    q: String,
}

/// `GET /api/subscribe?q=...` — Server-Sent Events for live query updates.
///
/// Authenticates via Bearer token, then streams events for the given query.
/// A heartbeat comment is sent every 15 seconds to keep the connection alive.
async fn subscribe(
    State(state): State<AppState>,
    Query(params): Query<SubscribeParams>,
    headers: HeaderMap,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if params.q.is_empty() {
        return Err(ApiError::bad_request("Query parameter 'q' is required"));
    }

    let rx = state.sse_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(payload) => {
            let data = serde_json::to_string(&payload.data).unwrap_or_default();
            Some(Ok(Event::default()
                .event("update")
                .data(data)
                .id(payload.tx_id.to_string())))
        }
        Err(_) => None,
    });

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("heartbeat"),
    ))
}

// ===========================================================================
// Admin handlers
// ===========================================================================

/// `GET /api/admin/schema` — Return the current inferred schema.
async fn admin_schema(
    State(_state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;
    require_admin_role(&headers)?;

    // TODO: wire to Schema introspection.
    let response = serde_json::json!({
        "entity_types": {},
        "as_of_tx": 0
    });

    Ok(negotiate_response(&headers, &response))
}

/// `GET /api/admin/functions` — List registered server-side functions.
async fn admin_functions(
    State(_state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;
    require_admin_role(&headers)?;

    // TODO: wire to FunctionRegistry.
    let response = serde_json::json!({
        "functions": []
    });

    Ok(negotiate_response(&headers, &response))
}

/// `GET /api/admin/sessions` — List active sync sessions.
async fn admin_sessions(
    State(_state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;
    require_admin_role(&headers)?;

    // TODO: wire to sync::SessionManager.
    let response = serde_json::json!({
        "sessions": [],
        "count": 0
    });

    Ok(negotiate_response(&headers, &response))
}

// ===========================================================================
// Docs handlers
// ===========================================================================

/// `GET /api/openapi.json` — Serve the OpenAPI 3.1 specification.
async fn openapi_json(State(state): State<AppState>) -> impl IntoResponse {
    axum::Json(state.openapi_spec.as_ref().clone())
}

/// `GET /api/docs` — Interactive Scalar API documentation viewer.
async fn docs(State(_state): State<AppState>) -> impl IntoResponse {
    Html(openapi::docs_html("/api/openapi.json"))
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Extract a Bearer token from the `Authorization` header.
///
/// Returns `Err(ApiError)` with a 401 status if the header is missing
/// or malformed.
fn extract_bearer_token(headers: &HeaderMap) -> Result<String, ApiError> {
    let header = headers
        .get(http::header::AUTHORIZATION)
        .ok_or_else(|| ApiError::unauthenticated("Missing Authorization header"))?;

    let value = header
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

/// Stub admin-role check. In production this decodes the JWT and checks roles.
fn require_admin_role(headers: &HeaderMap) -> Result<(), ApiError> {
    // TODO: decode JWT from bearer token, verify "admin" in roles.
    // For now, accept any authenticated request so the route compiles.
    let _ = headers;
    Ok(())
}

/// Validate that an entity name is safe and well-formed.
fn validate_entity_name(name: &str) -> Result<(), ApiError> {
    if name.is_empty() {
        return Err(ApiError::bad_request("Entity name is required"));
    }
    if name.len() > 128 {
        return Err(ApiError::bad_request(
            "Entity name is too long (max 128 chars)",
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ApiError::bad_request(
            "Entity name may only contain alphanumeric characters, underscores, and hyphens",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_extraction_valid() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer test_token_123"),
        );
        let token = extract_bearer_token(&headers);
        assert!(token.is_ok());
        assert_eq!(token.as_deref().ok(), Some("test_token_123"));
    }

    #[test]
    fn bearer_extraction_missing() {
        let headers = HeaderMap::new();
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn bearer_extraction_wrong_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic abc123"),
        );
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn entity_name_validation() {
        assert!(validate_entity_name("users").is_ok());
        assert!(validate_entity_name("my-entity").is_ok());
        assert!(validate_entity_name("my_entity_2").is_ok());
        assert!(validate_entity_name("").is_err());
        assert!(validate_entity_name("a/b").is_err());
        assert!(validate_entity_name("a b").is_err());
        assert!(validate_entity_name(&"a".repeat(129)).is_err());
    }

    #[test]
    fn wants_msgpack_detection() {
        let mut headers = HeaderMap::new();
        assert!(!wants_msgpack(&headers));

        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        assert!(!wants_msgpack(&headers));

        headers.insert(ACCEPT, HeaderValue::from_static("application/msgpack"));
        assert!(wants_msgpack(&headers));
    }
}
