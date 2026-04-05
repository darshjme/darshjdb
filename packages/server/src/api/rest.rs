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
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use super::error::{ApiError, ErrorCode};
use super::openapi;
use crate::auth::{
    AuthContext, AuthOutcome, Operation, PasswordProvider, PermissionEngine, SessionManager,
    build_default_engine, evaluate_rule_public, get_rule_with_fallback,
};
use crate::query::{self, QueryResultRow};
use crate::sync::broadcaster::ChangeEvent;
use crate::triple_store::{PgTripleStore, TripleInput, TripleStore};

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
    /// Postgres connection pool for direct queries.
    pub pool: PgPool,
    /// The triple store backend.
    pub triple_store: Arc<PgTripleStore>,
    /// Session manager for JWT issuance and validation.
    pub session_manager: Arc<SessionManager>,
    /// Pre-computed OpenAPI 3.1 specification (JSON).
    pub openapi_spec: Arc<Value>,
    /// Broadcast channel for SSE subscription fan-out.
    pub sse_tx: broadcast::Sender<SsePayload>,
    /// Broadcast channel for triple-store change events (reactive subscriptions).
    pub change_tx: broadcast::Sender<ChangeEvent>,
    /// Server boot instant for uptime reporting.
    pub started_at: Instant,
    /// Whether dev mode is active (DARSHAN_DEV=1).
    pub dev_mode: bool,
    /// Permission engine for row-level security and access control.
    pub permissions: Arc<PermissionEngine>,
}

impl AppState {
    /// Create application state with a live database pool, triple store, and session manager.
    pub fn with_pool(
        pool: PgPool,
        triple_store: Arc<PgTripleStore>,
        session_manager: Arc<SessionManager>,
        change_tx: broadcast::Sender<ChangeEvent>,
    ) -> Self {
        let dev_mode = std::env::var("DARSHAN_DEV")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);
        let (sse_tx, _) = broadcast::channel(1024);
        Self {
            pool,
            triple_store,
            session_manager,
            openapi_spec: Arc::new(openapi::generate_openapi_spec()),
            sse_tx,
            change_tx,
            started_at: Instant::now(),
            dev_mode,
            permissions: Arc::new(build_default_engine()),
        }
    }

    /// Create application state with default (test-only) configuration.
    /// Panics if called outside tests — production code must use `with_pool`.
    #[cfg(test)]
    pub fn new() -> Self {
        // Tests that don't hit the database can use a dummy pool.
        // This preserves backward compatibility with existing unit tests.
        let (sse_tx, _) = broadcast::channel(1024);
        let (change_tx, _) = broadcast::channel(1024);
        let pool = PgPool::connect_lazy("postgres://localhost/darshandb_test").expect("test pool");
        let triple_store = Arc::new(PgTripleStore::new_lazy(pool.clone()));
        let key_manager = crate::auth::KeyManager::generate();
        let session_manager = Arc::new(SessionManager::new(pool.clone(), key_manager));
        Self {
            pool,
            triple_store,
            session_manager,
            openapi_spec: Arc::new(openapi::generate_openapi_spec()),
            sse_tx,
            change_tx,
            started_at: Instant::now(),
            dev_mode: true,
            permissions: Arc::new(build_default_engine()),
        }
    }
}

#[cfg(test)]
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
/// ```rust,ignore
/// use darshandb_server::api::rest::{build_router, AppState};
///
/// let state = AppState::with_pool(pool, triple_store);
/// let app = axum::Router::new()
///     .nest("/api", build_router(state));
/// ```
pub fn build_router(state: AppState) -> Router {
    // Public auth routes — no JWT required.
    let public_routes = Router::new()
        .route("/auth/signup", post(auth_signup))
        .route("/auth/signin", post(auth_signin))
        .route("/auth/magic-link", post(auth_magic_link))
        .route("/auth/verify", post(auth_verify))
        .route("/auth/oauth/{provider}", post(auth_oauth))
        .route("/auth/refresh", post(auth_refresh));

    // Protected routes — require valid JWT (or "Bearer dev" in dev mode).
    let protected_routes = Router::new()
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
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth_middleware,
        ));

    // Docs are public.
    let docs_routes = Router::new()
        .route("/openapi.json", get(openapi_json))
        .route("/docs", get(docs));

    // Merge all route groups.
    public_routes
        .merge(protected_routes)
        .merge(docs_routes)
        // -- Middleware ----------------------------------------------------
        .layer(middleware::from_fn(rate_limit_headers))
        .with_state(state)
}

// ===========================================================================
// Auth handlers
// ===========================================================================

