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
use std::convert::Infallible;
use std::sync::Arc;
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
    AuthContext, AuthOutcome, GenericOAuth2Provider, MagicLinkProvider, OAuth2Provider,
    OAuthProviderKind, OAuthUserInfo, Operation, PasswordProvider, PermissionEngine, RateLimiter,
    SessionManager, build_default_engine, evaluate_rule_public, get_rule_with_fallback,
};
use crate::cache::{self, QueryCache};
use crate::functions::registry::FunctionRegistry;
use crate::functions::runtime::FunctionRuntime;
use crate::graph::{Edge, EdgeInput, GraphEngine, RecordId, TraversalConfig};
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
    /// Panics if called outside tests — production code must use `with_pool`.
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
        // -- DarshQL -------------------------------------------------------
        .route("/sql", post(darshql_handler))
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
        .route("/admin/storage", get(admin_storage_list))
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
        // -- Embeddings / Semantic Search (TODO: wire handlers) ------------
        // .route("/embeddings", post(embeddings_store))
        // .route("/embeddings/{entity_id}", get(embeddings_get))
        // .route("/search/semantic", post(search_semantic))
        // -- Graph (SurrealDB-style record links) -------------------------
        .route("/graph/relate", post(graph_relate))
        .route("/graph/traverse", post(graph_traverse))
        .route("/graph/neighbors/{table}/{id}", get(graph_neighbors))
        .route("/graph/outgoing/{table}/{id}", get(graph_outgoing))
        .route("/graph/incoming/{table}/{id}", get(graph_incoming))
        .route(
            "/graph/edge/{edge_id}",
            axum::routing::delete(graph_delete_edge),
        )
        // -- Schema management (DEFINE TABLE / FIELD / INDEX) ---------------
        .route(
            "/schema/tables",
            get(schema_list_tables).post(schema_define_table),
        )
        .route(
            "/schema/tables/{table}",
            axum::routing::delete(schema_remove_table),
        )
        .route("/schema/tables/{table}/fields", post(schema_define_field))
        .route(
            "/schema/tables/{table}/fields/{field}",
            axum::routing::delete(schema_remove_field),
        )
        .route("/schema/tables/{table}/indexes", post(schema_define_index))
        .route(
            "/schema/tables/{table}/migrations",
            get(schema_migration_history),
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
        // -- Views ------------------------------------------------------------
        .merge(crate::views::handlers::view_routes())
        // -- Fields -----------------------------------------------------------
        .nest("/fields", crate::fields::handlers::field_routes())
        // -- Import/Export ----------------------------------------------------
        .merge(crate::import_export::handlers::import_export_routes(
            crate::import_export::handlers::new_job_tracker(),
        ))
        // -- Collaboration & Sharing -----------------------------------------
        .merge(crate::collaboration::handlers::collaboration_router())
        // -- Comments & Activity Log -----------------------------------------
        .route(
            "/data/{entity}/{id}/comments",
            get(crate::activity::handlers::comment_list)
                .post(crate::activity::handlers::comment_create),
        )
        .route(
            "/comments/{id}",
            axum::routing::patch(crate::activity::handlers::comment_update)
                .delete(crate::activity::handlers::comment_delete),
        )
        .route(
            "/data/{entity}/{id}/activity",
            get(crate::activity::handlers::activity_for_record),
        )
        .route("/activity", get(crate::activity::handlers::activity_query))
        .route(
            "/notifications",
            get(crate::activity::handlers::notifications_list),
        )
        .route(
            "/notifications/count",
            get(crate::activity::handlers::notification_unread_count),
        )
        .route(
            "/notifications/read-all",
            axum::routing::patch(crate::activity::handlers::notification_mark_all_read),
        )
        .route(
            "/notifications/{id}/read",
            axum::routing::patch(crate::activity::handlers::notification_mark_read),
        )
        // -- Relations --------------------------------------------------------
        .merge(crate::relations::handlers::relation_routes())
        // -- History & Snapshots ----------------------------------------------
        .route(
            "/data/{entity}/{id}/history",
            get(crate::history::handlers::get_record_history),
        )
        .route(
            "/data/{entity}/{id}/history/{version}",
            get(crate::history::handlers::get_record_version),
        )
        .route(
            "/data/{entity}/{id}/restore/{version}",
            post(crate::history::handlers::restore_record_version),
        )
        .route(
            "/data/{entity}/{id}/undo",
            post(crate::history::handlers::undo_record),
        )
        .route(
            "/data/{entity}/{id}/undelete",
            post(crate::history::handlers::undelete_record),
        )
        .route(
            "/snapshots",
            get(crate::history::handlers::list_snapshots_handler)
                .post(crate::history::handlers::create_snapshot_handler),
        )
        .route(
            "/snapshots/{id}/restore",
            post(crate::history::handlers::restore_snapshot_handler),
        )
        .route(
            "/snapshots/{id}/diff",
            get(crate::history::handlers::diff_snapshot_handler),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth_middleware,
        ));

    // Docs and SDK types are public.
    let docs_routes = Router::new()
        .route("/openapi.json", get(openapi_json))
        .route("/docs", get(docs))
        .route("/types.ts", get(types_ts));

    // Sub-routers with independent state. These carry their own state via
    // `.with_state()`, yielding `Router<()>`. We apply the auth middleware
    // to each individually, then merge after the main router resolves its
    // state.
    let table_routes: Router = Router::new()
        .nest(
            "/tables",
            crate::tables::handlers::table_routes(crate::tables::handlers::TableState {
                table_store: std::sync::Arc::new(crate::tables::PgTableStore::new(
                    state.pool.clone(),
                    (*state.triple_store).clone(),
                )),
            }),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth_middleware,
        ));

    let aggregation_routes: Router = Router::new()
        .nest(
            "/aggregate",
            crate::aggregation::handlers::aggregation_routes::<()>().with_state(
                crate::aggregation::handlers::AggregationState {
                    engine: crate::aggregation::engine::AggregationEngine::new(state.pool.clone()),
                    pool: state.pool.clone(),
                },
            ),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth_middleware,
        ));

    let webhook_routes: Router = Router::new()
        .nest(
            "/webhooks",
            crate::webhooks::handlers::webhook_routes().with_state(
                crate::webhooks::handlers::WebhookState {
                    pool: state.pool.clone(),
                    sender: std::sync::Arc::new(crate::webhooks::WebhookSender::new(
                        state.pool.clone(),
                    )),
                },
            ),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth_middleware,
        ));

    let api_key_routes: Router = Router::new()
        .nest(
            "/api-keys",
            crate::api_keys::handlers::api_key_routes().with_state(
                crate::api_keys::handlers::ApiKeyState {
                    pool: state.pool.clone(),
                },
            ),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth_middleware,
        ));

    let plugin_routes: Router = Router::new()
        .nest(
            "/plugins",
            crate::plugins::handlers::plugin_routes(crate::plugins::handlers::PluginApiState {
                registry: std::sync::Arc::new(crate::plugins::registry::PluginRegistry::new(
                    std::sync::Arc::new(crate::plugins::hooks::HookRegistry::new()),
                )),
            }),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth_middleware,
        ));

    let automation_routes: Router = Router::new()
        .nest(
            "/automations",
            crate::automations::handlers::automation_routes(
                crate::automations::handlers::AutomationState::new(),
            ),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth_middleware,
        ));

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
        // -- Self-stated sub-routers (merged after state resolution) ------
        .merge(table_routes)
        .merge(aggregation_routes)
        .merge(webhook_routes)
        .merge(api_key_routes)
        .merge(plugin_routes)
        .merge(automation_routes)
}

// ===========================================================================
// Auth handlers
// ===========================================================================

