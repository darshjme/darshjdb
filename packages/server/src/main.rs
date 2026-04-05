//! DarshanDB server binary entry point.
//!
//! Initializes all subsystems (triple store, auth, sync, functions, storage),
//! builds the Axum router with middleware, and starts the HTTP server with
//! graceful shutdown support.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use darshandb_server::api::rest::{AppState, build_router};
use darshandb_server::api::ws::{WsState, ws_routes};
use darshandb_server::auth::middleware::RateLimiter;
use darshandb_server::auth::session::{KeyManager, SessionManager};
use darshandb_server::error::Result;
use darshandb_server::sync::presence::PresenceManager;
use darshandb_server::sync::registry::SubscriptionRegistry;
use darshandb_server::sync::session::SessionManager as SyncSessionManager;

use sqlx::postgres::PgPoolOptions;
use tokio::signal;
use tower_http::cors::{Any, CorsLayer};

/// Default server port when `DARSHAN_PORT` is not set.
const DEFAULT_PORT: u16 = 7700;

/// Maximum database connections in the pool.
const DEFAULT_MAX_CONNECTIONS: u32 = 20;

/// Rate limiter cleanup interval.
const RATE_LIMIT_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() -> Result<()> {
    // ── Tracing / Logging ──────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("DarshanDB server starting");

    // ── Configuration from environment ─────────────────────────────
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        tracing::warn!("DATABASE_URL not set, using default localhost connection");
        "postgres://darshan:darshan@localhost:5432/darshandb".to_string()
    });

    let port: u16 = std::env::var("DARSHAN_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let max_connections: u32 = std::env::var("DARSHAN_MAX_CONNECTIONS")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_MAX_CONNECTIONS);

    let jwt_secret = std::env::var("DARSHAN_JWT_SECRET").ok();
    let jwt_private_key_path = std::env::var("DARSHAN_JWT_PRIVATE_KEY").ok();
    let jwt_public_key_path = std::env::var("DARSHAN_JWT_PUBLIC_KEY").ok();

    // ── Database Pool ──────────────────────────────────────────────
    tracing::info!(database_url = %mask_url(&database_url), "connecting to database");

    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await
        .map_err(|e| {
            tracing::error!("Failed to connect to database: {e}");
            darshandb_server::error::DarshanError::Database(e)
        })?;

    tracing::info!("database connection pool established");

    // ── Triple Store ───────────────────────────────────────────────
    let triple_store = darshandb_server::triple_store::PgTripleStore::new(pool.clone()).await?;
    tracing::info!("triple store initialized (schema ensured)");

    // ── Auth Schema (users + sessions tables) ─────────────────────
    darshandb_server::api::rest::ensure_auth_schema(&pool)
        .await
        .map_err(|e| {
            tracing::error!("Failed to ensure auth schema: {e}");
            darshandb_server::error::DarshanError::Database(e)
        })?;
    tracing::info!("auth schema ensured (users + sessions tables)");

    // ── Auth Engine ────────────────────────────────────────────────
    let key_manager = match (&jwt_private_key_path, &jwt_public_key_path) {
        (Some(priv_path), Some(pub_path)) => {
            // Production: RS256 with PEM key files.
            let priv_pem = std::fs::read(priv_path).map_err(|e| {
                darshandb_server::error::DarshanError::Internal(format!(
                    "failed to read JWT private key at {priv_path}: {e}"
                ))
            })?;
            let pub_pem = std::fs::read(pub_path).map_err(|e| {
                darshandb_server::error::DarshanError::Internal(format!(
                    "failed to read JWT public key at {pub_path}: {e}"
                ))
            })?;
            KeyManager::new(&priv_pem, &pub_pem, "ddb-key-1".into(), None, None).map_err(|e| {
                darshandb_server::error::DarshanError::Internal(format!(
                    "failed to initialize RSA key manager: {e}"
                ))
            })?
        }
        _ => {
            // Development: HS256 with shared secret or ephemeral keys.
            match jwt_secret {
                Some(secret) => {
                    tracing::info!("using HMAC (HS256) JWT signing with DARSHAN_JWT_SECRET");
                    KeyManager::from_secret(secret.as_bytes())
                }
                None => {
                    tracing::warn!(
                        "no JWT keys configured, generating ephemeral keys (not for production)"
                    );
                    KeyManager::generate()
                }
            }
        }
    };

    let session_manager = Arc::new(SessionManager::new(pool.clone(), key_manager));
    let rate_limiter = Arc::new(RateLimiter::new());

    // Spawn background rate-limiter cleanup.
    let _rate_limit_cleanup = rate_limiter.spawn_cleanup_task(RATE_LIMIT_CLEANUP_INTERVAL);

    tracing::info!("auth engine initialized");

    // ── Sync Engine ────────────────────────────────────────────────
    let sync_sessions = Arc::new(SyncSessionManager::new());
    let subscription_registry = Arc::new(SubscriptionRegistry::new());
    let presence_manager = Arc::new(PresenceManager::new());
    let (diff_tx, _diff_rx) = tokio::sync::mpsc::channel(1024);

    let ws_state = WsState {
        sessions: sync_sessions,
        registry: subscription_registry,
        presence: presence_manager,
        diff_tx,
    };

    tracing::info!("sync engine initialized");

    // ── REST API State ─────────────────────────────────────────────
    let triple_store_arc = Arc::new(triple_store);
    let app_state = AppState::with_pool(
        pool.clone(),
        triple_store_arc.clone(),
        session_manager.clone(),
    );

    // ── CORS Layer ─────────────────────────────────────────────────
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers(Any)
        .max_age(Duration::from_secs(86400));

    // ── Count existing triples for startup log ──────────────────────
    let triple_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM triples")
        .fetch_one(&pool)
        .await
        .unwrap_or((0,));
    tracing::info!(triples = triple_count.0, "triple store stats");

    // ── Router Assembly ────────────────────────────────────────────
    let api_router = build_router(app_state);
    let health_pool = pool.clone();

    let app = axum::Router::new()
        // REST API routes under /api
        .nest("/api", api_router)
        // WebSocket route at /ws
        .merge(ws_routes(ws_state))
        // Health check at root
        .route(
            "/health",
            axum::routing::get(move || health_check(health_pool.clone())),
        )
        // CORS (outermost layer, runs first)
        .layer(cors);

    // ── Start Server ───────────────────────────────────────────────
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| darshandb_server::error::DarshanError::Internal(format!("bind error: {e}")))?;

    tracing::info!(%addr, "DarshanDB server listening");
    tracing::info!("  REST API:  http://{addr}/api");
    tracing::info!("  WebSocket: ws://{addr}/ws");
    tracing::info!("  Health:    http://{addr}/health");
    tracing::info!("  API Docs:  http://{addr}/api/docs");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .map_err(|e| darshandb_server::error::DarshanError::Internal(format!("server error: {e}")))?;

    tracing::info!("DarshanDB server shut down gracefully");

    Ok(())
}

/// Health check endpoint. Returns 200 with server status and pool info.
async fn health_check(pool: sqlx::PgPool) -> axum::Json<serde_json::Value> {
    let pool_size = pool.size();
    let idle = pool.num_idle();
    let triple_count: i64 = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM triples")
        .fetch_one(&pool)
        .await
        .map(|r| r.0)
        .unwrap_or(-1);

    axum::Json(serde_json::json!({
        "status": "ok",
        "service": "darshandb",
        "version": env!("CARGO_PKG_VERSION"),
        "pool": {
            "size": pool_size,
            "idle": idle,
        },
        "triples": triple_count,
    }))
}

/// Wait for SIGINT (Ctrl+C) or SIGTERM for graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received Ctrl+C, initiating shutdown"),
        _ = terminate => tracing::info!("received SIGTERM, initiating shutdown"),
    }
}

/// Mask the password in a database URL for safe logging.
fn mask_url(url: &str) -> String {
    if let Ok(mut parsed) = url::Url::parse(url) {
        if parsed.password().is_some() {
            let _ = parsed.set_password(Some("***"));
        }
        parsed.to_string()
    } else {
        "[invalid url]".to_string()
    }
}