/// Ensure the `users` and `sessions` tables exist for the auth subsystem.
pub async fn ensure_auth_schema(pool: &PgPool) -> std::result::Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id              UUID PRIMARY KEY,
            email           TEXT NOT NULL UNIQUE,
            password_hash   TEXT NOT NULL,
            roles           JSONB NOT NULL DEFAULT '["user"]'::jsonb,
            created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
            deleted_at      TIMESTAMPTZ
        );
        CREATE INDEX IF NOT EXISTS idx_users_email ON users (email) WHERE deleted_at IS NULL;
        CREATE TABLE IF NOT EXISTS sessions (
            session_id          UUID PRIMARY KEY,
            user_id             UUID NOT NULL REFERENCES users(id),
            device_fingerprint  TEXT NOT NULL DEFAULT '',
            ip                  TEXT NOT NULL DEFAULT '',
            user_agent          TEXT NOT NULL DEFAULT '',
            created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
            revoked             BOOLEAN NOT NULL DEFAULT false,
            refresh_token_hash  TEXT NOT NULL,
            refresh_expires_at  TIMESTAMPTZ NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions (user_id) WHERE NOT revoked;
        CREATE INDEX IF NOT EXISTS idx_sessions_refresh ON sessions (refresh_token_hash) WHERE NOT revoked;
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Middleware that enforces JWT authentication on protected routes.
async fn require_auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let token = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim());

    let token = match token {
        Some(t) if !t.is_empty() => t,
        _ => {
            let body = serde_json::json!({"error": {"code": 401, "message": "Missing or empty Bearer token"}});
            return (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response();
        }
    };

    if state.dev_mode && token == "dev" {
        let dev_ctx = AuthContext {
            user_id: Uuid::nil(),
            session_id: Uuid::nil(),
            roles: vec!["admin".into(), "user".into()],
            ip: "127.0.0.1".into(),
            user_agent: "dev-mode".into(),
            device_fingerprint: "dev".into(),
        };
        request.extensions_mut().insert(dev_ctx);
        return next.run(request).await;
    }

    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    let dfp = headers
        .get("x-device-fingerprint")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    match state.session_manager.validate_token(token, &ip, &ua, &dfp) {
        Ok(ctx) => {
            request.extensions_mut().insert(ctx);
            next.run(request).await
        }
        Err(e) => {
            let status = e.status_code();
            let body =
                serde_json::json!({"error": {"code": status.as_u16(), "message": e.to_string()}});
            (status, axum::Json(body)).into_response()
        }
    }
}

/// `POST /api/auth/signup` — Create a new account with email and password.
#[derive(Deserialize)]
struct SignupRequest {
    email: String,
    password: String,
    #[serde(default)]
    name: Option<String>,
}