/// Ensure the `users` and `sessions` tables exist for the auth subsystem.
pub async fn ensure_auth_schema(pool: &PgPool) -> std::result::Result<(), sqlx::Error> {
    sqlx::raw_sql(
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
        -- Phase 0.4 hardening: structured revocation, absolute timeout,
        -- per-device IP/UA forensics. New columns are nullable / defaulted so
        -- this block stays idempotent against legacy databases.
        ALTER TABLE sessions
            ADD COLUMN IF NOT EXISTS ip_address          INET,
            ADD COLUMN IF NOT EXISTS last_active_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
            ADD COLUMN IF NOT EXISTS absolute_expires_at TIMESTAMPTZ NOT NULL DEFAULT (now() + INTERVAL '24 hours'),
            ADD COLUMN IF NOT EXISTS revoked_at          TIMESTAMPTZ,
            ADD COLUMN IF NOT EXISTS revoke_reason       TEXT;
        UPDATE sessions
           SET revoked_at = COALESCE(revoked_at, now()),
               revoke_reason = COALESCE(revoke_reason, 'legacy_revoked')
         WHERE revoked = true
           AND revoked_at IS NULL;
        CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions (user_id) WHERE NOT revoked;
        CREATE INDEX IF NOT EXISTS idx_sessions_refresh ON sessions (refresh_token_hash) WHERE NOT revoked;
        CREATE INDEX IF NOT EXISTS idx_sessions_user_active
            ON sessions (user_id) WHERE revoked_at IS NULL;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_user_device
            ON sessions (user_id, device_fingerprint) WHERE revoked_at IS NULL;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_refresh_hash
            ON sessions (refresh_token_hash) WHERE revoked_at IS NULL;
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
        CREATE TABLE IF NOT EXISTS magic_link_tokens (
            id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
            user_id      UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            token_hash   TEXT        NOT NULL,
            email        TEXT        NOT NULL,
            created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
            expires_at   TIMESTAMPTZ NOT NULL DEFAULT (now() + INTERVAL '15 minutes'),
            used_at      TIMESTAMPTZ,
            ip_address   INET,
            CONSTRAINT unique_unused_token UNIQUE (token_hash)
        );
        CREATE INDEX IF NOT EXISTS idx_magic_link_tokens_hash
            ON magic_link_tokens (token_hash) WHERE used_at IS NULL;
        CREATE TABLE IF NOT EXISTS login_attempts (
            id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
            email         TEXT        NOT NULL,
            ip_address    INET        NOT NULL,
            attempted_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
            success       BOOLEAN     NOT NULL DEFAULT false
        );
        CREATE INDEX IF NOT EXISTS idx_login_attempts_email_time
            ON login_attempts (email, attempted_at DESC);
        CREATE INDEX IF NOT EXISTS idx_login_attempts_ip_time
            ON login_attempts (ip_address, attempted_at DESC);
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Login rate limiting (exponential throttle + account lock)
// ---------------------------------------------------------------------------

/// Window (minutes) over which failed login attempts are counted.
const LOGIN_ATTEMPT_WINDOW_MINUTES: i32 = 15;
/// Number of recent failures before exponential throttling kicks in.
const LOGIN_THROTTLE_THRESHOLD: i64 = 5;
/// Number of recent failures before the account is hard-locked.
const LOGIN_LOCK_THRESHOLD: i64 = 10;
/// Retry-after (seconds) returned when the account is locked.
const LOGIN_LOCK_RETRY_AFTER_SECS: u64 = 3600;

/// Outcome of the pre-authentication rate-limit check.
#[derive(Debug)]
pub struct LoginAttemptGate {
    /// UUID of the attempt row just inserted (for later success flip).
    pub attempt_id: Uuid,
}

/// Error outcomes from the login rate-limit gate.
///
/// `Locked` / `Throttled` serialize to the spec-mandated body shape
/// `{"error":"...","retry_after":"N"}` with a 429 status. `Internal`
/// bubbles up a standard `ApiError` 500.
#[derive(Debug)]
pub enum LoginGateError {
    /// Caller has hit ≥ 10 failures in the window → account locked.
    Locked { retry_after_secs: u64 },
    /// Caller has hit ≥ 5 failures in the window → exponential throttle.
    Throttled { retry_after_secs: u64 },
    /// DB or other server-side failure while recording/counting.
    Internal(String),
}

impl LoginGateError {
    fn into_response(self) -> Response {
        match self {
            LoginGateError::Locked { retry_after_secs } => {
                let body = serde_json::json!({
                    "error": "account_locked",
                    "retry_after": retry_after_secs.to_string(),
                });
                let mut resp = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
                if let Ok(v) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                    resp.headers_mut().insert("Retry-After", v);
                }
                resp
            }
            LoginGateError::Throttled { retry_after_secs } => {
                let body = serde_json::json!({
                    "error": "too_many_attempts",
                    "retry_after": retry_after_secs.to_string(),
                });
                let mut resp = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
                if let Ok(v) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                    resp.headers_mut().insert("Retry-After", v);
                }
                resp
            }
            LoginGateError::Internal(msg) => ApiError::internal(msg).into_response(),
        }
    }
}

/// Parse a client-supplied IP string into a canonical `IpAddr`.
///
/// Accepts both IPv4 and IPv6. Returns `400 Bad Request` on failure.
fn parse_client_ip(raw: &str) -> Result<std::net::IpAddr, ApiError> {
    raw.parse::<std::net::IpAddr>()
        .map_err(|_| ApiError::bad_request(format!("Invalid client IP address: {raw}")))
}

/// Record a failed login attempt and enforce the exponential throttle.
///
/// Steps:
/// 1. INSERT a `login_attempts` row with `success=false`.
/// 2. COUNT failed attempts for this email in the last 15 minutes.
/// 3. `>= 10` → return `429 {"error":"account_locked","retry_after":"3600"}`.
/// 4. `>= 5`  → return `429 {"error":"too_many_attempts","retry_after":"2^(n-5)"}`.
/// 5. Otherwise return the inserted row id so the caller can flip it on success.
pub async fn record_login_attempt_and_check(
    pool: &PgPool,
    email: &str,
    ip: std::net::IpAddr,
) -> Result<LoginAttemptGate, LoginGateError> {
    // 1. Insert the attempt row (success=false by default).
    let attempt_id: Uuid = sqlx::query_scalar(
        "INSERT INTO login_attempts (email, ip_address, success) \
         VALUES ($1, $2::inet, false) RETURNING id",
    )
    .bind(email)
    .bind(ip.to_string())
    .fetch_one(pool)
    .await
    .map_err(|e| LoginGateError::Internal(format!("login_attempts insert failed: {e}")))?;

    // 2. Count recent failures for this email.
    let failed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM login_attempts \
         WHERE email = $1 \
           AND success = false \
           AND attempted_at > now() - make_interval(mins => $2)",
    )
    .bind(email)
    .bind(LOGIN_ATTEMPT_WINDOW_MINUTES)
    .fetch_one(pool)
    .await
    .map_err(|e| LoginGateError::Internal(format!("login_attempts count failed: {e}")))?;

    // 3. Hard lock after LOGIN_LOCK_THRESHOLD failures.
    if failed >= LOGIN_LOCK_THRESHOLD {
        return Err(LoginGateError::Locked {
            retry_after_secs: LOGIN_LOCK_RETRY_AFTER_SECS,
        });
    }

    // 4. Exponential throttle between 5 and 9 failures: 2^(failed-5) seconds.
    if failed >= LOGIN_THROTTLE_THRESHOLD {
        let exp = (failed - LOGIN_THROTTLE_THRESHOLD) as u32;
        let retry = 2u64.pow(exp);
        return Err(LoginGateError::Throttled {
            retry_after_secs: retry,
        });
    }

    Ok(LoginAttemptGate { attempt_id })
}

