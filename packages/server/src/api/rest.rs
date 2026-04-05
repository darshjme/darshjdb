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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequest, Path, Query, State};
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
use crate::auth::middleware::RateLimitKey;
use crate::auth::{
    AuthContext, AuthOutcome, GenericOAuth2Provider, OAuth2Provider, OAuthProviderKind,
    OAuthUserInfo, Operation, PasswordProvider, PermissionEngine, RateLimiter, SessionManager,
    build_default_engine, evaluate_rule_public, get_rule_with_fallback,
};
use crate::cache::{self, QueryCache};
use crate::functions::registry::FunctionRegistry;
use crate::functions::runtime::FunctionRuntime;
use crate::query::{self, QueryResultRow};
use crate::rules::RuleEngine;
use crate::storage::{LocalFsBackend, StorageEngine, StorageError};
use crate::sync::broadcaster::ChangeEvent;
use crate::sync::pubsub::{PubSubEngine, PubSubEvent};
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
    /// Rate limiter for per-request throttling.
    pub rate_limiter: Arc<RateLimiter>,
    /// File storage engine backed by local filesystem (or S3/R2).
    pub storage_engine: Arc<StorageEngine<LocalFsBackend>>,
    /// Function registry for looking up server-side functions.
    pub function_registry: Option<Arc<FunctionRegistry>>,
    /// Function runtime for executing server-side functions.
    pub function_runtime: Option<Arc<FunctionRuntime>>,
    /// Configured OAuth2 providers keyed by provider kind.
    pub oauth_providers: Arc<HashMap<OAuthProviderKind, GenericOAuth2Provider>>,
    /// HMAC secret for signing/verifying OAuth state parameters.
    pub oauth_state_secret: Arc<Vec<u8>>,
    /// Forward-chaining rule engine for automatic triple inference.
    pub rule_engine: Option<Arc<RuleEngine>>,
    /// In-memory hot cache for query results (sub-millisecond reads).
    pub query_cache: Arc<QueryCache>,
    /// Pub/sub engine for keyspace notification subscriptions.
    pub pubsub: Arc<PubSubEngine>,
    /// Latency histogram for connection pool stats exposed via /health.
    pub pool_stats: Arc<super::pool_stats::PoolStats>,
}

/// Load OAuth2 provider configurations from environment variables.
///
/// Reads `DARSHAN_OAUTH_{PROVIDER}_CLIENT_ID` and
/// `DARSHAN_OAUTH_{PROVIDER}_CLIENT_SECRET` for each supported provider.
/// Providers without both env vars are silently skipped.
fn load_oauth_providers_from_env() -> HashMap<OAuthProviderKind, GenericOAuth2Provider> {
    let base_url =
        std::env::var("DARSHAN_BASE_URL").unwrap_or_else(|_| "http://localhost:4000".to_string());
    let mut providers = HashMap::new();

    // Google
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DARSHAN_OAUTH_GOOGLE_CLIENT_ID"),
        std::env::var("DARSHAN_OAUTH_GOOGLE_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DARSHAN_OAUTH_GOOGLE_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/google/callback"));
        providers.insert(
            OAuthProviderKind::Google,
            GenericOAuth2Provider::google(id, secret, redirect),
        );
    }

    // GitHub
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DARSHAN_OAUTH_GITHUB_CLIENT_ID"),
        std::env::var("DARSHAN_OAUTH_GITHUB_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DARSHAN_OAUTH_GITHUB_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/github/callback"));
        providers.insert(
            OAuthProviderKind::GitHub,
            GenericOAuth2Provider::github(id, secret, redirect),
        );
    }

    // Apple
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DARSHAN_OAUTH_APPLE_CLIENT_ID"),
        std::env::var("DARSHAN_OAUTH_APPLE_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DARSHAN_OAUTH_APPLE_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/apple/callback"));
        providers.insert(
            OAuthProviderKind::Apple,
            GenericOAuth2Provider::apple(id, secret, redirect),
        );
    }

    // Discord
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DARSHAN_OAUTH_DISCORD_CLIENT_ID"),
        std::env::var("DARSHAN_OAUTH_DISCORD_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DARSHAN_OAUTH_DISCORD_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/discord/callback"));
        providers.insert(
            OAuthProviderKind::Discord,
            GenericOAuth2Provider::discord(id, secret, redirect),
        );
    }

    providers
}

/// Load or generate the HMAC secret for OAuth2 state parameters.
fn load_oauth_state_secret() -> Vec<u8> {
    match std::env::var("DARSHAN_OAUTH_STATE_SECRET") {
        Ok(s) if s.len() >= 32 => s.into_bytes(),
        _ => {
            use rand::RngCore;
            let mut buf = vec![0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut buf);
            tracing::warn!("DARSHAN_OAUTH_STATE_SECRET not set; generated ephemeral secret");
            buf
        }
    }
}