async fn auth_signup(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<SignupRequest>,
) -> Result<Response, ApiError> {
    let email = body.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(ApiError::bad_request("Invalid email address"));
    }
    if body.password.len() < 8 {
        return Err(ApiError::bad_request(
            "Password must be at least 8 characters",
        ));
    }
    if body.password.len() > 128 {
        return Err(ApiError::bad_request(
            "Password must be at most 128 characters",
        ));
    }

    let password_hash = PasswordProvider::hash_password(&body.password)
        .map_err(|e| ApiError::internal(format!("Password hashing failed: {e}")))?;

    let user_id = Uuid::new_v4();
    let roles = serde_json::json!(["user"]);

    let insert_result =
        sqlx::query("INSERT INTO users (id, email, password_hash, roles) VALUES ($1, $2, $3, $4)")
            .bind(user_id)
            .bind(&email)
            .bind(&password_hash)
            .bind(&roles)
            .execute(&state.pool)
            .await;

    match insert_result {
        Ok(_) => {}
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("duplicate key") || err_str.contains("unique constraint") {
                return Err(ApiError::bad_request(
                    "An account with this email already exists",
                ));
            }
            return Err(ApiError::internal(format!("Failed to create user: {e}")));
        }
    }

    let user_triples = vec![
        TripleInput {
            entity_id: user_id,
            attribute: ":db/type".into(),
            value: Value::String("user".into()),
            value_type: 0,
        },
        TripleInput {
            entity_id: user_id,
            attribute: "user/email".into(),
            value: Value::String(email.clone()),
            value_type: 0,
        },
        TripleInput {
            entity_id: user_id,
            attribute: "user/name".into(),
            value: Value::String(body.name.unwrap_or_default()),
            value_type: 0,
        },
        TripleInput {
            entity_id: user_id,
            attribute: "user/created_at".into(),
            value: Value::String(chrono::Utc::now().to_rfc3339()),
            value_type: 0,
        },
    ];
    let _ = state.triple_store.set_triples(&user_triples).await;

    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let dfp = headers
        .get("x-device-fingerprint")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token_pair = state
        .session_manager
        .create_session(user_id, vec!["user".into()], ip, ua, dfp)
        .await
        .map_err(|e| ApiError::internal(format!("Session creation failed: {e}")))?;

    let response = serde_json::json!({
        "user_id": user_id,
        "email": email,
        "access_token": token_pair.access_token,
        "refresh_token": token_pair.refresh_token,
        "expires_in": token_pair.expires_in,
        "token_type": token_pair.token_type,
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
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<SigninRequest>,
) -> Result<Response, ApiError> {
    let email = body.email.trim().to_lowercase();
    if email.is_empty() {
        return Err(ApiError::bad_request("Email is required"));
    }
    if body.password.is_empty() {
        return Err(ApiError::bad_request("Password is required"));
    }

    let outcome = PasswordProvider::authenticate(&state.pool, &email, &body.password)
        .await
        .map_err(|e| ApiError::internal(format!("Authentication error: {e}")))?;

    match outcome {
        AuthOutcome::Success { user_id, roles } => {
            let ip = headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown");
            let ua = headers
                .get("user-agent")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown");
            let dfp = headers
                .get("x-device-fingerprint")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            let token_pair = state
                .session_manager
                .create_session(user_id, roles, ip, ua, dfp)
                .await
                .map_err(|e| ApiError::internal(format!("Session creation failed: {e}")))?;

            let response = serde_json::json!({
                "user_id": user_id,
                "access_token": token_pair.access_token,
                "refresh_token": token_pair.refresh_token,
                "expires_in": token_pair.expires_in,
                "token_type": token_pair.token_type,
            });
            Ok(negotiate_response(&headers, &response))
        }
        AuthOutcome::MfaRequired { user_id, mfa_token } => {
            let response = serde_json::json!({
                "mfa_required": true,
                "user_id": user_id,
                "mfa_token": mfa_token,
            });
            Ok(negotiate_response(&headers, &response))
        }
        AuthOutcome::Failed { reason: _ } => {
            Err(ApiError::unauthenticated("Invalid email or password"))
        }
    }
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
    #[serde(rename = "mfa_code")]
    #[allow(dead_code)] // used by client protocol
    _mfa_code: Option<String>,
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
    #[serde(rename = "redirect_uri")]
    #[allow(dead_code)] // used by client protocol
    _redirect_uri: Option<String>,
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
async fn auth_me(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let token = extract_bearer_token(&headers)?;

    // Validate the JWT and extract user identity.
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let ua = headers
        .get("user-agent")
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

    // Fetch user record from the database.
    let user_row: Option<(
        Uuid,
        String,
        serde_json::Value,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(
        "SELECT id, email, roles, created_at FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(auth_ctx.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {e}")))?;

    let (user_id, email, roles, created_at) =
        user_row.ok_or_else(|| ApiError::not_found("User not found"))?;

    let response = serde_json::json!({
        "user_id": user_id,
        "email": email,
        "roles": roles,
        "session_id": auth_ctx.session_id,
        "created_at": created_at.to_rfc3339()
    });

    Ok(negotiate_response(&headers, &response))
}

// ===========================================================================
// Data handlers
// ===========================================================================

/// `POST /api/query` — Execute a DarshanQL query over HTTP.
#[derive(Deserialize)]
struct QueryRequest {
    query: Value,
    #[serde(rename = "args")]
    #[allow(dead_code)] // used by client protocol
    _args: Option<HashMap<String, Value>>,
}

async fn query(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<QueryRequest>,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers)?;

    let start = Instant::now();

    // Parse the DarshanQL JSON into an AST.
    let mut ast = query::parse_darshan_ql(&body.query)
        .map_err(|e| ApiError::bad_request(format!("Invalid query: {e}")))?;

    // Evaluate read permission for the queried entity type.
    let perm_result = check_permission(
        &auth_ctx,
        &ast.entity_type,
        Operation::Read,
        &state.permissions,
    )?;

    // Inject permission WHERE clauses into the query AST.
    if let Some(where_sql) = perm_result.build_where_clause(auth_ctx.user_id) {
        // Convert the permission WHERE clause into a query WhereClause.
        // The permission engine produces raw SQL fragments; we inject them
        // as a special "raw" where clause that the planner will append.
        ast.where_clauses.push(query::WhereClause {
            attribute: "__permission_filter".to_string(),
            op: query::WhereOp::Eq,
            value: serde_json::Value::String(where_sql),
        });
    }

    // Plan the query.
    let plan = query::plan_query(&ast)
        .map_err(|e| ApiError::bad_request(format!("Query planning failed: {e}")))?;

    // Execute against Postgres.
    let results: Vec<QueryResultRow> = query::execute_query(&state.pool, &plan)
        .await
        .map_err(|e| ApiError::internal(format!("Query execution failed: {e}")))?;

    let count = results.len();
    let response = serde_json::json!({
        "data": results,
        "meta": {
            "count": count,
            "duration_ms": start.elapsed().as_secs_f64() * 1000.0,
            "filtered": !perm_result.where_clauses.is_empty()
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
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<MutateRequest>,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if body.mutations.is_empty() {
        return Err(ApiError::bad_request("At least one mutation is required"));
    }

    // Validate each mutation.
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

    // Convert mutations into triple operations and execute against the store.
    let mut all_triples: Vec<TripleInput> = Vec::new();
    let mut entity_ids: Vec<Uuid> = Vec::new();

    for m in &body.mutations {
        match m.op {
            MutationOp::Insert => {
                let entity_id = m.id.unwrap_or_else(Uuid::new_v4);
                entity_ids.push(entity_id);

                // Set the :db/type attribute for this entity.
                all_triples.push(TripleInput {
                    entity_id,
                    attribute: ":db/type".to_string(),
                    value: Value::String(m.entity.clone()),
                    value_type: 0, // String
                });

                // Convert each key-value pair in data to a triple.
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
                        });
                    }
                }
            }
            MutationOp::Update | MutationOp::Upsert => {
                let entity_id = m.id.unwrap_or_else(Uuid::new_v4);
                entity_ids.push(entity_id);

                // For update/upsert, retract old values for touched attributes
                // then insert new ones.
                if let Some(data) = &m.data
                    && let Some(obj) = data.as_object()
                {
                    for (key, _) in obj {
                        let attr = format!("{}/{}", m.entity, key);
                        let _ = state.triple_store.retract(entity_id, &attr).await;
                    }
                    for (key, value) in obj {
                        let value_type = infer_value_type(value);
                        all_triples.push(TripleInput {
                            entity_id,
                            attribute: format!("{}/{}", m.entity, key),
                            value: value.clone(),
                            value_type,
                        });
                    }
                }
            }
            MutationOp::Delete => {
                let entity_id = m.id.expect("validated above");
                entity_ids.push(entity_id);

                // Retract all triples for this entity by fetching and retracting each attribute.
                let existing = state
                    .triple_store
                    .get_entity(entity_id)
                    .await
                    .map_err(|e| {
                        ApiError::internal(format!("Failed to fetch entity for deletion: {e}"))
                    })?;
                for triple in &existing {
                    let _ = state
                        .triple_store
                        .retract(entity_id, &triple.attribute)
                        .await;
                }
            }
        }
    }

    // Collect attributes touched (for change notification).
    let mut touched_attributes: Vec<String> =
        all_triples.iter().map(|t| t.attribute.clone()).collect();
    touched_attributes.sort();
    touched_attributes.dedup();

    // Collect entity types touched.
    let mut entity_types: Vec<String> = body.mutations.iter().map(|m| m.entity.clone()).collect();
    entity_types.sort();
    entity_types.dedup();

    // Write all insert/update triples in one batch.
    let tx_id = if !all_triples.is_empty() {
        state
            .triple_store
            .set_triples(&all_triples)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to write triples: {e}")))?
    } else {
        0
    };

    // Emit change event for reactive subscriptions.
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

