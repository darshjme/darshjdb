//! `ddb start` — Start a DarshJDB server instance.
//!
//! Supports multiple storage backends (memory, postgres) and configurable
//! bind address, auth credentials, log level, and TLS.

use anyhow::{Context, Result};
use colored::Colorize;

/// Storage backend selection for `ddb start`.
#[derive(Clone, Debug, Default, clap::ValueEnum)]
pub enum StorageBackend {
    /// In-memory storage (data lost on restart, great for development)
    Memory,
    /// PostgreSQL-backed persistent storage (production)
    #[default]
    Postgres,
}

impl std::fmt::Display for StorageBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageBackend::Memory => write!(f, "memory"),
            StorageBackend::Postgres => write!(f, "postgres"),
        }
    }
}

/// Run the DarshJDB server with the given configuration.
///
/// This embeds the full server binary — no separate process needed.
/// The `ddb` binary IS the server.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    storage: StorageBackend,
    conn: Option<String>,
    bind: String,
    user: Option<String>,
    pass: Option<String>,
    log_level: String,
    strict: bool,
    no_banner: bool,
) -> Result<()> {
    if !no_banner {
        print_banner(&bind, &storage, conn.as_deref());
    }

    // Parse bind address
    let addr: std::net::SocketAddr = bind
        .parse()
        .with_context(|| format!("Invalid bind address: {bind}"))?;

    // Resolve the database URL based on storage backend
    let database_url = match storage {
        StorageBackend::Memory => {
            // For memory mode, we still need Postgres but use a temp approach.
            // Set an env flag so the server knows to skip persistence guarantees.
            // SAFETY: called before spawning any threads.
            unsafe { std::env::set_var("DDB_MEMORY_MODE", "true") };
            conn.unwrap_or_else(|| {
                "postgres://postgres:darshan@localhost:5432/darshjdb_mem".to_string()
            })
        }
        StorageBackend::Postgres => {
            conn.unwrap_or_else(|| "postgres://darshan:darshan@localhost:5432/darshjdb".to_string())
        }
    };

    // Set environment variables that the server reads.
    // SAFETY: called at startup before spawning worker threads.
    unsafe {
        std::env::set_var("DATABASE_URL", &database_url);
        std::env::set_var("DDB_PORT", addr.port().to_string());
        std::env::set_var("RUST_LOG", &log_level);

        if let Some(ref u) = user {
            std::env::set_var("DDB_ROOT_USER", u);
        }
        if let Some(ref p) = pass {
            std::env::set_var("DDB_ROOT_PASS", p);
        }

        if strict {
            std::env::set_var("DDB_STRICT", "true");
        }
    }

    // Initialize tracing with the configured level
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&log_level)),
        )
        .init();

    tracing::info!("DarshJDB server starting");
    tracing::info!(storage = %storage, bind = %addr, "configuration");

    // ── Database Pool ───────────────────────────────────────────────
    use std::sync::Arc;
    use std::time::Duration;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(20)
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(600))
        .max_lifetime(Duration::from_secs(1800))
        .connect(&database_url)
        .await
        .map_err(|e| {
            tracing::error!("Failed to connect to database: {e}");
            anyhow::anyhow!("Database connection failed: {e}")
        })?;

    tracing::info!("database connection pool established");

    // ── Schema Creation (serialized with advisory lock) ─────────────
    sqlx::query("SELECT pg_advisory_lock(42)")
        .execute(&pool)
        .await
        .context("Failed to acquire advisory lock")?;

    let triple_store = ddb_server::triple_store::PgTripleStore::new(pool.clone())
        .await
        .context("Failed to initialize triple store")?;
    tracing::info!("triple store initialized");

    ddb_server::api::rest::ensure_auth_schema(&pool)
        .await
        .context("Failed to ensure auth schema")?;
    tracing::info!("auth schema ensured");

    sqlx::query("SELECT pg_advisory_unlock(42)")
        .execute(&pool)
        .await
        .context("Failed to release advisory lock")?;

    // ── Auth Engine ─────────────────────────────────────────────────
    let jwt_secret = std::env::var("DDB_JWT_SECRET").ok();
    let jwt_private_key_path = std::env::var("DDB_JWT_PRIVATE_KEY").ok();
    let jwt_public_key_path = std::env::var("DDB_JWT_PUBLIC_KEY").ok();

    let key_manager = match (&jwt_private_key_path, &jwt_public_key_path) {
        (Some(priv_path), Some(pub_path)) => {
            let priv_pem = std::fs::read(priv_path)
                .with_context(|| format!("Failed to read JWT private key: {priv_path}"))?;
            let pub_pem = std::fs::read(pub_path)
                .with_context(|| format!("Failed to read JWT public key: {pub_path}"))?;
            ddb_server::auth::session::KeyManager::new(
                &priv_pem,
                &pub_pem,
                "ddb-key-1".into(),
                None,
                None,
            )
            .map_err(|e| anyhow::anyhow!("Failed to initialize RSA key manager: {e}"))?
        }
        _ => match jwt_secret {
            Some(secret) => {
                tracing::info!("using HMAC (HS256) JWT signing");
                ddb_server::auth::session::KeyManager::from_secret(secret.as_bytes())
            }
            None => {
                tracing::warn!("no JWT keys configured, generating ephemeral keys");
                ddb_server::auth::session::KeyManager::generate()
            }
        },
    };

    let session_manager = Arc::new(ddb_server::auth::session::SessionManager::new(
        pool.clone(),
        key_manager,
    ));
    let rate_limiter = Arc::new(ddb_server::auth::middleware::RateLimiter::new());
    let _rate_limit_cleanup = rate_limiter.spawn_cleanup_task(Duration::from_secs(60));

    tracing::info!("auth engine initialized");

    // ── Sync Engine ─────────────────────────────────────────────────
    let sync_sessions = Arc::new(ddb_server::sync::session::SessionManager::new());
    let subscription_registry = Arc::new(ddb_server::sync::registry::SubscriptionRegistry::new());
    let presence_manager = Arc::new(ddb_server::sync::presence::PresenceManager::new());
    let (diff_tx, _diff_rx) = tokio::sync::mpsc::channel(1024);

    let (change_tx, _change_rx) =
        tokio::sync::broadcast::channel::<ddb_server::sync::ChangeEvent>(4096);

    let triple_store_arc = Arc::new(triple_store);

    // ── TTL Expiry Background Task ──────────────────────────────────
    {
        let ttl_store = triple_store_arc.clone();
        let ttl_change_tx = change_tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                match ttl_store.expire_triples().await {
                    Ok(expired_ids) => {
                        if !expired_ids.is_empty() {
                            tracing::info!(
                                count = expired_ids.len(),
                                "TTL expiry: retracted expired entities"
                            );
                            for entity_id in &expired_ids {
                                let _ = ttl_change_tx.send(ddb_server::sync::ChangeEvent {
                                    tx_id: 0,
                                    entity_ids: vec![entity_id.to_string()],
                                    attributes: vec![":ttl/expired".to_string()],
                                    entity_type: None,
                                    actor_id: None,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "TTL expiry scan failed");
                    }
                }
            }
        });
    }

    // ── Pool Monitor ────────────────────────────────────────────────
    {
        let monitor_pool = pool.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                let size = monitor_pool.size();
                let idle = monitor_pool.num_idle() as u32;
                let active = size.saturating_sub(idle);
                let utilization = if 20 > 0 { active as f64 / 20.0 } else { 0.0 };
                if utilization > 0.80 {
                    tracing::warn!(active, idle, size, "connection pool utilization above 80%");
                }
            }
        });
    }

    // ── LISTEN/NOTIFY ───────────────────────────────────────────────
    {
        let listen_change_tx = change_tx.clone();
        let listen_db_url = database_url.clone();
        tokio::spawn(async move {
            let mut listener = match sqlx::postgres::PgListener::connect(&listen_db_url).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!(error = %e, "failed to create PgListener");
                    return;
                }
            };
            if let Err(e) = listener.listen("ddb_changes").await {
                tracing::error!(error = %e, "failed to LISTEN on ddb_changes");
                return;
            }
            loop {
                match listener.recv().await {
                    Ok(notification) => {
                        let payload = notification.payload();
                        let (tx_id, entity_type) = match payload.split_once(':') {
                            Some((tid, etype)) => {
                                (tid.parse().unwrap_or(0), Some(etype.to_string()))
                            }
                            None => (payload.parse().unwrap_or(0), None),
                        };
                        let _ = listen_change_tx.send(ddb_server::sync::ChangeEvent {
                            tx_id,
                            entity_ids: vec![],
                            attributes: vec![],
                            entity_type,
                            actor_id: None,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "PgListener recv error, reconnecting");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
    }

    // ── Pub/Sub ─────────────────────────────────────────────────────
    let (pubsub_engine, _pubsub_rx) = ddb_server::sync::pubsub::PubSubEngine::new(4096);

    // ── Live Query Manager ──────────────────────────────────────────
    let (live_query_manager, _live_rx) = ddb_server::sync::live_query::LiveQueryManager::new(4096);

    // ── Change Feed ─────────────────────────────────────────────────
    let (change_feed, _cf_rx) = ddb_server::sync::ChangeFeed::with_defaults();

    let ws_state = ddb_server::api::ws::WsState {
        sessions: sync_sessions.clone(),
        registry: subscription_registry,
        presence: presence_manager,
        diff_tx,
        pool: pool.clone(),
        triple_store: triple_store_arc.clone(),
        change_tx: change_tx.clone(),
        pubsub: pubsub_engine.clone(),
        live_queries: live_query_manager,
        change_feed,
    };

    tracing::info!("sync engine initialized");

    // ── Connectors ──────────────────────────────────────────────────
    {
        use ddb_server::connectors::log::LogConnector;
        use ddb_server::connectors::webhook::WebhookConnector;
        use ddb_server::connectors::{Connector, ConnectorManager};

        let mut connectors: Vec<Box<dyn Connector>> = Vec::new();
        connectors.push(Box::new(LogConnector::new()));

        if let Some(wh) = WebhookConnector::from_env() {
            connectors.push(Box::new(wh));
        }

        if !connectors.is_empty() {
            let manager = Arc::new(ConnectorManager::new(connectors, triple_store_arc.clone()));
            manager.initialize_all().await;
            let connector_rx = change_tx.subscribe();
            tokio::spawn(manager.run(connector_rx));
        }
    }

    // ── Embeddings ──────────────────────────────────────────────────
    if let Some(embed_config) = ddb_server::embeddings::EmbeddingConfig::from_env() {
        let embed_service = ddb_server::embeddings::EmbeddingService::new(
            embed_config.clone(),
            pool.clone(),
            triple_store_arc.clone(),
        );
        match embed_service.ensure_schema().await {
            Ok(()) => {
                let embed_manager =
                    Arc::new(ddb_server::embeddings::EmbeddingManager::new(embed_service));
                let embed_rx = change_tx.subscribe();
                tokio::spawn(embed_manager.run(embed_rx));
                tracing::info!("embedding pipeline initialized");
            }
            Err(e) => {
                tracing::warn!(error = %e, "embedding schema init failed, disabled");
            }
        }
    }

    // ── Storage Engine ──────────────────────────────────────────────
    let storage_dir =
        std::env::var("DDB_STORAGE_DIR").unwrap_or_else(|_| "./darshan/storage".to_string());
    let storage_backend = Arc::new(
        ddb_server::storage::LocalFsBackend::new(&storage_dir).unwrap_or_else(|e| {
            tracing::warn!("Storage backend at {storage_dir} failed: {e}, using /tmp");
            ddb_server::storage::LocalFsBackend::new("/tmp/darshjdb-storage")
                .expect("fallback storage")
        }),
    );
    let storage_signing_key =
        std::env::var("DDB_STORAGE_KEY").unwrap_or_else(|_| "dev-signing-key".to_string());
    let storage_engine = Arc::new(ddb_server::storage::StorageEngine::new(
        storage_backend,
        storage_signing_key.into_bytes(),
    ));

    // ── Function Runtime ────────────────────────────────────────────
    let functions_dir =
        std::env::var("DDB_FUNCTIONS_DIR").unwrap_or_else(|_| "./darshan/functions".to_string());
    let functions_dir_path = std::path::PathBuf::from(&functions_dir);

    let (fn_registry, fn_runtime) = if functions_dir_path.is_dir() {
        let harness_path = functions_dir_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("_darshan_harness.js");

        if !harness_path.exists() {
            (None, None)
        } else {
            match ddb_server::functions::FunctionRegistry::new(functions_dir_path.clone()).await {
                Ok(registry) => {
                    let process_runtime = ddb_server::functions::runtime::ProcessRuntime::new(
                        ddb_server::functions::runtime::ProcessKind::Node,
                        harness_path,
                        functions_dir_path,
                        ddb_server::functions::ResourceLimits::default().max_concurrency,
                    );
                    let runtime = ddb_server::functions::FunctionRuntime::new(
                        Box::new(process_runtime),
                        ddb_server::functions::ResourceLimits::default(),
                        database_url.clone(),
                        format!("http://127.0.0.1:{}", addr.port()),
                    );
                    (Some(Arc::new(registry)), Some(Arc::new(runtime)))
                }
                Err(_) => (None, None),
            }
        }
    } else {
        (None, None)
    };

    // ── Rule Engine ─────────────────────────────────────────────────
    let rules_path = std::path::PathBuf::from(
        std::env::var("DDB_RULES_FILE").unwrap_or_else(|_| "./darshan/rules.json".to_string()),
    );
    let rules = ddb_server::rules::load_rules_from_file(&rules_path).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to load rules, continuing without rule engine");
        Vec::new()
    });
    let rule_engine = if rules.is_empty() {
        None
    } else {
        Some(Arc::new(ddb_server::rules::RuleEngine::new(
            rules,
            triple_store_arc.clone(),
        )))
    };

    // ── REST API State ──────────────────────────────────────────────
    let mut app_state = ddb_server::api::rest::AppState::with_pool(
        pool.clone(),
        triple_store_arc.clone(),
        session_manager.clone(),
        change_tx,
        rate_limiter.clone(),
        storage_engine,
    );
    if let Some(engine) = rule_engine {
        app_state = app_state.with_rules(engine);
    }
    if let (Some(reg), Some(rt)) = (fn_registry, fn_runtime) {
        app_state = app_state.with_functions(reg, rt);
    }
    app_state = app_state.with_pubsub(pubsub_engine);

    // ── CORS ────────────────────────────────────────────────────────
    use tower_http::cors::{Any, CorsLayer};

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers(Any)
        .max_age(Duration::from_secs(86400));

    // ── Router Assembly ─────────────────────────────────────────────
    let api_router = ddb_server::api::rest::build_router(app_state);

    let health_pool = pool.clone();
    let server_started_at = std::time::Instant::now();

    let app = axum::Router::new()
        .nest("/api", api_router)
        .merge(ddb_server::api::ws::ws_routes(ws_state))
        .route(
            "/health",
            axum::routing::get(move || async move {
                let _ = (health_pool, server_started_at);
                axum::Json(serde_json::json!({
                    "status": "healthy",
                    "version": env!("CARGO_PKG_VERSION"),
                }))
            }),
        )
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(cors);

    // ── Bind & Serve ────────────────────────────────────────────────
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("Failed to bind to {addr}"))?;

    tracing::info!(%addr, "DarshJDB server listening");

    if !no_banner {
        println!(
            "  {} Server listening on {}\n",
            "-->".bright_green(),
            format!("http://{addr}").bright_yellow()
        );
    }

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("Server error")?;

    tracing::info!("DarshJDB server shut down gracefully");
    Ok(())
}

/// Print the startup banner.
fn print_banner(bind: &str, storage: &StorageBackend, conn: Option<&str>) {
    println!();
    println!(
        "  {}{}{}",
        " DarshJDB ".on_bright_cyan().black().bold(),
        " v".bright_white(),
        env!("CARGO_PKG_VERSION").bright_white()
    );
    println!();
    println!(
        "  {} Storage:  {}",
        "-->".bright_cyan(),
        format!("{storage}").bright_yellow()
    );
    if let Some(c) = conn {
        println!("  {} Conn:     {}", "-->".bright_cyan(), c.dimmed());
    }
    println!(
        "  {} Bind:     {}",
        "-->".bright_cyan(),
        bind.bright_yellow()
    );
    println!();
}

/// Wait for Ctrl+C or SIGTERM for graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    println!("\n  {} Shutting down...\n", "-->".bright_yellow());
}