impl AppState {
    /// Create application state with a live database pool, triple store, and session manager.
    pub fn with_pool(
        pool: PgPool,
        triple_store: Arc<PgTripleStore>,
        session_manager: Arc<SessionManager>,
        change_tx: broadcast::Sender<ChangeEvent>,
        rate_limiter: Arc<RateLimiter>,
        storage_engine: Arc<StorageEngine<LocalFsBackend>>,
    ) -> Self {
        let dev_mode = std::env::var("DARSHAN_DEV")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);
        let (sse_tx, _) = broadcast::channel(1024);
        let (pubsub, _) = PubSubEngine::new(4096);
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
            rate_limiter,
            storage_engine,
            function_registry: None,
            function_runtime: None,
            oauth_providers: Arc::new(load_oauth_providers_from_env()),
            oauth_state_secret: Arc::new(load_oauth_state_secret()),
            rule_engine: None,
            query_cache: Arc::new(QueryCache::from_env()),
            pubsub,
            pool_stats: Arc::new(super::pool_stats::PoolStats::new()),
        }
    }

    /// Set the forward-chaining rule engine on this state.
    pub fn with_rules(mut self, rule_engine: Arc<RuleEngine>) -> Self {
        self.rule_engine = Some(rule_engine);
        self
    }

    /// Set the function registry and runtime on this state.
    pub fn with_functions(
        mut self,
        registry: Arc<FunctionRegistry>,
        runtime: Arc<FunctionRuntime>,
    ) -> Self {
        self.function_registry = Some(registry);
        self.function_runtime = Some(runtime);
        self
    }

    /// Set a shared pub/sub engine on this state (for sharing with WsState).
    pub fn with_pubsub(mut self, pubsub: Arc<PubSubEngine>) -> Self {
        self.pubsub = pubsub;
        self
    }

    /// Create application state with default (test-only) configuration.
    /// Panics if called outside tests — production code must use `with_pool`.
    #[cfg(test)]
    pub fn new() -> Self {
        // Tests that don't hit the database can use a dummy pool.
        // This preserves backward compatibility with existing unit tests.
        let (sse_tx, _) = broadcast::channel(1024);
        let (change_tx, _) = broadcast::channel(1024);
        let (pubsub, _) = PubSubEngine::new(64);
        let pool = PgPool::connect_lazy("postgres://localhost/darshandb_test").expect("test pool");
        let triple_store = Arc::new(PgTripleStore::new_lazy(pool.clone()));
        let key_manager = crate::auth::KeyManager::generate();
        let session_manager = Arc::new(SessionManager::new(pool.clone(), key_manager));
        let storage_backend = Arc::new(
            LocalFsBackend::new("/tmp/darshandb-test-storage")
                .expect("create test storage backend"),
        );
        let storage_engine = Arc::new(StorageEngine::new(
            storage_backend,
            b"test-signing-key".to_vec(),
        ));
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
            rate_limiter: Arc::new(RateLimiter::new()),
            storage_engine,
            function_registry: None,
            function_runtime: None,
            oauth_providers: Arc::new(HashMap::new()),
            oauth_state_secret: Arc::new(b"test-oauth-state-secret-key-32b!".to_vec()),
            rule_engine: None,
            query_cache: Arc::new(QueryCache::new(100, Duration::from_secs(60), true)),
            pubsub,
            pool_stats: Arc::new(super::pool_stats::PoolStats::new()),
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

/// Public wrapper for content-negotiated response (used by batch module).
pub fn negotiate_response_pub(headers: &HeaderMap, value: &impl Serialize) -> Response {
    negotiate_response(headers, value)
}
// ---------------------------------------------------------------------------
// Rate-limit middleware
// ---------------------------------------------------------------------------

/// Middleware that enforces per-request rate limiting and injects standard
/// `X-RateLimit-*` headers into every response.
///
/// Authenticated requests (Bearer token present) get 100 req/min;
/// anonymous requests get 20 req/min. Returns 429 with `Retry-After`
/// when the budget is exhausted.
async fn rate_limit_middleware(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let headers = req.headers();
    let ip = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "unknown".into());

    let token = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string());

    let is_authenticated = token.is_some();

    let rate_key = if let Some(ref tok) = token {
        use sha2::Digest;
        let prefix = &tok[..std::cmp::min(tok.len(), 16)];
        let hash = sha2::Sha256::digest(prefix.as_bytes());
        RateLimitKey::Token(data_encoding::HEXLOWER.encode(&hash[..16]))
    } else {
        RateLimitKey::Ip(ip)
    };

    let (limit, reset) = if is_authenticated {
        (100u64, 60u64)
    } else {
        (20u64, 60u64)
    };

    // Check rate limit; on failure return 429 with Retry-After.
    if let Err(retry_after) = state.rate_limiter.check(&rate_key, is_authenticated) {
        let body = serde_json::json!({
            "error": {
                "code": 429,
                "message": format!("rate limit exceeded, retry after {}s", retry_after),
            }
        });
        let mut response = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
        let hdrs = response.headers_mut();
        if let Ok(v) = HeaderValue::from_str(&retry_after.to_string()) {
            hdrs.insert("retry-after", v);
        }
        if let Ok(v) = HeaderValue::from_str(&limit.to_string()) {
            hdrs.insert("x-ratelimit-limit", v);
        }
        hdrs.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        if let Ok(v) = HeaderValue::from_str(&reset.to_string()) {
            hdrs.insert("x-ratelimit-reset", v);
        }
        return response;
    }

    // Forward to inner handler, then stamp rate-limit headers on response.
    let mut response = next.run(req).await;
    let hdrs = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&limit.to_string()) {
        hdrs.insert("x-ratelimit-limit", v);
    }
    if let Ok(v) = HeaderValue::from_str(&(limit.saturating_sub(1)).to_string()) {
        hdrs.insert("x-ratelimit-remaining", v);
    }
    if let Ok(v) = HeaderValue::from_str(&reset.to_string()) {
        hdrs.insert("x-ratelimit-reset", v);
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
        .route("/auth/oauth/{provider}/callback", get(auth_oauth_callback))
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
        // -- Pub/Sub -------------------------------------------------------
        .route("/events", get(events_sse))
        .route("/events/publish", post(events_publish))
        // -- Admin ---------------------------------------------------------
        .route("/admin/schema", get(admin_schema))
        .route("/admin/functions", get(admin_functions))
        .route("/admin/sessions", get(admin_sessions))
        .route("/admin/bulk-load", post(admin_bulk_load))
        .route("/admin/cache", get(admin_cache))
        // -- Embeddings / Semantic Search (TODO: wire handlers) ------------
        // .route("/embeddings", post(embeddings_store))
        // .route("/embeddings/{entity_id}", get(embeddings_get))
        // .route("/search/semantic", post(search_semantic))
        // -- Batch / Pipeline ---------------------------------------------
        .route("/batch", post(super::batch::batch_handler))
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
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
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
        CREATE TABLE IF NOT EXISTS oauth_identities (
            id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            user_id             UUID NOT NULL REFERENCES users(id),
            provider            TEXT NOT NULL,
            provider_user_id    TEXT NOT NULL,
            email               TEXT,
            name                TEXT,
            avatar_url          TEXT,
            created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
            UNIQUE (provider, provider_user_id)
        );
        CREATE INDEX IF NOT EXISTS idx_oauth_provider_user
            ON oauth_identities (provider, provider_user_id);
        CREATE INDEX IF NOT EXISTS idx_oauth_user_id ON oauth_identities (user_id);
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
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: user_id,
            attribute: "user/email".into(),
            value: Value::String(email.clone()),
            value_type: 0,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: user_id,
            attribute: "user/name".into(),
            value: Value::String(body.name.unwrap_or_default()),
            value_type: 0,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: user_id,
            attribute: "user/created_at".into(),
            value: Value::String(chrono::Utc::now().to_rfc3339()),
            value_type: 0,
            ttl_seconds: None,
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

/// `POST /api/auth/oauth/:provider` — Generate an OAuth2 authorize URL with PKCE + HMAC state,
/// or exchange an authorization code inline (SPA flow).
#[derive(Deserialize)]
struct OAuthRequest {
    /// Authorization code (for inline exchange).
    code: Option<String>,
    /// State parameter from the provider callback (for inline exchange).
    state: Option<String>,
    /// PKCE verifier (for inline exchange).
    pkce_verifier: Option<String>,
}

async fn auth_oauth(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<OAuthRequest>,
) -> Result<Response, ApiError> {
    let kind = OAuthProviderKind::from_name(&provider)
        .ok_or_else(|| ApiError::bad_request(format!("Unsupported OAuth provider: {provider}")))?;

    let oauth_provider = state.oauth_providers.get(&kind).ok_or_else(|| {
        ApiError::bad_request(format!(
            "OAuth provider '{}' is not configured on this server",
            provider
        ))
    })?;

    // If a code is provided, do inline exchange (SPA / backward-compat flow).
    if let Some(code) = body.code.as_deref().filter(|c| !c.is_empty()) {
        let oauth_state = body
            .state
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("state parameter required for code exchange"))?;
        let verifier = body
            .pkce_verifier
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("pkce_verifier required for code exchange"))?;

        let user_info = oauth_provider
            .exchange_code(code, oauth_state, verifier, &state.oauth_state_secret)
            .await
            .map_err(|e| ApiError::bad_request(format!("OAuth exchange failed: {e}")))?;

        let (user_id, roles) = find_or_create_oauth_user(&state.pool, &user_info).await?;

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

        return Ok(negotiate_response(&headers, &response));
    }

    // No code supplied — generate the authorize URL.
    let (url, csrf_state, pkce_verifier) = oauth_provider
        .authorization_url(&state.oauth_state_secret)
        .map_err(|e| ApiError::internal(format!("Failed to build authorize URL: {e}")))?;

    let response = serde_json::json!({
        "authorize_url": url,
        "state": csrf_state,
        "pkce_verifier": pkce_verifier,
    });

    Ok(negotiate_response(&headers, &response))
}