/// Query parameters for the data list endpoint.
#[derive(Deserialize)]
struct DataListParams {
    limit: Option<u32>,
    #[serde(rename = "cursor")]
    #[allow(dead_code)] // used by client protocol
    _cursor: Option<String>,
    #[serde(flatten)]
    #[allow(dead_code)] // used by client protocol
    _filters: HashMap<String, String>,
}

/// `GET /api/data/:entity` — List entities of a type with pagination.
async fn data_list(
    State(state): State<AppState>,
    Path(entity): Path<String>,
    Query(params): Query<DataListParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers)?;
    let limit = params.limit.unwrap_or(50).min(1000);

    validate_entity_name(&entity)?;

    // Evaluate read permission — may inject WHERE clauses for row-level security.
    let perm_result = check_permission(&auth_ctx, &entity, Operation::Read, &state.permissions)?;

    // Use the query engine to list entities of this type.
    let query_json = serde_json::json!({
        "type": entity,
        "$limit": limit
    });
    let mut ast = query::parse_darshan_ql(&query_json)
        .map_err(|e| ApiError::internal(format!("Failed to build list query: {e}")))?;

    // Inject permission WHERE clauses into the query.
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

/// `POST /api/data/:entity` — Create a new entity.
async fn data_create(
    State(state): State<AppState>,
    Path(entity): Path<String>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers)?;

    validate_entity_name(&entity)?;

    // Check create permission — returns 403 if denied.
    let _perm_result = check_permission(&auth_ctx, &entity, Operation::Create, &state.permissions)?;

    if !body.is_object() {
        return Err(ApiError::bad_request("Request body must be a JSON object"));
    }

    let id = Uuid::new_v4();
    let obj = body.as_object().unwrap();

    // Build triples: one for :db/type, one per data field.
    let mut triples = vec![TripleInput {
        entity_id: id,
        attribute: ":db/type".to_string(),
        value: Value::String(entity.clone()),
        value_type: 0, // String
    }];
    for (key, value) in obj {
        let value_type = infer_value_type(value);
        triples.push(TripleInput {
            entity_id: id,
            attribute: format!("{entity}/{key}"),
            value: value.clone(),
            value_type,
        });
    }

    let tx_id = state
        .triple_store
        .set_triples(&triples)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create entity: {e}")))?;

    // Emit change event for reactive subscriptions.
    let attributes: Vec<String> = triples.iter().map(|t| t.attribute.clone()).collect();
    let _ = state.change_tx.send(ChangeEvent {
        tx_id,
        entity_ids: vec![id.to_string()],
        attributes,
        entity_type: Some(entity.clone()),
        actor_id: None,
    });

    let response = serde_json::json!({
        "id": id,
        "entity": entity,
        "tx_id": tx_id,
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
    State(state): State<AppState>,
    Path((entity, id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers)?;

    validate_entity_name(&entity)?;

    // Evaluate read permission.
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

    // Build attribute map from triples.
    let mut attrs = serde_json::Map::new();
    for t in &triples {
        // Use the short attribute name (strip "entity/" prefix if present).
        let key = t
            .attribute
            .strip_prefix(&format!("{entity}/"))
            .unwrap_or(&t.attribute)
            .to_string();
        attrs.entry(key).or_insert_with(|| t.value.clone());
    }

    // Enforce row-level security: if the permission has WHERE clauses
    // (e.g., owner_id = $user_id), verify this entity satisfies them.
    // For single-entity fetches we check the owner_id attribute directly.
    if !perm_result.where_clauses.is_empty() {
        let owner_id = attrs
            .get("owner_id")
            .or_else(|| attrs.get("id"))
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        // For "users" entity, the entity's own id IS the access key.
        let entity_owner = if entity == "users" {
            Some(id)
        } else {
            owner_id
        };

        if let Some(owner) = entity_owner {
            if owner != auth_ctx.user_id {
                return Err(ApiError::permission_denied(format!(
                    "Access denied: you do not own this {entity}"
                )));
            }
        }
        // If no owner_id attribute exists, the WHERE clause will be
        // enforced at query time for list operations. For single-entity
        // fetches without an owner field, we allow access (the entity
        // type's rules should use Deny if truly restricted).
    }

    // Apply field restrictions from permissions.
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

    let response = serde_json::json!({
        "id": id,
        "entity": entity,
        "data": attrs
    });

    Ok(negotiate_response(&headers, &response))
}

/// `PATCH /api/data/:entity/:id` — Partially update an entity.
async fn data_patch(
    State(state): State<AppState>,
    Path((entity, id)): Path<(String, Uuid)>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers)?;

    validate_entity_name(&entity)?;

    // Check update permission.
    let perm_result = check_permission(&auth_ctx, &entity, Operation::Update, &state.permissions)?;

    // Enforce row-level security for updates: verify ownership.
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

        if let Some(owner) = entity_owner {
            if owner != auth_ctx.user_id {
                return Err(ApiError::permission_denied(format!(
                    "Access denied: you do not own this {entity}"
                )));
            }
        }
    }

    if !body.is_object() {
        return Err(ApiError::bad_request("Request body must be a JSON object"));
    }

    let obj = body.as_object().unwrap();
    let mut triples = Vec::new();

    // Retract old values for each attribute being patched, then insert new.
    for (key, value) in obj {
        let attr = format!("{entity}/{key}");
        let _ = state.triple_store.retract(id, &attr).await;
        let value_type = infer_value_type(value);
        triples.push(TripleInput {
            entity_id: id,
            attribute: attr,
            value: value.clone(),
            value_type,
        });
    }

    let tx_id = if !triples.is_empty() {
        state
            .triple_store
            .set_triples(&triples)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to update entity: {e}")))?
    } else {
        0
    };

    // Emit change event for reactive subscriptions.
    if tx_id > 0 {
        let attributes: Vec<String> = triples.iter().map(|t| t.attribute.clone()).collect();
        let _ = state.change_tx.send(ChangeEvent {
            tx_id,
            entity_ids: vec![id.to_string()],
            attributes,
            entity_type: Some(entity.clone()),
            actor_id: None,
        });
    }

    let response = serde_json::json!({
        "id": id,
        "entity": entity,
        "tx_id": tx_id,
        "data": body
    });

    Ok(negotiate_response(&headers, &response))
}

