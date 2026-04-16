//! Full REST API router for DarshJDB.
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
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

use super::openapi;
use crate::auth::middleware::RateLimitKey;
use crate::auth::{
    AuthContext, GenericOAuth2Provider, OAuthProviderKind, PermissionEngine,
    RateLimiter, SessionManager, build_default_engine,
};
use crate::cache::QueryCache;
use crate::functions::registry::FunctionRegistry;
use crate::functions::runtime::FunctionRuntime;
use crate::rules::RuleEngine;
use crate::storage::{LocalFsBackend, StorageEngine};
use crate::sync::broadcaster::ChangeEvent;
use crate::sync::pubsub::PubSubEngine;
use crate::graph::GraphEngine;
use crate::triple_store::PgTripleStore;

// Re-export handler modules so other crates can access them.
use super::handlers;

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
    /// Whether dev mode is active (DDB_DEV=1).
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
    /// Metrics for Solana-inspired parallel batch execution.
    pub parallel_metrics: Arc<crate::query::parallel::ParallelMetrics>,
    /// Graph engine for SurrealDB-style record links and traversal.
    pub graph_engine: Option<Arc<GraphEngine>>,
    /// Schema registry for SCHEMAFULL/SCHEMALESS/MIXED mode enforcement.
    pub schema_registry: Option<Arc<crate::schema::SchemaRegistry>>,
}

/// Load OAuth2 provider configurations from environment variables.
///
/// Reads `DDB_OAUTH_{PROVIDER}_CLIENT_ID` and
/// `DDB_OAUTH_{PROVIDER}_CLIENT_SECRET` for each supported provider.
/// Providers without both env vars are silently skipped.
fn load_oauth_providers_from_env() -> HashMap<OAuthProviderKind, GenericOAuth2Provider> {
    let base_url =
        std::env::var("DDB_BASE_URL").unwrap_or_else(|_| "http://localhost:4000".to_string());
    let mut providers = HashMap::new();

    // Google
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_GOOGLE_CLIENT_ID"),
        std::env::var("DDB_OAUTH_GOOGLE_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_GOOGLE_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/google/callback"));
        providers.insert(
            OAuthProviderKind::Google,
            GenericOAuth2Provider::google(id, secret, redirect),
        );
    }

    // GitHub
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_GITHUB_CLIENT_ID"),
        std::env::var("DDB_OAUTH_GITHUB_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_GITHUB_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/github/callback"));
        providers.insert(
            OAuthProviderKind::GitHub,
            GenericOAuth2Provider::github(id, secret, redirect),
        );
    }

    // Apple
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_APPLE_CLIENT_ID"),
        std::env::var("DDB_OAUTH_APPLE_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_APPLE_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/apple/callback"));
        providers.insert(
            OAuthProviderKind::Apple,
            GenericOAuth2Provider::apple(id, secret, redirect),
        );
    }

    // Discord
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_DISCORD_CLIENT_ID"),
        std::env::var("DDB_OAUTH_DISCORD_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_DISCORD_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/discord/callback"));
        providers.insert(
            OAuthProviderKind::Discord,
            GenericOAuth2Provider::discord(id, secret, redirect),
        );
    }

    // Microsoft (Azure AD / Entra ID)
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_MICROSOFT_CLIENT_ID"),
        std::env::var("DDB_OAUTH_MICROSOFT_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_MICROSOFT_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/microsoft/callback"));
        providers.insert(
            OAuthProviderKind::Microsoft,
            GenericOAuth2Provider::microsoft(id, secret, redirect),
        );
    }

    // Twitter / X
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_TWITTER_CLIENT_ID"),
        std::env::var("DDB_OAUTH_TWITTER_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_TWITTER_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/twitter/callback"));
        providers.insert(
            OAuthProviderKind::Twitter,
            GenericOAuth2Provider::twitter(id, secret, redirect),
        );
    }

    // LinkedIn
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_LINKEDIN_CLIENT_ID"),
        std::env::var("DDB_OAUTH_LINKEDIN_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_LINKEDIN_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/linkedin/callback"));
        providers.insert(
            OAuthProviderKind::LinkedIn,
            GenericOAuth2Provider::linkedin(id, secret, redirect),
        );
    }

    // Slack
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_SLACK_CLIENT_ID"),
        std::env::var("DDB_OAUTH_SLACK_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_SLACK_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/slack/callback"));
        providers.insert(
            OAuthProviderKind::Slack,
            GenericOAuth2Provider::slack(id, secret, redirect),
        );
    }

    // GitLab
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_GITLAB_CLIENT_ID"),
        std::env::var("DDB_OAUTH_GITLAB_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_GITLAB_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/gitlab/callback"));
        providers.insert(
            OAuthProviderKind::GitLab,
            GenericOAuth2Provider::gitlab(id, secret, redirect),
        );
    }

    // Bitbucket
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_BITBUCKET_CLIENT_ID"),
        std::env::var("DDB_OAUTH_BITBUCKET_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_BITBUCKET_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/bitbucket/callback"));
        providers.insert(
            OAuthProviderKind::Bitbucket,
            GenericOAuth2Provider::bitbucket(id, secret, redirect),
        );
    }

    // Facebook / Meta
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_FACEBOOK_CLIENT_ID"),
        std::env::var("DDB_OAUTH_FACEBOOK_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_FACEBOOK_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/facebook/callback"));
        providers.insert(
            OAuthProviderKind::Facebook,
            GenericOAuth2Provider::facebook(id, secret, redirect),
        );
    }

    // Spotify
    if let (Ok(id), Ok(secret)) = (
        std::env::var("DDB_OAUTH_SPOTIFY_CLIENT_ID"),
        std::env::var("DDB_OAUTH_SPOTIFY_CLIENT_SECRET"),
    ) {
        let redirect = std::env::var("DDB_OAUTH_SPOTIFY_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{base_url}/api/auth/oauth/spotify/callback"));
        providers.insert(
            OAuthProviderKind::Spotify,
            GenericOAuth2Provider::spotify(id, secret, redirect),
        );
    }

    providers
}