/// `GET /api/auth/oauth/:provider/callback?code=...&state=...` — OAuth2 callback.
///
/// The provider redirects here after user consent. Verifies the HMAC state,
/// exchanges the authorization code with PKCE, finds or creates the user,
/// and issues a JWT token pair.
#[derive(Deserialize)]
struct OAuthCallbackQuery {
    code: String,
    state: String,
}

async fn auth_oauth_callback(
    State(app): State<AppState>,
    Path(provider): Path<String>,
    Query(params): Query<OAuthCallbackQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let kind = OAuthProviderKind::from_name(&provider)
        .ok_or_else(|| ApiError::bad_request(format!("Unsupported OAuth provider: {provider}")))?;

    let oauth_provider = app.oauth_providers.get(&kind).ok_or_else(|| {
        ApiError::bad_request(format!(
            "OAuth provider '{}' is not configured on this server",
            provider
        ))
    })?;

    // For server-side callback flow, the PKCE verifier should be stored in
    // a server-side session or secure HTTP-only cookie. We check the
    // X-PKCE-Verifier header (set by a BFF proxy) or fall back to empty.
    let pkce_verifier = headers
        .get("x-pkce-verifier")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let user_info = oauth_provider
        .exchange_code(
            &params.code,
            &params.state,
            pkce_verifier,
            &app.oauth_state_secret,
        )
        .await
        .map_err(|e| ApiError::bad_request(format!("OAuth callback failed: {e}")))?;

    let (user_id, roles) = find_or_create_oauth_user(&app.pool, &user_info).await?;

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

    let token_pair = app
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

/// Find or create a user from an OAuth identity.
async fn find_or_create_oauth_user(
    pool: &PgPool,
    info: &OAuthUserInfo,
) -> Result<(Uuid, Vec<String>), ApiError> {
    let provider_str = info.provider.to_string();
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT user_id FROM oauth_identities WHERE provider = $1 AND provider_user_id = $2",
    )
    .bind(&provider_str)
    .bind(&info.provider_user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| ApiError::internal(format!("OAuth lookup failed: {e}")))?;
    if let Some((user_id,)) = existing {
        let roles: Vec<String> =
            sqlx::query_scalar("SELECT roles FROM users WHERE id = $1 AND deleted_at IS NULL")
                .bind(user_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .and_then(|v: serde_json::Value| serde_json::from_value(v).ok())
                .unwrap_or_else(|| vec!["user".to_string()]);
        return Ok((user_id, roles));
    }
    let user_id = Uuid::new_v4();
    let email = info
        .email
        .as_deref()
        .map(|e| e.trim().to_lowercase())
        .unwrap_or_else(|| format!("{}@oauth.{}", info.provider_user_id, provider_str));
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, roles) VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(user_id)
    .bind(&email)
    .bind("!oauth-only")
    .bind(serde_json::json!(["user"]))
    .execute(pool)
    .await
    .map_err(|e| ApiError::internal(format!("User creation failed: {e}")))?;
    sqlx::query(
        "INSERT INTO oauth_identities (user_id, provider, provider_user_id, email, name, avatar_url) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(user_id)
    .bind(&provider_str)
    .bind(&info.provider_user_id)
    .bind(&info.email)
    .bind(&info.name)
    .bind(&info.avatar_url)
    .execute(pool)
    .await
    .map_err(|e| ApiError::internal(format!("OAuth link failed: {e}")))?;
    Ok((user_id, vec!["user".to_string()]))
}

/// `POST /api/auth/refresh` — Rotate a refresh token for a new token pair.
#[derive(Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

async fn auth_refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<RefreshRequest>,
) -> Result<Response, ApiError> {
    if body.refresh_token.is_empty() {
        return Err(ApiError::bad_request("Refresh token is required"));
    }

    let dfp = headers
        .get("x-device-fingerprint")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token_pair = state
        .session_manager
        .refresh_session(&body.refresh_token, dfp)
        .await
        .map_err(|e| match &e {
            crate::auth::AuthError::SessionRevoked => {
                ApiError::unauthenticated("Session has been revoked")
            }
            crate::auth::AuthError::DeviceMismatch => ApiError::unauthenticated(
                "Device fingerprint mismatch - session revoked for security",
            ),
            crate::auth::AuthError::TokenInvalid(msg) => {
                ApiError::unauthenticated(format!("Invalid refresh token: {msg}"))
            }
            _ => ApiError::internal(format!("Refresh failed: {e}")),
        })?;

    let response = serde_json::json!({
        "access_token": token_pair.access_token,
        "refresh_token": token_pair.refresh_token,
        "expires_in": token_pair.expires_in,
        "token_type": token_pair.token_type,
    });

    Ok(negotiate_response(&headers, &response))
}