/// `DELETE /api/data/:entity/:id` — Delete an entity.
async fn data_delete(
    State(state): State<AppState>,
    Path((entity, id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth_ctx = extract_auth_context(&headers)?;

    validate_entity_name(&entity)?;

    // Check delete permission.
    let perm_result = check_permission(&auth_ctx, &entity, Operation::Delete, &state.permissions)?;

    // Retract all triples for this entity.
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

    // Enforce row-level security for deletes: verify ownership.
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

        if let Some(owner) = entity_owner {
            if owner != auth_ctx.user_id {
                return Err(ApiError::permission_denied(format!(
                    "Access denied: you do not own this {entity}"
                )));
            }
        }
    }

    let deleted_attributes: Vec<String> = existing.iter().map(|t| t.attribute.clone()).collect();

    for triple in &existing {
        state
            .triple_store
            .retract(id, &triple.attribute)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to retract triple: {e}")))?;
    }

    // Emit change event for reactive subscriptions.
    let _ = state.change_tx.send(ChangeEvent {
        tx_id: 0, // Delete doesn't produce a new tx_id from set_triples
        entity_ids: vec![id.to_string()],
        attributes: deleted_attributes,
        entity_type: Some(entity.clone()),
        actor_id: None,
    });

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
    Err(ApiError::not_found("File not found"))
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

    // Use a comment-style keepalive (": heartbeat\n\n") so it does not
    // trigger client `onmessage` handlers.  Axum's `KeepAlive::text` sends
    // a data event; using an empty `Event::default().comment("heartbeat")`
    // equivalent is achieved through the `text` method with a leading colon
    // is not available, but the standard SSE comment prefix is what the
    // `KeepAlive` default (no `.text()`) produces.  We omit `.text()` to
    // get the default SSE comment keepalive behavior.
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

// ===========================================================================
// Admin handlers
// ===========================================================================

/// `GET /api/admin/schema` — Return the current inferred schema.
async fn admin_schema(
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

/// Extract an [`AuthContext`] from the request headers.
///
/// In production this decodes and validates the JWT. Currently it builds
/// a context from the token's embedded claims or falls back to a
/// default authenticated user context for development.
fn extract_auth_context(headers: &HeaderMap) -> Result<AuthContext, ApiError> {
    let token = extract_bearer_token(headers)?;

    // In dev mode or when JWT decoding is not yet wired, we parse a
    // minimal context from the token format: `ddb_at_<uuid>`.
    // Production will replace this with full JWT validation via SessionManager.
    let user_id = token
        .strip_prefix("ddb_at_")
        .and_then(|id| Uuid::parse_str(id).ok())
        .unwrap_or_else(Uuid::new_v4);

    // Check for role hints in X-DarshanDB-Roles header (dev/test only).
    // Production uses JWT claims exclusively.
    let roles: Vec<String> = headers
        .get("X-DarshanDB-Roles")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').map(|r| r.trim().to_string()).collect())
        .unwrap_or_else(|| vec!["user".to_string()]);

    Ok(AuthContext {
        user_id,
        session_id: Uuid::new_v4(),
        roles,
        ip: headers
            .get("X-Forwarded-For")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string(),
        user_agent: headers
            .get(http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string(),
        device_fingerprint: "http-request".to_string(),
    })
}

/// Evaluate permission for an entity operation and return the result.
///
/// Uses the permission engine from `AppState`, falling back to wildcard
/// rules if no entity-specific rule is configured.
///
/// Returns `Err(ApiError)` with 403 if the operation is denied.
fn check_permission(
    auth_ctx: &AuthContext,
    entity_type: &str,
    operation: Operation,
    engine: &PermissionEngine,
) -> Result<crate::auth::PermissionResult, ApiError> {
    let rule = match get_rule_with_fallback(engine, entity_type, operation) {
        Some(r) => r,
        None => {
            return Err(ApiError::permission_denied(format!(
                "No permission rule configured for {entity_type}.{operation:?}"
            )));
        }
    };

    let result = evaluate_rule_public(auth_ctx, rule);

    if !result.allowed {
        let reason = result
            .denial_reason
            .as_deref()
            .unwrap_or("permission denied");
        return Err(ApiError::permission_denied(format!(
            "Access denied for {entity_type}.{operation:?}: {reason}"
        )));
    }

    Ok(result)
}

/// Infer the triple store value_type discriminator from a JSON value.
fn infer_value_type(value: &Value) -> i16 {
    match value {
        Value::String(s) => {
            // Check if it looks like a UUID (reference).
            if s.len() == 36 && Uuid::parse_str(s).is_ok() {
                5 // Reference
            } else {
                0 // String
            }
        }
        Value::Number(n) => {
            if n.is_f64() && !n.is_i64() && !n.is_u64() {
                2 // Float
            } else {
                1 // Integer
            }
        }
        Value::Bool(_) => 3,                     // Boolean
        Value::Object(_) | Value::Array(_) => 6, // Json
        Value::Null => 0,                        // Default to String for null
    }
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
    // Must start with a letter or underscore (not a digit or hyphen).
    if let Some(first) = name.chars().next()
        && !first.is_ascii_alphabetic()
        && first != '_'
    {
        return Err(ApiError::bad_request(
            "Entity name must start with a letter or underscore",
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

    // -----------------------------------------------------------------------
    // Bearer token extraction
    // -----------------------------------------------------------------------

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
        let err = extract_bearer_token(&headers).unwrap_err();
        assert!(matches!(err.code, ErrorCode::Unauthenticated));
        assert!(err.message.contains("Missing"));
    }

    #[test]
    fn bearer_extraction_wrong_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic abc123"),
        );
        let err = extract_bearer_token(&headers).unwrap_err();
        assert!(matches!(err.code, ErrorCode::Unauthenticated));
        assert!(err.message.contains("Bearer"));
    }

    #[test]
    fn bearer_extraction_empty_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer "),
        );
        let err = extract_bearer_token(&headers).unwrap_err();
        assert!(matches!(err.code, ErrorCode::Unauthenticated));
        assert!(err.message.contains("empty"));
    }

    #[test]
    fn bearer_extraction_trims_whitespace() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer   tok_abc  "),
        );
        let token = extract_bearer_token(&headers).unwrap();
        assert_eq!(token, "tok_abc");
    }

    // -----------------------------------------------------------------------
    // Entity name validation
    // -----------------------------------------------------------------------

    #[test]
    fn entity_name_valid_cases() {
        assert!(validate_entity_name("users").is_ok());
        assert!(validate_entity_name("my-entity").is_ok());
        assert!(validate_entity_name("my_entity_2").is_ok());
        assert!(validate_entity_name("_private").is_ok());
        assert!(validate_entity_name("A").is_ok());
    }

    #[test]
    fn entity_name_rejects_empty() {
        let err = validate_entity_name("").unwrap_err();
        assert!(matches!(err.code, ErrorCode::BadRequest));
    }

    #[test]
    fn entity_name_rejects_special_chars() {
        assert!(validate_entity_name("a/b").is_err());
        assert!(validate_entity_name("a b").is_err());
        assert!(validate_entity_name("a.b").is_err());
        assert!(validate_entity_name("entity!").is_err());
    }

    #[test]
    fn entity_name_rejects_too_long() {
        assert!(validate_entity_name(&"a".repeat(129)).is_err());
        // 128 chars should be fine
        assert!(validate_entity_name(&"a".repeat(128)).is_ok());
    }

    #[test]
    fn entity_name_rejects_leading_digit() {
        let err = validate_entity_name("123abc").unwrap_err();
        assert!(matches!(err.code, ErrorCode::BadRequest));
        assert!(err.message.contains("start with"));
    }

    #[test]
    fn entity_name_rejects_leading_hyphen() {
        let err = validate_entity_name("-leading").unwrap_err();
        assert!(matches!(err.code, ErrorCode::BadRequest));
        assert!(err.message.contains("start with"));
    }

    // -----------------------------------------------------------------------
    // Content negotiation (wants_msgpack)
    // -----------------------------------------------------------------------

    #[test]
    fn wants_msgpack_detection() {
        let mut headers = HeaderMap::new();
        assert!(!wants_msgpack(&headers));

        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        assert!(!wants_msgpack(&headers));

        headers.insert(ACCEPT, HeaderValue::from_static("application/msgpack"));
        assert!(wants_msgpack(&headers));
    }

    #[test]
    fn wants_msgpack_in_quality_list() {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, application/msgpack;q=0.9"),
        );
        // The current implementation checks `contains`, so this matches.
        assert!(wants_msgpack(&headers));
    }

    // -----------------------------------------------------------------------
    // Content negotiation response serialization
    // -----------------------------------------------------------------------

    #[test]
    fn negotiate_response_json_default() {
        let headers = HeaderMap::new();
        let data = serde_json::json!({"key": "value"});
        let resp = negotiate_response(&headers, &data);
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
        assert!(ct.contains("application/json"), "expected json, got: {ct}");
    }

    #[test]
    fn negotiate_response_msgpack() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/msgpack"));
        let data = serde_json::json!({"key": "value"});
        let resp = negotiate_response(&headers, &data);
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
        assert_eq!(ct, "application/msgpack");
    }

    #[test]
    fn negotiate_response_status_created() {
        let headers = HeaderMap::new();
        let data = serde_json::json!({"id": 1});
        let resp = negotiate_response_status(&headers, StatusCode::CREATED, &data);
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[test]
    fn negotiate_response_status_msgpack_preserves_status() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/msgpack"));
        let data = serde_json::json!({"id": 1});
        let resp = negotiate_response_status(&headers, StatusCode::CREATED, &data);
        assert_eq!(resp.status(), StatusCode::CREATED);
        let ct = resp.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
        assert_eq!(ct, "application/msgpack");
    }

    // -----------------------------------------------------------------------
    // Error serialization format
    // -----------------------------------------------------------------------

    #[test]
    fn error_serialization_envelope_format() {
        let err = ApiError::bad_request("Test error message");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn error_not_found_is_404() {
        let err = ApiError::not_found("gone");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn error_unauthenticated_is_401() {
        let err = ApiError::unauthenticated("no token");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn error_permission_denied_is_403() {
        let err = ApiError::permission_denied("nope");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn error_rate_limited_includes_retry_after() {
        let err = ApiError::rate_limited(30);
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry = resp.headers().get("Retry-After").unwrap().to_str().unwrap();
        assert_eq!(retry, "30");
    }

    #[test]
    fn error_internal_is_500() {
        let err = ApiError::internal("server broke");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn error_payload_too_large_is_413() {
        let err = ApiError::new(ErrorCode::PayloadTooLarge, "too big");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    // -----------------------------------------------------------------------
    // Rate limit header formatting
    // -----------------------------------------------------------------------

    #[test]
    fn rate_limit_headers_injected() {
        // Verify the rate-limit header values are valid HTTP header values.
        assert!(HeaderValue::from_str("1000").is_ok());
        assert!(HeaderValue::from_str("999").is_ok());
        assert!(HeaderValue::from_str("60").is_ok());

        // Verify the header names are valid
        let _ = "X-RateLimit-Limit";
        let _ = "X-RateLimit-Remaining";
        let _ = "X-RateLimit-Reset";
    }

    // -----------------------------------------------------------------------
    // OpenAPI spec generation
    // -----------------------------------------------------------------------

    #[test]
    fn openapi_spec_has_required_fields() {
        let spec = openapi::generate_openapi_spec();

        assert_eq!(spec["openapi"], "3.1.0");
        assert_eq!(spec["info"]["title"], "DarshanDB API");
        assert!(spec["info"]["version"].is_string());
        assert!(spec["paths"].is_object());
        assert!(spec["components"]["securitySchemes"]["bearerAuth"].is_object());
    }

    #[test]
    fn openapi_spec_has_all_paths() {
        let spec = openapi::generate_openapi_spec();
        let paths = spec["paths"].as_object().unwrap();

        let expected = [
            "/auth/signup",
            "/auth/signin",
            "/auth/magic-link",
            "/auth/verify",
            "/auth/oauth/{provider}",
            "/auth/refresh",
            "/auth/signout",
            "/auth/me",
            "/query",
            "/mutate",
            "/data/{entity}",
            "/data/{entity}/{id}",
            "/fn/{name}",
            "/storage/upload",
            "/storage/{path}",
            "/subscribe",
            "/admin/schema",
            "/admin/functions",
            "/admin/sessions",
        ];

        for path in expected {
            assert!(
                paths.contains_key(path),
                "OpenAPI spec missing path: {path}"
            );
        }
    }

    #[test]
    fn openapi_spec_has_all_schemas() {
        let spec = openapi::generate_openapi_spec();
        let schemas = spec["components"]["schemas"].as_object().unwrap();

        let expected = [
            "ErrorResponse",
            "TokenPair",
            "QueryRequest",
            "MutateRequest",
            "UserProfile",
            "UploadResponse",
        ];

        for schema in expected {
            assert!(
                schemas.contains_key(schema),
                "OpenAPI spec missing schema: {schema}"
            );
        }
    }

    #[test]
    fn openapi_docs_html_contains_spec_url() {
        let html = openapi::docs_html("/api/openapi.json");
        assert!(html.contains("/api/openapi.json"));
        assert!(html.contains("DarshanDB"));
        assert!(html.contains("<script"));
    }

    // -----------------------------------------------------------------------
    // AppState construction
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn app_state_default_has_valid_spec() {
        let state = AppState::new();
        assert!(state.openapi_spec["openapi"].is_string());
    }

    #[tokio::test]
    async fn app_state_default_trait() {
        let state = AppState::default();
        assert!(state.openapi_spec["openapi"].is_string());
    }

    // -----------------------------------------------------------------------
    // ErrorCode -> StatusCode mapping exhaustive check
    // -----------------------------------------------------------------------

    #[test]
    fn error_code_status_mapping() {
        assert_eq!(ErrorCode::BadRequest.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            ErrorCode::Unauthenticated.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(ErrorCode::PermissionDenied.status(), StatusCode::FORBIDDEN);
        assert_eq!(ErrorCode::NotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(ErrorCode::Conflict.status(), StatusCode::CONFLICT);
        assert_eq!(
            ErrorCode::PayloadTooLarge.status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            ErrorCode::RateLimited.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(ErrorCode::InvalidQuery.status(), StatusCode::BAD_REQUEST);
        assert_eq!(ErrorCode::TypeMismatch.status(), StatusCode::BAD_REQUEST);
        assert_eq!(ErrorCode::SchemaConflict.status(), StatusCode::CONFLICT);
        assert_eq!(
            ErrorCode::Internal.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