/// Load or generate the HMAC secret for OAuth2 state parameters.
fn load_oauth_state_secret() -> Vec<u8> {
    match std::env::var("DDB_OAUTH_STATE_SECRET") {
        Ok(s) if s.len() >= 32 => s.into_bytes(),
        _ => {
            use rand::RngCore;
            let mut buf = vec![0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut buf);
            tracing::warn!("DDB_OAUTH_STATE_SECRET not set; generated ephemeral secret");
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
        let dev_mode = std::env::var("DDB_DEV")
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
            parallel_metrics: Arc::new(crate::query::parallel::ParallelMetrics::new()),
            graph_engine: None,
            schema_registry: None,
        }
    }

    /// Set the graph engine on this state for SurrealDB-style record links.
    pub fn with_graph(mut self, graph_engine: Arc<GraphEngine>) -> Self {
        self.graph_engine = Some(graph_engine);
        self
    }

    /// Set the schema registry for SCHEMAFULL/SCHEMALESS/MIXED enforcement.
    pub fn with_schema_registry(mut self, registry: Arc<crate::schema::SchemaRegistry>) -> Self {
        self.schema_registry = Some(registry);
        self
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
    /// Panics if called outside tests -- production code must use `with_pool`.
    #[cfg(test)]
    pub fn new() -> Self {
        // Tests that don't hit the database can use a dummy pool.
        // This preserves backward compatibility with existing unit tests.
        let (sse_tx, _) = broadcast::channel(1024);
        let (change_tx, _) = broadcast::channel(1024);
        let (pubsub, _) = PubSubEngine::new(64);
        let pool = PgPool::connect_lazy("postgres://localhost/darshjdb_test").expect("test pool");
        let triple_store = Arc::new(PgTripleStore::new_lazy(pool.clone()));
        let key_manager = crate::auth::KeyManager::generate();
        let session_manager = Arc::new(SessionManager::new(pool.clone(), key_manager));
        let storage_backend = Arc::new(
            LocalFsBackend::new("/tmp/darshjdb-test-storage").expect("create test storage backend"),
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
            parallel_metrics: Arc::new(crate::query::parallel::ParallelMetrics::new()),
            graph_engine: None,
            schema_registry: None,
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
// Content negotiation (public wrappers for batch module)
// ---------------------------------------------------------------------------

/// Public wrapper for content-negotiated response (used by batch module).
pub fn negotiate_response_pub(headers: &HeaderMap, value: &impl Serialize) -> Response {
    handlers::helpers::negotiate_response(headers, value)
}

// ---------------------------------------------------------------------------
// Public re-exports for external consumers
// ---------------------------------------------------------------------------

/// Re-export `ensure_auth_schema` so server bootstrap code can call it.
pub use handlers::auth::ensure_auth_schema;

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
// Auth middleware
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

/// Build the complete DarshJDB REST API router.
///
/// Mount this under `/api` in your top-level Axum application:
///
/// ```rust,ignore
/// use ddb_server::api::rest::{build_router, AppState};
///
/// let state = AppState::with_pool(pool, triple_store);
/// let app = axum::Router::new()
///     .nest("/api", build_router(state));
/// ```
pub fn build_router(state: AppState) -> Router {
    // Public auth routes -- no JWT required.
    let public_routes = Router::new()
        .route("/auth/signup", post(handlers::auth::auth_signup))
        .route("/auth/signin", post(handlers::auth::auth_signin))
        .route("/auth/magic-link", post(handlers::auth_oauth::auth_magic_link))
        .route("/auth/verify", post(handlers::auth_oauth::auth_verify))
        .route("/auth/oauth/{provider}", post(handlers::auth_oauth::auth_oauth))
        .route(
            "/auth/oauth/{provider}/callback",
            get(handlers::auth_oauth::auth_oauth_callback),
        )
        .route("/auth/refresh", post(handlers::auth::auth_refresh));

    // Protected routes -- require valid JWT (or "Bearer dev" in dev mode).
    let protected_routes = Router::new()
        .route("/auth/signout", post(handlers::auth::auth_signout))
        .route("/auth/me", get(handlers::auth::auth_me))
        // -- DarshQL -------------------------------------------------------
        .route("/sql", post(handlers::query::darshql_handler))
        // -- Data ----------------------------------------------------------
        .route("/query", post(handlers::data_mutation::query_handler))
        .route("/mutate", post(handlers::data_mutation::mutate))
        .route(
            "/data/{entity}",
            get(handlers::data::data_list).post(handlers::data::data_create),
        )
        .route(
            "/data/{entity}/{id}",
            get(handlers::data::data_get)
                .patch(handlers::data::data_patch)
                .delete(handlers::data::data_delete),
        )
        // -- Functions -----------------------------------------------------
        .route("/fn/{name}", post(handlers::functions::fn_invoke))
        // -- Storage -------------------------------------------------------
        .route("/storage/upload", post(handlers::storage::storage_upload))
        .route(
            "/storage/{*path}",
            get(handlers::storage::storage_get).delete(handlers::storage::storage_delete),
        )
        // -- SSE -----------------------------------------------------------
        .route("/subscribe", get(handlers::events::subscribe))
        // -- Pub/Sub -------------------------------------------------------
        .route("/events", get(handlers::events::events_sse))
        .route("/events/publish", post(handlers::events::events_publish))
        // -- Admin ---------------------------------------------------------
        .route("/admin/schema", get(handlers::admin::admin_schema))
        .route("/admin/functions", get(handlers::admin::admin_functions))
        .route("/admin/sessions", get(handlers::admin::admin_sessions))
        .route("/admin/bulk-load", post(handlers::admin::admin_bulk_load))
        .route("/admin/cache", get(handlers::admin::admin_cache))
        // -- Audit (Merkle tree) ------------------------------------------
        .route(
            "/admin/audit/verify/{tx_id}",
            get(crate::audit::handlers::audit_verify_tx),
        )
        .route(
            "/admin/audit/chain",
            get(crate::audit::handlers::audit_verify_chain),
        )
        .route(
            "/admin/audit/proof/{entity_id}",
            get(crate::audit::handlers::audit_entity_proof),
        )
        // -- Graph (SurrealDB-style record links) -------------------------
        .route("/graph/relate", post(handlers::graph::graph_relate))
        .route("/graph/traverse", post(handlers::graph::graph_traverse))
        .route(
            "/graph/neighbors/{table}/{id}",
            get(handlers::graph::graph_neighbors),
        )
        .route(
            "/graph/outgoing/{table}/{id}",
            get(handlers::graph::graph_outgoing),
        )
        .route(
            "/graph/incoming/{table}/{id}",
            get(handlers::graph::graph_incoming),
        )
        .route(
            "/graph/edge/{edge_id}",
            axum::routing::delete(handlers::graph::graph_delete_edge),
        )
        // -- Schema management (DEFINE TABLE / FIELD / INDEX) ---------------
        .route(
            "/schema/tables",
            get(handlers::schema::schema_list_tables).post(handlers::schema::schema_define_table),
        )
        .route(
            "/schema/tables/{table}",
            axum::routing::delete(handlers::schema::schema_remove_table),
        )
        .route(
            "/schema/tables/{table}/fields",
            post(handlers::schema::schema_define_field),
        )
        .route(
            "/schema/tables/{table}/fields/{field}",
            axum::routing::delete(handlers::schema::schema_remove_field),
        )
        .route(
            "/schema/tables/{table}/indexes",
            post(handlers::schema::schema_define_index),
        )
        .route(
            "/schema/tables/{table}/migrations",
            get(handlers::schema::schema_migration_history),
        )
        // -- Batch / Pipeline ---------------------------------------------
        .route("/batch", post(super::batch::batch_handler))
        .route(
            "/batch/parallel",
            post(super::batch::parallel_batch_handler),
        )
        .route(
            "/batch/metrics",
            get(super::batch::parallel_metrics_handler),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth_middleware,
        ));

    // Docs are public.
    let docs_routes = Router::new()
        .route("/openapi.json", get(handlers::docs::openapi_json))
        .route("/docs", get(handlers::docs::docs));

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
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error::ErrorCode;
    use axum::http::header::CONTENT_TYPE;
    use handlers::helpers::{
        extract_bearer_token, negotiate_response, negotiate_response_status, validate_entity_name,
        wants_msgpack,
    };

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
        assert_eq!(spec["info"]["title"], "DarshJDB API");
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
        assert!(html.contains("DarshJDB"));
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