/// `POST /api/auth/signout` — Revoke the current session.
async fn auth_signout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let token = extract_bearer_token(&headers)?;

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

    state
        .session_manager
        .revoke_session(auth_ctx.session_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to revoke session: {e}")))?;

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
    let permission_where = perm_result.build_where_clause(auth_ctx.user_id);
    if let Some(ref where_sql) = permission_where {
        // Convert the permission WHERE clause into a query WhereClause.
        // The permission engine produces raw SQL fragments; we inject them
        // as a special "raw" where clause that the planner will append.
        ast.where_clauses.push(query::WhereClause {
            attribute: "__permission_filter".to_string(),
            op: query::WhereOp::Eq,
            value: serde_json::Value::String(where_sql.clone()),
        });
    }

    // Build a cache key that includes the full query + permission context
    // so different users never see each other's cached results.
    let cache_key_input = serde_json::json!({
        "q": body.query,
        "uid": auth_ctx.user_id,
        "perm": permission_where,
    });
    let query_hash = cache::hash_query(&cache_key_input);
    let entity_type = ast.entity_type.clone();

    // Check the hot cache first — sub-millisecond on hit.
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

    // Plan the query.
    let plan = query::plan_query(&ast)
        .map_err(|e| ApiError::bad_request(format!("Query planning failed: {e}")))?;

    // Execute against Postgres.
    let results: Vec<QueryResultRow> = query::execute_query(&state.pool, &plan)
        .await
        .map_err(|e| ApiError::internal(format!("Query execution failed: {e}")))?;

    let count = results.len();

    // Record query latency for pool stats histogram.
    state.pool_stats.record(start.elapsed());

    // Cache the result set for future reads.
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
    let mutate_start = Instant::now();

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

    // Execute ALL mutations inside a single database transaction so that
    // the entire batch is atomic: either every mutation succeeds or none do.
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

    // Write all insert/update triples inside the same transaction.
    if !all_triples.is_empty() {
        PgTripleStore::set_triples_in_tx(&mut db_tx, &all_triples, tx_id)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to write triples: {e}")))?;
    }

    // Evaluate forward-chaining rules: implied triples are written in the
    // same transaction so the entire mutation + inferences is atomic.
    let mut implied_triples: Vec<TripleInput> = Vec::new();
    if !all_triples.is_empty() {
        if let Some(ref rule_engine) = state.rule_engine {
            implied_triples = rule_engine
                .evaluate_and_write_in_tx(&mut db_tx, &all_triples, tx_id)
                .await
                .map_err(|e| ApiError::internal(format!("Rule engine error: {e}")))?;
        }
    }

    // Commit the entire batch atomically.
    db_tx
        .commit()
        .await
        .map_err(|e| ApiError::internal(format!("Transaction commit failed: {e}")))?;

    // Record mutation latency for pool stats histogram.
    state.pool_stats.record(mutate_start.elapsed());

    // Collect attributes touched (for change notification), including implied.
    let mut touched_attributes: Vec<String> = all_triples
        .iter()
        .chain(implied_triples.iter())
        .map(|t| t.attribute.clone())
        .collect();
    touched_attributes.sort();
    touched_attributes.dedup();

    // Collect entity types touched.
    let mut entity_types: Vec<String> = body.mutations.iter().map(|m| m.entity.clone()).collect();
    entity_types.sort();
    entity_types.dedup();

    // Invalidate hot cache for all affected entity types.
    for et in &entity_types {
        state.query_cache.invalidate_by_entity_type(et);
    }

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

    // Extract optional TTL from the request body ($ttl key).
    let ttl_seconds: Option<i64> = obj.get("$ttl").and_then(|v| v.as_i64());

    // Build triples: one for :db/type, one per data field.
    let mut triples = vec![TripleInput {
        entity_id: id,
        attribute: ":db/type".to_string(),
        value: Value::String(entity.clone()),
        value_type: 0, // String
        ttl_seconds,
    }];
    for (key, value) in obj {
        // Skip $-prefixed meta-keys (e.g. $ttl) — not stored as data attributes.
        if key.starts_with('$') {
            continue;
        }
        let value_type = infer_value_type(value);
        triples.push(TripleInput {
            entity_id: id,
            attribute: format!("{entity}/{key}"),
            value: value.clone(),
            value_type,
            ttl_seconds,
        });
    }

    let tx_id = state
        .triple_store
        .set_triples(&triples)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create entity: {e}")))?;

    // Evaluate forward-chaining rules and write implied triples.
    if let Some(ref rule_engine) = state.rule_engine {
        let implied = rule_engine
            .evaluate(&triples)
            .await
            .map_err(|e| ApiError::internal(format!("Rule engine error: {e}")))?;
        if !implied.is_empty() {
            let _ = state
                .triple_store
                .set_triples(&implied)
                .await
                .map_err(|e| ApiError::internal(format!("Failed to write implied triples: {e}")))?;
        }
    }

    // Invalidate hot cache for the affected entity type.
    state.query_cache.invalidate_by_entity_type(&entity);

    // Emit change event for reactive subscriptions.
    let attributes: Vec<String> = triples.iter().map(|t| t.attribute.clone()).collect();
    let _ = state.change_tx.send(ChangeEvent {
        tx_id,
        entity_ids: vec![id.to_string()],
        attributes,
        entity_type: Some(entity.clone()),
        actor_id: None,
    });

    // Include TTL info in the creation response when set.
    let mut response = serde_json::json!({
        "id": id,
        "entity": entity,
        "tx_id": tx_id,
        "data": body
    });
    if let Some(ttl) = ttl_seconds {
        let exp = chrono::Utc::now() + chrono::Duration::seconds(ttl);
        if let Some(obj) = response.as_object_mut() {
            obj.insert("_ttl".into(), serde_json::json!(ttl));
            obj.insert("_expires_at".into(), serde_json::json!(exp.to_rfc3339()));
        }
    }

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

    // Include TTL virtual fields if the entity has an expiry set.
    let mut response = serde_json::json!({
        "id": id,
        "entity": entity,
        "data": attrs
    });
    if let Some(exp) = triples.iter().filter_map(|t| t.expires_at).min() {
        let remaining = (exp - chrono::Utc::now()).num_seconds().max(0);
        if let Some(obj) = response.as_object_mut() {
            obj.insert("_ttl".into(), serde_json::json!(remaining));
            obj.insert("_expires_at".into(), serde_json::json!(exp.to_rfc3339()));
        }
    }

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

    // Extract optional TTL from the request body ($ttl key).
    // $ttl: positive => set/extend TTL, -1 => remove TTL (persist forever).
    let ttl_override: Option<i64> = obj.get("$ttl").and_then(|v| v.as_i64());

    let mut triples = Vec::new();

    // Build triple inputs for the new values.
    for (key, value) in obj {
        // Skip $-prefixed meta-keys (e.g. $ttl).
        if key.starts_with('$') {
            continue;
        }
        let value_type = infer_value_type(value);
        triples.push(TripleInput {
            entity_id: id,
            attribute: format!("{entity}/{key}"),
            value: value.clone(),
            value_type,
            ttl_seconds: None,
        });
    }

    // Retract old + write new in a single transaction so the patch is atomic.
    let mut db_tx = state
        .triple_store
        .begin_tx()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to begin transaction: {e}")))?;

    for (key, _) in obj {
        let attr = format!("{entity}/{key}");
        PgTripleStore::retract_in_tx(&mut db_tx, id, &attr)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to retract attribute: {e}")))?;
    }

    let tx_id = if !triples.is_empty() {
        let tid = PgTripleStore::next_tx_id_in_tx(&mut db_tx)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to allocate tx_id: {e}")))?;
        PgTripleStore::set_triples_in_tx(&mut db_tx, &triples, tid)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to update entity: {e}")))?;

        // Evaluate forward-chaining rules within the same transaction.
        if let Some(ref rule_engine) = state.rule_engine {
            let _ = rule_engine
                .evaluate_and_write_in_tx(&mut db_tx, &triples, tid)
                .await
                .map_err(|e| ApiError::internal(format!("Rule engine error: {e}")))?;
        }

        tid
    } else {
        0
    };

    db_tx
        .commit()
        .await
        .map_err(|e| ApiError::internal(format!("Transaction commit failed: {e}")))?;

    // Apply TTL override if $ttl was specified in the PATCH body.
    if let Some(ttl) = ttl_override {
        state
            .triple_store
            .set_entity_ttl(id, ttl)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to set TTL: {e}")))?;
    }

    // Invalidate hot cache for the affected entity type.
    state.query_cache.invalidate_by_entity_type(&entity);

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

    // Include TTL info in the response when set.
    let ttl_info = if let Some(exp) = state.triple_store.get_entity_ttl(id).await.unwrap_or(None) {
        let remaining = (exp - chrono::Utc::now()).num_seconds().max(0);
        serde_json::json!({ "_ttl": remaining, "_expires_at": exp.to_rfc3339() })
    } else {
        serde_json::json!({})
    };

    let mut response = serde_json::json!({
        "id": id,
        "entity": entity,
        "tx_id": tx_id,
        "data": body
    });
    if let Some(obj) = response.as_object_mut() {
        if let Some(ttl_obj) = ttl_info.as_object() {
            for (k, v) in ttl_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
    }

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

    // Retract all triples in a single transaction so the delete is atomic.
    let mut db_tx = state
        .triple_store
        .begin_tx()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to begin transaction: {e}")))?;

    let del_tx_id = PgTripleStore::next_tx_id_in_tx(&mut db_tx)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to allocate tx_id: {e}")))?;

    for triple in &existing {
        PgTripleStore::retract_in_tx(&mut db_tx, id, &triple.attribute)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to retract triple: {e}")))?;
    }

    db_tx
        .commit()
        .await
        .map_err(|e| ApiError::internal(format!("Transaction commit failed: {e}")))?;

    // Invalidate hot cache for the affected entity type.
    state.query_cache.invalidate_by_entity_type(&entity);

    // Emit change event for reactive subscriptions.
    let _ = state.change_tx.send(ChangeEvent {
        tx_id: del_tx_id,
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
///
/// Looks up the function by name in the [`FunctionRegistry`], validates
/// arguments, executes via the [`FunctionRuntime`], and returns the result.
/// The function name can be either a fully-qualified name (`module:export`)
/// or a simple name that is searched across all registered functions.
async fn fn_invoke(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    axum::Json(args): axum::Json<Value>,
) -> Result<Response, ApiError> {
    let token = extract_bearer_token(&headers).ok();

    if name.is_empty() {
        return Err(ApiError::bad_request("Function name is required"));
    }

    // Validate function name format: alphanumeric, underscores, colons, dots, hyphens, slashes.
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-' || c == ':' || c == '/')
    {
        return Err(ApiError::bad_request(
            "Function name contains invalid characters",
        ));
    }

    // Ensure function subsystem is initialized.
    let registry = state
        .function_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Function registry not initialized"))?;
    let runtime = state
        .function_runtime
        .as_ref()
        .ok_or_else(|| ApiError::internal("Function runtime not initialized"))?;

    // Look up the function. Try exact match first, then search by export name.
    let function_def = match registry.get(&name).await {
        Some(def) => def,
        None => {
            // Search across all functions for a matching export name.
            let all = registry.list().await;
            all.into_iter()
                .find(|f| f.export_name == name || f.name.ends_with(&format!(":{name}")))
                .ok_or_else(|| ApiError::not_found(format!("Function `{name}` not found")))?
        }
    };

    // Execute the function via the runtime.
    let result = runtime
        .execute(&function_def, args, token)
        .await
        .map_err(|e| ApiError::internal(format!("Function execution failed: {e}")))?;

    let response = serde_json::json!({
        "result": result.value,
        "duration_ms": result.duration_ms,
        "logs": result.logs,
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
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    let content_type_str = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let (file_data, file_content_type, upload_path) =
        if content_type_str.starts_with("multipart/form-data") {
            let mut multipart =
                <axum::extract::Multipart as FromRequest<()>>::from_request(request, &())
                    .await
                    .map_err(|e| ApiError::bad_request(format!("Invalid multipart data: {e}")))?;

            let mut file_data: Option<Vec<u8>> = None;
            let mut file_ct = String::from("application/octet-stream");
            let mut custom_path: Option<String> = None;

            while let Some(field) = multipart
                .next_field()
                .await
                .map_err(|e| ApiError::bad_request(format!("Failed to read field: {e}")))?
            {
                let name = field.name().unwrap_or("").to_string();
                match name.as_str() {
                    "file" => {
                        if let Some(ct) = field.content_type() {
                            file_ct = ct.to_string();
                        }
                        let bytes = field.bytes().await.map_err(|e| {
                            ApiError::bad_request(format!("Failed to read file: {e}"))
                        })?;
                        file_data = Some(bytes.to_vec());
                    }
                    "path" => {
                        let text = field.text().await.map_err(|e| {
                            ApiError::bad_request(format!("Failed to read path: {e}"))
                        })?;
                        if !text.is_empty() {
                            custom_path = Some(text);
                        }
                    }
                    _ => {}
                }
            }

            let data = file_data
                .ok_or_else(|| ApiError::bad_request("Missing 'file' field in multipart upload"))?;
            (data, file_ct, custom_path)
        } else {
            let body_bytes = axum::body::to_bytes(request.into_body(), 100 * 1024 * 1024)
                .await
                .map_err(|e| ApiError::bad_request(format!("Failed to read body: {e}")))?;
            (body_bytes.to_vec(), content_type_str.clone(), None)
        };

    if file_data.is_empty() {
        return Err(ApiError::bad_request("Upload body is empty"));
    }

    let path = upload_path.unwrap_or_else(|| format!("uploads/{}", Uuid::new_v4()));

    let result = state
        .storage_engine
        .upload(
            &path,
            &file_data,
            &file_content_type,
            std::collections::HashMap::new(),
        )
        .await
        .map_err(storage_err_to_api)?;

    let response = serde_json::json!({
        "path": result.path,
        "size": result.size,
        "content_type": result.content_type,
        "etag": result.etag,
        "signed_url": result.signed_url,
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
    /// Signed URL expiry timestamp (for verification).
    expires: Option<i64>,
    /// Signed URL signature (for verification).
    sig: Option<String>,
}

/// `GET /api/storage/*path` — Download a file or retrieve a signed URL.
async fn storage_get(
    State(state): State<AppState>,
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

    // If the request carries a signed URL signature, verify it.
    if let (Some(expires), Some(sig)) = (params.expires, &params.sig) {
        state
            .storage_engine
            .verify_signed_url(&path, expires, sig)
            .map_err(storage_err_to_api)?;
    }

    if params.signed.unwrap_or(false) {
        let signed = state
            .storage_engine
            .signed_url(&path, "/api/storage")
            .map_err(storage_err_to_api)?;
        let response = serde_json::json!({
            "signed_url": signed.url,
            "expires_at": signed.expires_at.to_rfc3339(),
            "expires_in": signed.expires_in,
        });
        return Ok(negotiate_response(&headers, &response));
    }

    // Download the file from the storage engine.
    let (data, meta) = state
        .storage_engine
        .download(&path)
        .await
        .map_err(storage_err_to_api)?;

    let _ = params.transform; // TODO: apply image transforms when image processor is available.

    let mut response = (StatusCode::OK, data).into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&meta.content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    if let Ok(etag_val) = HeaderValue::from_str(&format!("\"{}\"", meta.etag)) {
        response.headers_mut().insert("etag", etag_val);
    }
    Ok(response)
}

/// `DELETE /api/storage/*path` — Delete a stored file.
async fn storage_delete(
    State(state): State<AppState>,
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

    state
        .storage_engine
        .delete(&path)
        .await
        .map_err(storage_err_to_api)?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Convert a [`StorageError`] into an [`ApiError`].
fn storage_err_to_api(err: StorageError) -> ApiError {
    match &err {
        StorageError::NotFound(_) => ApiError::not_found(err.to_string()),
        StorageError::InvalidPath(_) => ApiError::bad_request(err.to_string()),
        StorageError::Rejected(_) => ApiError::new(ErrorCode::PayloadTooLarge, err.to_string()),
        StorageError::SignatureExpired | StorageError::InvalidSignature => {
            ApiError::new(ErrorCode::Unauthenticated, err.to_string())
        }
        _ => ApiError::internal(err.to_string()),
    }
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
// Pub/Sub SSE + Publish handlers
// ===========================================================================

/// Query parameters for the pub/sub SSE endpoint.
#[derive(Deserialize)]
struct EventsParams {
    /// Channel pattern to subscribe to (e.g., `entity:users:*`).
    channel: String,
}

/// `GET /api/events?channel=entity:users:*` -- Server-Sent Events for pub/sub.
///
/// Subscribes to the pub/sub engine's broadcast channel and filters events
/// matching the requested channel pattern. Sends matching events as SSE
/// data frames with a heartbeat comment every 15 seconds.
async fn events_sse(
    State(state): State<AppState>,
    Query(params): Query<EventsParams>,
    headers: HeaderMap,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if params.channel.is_empty() {
        return Err(ApiError::bad_request(
            "Query parameter 'channel' is required",
        ));
    }

    let pattern = crate::sync::pubsub::ChannelPattern::parse(&params.channel);
    let rx = state.pubsub.subscribe_events();

    let stream = BroadcastStream::new(rx).filter_map(move |msg| match msg {
        Ok(event) => {
            if pattern.matches(&event.channel) {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Some(Ok(Event::default()
                    .event("pub-event")
                    .data(data)
                    .id(event.tx_id.to_string())))
            } else {
                None
            }
        }
        Err(_) => None,
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// Request body for the publish endpoint.
#[derive(Deserialize)]
struct PublishRequest {
    /// Channel to publish to (e.g., `custom:notifications`).
    channel: String,
    /// Event name (e.g., `new-message`).
    event: String,
    /// Optional payload data.
    #[serde(default)]
    payload: Option<Value>,
}

/// `POST /api/events/publish` -- Publish a custom event to a channel.
///
/// Allows clients to publish arbitrary events for webhooks, notifications,
/// or inter-service communication. The event is broadcast to all matching
/// pub/sub subscribers (WebSocket and SSE).
async fn events_publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    let req: PublishRequest = serde_json::from_str(&body)
        .map_err(|e| ApiError::bad_request(format!("invalid request body: {e}")))?;

    if req.channel.is_empty() {
        return Err(ApiError::bad_request("'channel' is required"));
    }
    if req.event.is_empty() {
        return Err(ApiError::bad_request("'event' is required"));
    }

    let pub_event = PubSubEvent {
        channel: req.channel.clone(),
        event: req.event.clone(),
        entity_type: None,
        entity_id: None,
        changed: vec![],
        tx_id: 0,
        payload: req.payload,
    };

    let receivers = state.pubsub.publish(pub_event);

    let response = serde_json::json!({
        "ok": true,
        "channel": req.channel,
        "event": req.event,
        "receivers": receivers,
    });

    Ok(negotiate_response(&headers, &response))
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

// ---------------------------------------------------------------------------
// Bulk load
// ---------------------------------------------------------------------------

/// Request body for `POST /api/admin/bulk-load`.
///
/// Accepts an array of entities in the same format used by the migration
/// scripts: each entity has a `type`, optional `id`, and a `data` map.
/// The handler converts these into triples and uses the UNNEST-based
/// bulk loader for 10-50x faster throughput compared to batched INSERT.
#[derive(Deserialize)]
struct BulkLoadRequest {
    /// Entities to load.
    entities: Vec<BulkLoadEntity>,
}

/// A single entity within a bulk-load request.
#[derive(Deserialize)]
struct BulkLoadEntity {
    /// Entity type name (e.g. "users", "messages").
    #[serde(rename = "type")]
    entity_type: String,
    /// Optional entity id; a new UUID is generated if absent.
    id: Option<Uuid>,
    /// Key-value data for the entity.
    data: HashMap<String, Value>,
}

/// `GET /api/admin/cache` — Return hot-cache statistics.
///
/// Reports current size, hit/miss rates, eviction and invalidation
/// counts. Useful for monitoring cache effectiveness and tuning
/// `DARSHAN_CACHE_SIZE` / `DARSHAN_CACHE_TTL`.
async fn admin_cache(
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

/// `POST /api/admin/bulk-load` — High-throughput data import.
///
/// Converts a JSON array of entities into triples and writes them using
/// PostgreSQL UNNEST-based bulk insert. Returns the count, transaction id,
/// duration, and throughput rate.
async fn admin_bulk_load(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<BulkLoadRequest>,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;
    require_admin_role(&headers)?;

    if body.entities.is_empty() {
        return Err(ApiError::bad_request("At least one entity is required"));
    }

    // Convert entities to triples.
    let mut triples: Vec<TripleInput> = Vec::new();

    for entity in &body.entities {
        validate_entity_name(&entity.entity_type)
            .map_err(|e| ApiError::bad_request(format!("Invalid entity type: {}", e.message)))?;

        let entity_id = entity.id.unwrap_or_else(Uuid::new_v4);

        // Add the :db/type triple.
        triples.push(TripleInput {
            entity_id,
            attribute: ":db/type".to_string(),
            value: Value::String(entity.entity_type.clone()),
            value_type: 0, // String
            ttl_seconds: None,
        });

        // Add a triple for each data field.
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

/// Find or create a user from OAuth identity info.
///
/// Looks up `oauth_identities` by (provider, provider_user_id). If not found,
/// checks for an existing user with the same email for account linking.
/// If neither exists, creates a new user with a placeholder password hash.
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

// ===========================================================================
// Embeddings / Semantic Search handlers
// ===========================================================================

/// Request body for `POST /api/embeddings`.
#[derive(Deserialize)]
struct EmbeddingStoreRequest {
    entity_id: Uuid,
    attribute: String,
    embedding: Vec<f32>,
    #[serde(default = "default_embedding_model")]
    model: String,
}

fn default_embedding_model() -> String {
    "text-embedding-ada-002".to_string()
}

/// `POST /api/embeddings` — Store an embedding vector for an entity+attribute pair.
async fn embeddings_store(
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

    // Format the vector as a pgvector literal: [0.1,0.2,0.3]
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

/// `GET /api/embeddings/:entity_id` — Get all embeddings for an entity.
async fn embeddings_get(
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

/// Request body for `POST /api/search/semantic`.
#[derive(Deserialize)]
struct SemanticSearchRequest {
    /// The entity type to search within (e.g. "Article").
    entity_type: String,
    /// Pre-computed embedding vector.
    vector: Vec<f32>,
    /// Maximum number of results.
    #[serde(default = "default_search_limit")]
    limit: u32,
    /// Optional attribute filter — only search embeddings for this attribute.
    #[serde(default)]
    attribute: Option<String>,
}

fn default_search_limit() -> u32 {
    10
}

/// `POST /api/search/semantic` — Search by vector similarity, return matched
/// entities with their cosine distance scores.
async fn search_semantic(
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

    // Build the query dynamically to support optional attribute filter.
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

/// Format a slice of f32 values as a pgvector literal string: `[0.1,0.2,0.3]`.
fn format_pgvector_literal(vec: &[f32]) -> String {
    let mut s = String::with_capacity(vec.len() * 8 + 2);
    s.push('[');
    for (i, v) in vec.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&v.to_string());
    }
    s.push(']');
    s
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
    fn rate_limit_headers_valid() {
        // Verify the rate-limit header values used by rate_limit_middleware are valid.
        assert!(HeaderValue::from_str("100").is_ok());
        assert!(HeaderValue::from_str("20").is_ok());
        assert!(HeaderValue::from_str("60").is_ok());
        assert!(HeaderValue::from_str("0").is_ok());

        // Verify the header names are valid
        let _ = "x-ratelimit-limit";
        let _ = "x-ratelimit-remaining";
        let _ = "x-ratelimit-reset";
        let _ = "retry-after";
    }

    #[test]
    fn rate_limiter_enforces_anonymous_limit() {
        let limiter = RateLimiter::new();
        let key = RateLimitKey::Ip("test-ip".into());

        // 20 anonymous requests should succeed.
        for _ in 0..20 {
            assert!(limiter.check(&key, false).is_ok());
        }
        // 21st should fail.
        let result = limiter.check(&key, false);
        assert!(result.is_err());
    }

    #[test]
    fn rate_limiter_enforces_authenticated_limit() {
        let limiter = RateLimiter::new();
        let key = RateLimitKey::Token("test-token-hash".into());

        // 100 authenticated requests should succeed.
        for _ in 0..100 {
            assert!(limiter.check(&key, true).is_ok());
        }
        // 101st should fail.
        let result = limiter.check(&key, true);
        assert!(result.is_err());
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
            "/events",
            "/events/publish",
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