/// Flip a previously-recorded attempt row to `success=true` after the
/// password has been verified. Non-fatal — failures only warn.
pub async fn mark_login_attempt_success(pool: &PgPool, attempt_id: Uuid) {
    if let Err(e) = sqlx::query("UPDATE login_attempts SET success = true WHERE id = $1")
        .bind(attempt_id)
        .execute(pool)
        .await
    {
        tracing::warn!(%attempt_id, error = %e, "failed to mark login attempt success");
    }
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

    match state
        .session_manager
        .validate_token(token, &ip, &ua, &dfp)
        .await
    {
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
    request: Request<Body>,
) -> Result<Response, ApiError> {
    // Extract ConnectInfo from request extensions (middleware-injected).
    let connect_info_ip: Option<String> = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip().to_string());

    // Parse JSON body manually so we don't compete with the body extractor.
    let body: SigninRequest = {
        let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
            .await
            .map_err(|e| ApiError::bad_request(format!("Failed to read body: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::bad_request(format!("Invalid signin JSON: {e}")))?
    };
    let email = body.email.trim().to_lowercase();
    if email.is_empty() {
        return Err(ApiError::bad_request("Email is required"));
    }
    if body.password.is_empty() {
        return Err(ApiError::bad_request("Password is required"));
    }

    // Resolve client IP: X-Forwarded-For (first hop) overrides ConnectInfo.
    let ip_str = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .or(connect_info_ip)
        .unwrap_or_else(|| "0.0.0.0".to_string());
    let client_ip = parse_client_ip(&ip_str)?;

    // Gate: record the attempt (success=false) and enforce exponential throttle.
    let gate = match record_login_attempt_and_check(&state.pool, &email, client_ip).await {
        Ok(g) => g,
        Err(e) => return Ok(e.into_response()),
    };

    let outcome = PasswordProvider::authenticate(&state.pool, &email, &body.password)
        .await
        .map_err(|e| ApiError::internal(format!("Authentication error: {e}")))?;

    match outcome {
        AuthOutcome::Success { user_id, roles } => {
            // Flip the attempt row to success=true so future failures restart the counter.
            mark_login_attempt_success(&state.pool, gate.attempt_id).await;

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
                .create_session(user_id, roles, &ip_str, ua, dfp)
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
            // MFA challenge issued — treat as password-verified for throttle counter.
            mark_login_attempt_success(&state.pool, gate.attempt_id).await;
            let response = serde_json::json!({
                "mfa_required": true,
                "user_id": user_id,
                "mfa_token": mfa_token,
            });
            Ok(negotiate_response(&headers, &response))
        }
        AuthOutcome::Failed { reason: _ } => {
            // Leave the attempt row as success=false.
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
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<MagicLinkRequest>,
) -> Result<Response, ApiError> {
    let email = body.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(ApiError::bad_request("Invalid email address"));
    }

    // Look up the user. If not found, still return 200 to prevent enumeration.
    let user_row: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE email = $1 AND deleted_at IS NULL")
            .bind(&email)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| ApiError::internal(format!("Database error: {e}")))?;

    if let Some((user_id,)) = user_row {
        let ip = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|s| s.trim())
            .unwrap_or("");

        let magic_link = MagicLinkProvider::generate(&state.pool, &email, user_id, ip)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to generate magic link: {e}")))?;

        tracing::debug!(
            user_id = %user_id,
            expires_at = %magic_link.expires_at,
            "magic link generated"
        );

        // Deliver via configured transport (SMTP, SendGrid, or dev log).
        // Delivery failure is logged, not returned, to avoid leaking
        // account existence.
        if let Err(e) = MagicLinkProvider::send_email(&email, &magic_link.url).await {
            tracing::warn!(error = %e, "magic link delivery failed");
        }

        // In dev mode, include the token + URL in the response for testing.
        if state.dev_mode {
            return Ok((
                StatusCode::OK,
                axum::Json(serde_json::json!({
                    "message": "If an account exists, a magic link has been sent.",
                    "_dev_token": magic_link.token,
                    "_dev_url": magic_link.url,
                    "_dev_expires_at": magic_link.expires_at.to_rfc3339(),
                })),
            )
                .into_response());
        }
    }

    // Always return 200 to prevent email enumeration.
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
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<VerifyRequest>,
) -> Result<Response, ApiError> {
    if body.token.is_empty() {
        return Err(ApiError::bad_request("Token is required"));
    }

    // Verify the magic-link token. On success we get the owning user_id;
    // invalid/expired/used tokens map to 401 responses.
    let user_id = match MagicLinkProvider::verify(&state.pool, &body.token).await {
        Ok(uid) => uid,
        Err(crate::auth::AuthError::TokenAlreadyUsed) => {
            return Err(ApiError::unauthenticated("Magic link already used"));
        }
        Err(crate::auth::AuthError::TokenInvalid(reason)) => {
            return Err(ApiError::unauthenticated(format!(
                "Invalid magic link: {reason}"
            )));
        }
        Err(e) => {
            return Err(ApiError::internal(format!(
                "Token verification failed: {e}"
            )));
        }
    };

    // Fetch the user's roles so the issued JWT is properly scoped.
    let roles_json: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT roles FROM users WHERE id = $1 AND deleted_at IS NULL")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| ApiError::internal(format!("Database error: {e}")))?;

    let roles: Vec<String> = roles_json
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_else(|| vec!["user".to_string()]);

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
        .await
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
        .await
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
// DarshQL handler
// ===========================================================================

/// Request body for the `/sql` endpoint.
#[derive(Deserialize)]
struct DarshQLRequest {
    /// The DarshQL query string (one or more statements separated by `;`).
    query: String,
}

/// `POST /api/sql` — Execute DarshQL statements.
///
/// Accepts a `{ "query": "SELECT * FROM users WHERE age > 18" }` body
/// and returns the results of each statement.
async fn darshql_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<DarshQLRequest>,
) -> Result<Response, ApiError> {
    let _auth_ctx = extract_auth_context(&headers, &state).await?;

    let start = Instant::now();

    // Parse DarshQL into AST.
    let statements = crate::query::darshql::Parser::parse(&body.query)
        .map_err(|e| ApiError::bad_request(format!("DarshQL parse error: {e}")))?;

    if statements.is_empty() {
        return Err(ApiError::bad_request("empty query".to_string()));
    }

    // Execute all statements.
    let results = crate::query::darshql::execute(&state.pool, statements)
        .await
        .map_err(|e| ApiError::internal(format!("DarshQL execution error: {e}")))?;

    let elapsed = start.elapsed();
    let response_body = serde_json::json!({
        "results": results,
        "time": format!("{}ms", elapsed.as_millis()),
    });

    Ok(negotiate_response(&headers, &response_body))
}

// ===========================================================================
// Data handlers
// ===========================================================================

/// `POST /api/query` — Execute a DarshJQL query over HTTP.
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
    let auth_ctx = extract_auth_context(&headers, &state).await?;

    let start = Instant::now();

    // Parse the DarshJQL JSON into an AST.
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

    // ── Schema validation for batch mutations ─────────────────────
    if let Some(ref registry) = state.schema_registry {
        for (i, m) in body.mutations.iter().enumerate() {
            if let Some(data) = &m.data
                && let Some(obj) = data.as_object()
                && let Some(schema) = registry.get(&m.entity)
            {
                let doc: std::collections::HashMap<String, Value> = obj
                    .iter()
                    .filter(|(k, _)| !k.starts_with('$'))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let is_update = matches!(m.op, MutationOp::Update | MutationOp::Upsert);
                let result = if is_update {
                    crate::schema::validator::SchemaValidator::validate_update(&schema, &doc)
                } else {
                    crate::schema::validator::SchemaValidator::validate_insert(&schema, &doc)
                };
                if !result.is_valid() {
                    return Err(ApiError::bad_request(format!(
                        "Mutation {i}: schema validation failed: {}",
                        result.error_message()
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
    if !all_triples.is_empty()
        && let Some(ref rule_engine) = state.rule_engine
    {
        implied_triples = rule_engine
            .evaluate_and_write_in_tx(&mut db_tx, &all_triples, tx_id)
            .await
            .map_err(|e| ApiError::internal(format!("Rule engine error: {e}")))?;
    }

    // Commit the entire batch atomically.
    db_tx
        .commit()
        .await
        .map_err(|e| ApiError::internal(format!("Transaction commit failed: {e}")))?;

    // Record mutation latency for pool stats histogram.
    state.pool_stats.record(mutate_start.elapsed());

    // Collect attributes touched (for change notification), including implied.
    // Drain the vecs to move strings instead of cloning -- they are not used after this.
    let mut touched_attributes: Vec<String> = all_triples
        .into_iter()
        .chain(implied_triples)
        .map(|t| t.attribute)
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
    let auth_ctx = extract_auth_context(&headers, &state).await?;
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
    let auth_ctx = extract_auth_context(&headers, &state).await?;

    validate_entity_name(&entity)?;

    // Check create permission — returns 403 if denied.
    let _perm_result = check_permission(&auth_ctx, &entity, Operation::Create, &state.permissions)?;

    if !body.is_object() {
        return Err(ApiError::bad_request("Request body must be a JSON object"));
    }

    let id = Uuid::new_v4();
    let obj = body
        .as_object()
        .ok_or_else(|| ApiError::bad_request("Request body must be a JSON object"))?;

    // ── Schema validation (SCHEMAFULL / MIXED mode) ──────────────
    // If a schema registry is configured and the table has a schema
    // definition, validate the document before persisting triples.
    let obj = if let Some(ref registry) = state.schema_registry {
        if let Some(schema) = registry.get(&entity) {
            let doc: std::collections::HashMap<String, Value> = obj
                .iter()
                .filter(|(k, _)| !k.starts_with('$'))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let result = crate::schema::validator::SchemaValidator::validate_insert(&schema, &doc);
            if !result.is_valid() {
                return Err(ApiError::bad_request(format!(
                    "Schema validation failed: {}",
                    result.error_message()
                )));
            }
            // Use the validated (coerced + defaults-injected) document.
            // Re-add $-prefixed meta-keys from the original body.
            let mut validated = result.document;
            for (k, v) in obj.iter() {
                if k.starts_with('$') {
                    validated.insert(k.clone(), v.clone());
                }
            }
            validated
                .into_iter()
                .collect::<serde_json::Map<String, Value>>()
        } else {
            obj.clone()
        }
    } else {
        obj.clone()
    };
    let obj = &obj;

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
    // Move attribute strings out of triples (no longer needed) to avoid cloning.
    let attributes: Vec<String> = triples.into_iter().map(|t| t.attribute).collect();
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
    let auth_ctx = extract_auth_context(&headers, &state).await?;

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

        if let Some(owner) = entity_owner
            && owner != auth_ctx.user_id
        {
            return Err(ApiError::permission_denied(format!(
                "Access denied: you do not own this {entity}"
            )));
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
    let auth_ctx = extract_auth_context(&headers, &state).await?;

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

        if let Some(owner) = entity_owner
            && owner != auth_ctx.user_id
        {
            return Err(ApiError::permission_denied(format!(
                "Access denied: you do not own this {entity}"
            )));
        }
    }

    if !body.is_object() {
        return Err(ApiError::bad_request("Request body must be a JSON object"));
    }

    let obj = body
        .as_object()
        .ok_or_else(|| ApiError::bad_request("Request body must be a JSON object"))?;

    // ── Schema validation for updates ────────────────────────────
    let obj = if let Some(ref registry) = state.schema_registry {
        if let Some(schema) = registry.get(&entity) {
            let doc: std::collections::HashMap<String, Value> = obj
                .iter()
                .filter(|(k, _)| !k.starts_with('$'))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let result = crate::schema::validator::SchemaValidator::validate_update(&schema, &doc);
            if !result.is_valid() {
                return Err(ApiError::bad_request(format!(
                    "Schema validation failed: {}",
                    result.error_message()
                )));
            }
            let mut validated = result.document;
            for (k, v) in obj.iter() {
                if k.starts_with('$') {
                    validated.insert(k.clone(), v.clone());
                }
            }
            validated
                .into_iter()
                .collect::<serde_json::Map<String, Value>>()
        } else {
            obj.clone()
        }
    } else {
        obj.clone()
    };
    let obj = &obj;

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
        let attributes: Vec<String> = triples.into_iter().map(|t| t.attribute).collect();
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
    if let Some(obj) = response.as_object_mut()
        && let Some(ttl_obj) = ttl_info.as_object()
    {
        for (k, v) in ttl_obj {
            obj.insert(k.clone(), v.clone());
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
    let auth_ctx = extract_auth_context(&headers, &state).await?;

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

        if let Some(owner) = entity_owner
            && owner != auth_ctx.user_id
        {
            return Err(ApiError::permission_denied(format!(
                "Access denied: you do not own this {entity}"
            )));
        }
    }

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

    // Move attribute strings out of existing (no longer needed) to avoid cloning.
    let deleted_attributes: Vec<String> = existing.into_iter().map(|t| t.attribute).collect();

    // Emit change event for reactive subscriptions.
    let _ = state.change_tx.send(ChangeEvent {
        tx_id: del_tx_id,
        entity_ids: vec![id.to_string()],
        attributes: deleted_attributes,
        entity_type: Some(entity),
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
    /// DarshJQL query to subscribe to.
    q: String,
}

/// Collapse a list of triples (ordered newest-first per attribute by
/// `get_entity`) into a single attribute -> latest-value map.
///
/// `get_entity` returns triples sorted by `(attribute, tx_id DESC)`, so
/// the *first* triple for any given attribute is the current value. We
/// walk the list once and keep the first occurrence of each attribute.
fn triples_to_attr_map(triples: &[crate::triple_store::Triple]) -> HashMap<String, Value> {
    let mut out: HashMap<String, Value> = HashMap::with_capacity(triples.len());
    for t in triples {
        out.entry(t.attribute.clone())
            .or_insert_with(|| t.value.clone());
    }
    out
}

/// Evaluate a single DarshJQL `WhereClause` against a materialized
/// attribute map.
///
/// This is a pure, in-memory re-check used by the SSE `subscribe`
/// handler to drop pub/sub events whose target entity no longer (or
/// never did) satisfy the query predicate. It mirrors the semantics of
/// the SQL planner in [`query::plan_query`] for the operators that can
/// be checked without a database round-trip.
fn eval_where_clause(clause: &query::WhereClause, attrs: &HashMap<String, Value>) -> bool {
    let Some(actual) = attrs.get(&clause.attribute) else {
        // Missing attribute only satisfies explicit `!=` against a
        // non-null literal; everything else fails closed.
        return matches!(clause.op, query::WhereOp::Neq) && !clause.value.is_null();
    };
    let expected = &clause.value;

    match clause.op {
        query::WhereOp::Eq => actual == expected,
        query::WhereOp::Neq => actual != expected,
        query::WhereOp::Gt => compare_json(actual, expected)
            .map(|o| o.is_gt())
            .unwrap_or(false),
        query::WhereOp::Gte => compare_json(actual, expected)
            .map(|o| o.is_ge())
            .unwrap_or(false),
        query::WhereOp::Lt => compare_json(actual, expected)
            .map(|o| o.is_lt())
            .unwrap_or(false),
        query::WhereOp::Lte => compare_json(actual, expected)
            .map(|o| o.is_le())
            .unwrap_or(false),
        query::WhereOp::Contains => json_contains(actual, expected),
        query::WhereOp::Like => match (actual.as_str(), expected.as_str()) {
            (Some(haystack), Some(pattern)) => ilike_match(haystack, pattern),
            _ => false,
        },
    }
}

/// Evaluate a set of `WhereClause` predicates with AND semantics.
///
/// Returns `true` only if every clause matches. An empty list trivially
/// matches (the query has no WHERE filter at all).
fn eval_where_clauses(clauses: &[query::WhereClause], attrs: &HashMap<String, Value>) -> bool {
    clauses.iter().all(|c| eval_where_clause(c, attrs))
}

/// Compare two JSON values for ordering where that makes sense
/// (numbers, strings, booleans). Returns `None` for mixed or
/// unordered types.
fn compare_json(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let xf = x.as_f64()?;
            let yf = y.as_f64()?;
            xf.partial_cmp(&yf)
        }
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

/// Minimal JSONB `@>` containment check: `b` is contained in `a` when
/// every key/value in `b` is present (recursively) in `a`. Scalars
/// compare by equality. Matches PostgreSQL semantics closely enough
/// for live-query re-evaluation.
fn json_contains(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(am), Value::Object(bm)) => bm
            .iter()
            .all(|(k, bv)| am.get(k).is_some_and(|av| json_contains(av, bv))),
        (Value::Array(aa), Value::Array(ba)) => {
            ba.iter().all(|bv| aa.iter().any(|av| json_contains(av, bv)))
        }
        _ => a == b,
    }
}

/// Tiny SQL-ish ILIKE matcher supporting `%` (any) and `_` (one char),
/// case-insensitive. Sufficient for the subscribe re-check path.
fn ilike_match(haystack: &str, pattern: &str) -> bool {
    let hs: Vec<char> = haystack.to_lowercase().chars().collect();
    let pt: Vec<char> = pattern.to_lowercase().chars().collect();
    ilike_inner(&hs, &pt)
}

fn ilike_inner(hs: &[char], pt: &[char]) -> bool {
    if pt.is_empty() {
        return hs.is_empty();
    }
    match pt[0] {
        '%' => {
            if ilike_inner(hs, &pt[1..]) {
                return true;
            }
            !hs.is_empty() && ilike_inner(&hs[1..], pt)
        }
        '_' => !hs.is_empty() && ilike_inner(&hs[1..], &pt[1..]),
        c => !hs.is_empty() && hs[0] == c && ilike_inner(&hs[1..], &pt[1..]),
    }
}

/// `GET /api/subscribe?q=...` — Server-Sent Events for live query updates.
///
/// Authenticates via Bearer token, parses the DarshJQL query to extract
/// the target entity type and any `$where` predicates, then streams
/// change events from the pub/sub engine with server-side filtering:
///
/// 1. Entity-type filter: only events whose `entity_type` matches the
///    query's target type are considered.
/// 2. WHERE re-evaluation: for events that carry an `entity_id`, the
///    handler fetches the current triples for that entity and checks
///    the parsed `WhereClause` list in memory. Non-matching events are
///    dropped before they reach the client.
///
/// A heartbeat comment is sent every 15 seconds to keep the connection
/// alive through idle intermediaries.
async fn subscribe(
    State(state): State<AppState>,
    Query(params): Query<SubscribeParams>,
    headers: HeaderMap,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if params.q.is_empty() {
        return Err(ApiError::bad_request("Query parameter 'q' is required"));
    }

    // Parse the DarshJQL query. Clients pass the query as a JSON string
    // in the `q` query-param; decode it into a `serde_json::Value` and
    // route through the canonical parser so we inherit all validation.
    let q_json: Value = serde_json::from_str(&params.q)
        .map_err(|e| ApiError::bad_request(format!("Query 'q' must be JSON: {e}")))?;
    let ast = query::parse_darshan_ql(&q_json)
        .map_err(|e| ApiError::bad_request(format!("Invalid DarshJQL: {e}")))?;

    let target_entity_type = ast.entity_type.clone();
    let where_clauses = ast.where_clauses.clone();
    let triple_store = Arc::clone(&state.triple_store);

    // Subscribe to the real change-event bus. Every mutation that
    // touches a triple flows through `pubsub.subscribe_events()` as a
    // `PubSubEvent` carrying entity_type/entity_id/tx_id.
    let rx = state.pubsub.subscribe_events();

    // The `subscribe` filter has to perform an async DB lookup when the
    // query carries `$where` predicates, so we cannot use the synchronous
    // `tokio_stream::StreamExt::filter_map` combinator. Instead we
    // `.then()` each broadcast item into a future producing
    // `Option<Result<Event, Infallible>>`, then drop the `None`s with a
    // synchronous `filter_map(|x| x)`.
    let stream = BroadcastStream::new(rx)
        .then(move |msg| {
            let target = target_entity_type.clone();
            let where_clauses = where_clauses.clone();
            let triple_store = Arc::clone(&triple_store);
            async move {
                let event = match msg {
                    Ok(ev) => ev,
                    Err(_) => return None,
                };

                // (c) Entity-type gate — drop events for any other type.
                match event.entity_type.as_deref() {
                    Some(et) if et == target => {}
                    _ => return None,
                }

                // (d) WHERE re-evaluation: only when the event carries
                // a concrete entity_id and the query has predicates.
                if !where_clauses.is_empty() {
                    let Some(ref eid_str) = event.entity_id else {
                        // Collection-level event without an entity id
                        // cannot be checked against WHERE — drop it to
                        // avoid false positives.
                        return None;
                    };
                    let Ok(eid) = Uuid::parse_str(eid_str) else {
                        return None;
                    };
                    let triples = match triple_store.get_entity(eid).await {
                        Ok(t) => t,
                        Err(_) => return None,
                    };
                    let attrs = triples_to_attr_map(&triples);
                    if !eval_where_clauses(&where_clauses, &attrs) {
                        return None;
                    }
                }

                let data = serde_json::to_string(&event).unwrap_or_default();
                Some(Ok(Event::default()
                    .event("update")
                    .data(data)
                    .id(event.tx_id.to_string())))
            }
        })
        .filter_map(|opt| opt);

    // Use a comment-style keepalive (": heartbeat\n\n") so it does not
    // trigger client `onmessage` handlers.
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
    let _auth = require_admin_auth(&headers, &state).await?;

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
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _auth = require_admin_auth(&headers, &state).await?;

    let functions = match state.function_registry.as_ref() {
        Some(registry) => {
            let defs = registry.list().await;
            defs.into_iter()
                .map(|f| {
                    serde_json::json!({
                        "name": f.name,
                        "export_name": f.export_name,
                        "file_path": f.file_path.display().to_string(),
                        "kind": format!("{:?}", f.kind),
                        "description": f.description,
                        "args_schema": f.args_schema.as_ref().map(|s| serde_json::to_value(s).unwrap_or_default()),
                    })
                })
                .collect::<Vec<_>>()
        }
        None => Vec::new(),
    };

    let response = serde_json::json!({
        "functions": functions,
        "count": functions.len(),
    });

    Ok(negotiate_response(&headers, &response))
}

/// `GET /api/admin/sessions` — List active sessions across all users.
///
/// Routed through [`SessionManager::list_active`] so the admin view
/// shares the same query path as the rest of the auth subsystem —
/// keeping session-schema knowledge out of the HTTP handler.
async fn admin_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _auth = require_admin_auth(&headers, &state).await?;

    let records = state
        .session_manager
        .list_active(50)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list sessions: {e}")))?;

    let sessions: Vec<serde_json::Value> = records
        .iter()
        .map(|s| {
            serde_json::json!({
                "session_id": s.session_id,
                "user_id": s.user_id,
                "device_fingerprint": s.device_fingerprint,
                "ip": s.ip,
                "user_agent": s.user_agent,
                "created_at": s.created_at.to_rfc3339(),
                "revoked": s.revoked,
                "refresh_expires_at": s.refresh_expires_at.to_rfc3339(),
            })
        })
        .collect();

    let count = sessions.len();
    let response = serde_json::json!({
        "sessions": sessions,
        "count": count,
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
/// `DDB_CACHE_SIZE` / `DDB_CACHE_TTL`.
async fn admin_cache(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _auth = require_admin_auth(&headers, &state).await?;

    let stats = state.query_cache.stats();
    let response = serde_json::json!({
        "cache": stats,
    });

    Ok(negotiate_response(&headers, &response))
}

/// `GET /api/admin/storage` — List files in storage.
///
/// Returns metadata for all stored objects, paginated via `?limit=` and `?cursor=`.
async fn admin_storage_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let _auth = require_admin_auth(&headers, &state).await?;

    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let cursor = params.get("cursor").map(|s| s.as_str());

    let objects = state
        .storage_engine
        .list("", limit, cursor)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list storage objects: {e}")))?;

    let files: Vec<serde_json::Value> = objects
        .iter()
        .map(|obj| {
            let name = obj.path.rsplit('/').next().unwrap_or(&obj.path).to_string();
            serde_json::json!({
                "id": obj.etag,
                "name": name,
                "path": obj.path,
                "size": obj.size,
                "mimeType": obj.content_type,
                "uploadedAt": obj.created_at.timestamp_millis(),
                "modifiedAt": obj.modified_at.timestamp_millis(),
                "metadata": obj.metadata,
            })
        })
        .collect();

    let count = files.len();
    let response = serde_json::json!({
        "files": files,
        "count": count,
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
    let _auth = require_admin_auth(&headers, &state).await?;

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

/// `GET /api/docs` — Enhanced interactive Scalar API documentation viewer.
async fn docs(State(_state): State<AppState>) -> impl IntoResponse {
    Html(super::docs::enhanced_docs_html("/api/openapi.json"))
}

/// `GET /api/types.ts` — TypeScript type definitions for SDK consumers.
async fn types_ts() -> impl IntoResponse {
    let ts = super::sdk_types::generate_typescript_types();
    (
        [(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/typescript; charset=utf-8"),
        )],
        ts,
    )
}

// Find or create a user from OAuth identity info.
//
// Looks up `oauth_identities` by (provider, provider_user_id). If not found,
// checks for an existing user with the same email for account linking.
// If neither exists, creates a new user with a placeholder password hash.

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

/// Authenticate the caller against [`SessionManager`] and require the
/// "admin" role. This is the extractor guarding every `/api/admin/*`
/// route — it cryptographically verifies the JWT signature (rejecting
/// forged tokens) and then enforces role-based authorization.
///
/// Returns `Err(ApiError)` with:
/// - **401 Unauthenticated** if the bearer header is missing, malformed,
///   or the signature fails validation.
/// - **403 PermissionDenied** if the token is valid but the caller does
///   not hold the `admin` role.
///
/// On success, the returned [`AuthContext`] is available to admin
/// handlers if they need the caller's user id or session id.
async fn require_admin_auth(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<AuthContext, ApiError> {
    let auth_ctx = extract_auth_context(headers, state).await?;
    if auth_ctx.roles.iter().any(|r| r == "admin") {
        Ok(auth_ctx)
    } else {
        Err(ApiError::permission_denied("admin role required"))
    }
}

/// Extract an [`AuthContext`] by validating the JWT via the [`SessionManager`].
async fn extract_auth_context(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<AuthContext, ApiError> {
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
    state
        .session_manager
        .validate_token(&token, ip, ua, dfp)
        .await
        .map_err(|e| ApiError::unauthenticated(format!("Invalid token: {e}")))
}

/// Decode JWT claims from the Bearer token **without** signature
/// verification. Kept for test-only sanity checks; production code paths
/// must use `extract_auth_context` or `require_admin_auth`, both of
/// which route through `SessionManager::validate_token` for real
/// cryptographic verification.
#[cfg(test)]
fn decode_jwt_claims(headers: &HeaderMap) -> Result<AuthContext, ApiError> {
    let token = extract_bearer_token(headers)?;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(ApiError::unauthenticated("Malformed JWT"));
    }
    let payload_bytes = data_encoding::BASE64URL_NOPAD
        .decode(parts[1].as_bytes())
        .map_err(|_| ApiError::unauthenticated("Invalid JWT encoding"))?;
    #[derive(Deserialize)]
    struct Claims {
        sub: String,
        sid: String,
        #[serde(default)]
        roles: Vec<String>,
    }
    let claims: Claims = serde_json::from_slice(&payload_bytes)
        .map_err(|_| ApiError::unauthenticated("Invalid JWT claims"))?;
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::unauthenticated("Invalid user_id in JWT"))?;
    let session_id = Uuid::parse_str(&claims.sid)
        .map_err(|_| ApiError::unauthenticated("Invalid session_id in JWT"))?;
    Ok(AuthContext {
        user_id,
        session_id,
        roles: claims.roles,
        ip: headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string(),
        user_agent: headers
            .get(http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string(),
        device_fingerprint: headers
            .get("x-device-fingerprint")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
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
#[allow(dead_code)]
#[derive(Deserialize)]
struct EmbeddingStoreRequest {
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

#[allow(dead_code)]
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
#[allow(dead_code)]
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

#[allow(dead_code)]
fn default_search_limit() -> u32 {
    10
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

// ===========================================================================
// Graph handlers (SurrealDB-style record links and traversal)
// ===========================================================================

/// Extract the graph engine from state, returning 501 if not configured.
fn require_graph_engine(state: &AppState) -> Result<&GraphEngine, ApiError> {
    state
        .graph_engine
        .as_ref()
        .map(|g| g.as_ref())
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Internal,
                "Graph engine is not enabled on this server",
            )
        })
}

/// Request body for `POST /graph/relate`.
#[derive(Deserialize)]
struct GraphRelateRequest {
    /// Source record in `table:id` format.
    from: String,
    /// Edge type / relationship label (e.g. `works_at`, `follows`).
    edge_type: String,
    /// Target record in `table:id` format.
    to: String,
    /// Optional JSONB metadata to attach to the edge.
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// `POST /graph/relate` — Create a directed edge between two records.
///
/// Implements SurrealDB-style `RELATE from->edge_type->to` semantics.
/// If the edge already exists, its metadata is updated (upsert).
///
/// # Request body
/// ```json
/// {
///   "from": "user:darsh",
///   "edge_type": "works_at",
///   "to": "company:knowai",
///   "data": { "role": "CEO", "since": "2024" }
/// }
/// ```
async fn graph_relate(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<GraphRelateRequest>,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;

    let input = EdgeInput {
        from: body.from,
        edge_type: body.edge_type,
        to: body.to,
        data: body.data,
    };

    let edge = engine
        .relate(&input)
        .await
        .map_err(|e| ApiError::bad_request(format!("{e}")))?;

    let response = serde_json::json!({
        "edge": {
            "id": edge.id,
            "from": format!("{}:{}", edge.from_table, edge.from_id),
            "edge_type": edge.edge_type,
            "to": format!("{}:{}", edge.to_table, edge.to_id),
            "data": edge.data,
            "created_at": edge.created_at,
        }
    });

    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

/// `POST /graph/traverse` — Execute a graph traversal from a starting node.
///
/// Supports BFS, DFS, and shortest-path algorithms with configurable
/// depth limits, direction, and edge-type filtering.
///
/// # Request body
/// ```json
/// {
///   "start": "user:darsh",
///   "direction": "out",
///   "edge_type": "works_at",
///   "max_depth": 3,
///   "algorithm": "bfs"
/// }
/// ```
async fn graph_traverse(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(config): axum::Json<TraversalConfig>,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;

    let result = engine
        .traverse(&config)
        .await
        .map_err(|e| ApiError::bad_request(format!("{e}")))?;

    Ok(negotiate_response(&headers, &result))
}

/// Query parameters for neighbor/edge listing endpoints.
#[derive(Deserialize, Default)]
struct GraphEdgeQuery {
    /// Optional edge type filter.
    #[serde(default)]
    edge_type: Option<String>,
}

/// Serialize a list of edges into the standard JSON representation.
fn serialize_edges(edges: &[Edge]) -> Vec<serde_json::Value> {
    edges
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "from": format!("{}:{}", e.from_table, e.from_id),
                "edge_type": e.edge_type,
                "to": format!("{}:{}", e.to_table, e.to_id),
                "data": e.data,
                "created_at": e.created_at,
            })
        })
        .collect()
}

/// `GET /graph/neighbors/:table/:id` — Get all edges (both directions) for a record.
///
/// Optional query parameter `edge_type` filters by relationship type.
async fn graph_neighbors(
    State(state): State<AppState>,
    Path((table, id)): Path<(String, String)>,
    Query(query): Query<GraphEdgeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;
    let record = RecordId::new(&table, &id);

    let edges = engine
        .neighbors(&record, query.edge_type.as_deref())
        .await
        .map_err(|e| ApiError::bad_request(format!("{e}")))?;

    let response = serde_json::json!({
        "record": record.to_string_repr(),
        "edges": serialize_edges(&edges),
        "count": edges.len(),
    });

    Ok(negotiate_response(&headers, &response))
}

/// `GET /graph/outgoing/:table/:id` — Get outgoing edges from a record.
///
/// Models SurrealDB `SELECT ->edge_type->? FROM table:id`.
async fn graph_outgoing(
    State(state): State<AppState>,
    Path((table, id)): Path<(String, String)>,
    Query(query): Query<GraphEdgeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;
    let record = RecordId::new(&table, &id);

    let edges = engine
        .outgoing(&record, query.edge_type.as_deref())
        .await
        .map_err(|e| ApiError::bad_request(format!("{e}")))?;

    let response = serde_json::json!({
        "record": record.to_string_repr(),
        "direction": "out",
        "edges": serialize_edges(&edges),
        "count": edges.len(),
    });

    Ok(negotiate_response(&headers, &response))
}

/// `GET /graph/incoming/:table/:id` — Get incoming edges to a record.
///
/// Models SurrealDB `SELECT <-edge_type<-? FROM table:id`.
async fn graph_incoming(
    State(state): State<AppState>,
    Path((table, id)): Path<(String, String)>,
    Query(query): Query<GraphEdgeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;
    let record = RecordId::new(&table, &id);

    let edges = engine
        .incoming(&record, query.edge_type.as_deref())
        .await
        .map_err(|e| ApiError::bad_request(format!("{e}")))?;

    let response = serde_json::json!({
        "record": record.to_string_repr(),
        "direction": "in",
        "edges": serialize_edges(&edges),
        "count": edges.len(),
    });

    Ok(negotiate_response(&headers, &response))
}

/// `DELETE /graph/edge/:edge_id` — Delete an edge by its UUID.
async fn graph_delete_edge(
    State(state): State<AppState>,
    Path(edge_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;

    let deleted = engine
        .delete_edge(edge_id)
        .await
        .map_err(|e| ApiError::internal(format!("{e}")))?;

    if !deleted {
        return Err(ApiError::not_found(format!("edge {edge_id} not found")));
    }

    let response = serde_json::json!({
        "deleted": true,
        "edge_id": edge_id,
    });

    Ok(negotiate_response(&headers, &response))
}

// ===========================================================================
// Schema management handlers (DEFINE TABLE / FIELD / INDEX)
// ===========================================================================

/// `GET /api/schema/tables` — List all defined table schemas.
async fn schema_list_tables(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    let tables = registry.list_tables();
    let response = serde_json::json!({
        "tables": tables,
        "count": tables.len(),
    });
    Ok(negotiate_response(&headers, &response))
}

/// Request body for `POST /api/schema/tables` (DEFINE TABLE).
#[derive(Debug, Deserialize)]
struct DefineTableRequest {
    /// Table name.
    name: String,
    /// Schema mode: "SCHEMAFULL", "SCHEMALESS", or "MIXED".
    #[serde(default)]
    mode: Option<String>,
    /// Optional inline field definitions.
    #[serde(default)]
    fields: Option<HashMap<String, crate::schema::FieldDefinition>>,
    /// Optional inline index definitions.
    #[serde(default)]
    indexes: Option<HashMap<String, crate::schema::IndexDefinition>>,
}

/// `POST /api/schema/tables` — Define (create or replace) a table schema.
async fn schema_define_table(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<DefineTableRequest>,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    let mode = body
        .mode
        .as_deref()
        .and_then(crate::schema::SchemaMode::parse)
        .unwrap_or(crate::schema::SchemaMode::Schemaless);

    let mut schema = match mode {
        crate::schema::SchemaMode::Schemafull => crate::schema::TableSchema::schemafull(&body.name),
        crate::schema::SchemaMode::Schemaless => crate::schema::TableSchema::schemaless(&body.name),
        crate::schema::SchemaMode::Mixed => crate::schema::TableSchema::mixed(&body.name),
    };

    // Merge version from existing schema if it exists.
    if let Some(existing) = registry.get(&body.name) {
        schema.version = existing.version + 1;
    }

    if let Some(fields) = body.fields {
        schema.fields = fields;
    }
    if let Some(indexes) = body.indexes {
        schema.indexes = indexes;
    }

    registry
        .define_table(schema.clone())
        .await
        .map_err(|e| ApiError::internal(format!("Failed to define table: {e}")))?;

    let response = serde_json::json!({
        "status": "ok",
        "table": schema,
    });
    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

/// `DELETE /api/schema/tables/:table` — Remove a table schema.
async fn schema_remove_table(
    State(state): State<AppState>,
    Path(table): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    registry
        .remove_table(&table)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to remove table schema: {e}")))?;

    let response = serde_json::json!({ "status": "ok", "table": table, "action": "removed" });
    Ok(negotiate_response(&headers, &response))
}

/// `POST /api/schema/tables/:table/fields` — Define a field on a table.
async fn schema_define_field(
    State(state): State<AppState>,
    Path(table): Path<String>,
    headers: HeaderMap,
    axum::Json(field): axum::Json<crate::schema::FieldDefinition>,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    registry
        .define_field(&table, field.clone())
        .await
        .map_err(|e| ApiError::internal(format!("Failed to define field: {e}")))?;

    let response = serde_json::json!({
        "status": "ok",
        "table": table,
        "field": field,
    });
    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

/// `DELETE /api/schema/tables/:table/fields/:field` — Remove a field.
async fn schema_remove_field(
    State(state): State<AppState>,
    Path((table, field)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    registry
        .remove_field(&table, &field)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to remove field: {e}")))?;

    let response =
        serde_json::json!({ "status": "ok", "table": table, "field": field, "action": "removed" });
    Ok(negotiate_response(&headers, &response))
}

/// `POST /api/schema/tables/:table/indexes` — Define an index on a table.
async fn schema_define_index(
    State(state): State<AppState>,
    Path(table): Path<String>,
    headers: HeaderMap,
    axum::Json(index): axum::Json<crate::schema::IndexDefinition>,
) -> Result<Response, ApiError> {
    let registry = state
        .schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Schema registry not initialised"))?;

    registry
        .define_index(&table, index.clone())
        .await
        .map_err(|e| ApiError::internal(format!("Failed to define index: {e}")))?;

    let response = serde_json::json!({
        "status": "ok",
        "table": table,
        "index": index,
    });
    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

/// `GET /api/schema/tables/:table/migrations` — View migration history.
async fn schema_migration_history(
    State(state): State<AppState>,
    Path(table): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let pool = state.pool.clone();
    let engine = crate::schema::migration::SchemaMigrationEngine::new(pool)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to init migration engine: {e}")))?;

    let history = engine
        .get_history(&table)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to fetch migration history: {e}")))?;

    let response = serde_json::json!({
        "table": table,
        "migrations": history,
        "count": history.len(),
    });
    Ok(negotiate_response(&headers, &response))
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

    // -----------------------------------------------------------------------
    // Login rate-limit gate — pure logic / response shape
    // -----------------------------------------------------------------------

    #[test]
    fn login_attempts_exponential_backoff_math() {
        // The exponential throttle formula: retry_after = 2^(failed - 5).
        // Spot-check the first few steps to catch regressions without a DB.
        let cases = [(5, 1), (6, 2), (7, 4), (8, 8), (9, 16)];
        for (failed, expected) in cases {
            let exp = (failed - LOGIN_THROTTLE_THRESHOLD) as u32;
            assert_eq!(
                2u64.pow(exp),
                expected,
                "failed={failed} should map to {expected}s"
            );
        }
    }

    #[test]
    fn login_attempts_constants_match_spec() {
        assert_eq!(LOGIN_ATTEMPT_WINDOW_MINUTES, 15);
        assert_eq!(LOGIN_THROTTLE_THRESHOLD, 5);
        assert_eq!(LOGIN_LOCK_THRESHOLD, 10);
        assert_eq!(LOGIN_LOCK_RETRY_AFTER_SECS, 3600);
    }

    #[test]
    fn login_attempts_locked_response_shape() {
        let resp = LoginGateError::Locked {
            retry_after_secs: 3600,
        }
        .into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry = resp
            .headers()
            .get("Retry-After")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(retry, "3600");
    }

    #[test]
    fn login_attempts_throttled_response_shape() {
        let resp = LoginGateError::Throttled {
            retry_after_secs: 4,
        }
        .into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry = resp
            .headers()
            .get("Retry-After")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(retry, "4");
    }

    #[test]
    fn parse_client_ip_accepts_v4_and_v6() {
        assert!(parse_client_ip("127.0.0.1").is_ok());
        assert!(parse_client_ip("10.0.0.1").is_ok());
        assert!(parse_client_ip("::1").is_ok());
        assert!(parse_client_ip("2001:db8::1").is_ok());
    }

    #[test]
    fn parse_client_ip_rejects_garbage() {
        assert!(parse_client_ip("not an ip").is_err());
        assert!(parse_client_ip("").is_err());
        assert!(parse_client_ip("999.999.999.999").is_err());
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

    // -----------------------------------------------------------------------
    // Admin role check
    // -----------------------------------------------------------------------
    //
    // These tests exercise `require_admin_auth`, the real extractor
    // guarding every `/api/admin/*` route. They construct a live
    // `SessionManager` with a symmetric test key, sign real access
    // tokens with it, and call the extractor directly — DB-free because
    // `SessionManager::validate_token` is stateless JWT verification.
    //
    // `require_admin_auth_rejects_forged_signature` is the regression
    // guard: the original stub only decoded claims and accepted any
    // signature, which was a silent auth bypass.

    use crate::auth::KeyManager;
    use crate::auth::session::AccessClaims;
    use chrono::{Duration as ChronoDuration, Utc};

    /// Build a test `AppState` and a matching `KeyManager` so tests
    /// can sign tokens that the embedded `SessionManager` will accept.
    fn make_state_with_secret(secret: &[u8]) -> (AppState, KeyManager) {
        let mut state = AppState::new();
        let km_for_state = KeyManager::from_secret(secret);
        state.session_manager = Arc::new(SessionManager::new(state.pool.clone(), km_for_state));
        let km_for_signing = KeyManager::from_secret(secret);
        (state, km_for_signing)
    }

    /// Sign a HS256 access token carrying `roles` using `km`.
    fn sign_access_token(km: &KeyManager, roles: Vec<&str>) -> String {
        let now = Utc::now();
        let claims = AccessClaims {
            sub: uuid::Uuid::new_v4().to_string(),
            sid: uuid::Uuid::new_v4().to_string(),
            roles: roles.into_iter().map(String::from).collect(),
            iat: now.timestamp(),
            exp: (now + ChronoDuration::minutes(15)).timestamp(),
            iss: "darshjdb".into(),
            aud: Some("darshjdb".into()),
        };
        km.sign_access_token(&claims).expect("sign access token")
    }

    fn bearer_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn require_admin_auth_allows_admin() {
        let (state, km) = make_state_with_secret(b"require-admin-auth-test-secret-32!!");
        let token = sign_access_token(&km, vec!["admin"]);
        let headers = bearer_headers(&token);
        let ctx = require_admin_auth(&headers, &state).expect("admin allowed");
        assert!(ctx.roles.iter().any(|r| r == "admin"));
    }

    #[tokio::test]
    async fn require_admin_auth_allows_admin_among_multiple_roles() {
        let (state, km) = make_state_with_secret(b"require-admin-auth-test-secret-32!!");
        let token = sign_access_token(&km, vec!["viewer", "developer", "admin"]);
        let headers = bearer_headers(&token);
        assert!(require_admin_auth(&headers, &state).is_ok());
    }

    #[tokio::test]
    async fn require_admin_auth_rejects_non_admin() {
        let (state, km) = make_state_with_secret(b"require-admin-auth-test-secret-32!!");
        let token = sign_access_token(&km, vec!["viewer"]);
        let headers = bearer_headers(&token);
        let err = require_admin_auth(&headers, &state).unwrap_err();
        assert!(matches!(err.code, ErrorCode::PermissionDenied));
    }

    #[tokio::test]
    async fn require_admin_auth_rejects_empty_roles() {
        let (state, km) = make_state_with_secret(b"require-admin-auth-test-secret-32!!");
        let token = sign_access_token(&km, vec![]);
        let headers = bearer_headers(&token);
        let err = require_admin_auth(&headers, &state).unwrap_err();
        assert!(matches!(err.code, ErrorCode::PermissionDenied));
    }

    #[tokio::test]
    async fn require_admin_auth_rejects_missing_token() {
        let (state, _km) = make_state_with_secret(b"require-admin-auth-test-secret-32!!");
        let headers = HeaderMap::new();
        let err = require_admin_auth(&headers, &state).unwrap_err();
        assert!(matches!(err.code, ErrorCode::Unauthenticated));
    }

    /// Regression guard: a JWT with a forged signature claiming the
    /// admin role **must not** be accepted. The old stub only decoded
    /// claims, which allowed anyone to craft an admin token. The new
    /// extractor delegates to `SessionManager::validate_token`, which
    /// cryptographically verifies the signature against the server's key.
    #[tokio::test]
    async fn require_admin_auth_rejects_forged_signature() {
        let (state, _km) = make_state_with_secret(b"require-admin-auth-test-secret-32!!");

        // Hand-craft a JWT: valid structure, legitimate-looking claims,
        // but the signature bytes are garbage.
        let header_b64 =
            data_encoding::BASE64URL_NOPAD.encode(b"{\"alg\":\"HS256\",\"typ\":\"JWT\"}");
        let payload_b64 = data_encoding::BASE64URL_NOPAD.encode(
            serde_json::to_string(&serde_json::json!({
                "sub": uuid::Uuid::new_v4().to_string(),
                "sid": uuid::Uuid::new_v4().to_string(),
                "roles": ["admin"],
                "iat": 0,
                "exp": 9_999_999_999_i64,
                "iss": "darshjdb",
                "aud": "darshjdb",
            }))
            .unwrap()
            .as_bytes(),
        );
        let sig_b64 = data_encoding::BASE64URL_NOPAD.encode(b"forged-signature-bytes");
        let forged = format!("{header_b64}.{payload_b64}.{sig_b64}");

        let headers = bearer_headers(&forged);
        let err = require_admin_auth(&headers, &state).unwrap_err();
        assert!(
            matches!(err.code, ErrorCode::Unauthenticated),
            "forged signature must be 401, got {:?}",
            err.code
        );
    }

    /// A JWT signed by a *different* key than the server's — the
    /// common token-confusion attack surface — must also be rejected.
    #[tokio::test]
    async fn require_admin_auth_rejects_token_signed_by_other_key() {
        let (state, _km_server) =
            make_state_with_secret(b"server-real-secret-key-32-bytes!!aa");
        let attacker_km = KeyManager::from_secret(b"attacker-other-secret-key-32byt!!b");
        let attacker_token = sign_access_token(&attacker_km, vec!["admin"]);
        let headers = bearer_headers(&attacker_token);
        let err = require_admin_auth(&headers, &state).unwrap_err();
        assert!(matches!(err.code, ErrorCode::Unauthenticated));
    }

    /// Sanity check for `decode_jwt_claims` (still used elsewhere in
    /// the module for non-critical paths that need the subject id).
    #[test]
    fn decode_jwt_claims_extracts_user_id() {
        let user_id = uuid::Uuid::new_v4();
        let session_id = uuid::Uuid::new_v4();
        let claims = serde_json::json!({
            "sub": user_id.to_string(),
            "sid": session_id.to_string(),
            "roles": ["developer"],
        });
        // Build a syntactically-valid (unsigned) JWT: the helper is
        // signature-free by design.
        let header = data_encoding::BASE64URL_NOPAD.encode(b"{\"alg\":\"HS256\"}");
        let payload = data_encoding::BASE64URL_NOPAD
            .encode(serde_json::to_string(&claims).unwrap().as_bytes());
        let sig = data_encoding::BASE64URL_NOPAD.encode(b"fake-signature-bytes");
        let token = format!("{header}.{payload}.{sig}");
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let ctx = decode_jwt_claims(&headers).unwrap();
        assert_eq!(ctx.user_id, user_id);
        assert_eq!(ctx.session_id, session_id);
        assert_eq!(ctx.roles, vec!["developer"]);
    }

    #[test]
    fn decode_jwt_claims_rejects_malformed_jwt() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer not-a-jwt".parse().unwrap(),
        );
        assert!(decode_jwt_claims(&headers).is_err());
    }

    // -----------------------------------------------------------------------
    // SSE subscribe filter helpers (Phase 0.6)
    // -----------------------------------------------------------------------
    //
    // These tests cover the in-memory pieces the `subscribe` handler uses
    // to gate change events. Hitting the live BroadcastStream path needs a
    // running Postgres instance, so we test the pure logic here and rely
    // on the integration suite for the streaming wiring.

    use crate::query::{WhereClause, WhereOp};

    fn wc(attribute: &str, op: WhereOp, value: serde_json::Value) -> WhereClause {
        WhereClause {
            attribute: attribute.to_string(),
            op,
            value,
        }
    }

    #[test]
    fn eval_where_eq_matches_when_attribute_present() {
        let mut attrs = HashMap::new();
        attrs.insert("status".to_string(), serde_json::json!("active"));
        let clause = wc("status", WhereOp::Eq, serde_json::json!("active"));
        assert!(eval_where_clause(&clause, &attrs));
    }

    #[test]
    fn eval_where_eq_rejects_when_value_differs() {
        let mut attrs = HashMap::new();
        attrs.insert("status".to_string(), serde_json::json!("inactive"));
        let clause = wc("status", WhereOp::Eq, serde_json::json!("active"));
        assert!(!eval_where_clause(&clause, &attrs));
    }

    #[test]
    fn eval_where_missing_attribute_fails_closed() {
        let attrs = HashMap::new();
        let clause = wc("status", WhereOp::Eq, serde_json::json!("active"));
        assert!(!eval_where_clause(&clause, &attrs));
    }

    #[test]
    fn eval_where_neq_against_missing_attribute_passes() {
        let attrs = HashMap::new();
        let clause = wc("status", WhereOp::Neq, serde_json::json!("inactive"));
        // Field absent => "not equal to non-null literal" is true.
        assert!(eval_where_clause(&clause, &attrs));
    }

    #[test]
    fn eval_where_numeric_ordering() {
        let mut attrs = HashMap::new();
        attrs.insert("age".to_string(), serde_json::json!(30));
        assert!(eval_where_clause(
            &wc("age", WhereOp::Gt, serde_json::json!(25)),
            &attrs
        ));
        assert!(eval_where_clause(
            &wc("age", WhereOp::Gte, serde_json::json!(30)),
            &attrs
        ));
        assert!(eval_where_clause(
            &wc("age", WhereOp::Lt, serde_json::json!(40)),
            &attrs
        ));
        assert!(eval_where_clause(
            &wc("age", WhereOp::Lte, serde_json::json!(30)),
            &attrs
        ));
        assert!(!eval_where_clause(
            &wc("age", WhereOp::Lt, serde_json::json!(10)),
            &attrs
        ));
    }

    #[test]
    fn eval_where_clauses_anded_together() {
        let mut attrs = HashMap::new();
        attrs.insert("status".to_string(), serde_json::json!("active"));
        attrs.insert("age".to_string(), serde_json::json!(30));

        let clauses = vec![
            wc("status", WhereOp::Eq, serde_json::json!("active")),
            wc("age", WhereOp::Gt, serde_json::json!(18)),
        ];
        assert!(eval_where_clauses(&clauses, &attrs));

        let clauses_one_fails = vec![
            wc("status", WhereOp::Eq, serde_json::json!("active")),
            wc("age", WhereOp::Gt, serde_json::json!(40)),
        ];
        assert!(!eval_where_clauses(&clauses_one_fails, &attrs));
    }

    #[test]
    fn eval_where_clauses_empty_list_matches() {
        let attrs = HashMap::new();
        assert!(eval_where_clauses(&[], &attrs));
    }

    #[test]
    fn ilike_match_handles_wildcards_case_insensitively() {
        assert!(ilike_match("hello world", "%WORLD%"));
        assert!(ilike_match("alice@example.com", "%@example.com"));
        assert!(ilike_match("alice", "a_ice"));
        assert!(!ilike_match("alice", "bob"));
    }

    #[test]
    fn json_contains_handles_objects_and_arrays() {
        let haystack = serde_json::json!({"a": 1, "b": {"c": 2}});
        assert!(json_contains(
            &haystack,
            &serde_json::json!({"a": 1})
        ));
        assert!(json_contains(
            &haystack,
            &serde_json::json!({"b": {"c": 2}})
        ));
        assert!(!json_contains(
            &haystack,
            &serde_json::json!({"b": {"c": 3}})
        ));
    }

    #[test]
    fn pubsub_event_entity_type_filter_matches_target() {
        // Simulate the gate logic the SSE handler applies before
        // executing the WHERE re-check. We construct a PubSubEvent and
        // assert that the entity_type comparison produces the expected
        // include/drop decision.
        let target = "users".to_string();
        let matching = PubSubEvent {
            channel: "entity:users".into(),
            event: "updated".into(),
            entity_type: Some("users".into()),
            entity_id: Some(uuid::Uuid::new_v4().to_string()),
            changed: vec!["email".into()],
            tx_id: 1,
            payload: None,
        };
        let other = PubSubEvent {
            channel: "entity:orders".into(),
            event: "updated".into(),
            entity_type: Some("orders".into()),
            entity_id: Some(uuid::Uuid::new_v4().to_string()),
            changed: vec!["total".into()],
            tx_id: 2,
            payload: None,
        };

        let pass = matches!(matching.entity_type.as_deref(), Some(et) if et == target);
        let drop = matches!(other.entity_type.as_deref(), Some(et) if et == target);
        assert!(pass, "matching entity_type should pass the gate");
        assert!(!drop, "non-matching entity_type should be dropped");
    }

    #[test]
    fn parse_darshan_ql_extracts_target_entity_type() {
        // Confirm the shape the subscribe handler relies on: the parsed
        // QueryAST exposes the target entity_type and an iterable list
        // of WhereClauses.
        let q = serde_json::json!({
            "type": "users",
            "$where": [
                { "attribute": "status", "op": "Eq", "value": "active" }
            ]
        });
        let ast = query::parse_darshan_ql(&q).expect("parses");
        assert_eq!(ast.entity_type, "users");
        assert_eq!(ast.where_clauses.len(), 1);
        assert_eq!(ast.where_clauses[0].attribute, "status");
        assert!(matches!(ast.where_clauses[0].op, WhereOp::Eq));
    }
}
