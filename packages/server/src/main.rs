//! DarshJDB server binary entry point.
//!
//! Initializes all subsystems (triple store, auth, sync, functions, storage),
//! builds the Axum router with middleware, and starts the HTTP server with
//! graceful shutdown support.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use ddb_server::admin::static_assets::admin_router;
use ddb_server::api::rest::{AppState, build_router};
use ddb_server::api::ws::{WsState, ws_routes};
use ddb_server::auth::middleware::RateLimiter;
use ddb_server::auth::session::{KeyManager, SessionManager};
use ddb_server::error::Result;
use ddb_server::sync::presence::PresenceManager;
use ddb_server::sync::registry::SubscriptionRegistry;
use ddb_server::sync::session::SessionManager as SyncSessionManager;

use sqlx::postgres::PgPoolOptions;
use tokio::signal;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::timeout::TimeoutLayer;

/// Default server port when `DDB_PORT` is not set.
const DEFAULT_PORT: u16 = 7700;

/// Maximum database connections in the pool.
const DEFAULT_MAX_CONNECTIONS: u32 = 20;

/// Minimum idle connections in the pool.
const DEFAULT_MIN_CONNECTIONS: u32 = 2;

/// Timeout (seconds) to acquire a connection from the pool.
const DEFAULT_ACQUIRE_TIMEOUT_SECS: u64 = 5;

/// Idle timeout (seconds) before a connection is released.
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 600;

/// Pool utilization percentage that triggers a warning log.
const POOL_HIGH_WATER_MARK: f64 = 0.80;

/// Rate limiter cleanup interval.
const RATE_LIMIT_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

/// Request timeout for all REST handlers.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> Result<()> {
    // -- Tracing / Logging (Phase 10: JSON + request_id) ----------------------
    ddb_server::observability::init_json_logging().map_err(|e| {
        ddb_server::error::DarshJError::Internal(format!("tracing init failed: {e}"))
    })?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        author = "Darshankumar Joshi",
        "DarshJDB server starting"
    );

    // -- Prometheus recorder (Phase 10) ---------------------------------------
    let (metrics_handle, metrics_allow_list) = ddb_server::observability::init_prometheus()
        .map_err(|e| {
            ddb_server::error::DarshJError::Internal(format!("prometheus init failed: {e}"))
        })?;
    tracing::info!(
        allow_list = ?metrics_allow_list,
        "Prometheus recorder installed"
    );

    // -- Configuration from environment ---------------------------------------
    //
    // Zero-dep dev mode (slice 17/30): if `DATABASE_URL` is unset AND the
    // binary was compiled with `--features embedded-db`, launch a local
    // Postgres 16 instance via `pg_embed` and use its URI. The `PgEmbed`
    // handle is bound to `_embedded_pg` so the child process lives for the
    // full server lifetime.
    let configured_url = std::env::var("DATABASE_URL").ok();

    #[cfg(feature = "embedded-db")]
    let (database_url, _embedded_pg) = match configured_url {
        Some(url) => (url, None),
        None => {
            let data_dir = dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".darshjdb")
                .join("data");
            tracing::info!(
                data_dir = %data_dir.display(),
                "no DATABASE_URL set — booting embedded Postgres 16 (zero-dep dev mode)"
            );
            let (pg, uri) = ddb_server::embedded_pg::start_embedded_postgres(&data_dir)
                .await
                .map_err(|e| {
                    ddb_server::error::DarshJError::Internal(format!(
                        "embedded Postgres startup failed: {e}"
                    ))
                })?;
            (uri, Some(pg))
        }
    };

    #[cfg(not(feature = "embedded-db"))]
    let database_url = configured_url.unwrap_or_else(|| {
        tracing::warn!("DATABASE_URL not set, using default localhost connection");
        "postgres://ddb:ddb@localhost:5432/darshjdb".to_string()
    });

    let port: u16 = std::env::var("DDB_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let max_connections: u32 = std::env::var("DDB_DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_MAX_CONNECTIONS);

    let min_connections: u32 = std::env::var("DDB_DB_MIN_CONNECTIONS")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_MIN_CONNECTIONS);

    let acquire_timeout_secs: u64 = std::env::var("DDB_DB_ACQUIRE_TIMEOUT_SECS")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_ACQUIRE_TIMEOUT_SECS);

    let idle_timeout_secs: u64 = std::env::var("DDB_DB_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECS);

    let jwt_secret = std::env::var("DDB_JWT_SECRET").ok();
    let jwt_private_key_path = std::env::var("DDB_JWT_PRIVATE_KEY").ok();
    let jwt_public_key_path = std::env::var("DDB_JWT_PUBLIC_KEY").ok();

    // -- Database Pool --------------------------------------------------------
    tracing::info!(database_url = %mask_url(&database_url), "connecting to database");

    tracing::info!(
        max_connections,
        min_connections,
        acquire_timeout_secs,
        idle_timeout_secs,
        "database pool configuration"
    );

    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .min_connections(min_connections)
        .acquire_timeout(Duration::from_secs(acquire_timeout_secs))
        .idle_timeout(Duration::from_secs(idle_timeout_secs))
        .max_lifetime(Duration::from_secs(1800))
        .connect(&database_url)
        .await
        .map_err(|e| {
            tracing::error!("Failed to connect to database: {e}");
            ddb_server::error::DarshJError::Database(e)
        })?;

    tracing::info!("database connection pool established");

    // -- Schema Creation (serialized with advisory lock) -----------------------
    // Prevent concurrent connections from deadlocking on DDL.
    sqlx::query("SELECT pg_advisory_lock(42)")
        .execute(&pool)
        .await
        .map_err(ddb_server::error::DarshJError::Database)?;

    let triple_store = ddb_server::triple_store::PgTripleStore::new(pool.clone()).await?;
    tracing::info!("triple store initialized (schema ensured)");

    ddb_server::api::rest::ensure_auth_schema(&pool)
        .await
        .map_err(|e| {
            tracing::error!("Failed to ensure auth schema: {e}");
            ddb_server::error::DarshJError::Database(e)
        })?;
    tracing::info!("auth schema ensured (users + sessions tables)");

    // Phase 5.3 (slice 23/30) — ensure anchor_receipts exists so the
    // background anchor task can write without a follow-up migration.
    ddb_server::anchor::ensure_anchor_schema(&pool)
        .await
        .map_err(|e| {
            tracing::error!("Failed to ensure anchor schema: {e}");
            ddb_server::error::DarshJError::Database(e)
        })?;
    tracing::info!("anchor schema ensured (anchor_receipts table)");

    // Phase 3 (slice 16) — ensure vector + embeddings + FTS indexes exist
    // so /api/search/{semantic,text,hybrid} work without a separate migration.
    ddb_server::api::rest::ensure_search_schema(&pool)
        .await
        .map_err(|e| {
            tracing::error!("Failed to ensure search schema: {e}");
            ddb_server::error::DarshJError::Database(e)
        })?;
    tracing::info!("search schema ensured (pgvector + embeddings + FTS)");

    // Slice 14/30 — ensure agent memory tables (sessions + entries + facts)
    // so the context builder / memory API can write on first request.
    ddb_server::agent_memory::ensure_agent_memory_schema(&pool)
        .await
        .map_err(|e| {
            tracing::error!("Failed to ensure agent memory schema: {e}");
            ddb_server::error::DarshJError::Database(e)
        })?;
    tracing::info!("agent memory schema ensured (sessions + entries + facts tables)");

    // Phase 7.1 (slice 25/30): ensure chunked_uploads table exists so
    // resumable uploads and the cleanup task can run without a
    // separate migrator step.
    ddb_server::api::chunked_upload::ensure_chunked_uploads_schema(&pool)
        .await
        .map_err(|e| {
            tracing::error!("Failed to ensure chunked_uploads schema: {e}");
            ddb_server::error::DarshJError::Database(e)
        })?;
    tracing::info!("chunked_uploads schema ensured");

    sqlx::query("SELECT pg_advisory_unlock(42)")
        .execute(&pool)
        .await
        .map_err(ddb_server::error::DarshJError::Database)?;

    // -- Auth Engine ----------------------------------------------------------
    let key_manager = match (&jwt_private_key_path, &jwt_public_key_path) {
        (Some(priv_path), Some(pub_path)) => {
            // Production: RS256 with PEM key files.
            let priv_pem = std::fs::read(priv_path).map_err(|e| {
                ddb_server::error::DarshJError::Internal(format!(
                    "failed to read JWT private key at {priv_path}: {e}"
                ))
            })?;
            let pub_pem = std::fs::read(pub_path).map_err(|e| {
                ddb_server::error::DarshJError::Internal(format!(
                    "failed to read JWT public key at {pub_path}: {e}"
                ))
            })?;
            KeyManager::new(&priv_pem, &pub_pem, "ddb-key-1".into(), None, None).map_err(|e| {
                ddb_server::error::DarshJError::Internal(format!(
                    "failed to initialize RSA key manager: {e}"
                ))
            })?
        }
        _ => {
            // Development: HS256 with shared secret or ephemeral keys.
            match jwt_secret {
                Some(secret) => {
                    tracing::info!("using HMAC (HS256) JWT signing with DDB_JWT_SECRET");
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

    // -- Sync Engine ----------------------------------------------------------
    let sync_sessions = Arc::new(SyncSessionManager::new());
    let subscription_registry = Arc::new(SubscriptionRegistry::new());
    let presence_manager = Arc::new(PresenceManager::new());
    let (diff_tx, _diff_rx) = tokio::sync::mpsc::channel(1024);

    // Broadcast channel for triple-store change events (REST -> WS fan-out).
    let (change_tx, _change_rx) =
        tokio::sync::broadcast::channel::<ddb_server::sync::ChangeEvent>(4096);

    let triple_store_arc = Arc::new(triple_store);

    // -- TTL Expiry Background Task -------------------------------------------
    // Every 30 seconds, scan for expired triples and retract them.
    // Uses the idx_triples_expiry partial index for efficient scans.
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
                            // Emit change events so WebSocket subscriptions update.
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
        tracing::info!("TTL expiry background task started (30s interval)");
    }

    // -- Blockchain Anchor Background Task (Phase 5.3, slice 23/30) -----------
    // Every `DARSH_ANCHOR_EVERY_N_TX` transactions, compute the
    // Keccak-256 batch root over the most recent N `tx_merkle_roots`
    // rows and submit it to the configured anchor backend (IPFS,
    // Ethereum, or `none`). The receipt is persisted regardless of
    // outcome so operators always have a complete audit trail.
    //
    // Only spawned when `DARSH_BLOCKCHAIN_ANCHOR` is set to something
    // other than `none` — with anchoring disabled we'd just be writing
    // `skipped` rows on a timer, which wastes SQL round-trips for no
    // operational benefit.
    {
        let anchor_chain =
            std::env::var("DARSH_BLOCKCHAIN_ANCHOR").unwrap_or_else(|_| "none".to_string());
        let every_n: u64 = std::env::var("DARSH_ANCHOR_EVERY_N_TX")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);

        if anchor_chain != "none" && every_n > 0 {
            let anchor_pool = pool.clone();
            let chain_for_task = anchor_chain.clone();
            tokio::spawn(async move {
                let anchorer = ddb_server::anchor::build_anchorer(&chain_for_task);

                // Baseline: the `tx_id` we saw last cycle. We trigger a
                // new anchor run every time the current max exceeds
                // `baseline + every_n`. Using a DB-side counter keeps
                // the task stateless across process restarts.
                let mut baseline: i64 = match sqlx::query_scalar::<_, Option<i64>>(
                    "SELECT MAX(tx_id) FROM tx_merkle_roots",
                )
                .fetch_one(&anchor_pool)
                .await
                {
                    Ok(v) => v.unwrap_or(0),
                    Err(e) => {
                        tracing::warn!(error = %e, "anchor task: failed to read tx_merkle_roots baseline");
                        0
                    }
                };

                let mut interval = tokio::time::interval(Duration::from_secs(15));
                loop {
                    interval.tick().await;
                    let current: Option<i64> = match sqlx::query_scalar::<_, Option<i64>>(
                        "SELECT MAX(tx_id) FROM tx_merkle_roots",
                    )
                    .fetch_one(&anchor_pool)
                    .await
                    {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(error = %e, "anchor task: tx_merkle_roots probe failed");
                            continue;
                        }
                    };

                    let Some(current) = current else { continue };
                    if current.saturating_sub(baseline) < every_n as i64 {
                        continue;
                    }

                    match ddb_server::anchor::run_anchor_cycle(
                        &anchor_pool,
                        anchorer.as_ref(),
                        every_n,
                    )
                    .await
                    {
                        Ok(receipt) => {
                            tracing::info!(
                                chain = %receipt.chain,
                                status = %receipt.status,
                                tx_count = receipt.tx_count,
                                "anchor cycle completed"
                            );
                            baseline = current;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "anchor cycle failed");
                        }
                    }
                }
            });
            tracing::info!(
                chain = %anchor_chain,
                every_n,
                "blockchain anchor background task started"
            );
        } else {
            tracing::info!("blockchain anchor disabled (DARSH_BLOCKCHAIN_ANCHOR=none)");
        }
    }

    // -- Pool Utilization Monitor ------------------------------------------------
    {
        let monitor_pool = pool.clone();
        let monitor_max = max_connections;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                let size = monitor_pool.size();
                let idle = monitor_pool.num_idle() as u32;
                let active = size.saturating_sub(idle);
                let utilization = if monitor_max > 0 {
                    active as f64 / monitor_max as f64
                } else {
                    0.0
                };
                if utilization > POOL_HIGH_WATER_MARK {
                    tracing::warn!(
                        active,
                        idle,
                        size,
                        max = monitor_max,
                        utilization_pct = format!("{:.1}", utilization * 100.0),
                        "connection pool utilization above 80%"
                    );
                }
            }
        });
        tracing::info!("pool utilization monitor started (10s interval, warn >80%)");
    }

    // -- Chunked Upload Cleanup (VYASA 7.1) -----------------------------------
    // Every 5 minutes, delete `chunked_uploads` rows in `in_progress` state
    // that are older than 24h and purge their `/tmp` staging directories.
    {
        let cleanup_pool = pool.clone();
        ddb_server::api::chunked_upload::spawn_cleanup_task(cleanup_pool);
        tracing::info!("chunked upload cleanup task started (5min interval, 24h stale threshold)");
    }

    // -- Postgres LISTEN/NOTIFY for multi-process sync -------------------------
    // A background task LISTENs on the `ddb_changes` channel. When another
    // process mutates via set_triples, the NOTIFY fires and this task parses
    // the payload (`{tx_id}:{entity_type}`) into ChangeEvents, feeding the
    // existing broadcast channel so WebSocket subscribers get updates.
    {
        let listen_change_tx = change_tx.clone();
        let listen_db_url = database_url.clone();
        tokio::spawn(async move {
            // Use a dedicated connection (not from the pool) for LISTEN.
            let mut listener = match sqlx::postgres::PgListener::connect(&listen_db_url).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!(error = %e, "failed to create PgListener for ddb_changes");
                    return;
                }
            };
            if let Err(e) = listener.listen("ddb_changes").await {
                tracing::error!(error = %e, "failed to LISTEN on ddb_changes channel");
                return;
            }
            tracing::info!("LISTEN/NOTIFY: subscribed to ddb_changes channel");

            loop {
                match listener.recv().await {
                    Ok(notification) => {
                        let payload = notification.payload();
                        // Parse "{tx_id}:{entity_type}"
                        let (tx_id, entity_type) = match payload.split_once(':') {
                            Some((tid, etype)) => {
                                let tid: i64 = tid.parse().unwrap_or(0);
                                (tid, Some(etype.to_string()))
                            }
                            None => {
                                let tid: i64 = payload.parse().unwrap_or(0);
                                (tid, None)
                            }
                        };
                        tracing::debug!(tx_id, entity_type = ?entity_type, "received ddb_changes notification");
                        let _ = listen_change_tx.send(ddb_server::sync::ChangeEvent {
                            tx_id,
                            entity_ids: vec![],
                            attributes: vec![],
                            entity_type,
                            actor_id: None,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "PgListener recv error, reconnecting in 1s");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
        tracing::info!("LISTEN/NOTIFY background task started for multi-process sync");
    }

    // Pub/sub engine for keyspace notifications (shared between WS and REST).
    let (pubsub_engine, _pubsub_rx) = ddb_server::sync::pubsub::PubSubEngine::new(4096);

    // Change feed for mutation logging and cursor-based replay.
    let (change_feed, _change_feed_rx) = ddb_server::sync::change_feed::ChangeFeed::with_defaults();

    // Live query manager for LIVE SELECT subscriptions.
    // Slice 28/30 — the same Arc is shared between the WS handler and
    // the REST DarshanQL handler so HTTP `LIVE SELECT` requests with
    // `X-Subscription-Upgrade` register against the identical
    // subscription pool that powers the WebSocket channel.
    let (live_query_manager, _live_query_rx) =
        ddb_server::sync::live_query::LiveQueryManager::new(4096);
    let live_query_manager_for_rest = live_query_manager.clone();

    // -- Rule Engine (built early so WS handler can fire it inside the same
    //    transaction as user writes, matching the REST mutation path).
    let rules_path = std::path::PathBuf::from(
        std::env::var("DDB_RULES_FILE").unwrap_or_else(|_| "./darshan/rules.json".to_string()),
    );
    let rules = ddb_server::rules::load_rules_from_file(&rules_path).unwrap_or_else(|e| {
        tracing::error!(error = %e, "failed to load rules, continuing without rule engine");
        Vec::new()
    });
    let rule_engine: Option<Arc<ddb_server::rules::RuleEngine>> = if rules.is_empty() {
        None
    } else {
        Some(Arc::new(ddb_server::rules::RuleEngine::new(
            rules,
            triple_store_arc.clone(),
        )))
    };

    // Shared hot query cache — the same Arc is handed to both the WS handler
    // (for invalidation on mutation) and the REST `AppState` (for reads), so
    // a WS mutation evicts subsequent REST reads and vice versa.
    let query_cache = Arc::new(ddb_server::cache::QueryCache::from_env());

    let ws_state = WsState {
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
        rule_engine: rule_engine.clone(),
        query_cache: query_cache.clone(),
        subscription_snapshots: Arc::new(dashmap::DashMap::new()),
    };

    tracing::info!("sync engine initialized");

    // -- Connector Plugin System ----------------------------------------------
    {
        use ddb_server::connectors::log::LogConnector;
        use ddb_server::connectors::webhook::WebhookConnector;
        use ddb_server::connectors::{Connector, ConnectorManager};

        let mut connectors: Vec<Box<dyn Connector>> = Vec::new();

        // Always register the log connector for observability.
        connectors.push(Box::new(LogConnector::new()));

        // Optionally register the webhook connector if DDB_WEBHOOK_URL is set.
        if let Some(wh) = WebhookConnector::from_env() {
            tracing::info!("webhook connector enabled");
            connectors.push(Box::new(wh));
        }

        if !connectors.is_empty() {
            let manager = Arc::new(ConnectorManager::new(connectors, triple_store_arc.clone()));

            // Initialize all connectors.
            manager.initialize_all().await;

            // Subscribe to the broadcast channel and spawn the fan-out loop.
            let connector_rx = change_tx.subscribe();
            tokio::spawn(manager.run(connector_rx));

            tracing::info!("connector plugin system initialized");
        }
    }

    // -- Embedding Pipeline ---------------------------------------------------
    if let Some(embed_config) = ddb_server::embeddings::EmbeddingConfig::from_env() {
        let embed_service = ddb_server::embeddings::EmbeddingService::new(
            embed_config.clone(),
            pool.clone(),
            triple_store_arc.clone(),
        );

        // Ensure pgvector extension and entity_embeddings table exist.
        // Non-fatal: log warning and continue without embeddings if schema fails.
        match embed_service.ensure_schema().await {
            Ok(()) => {
                let embed_manager =
                    Arc::new(ddb_server::embeddings::EmbeddingManager::new(embed_service));
                let embed_rx = change_tx.subscribe();
                tokio::spawn(embed_manager.run(embed_rx));

                tracing::info!(
                    provider = ?embed_config.provider,
                    dimensions = embed_config.dimensions,
                    auto_attributes = ?embed_config.auto_embed_attributes,
                    "embedding pipeline initialized"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "failed to initialize embedding schema, auto-embedding disabled. \
                     Ensure pgvector is installed: https://github.com/pgvector/pgvector"
                );
            }
        }
    } else {
        tracing::info!("embedding pipeline disabled (DDB_EMBEDDING_PROVIDER=none or unset)");
    }

    // -- Agent Memory Embedding Worker (Phase 2.5) ----------------------------
    // Spawns a background task that fills `embedding`/`content_tokens` on
    // `memory_entries` and `agent_facts` rows. The schema is assumed to exist
    // from the Phase 2 migration (slice 12). If it doesn't, the worker logs a
    // warning on its first tick and keeps retrying — it never crashes the
    // server.
    {
        let provider_kind = std::env::var("DARSH_EMBEDDING_PROVIDER")
            .unwrap_or_else(|_| "none".to_string())
            .to_lowercase();
        if provider_kind != "none" && !provider_kind.is_empty() {
            let provider: std::sync::Arc<dyn ddb_agent_memory::EmbeddingProvider> =
                std::sync::Arc::from(ddb_agent_memory::from_env());
            tracing::info!(
                provider = %provider_kind,
                model = provider.model(),
                dimensions = provider.dimensions(),
                "agent-memory embedding worker starting"
            );
            let _handle = ddb_agent_memory::spawn_embedding_worker(pool.clone(), provider);
        } else {
            tracing::info!(
                "agent-memory embedding worker disabled (DARSH_EMBEDDING_PROVIDER=none or unset)"
            );
        }
    }

    // -- Storage Engine -------------------------------------------------------
    let storage_dir =
        std::env::var("DDB_STORAGE_DIR").unwrap_or_else(|_| "./darshan/storage".to_string());
    let storage_backend = Arc::new(
        ddb_server::storage::LocalFsBackend::new(&storage_dir).unwrap_or_else(|e| {
            tracing::warn!("Failed to create storage backend at {storage_dir}: {e}, using /tmp");
            ddb_server::storage::LocalFsBackend::new("/tmp/darshjdb-storage")
                .expect("fallback storage backend")
        }),
    );
    let storage_signing_key =
        std::env::var("DDB_STORAGE_KEY").unwrap_or_else(|_| "dev-signing-key".to_string());
    let storage_engine = Arc::new(ddb_server::storage::StorageEngine::new(
        storage_backend,
        storage_signing_key.into_bytes(),
    ));
    tracing::info!(%storage_dir, "storage engine initialized");

    // -- Function Runtime -----------------------------------------------------
    let functions_dir =
        std::env::var("DDB_FUNCTIONS_DIR").unwrap_or_else(|_| "./darshan/functions".to_string());
    let functions_dir_path = std::path::PathBuf::from(&functions_dir);

    let (fn_registry, fn_runtime) = if functions_dir_path.is_dir() {
        // Harness lives next to the functions directory.
        let harness_path = functions_dir_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("_darshan_harness.js");

        if !harness_path.exists() {
            tracing::warn!(
                harness = %harness_path.display(),
                "function harness not found, function execution disabled"
            );
            (None, None)
        } else {
            match ddb_server::functions::FunctionRegistry::new(functions_dir_path.clone()).await {
                Ok(registry) => {
                    let fn_count = registry.count().await;
                    tracing::info!(
                        count = fn_count,
                        dir = %functions_dir,
                        "function registry initialized"
                    );

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
                        format!("http://127.0.0.1:{port}"),
                    );

                    (Some(Arc::new(registry)), Some(Arc::new(runtime)))
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to initialize function registry, functions disabled"
                    );
                    (None, None)
                }
            }
        }
    } else {
        tracing::info!(
            dir = %functions_dir,
            "functions directory not found, function execution disabled"
        );
        (None, None)
    };

    // Rule engine was built earlier (before ws_state) so the WS mutation
    // handler can fire it inside the same transaction as user writes.

    // -- REST API State -------------------------------------------------------
    let mut app_state = AppState::with_pool(
        pool.clone(),
        triple_store_arc.clone(),
        session_manager.clone(),
        change_tx,
        rate_limiter.clone(),
        storage_engine,
    );
    // Share the same query cache with the WS handler so mutations over either
    // channel invalidate reads on the other.
    app_state.query_cache = query_cache.clone();
    if let Some(engine) = rule_engine {
        app_state = app_state.with_rules(engine);
    }
    if let (Some(reg), Some(rt)) = (fn_registry, fn_runtime) {
        app_state = app_state.with_functions(reg, rt);
    }
    app_state = app_state.with_pubsub(pubsub_engine);

    // -- Graph Engine --------------------------------------------------------
    let edge_store = ddb_server::graph::PgEdgeStore::new(pool.clone()).await?;
    let graph_engine = Arc::new(ddb_server::graph::GraphEngine::new(Arc::new(edge_store)));
    app_state = app_state.with_graph(graph_engine);
    tracing::info!("Graph engine initialized (SurrealDB-style record links)");

    // -- Schema Registry (SCHEMAFULL / SCHEMALESS / MIXED) --------------------
    match ddb_server::schema::SchemaRegistry::new(pool.clone()).await {
        Ok(registry) => {
            let table_count = registry.list_tables().len();
            let registry = Arc::new(registry);
            app_state = app_state.with_schema_registry(registry);
            tracing::info!(
                tables = table_count,
                "Schema registry initialized (SurrealDB-style schema modes)"
            );
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Failed to initialize schema registry, running in schemaless-only mode"
            );
        }
    }

    // -- Schema Migration Engine -----------------------------------------------
    match ddb_server::schema::migration::SchemaMigrationEngine::new(pool.clone()).await {
        Ok(_engine) => {
            tracing::info!("Schema migration engine initialized");
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to initialize schema migration engine");
        }
    }

    // -- Slice 28/30 — Strict Schema Enforcer (Phase 9 SurrealDB parity) -------
    // Gated by `DdbConfig.schema.schema_mode`. When the mode is
    // "strict" the enforcer short-circuits writes that violate
    // `schema_definitions`; in any other mode it still loads
    // definitions so the admin routes can manage them, but
    // validation always passes.
    // The slice's gating predicate is
    // `DdbConfig.schema.schema_mode == "strict"`. The v0.2.0 baseline
    // does not yet thread `DdbConfig` through `main.rs`, so we read
    // the same value from `DARSH_SCHEMA__SCHEMA_MODE` (the canonical
    // env var the typed loader would consume) with a `DDB_SCHEMA_MODE`
    // alias for ergonomic ops use.
    let schema_mode_env = std::env::var("DARSH_SCHEMA__SCHEMA_MODE")
        .or_else(|_| std::env::var("DDB_SCHEMA_MODE"))
        .unwrap_or_else(|_| "flexible".to_string());
    let strict_mode_active = schema_mode_env.eq_ignore_ascii_case("strict");
    match ddb_server::schema::strict::StrictSchemaEnforcer::new(pool.clone(), strict_mode_active)
        .await
    {
        Ok(enforcer) => {
            app_state = app_state.with_strict_schema(enforcer);
            tracing::info!(
                strict = strict_mode_active,
                "Strict schema enforcer initialized (slice 28/30)"
            );
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Failed to initialize strict schema enforcer, continuing without strict mode"
            );
        }
    }

    // Slice 28/30 — share the live-query manager with the REST
    // DarshanQL handler so `LIVE SELECT` statements submitted over
    // HTTP (with the `X-Subscription-Upgrade` header) register
    // against the same `LiveQueryManager` as WebSocket clients.
    app_state = app_state.with_live_queries(live_query_manager_for_rest);

    // Bootstrap the `admin_audit_log` table so the SQL passthrough
    // handler can always append audit rows even on fresh installs.
    if let Err(e) = ddb_server::api::sql_passthrough::ensure_audit_schema(&pool).await {
        tracing::warn!(error = %e, "Failed to bootstrap admin_audit_log table");
    }

    // -- CORS Layer -----------------------------------------------------------
    let dev_mode = std::env::var("DDB_DEV")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);

    // Determine allowed origins: DDB_CORS_ORIGINS takes precedence, then
    // dev-mode defaults to localhost, production defaults to deny-all.
    let cors_origins = std::env::var("DDB_CORS_ORIGINS").unwrap_or_default();

    let cors = if cors_origins.trim() == "*" {
        // Explicit wildcard: allow all origins regardless of mode.
        tracing::warn!("CORS: wildcard (*) — all origins allowed");
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
            .expose_headers(Any)
            .max_age(Duration::from_secs(86400))
    } else if !cors_origins.is_empty() {
        // Explicit origin list (comma-separated).
        let parsed: Vec<axum::http::HeaderValue> = cors_origins
            .split(',')
            .filter_map(|o| o.trim().parse().ok())
            .collect();
        tracing::info!(origins = ?parsed, "CORS: explicit origins");
        CorsLayer::new()
            .allow_origin(parsed)
            .allow_methods(Any)
            .allow_headers(Any)
            .expose_headers(Any)
            .max_age(Duration::from_secs(86400))
    } else if dev_mode {
        // Dev mode with no explicit origins: allow localhost only.
        tracing::info!("CORS: dev mode, allowing localhost origins");
        let localhost_origins: Vec<axum::http::HeaderValue> = vec![
            "http://localhost:3000".parse().expect("valid origin"),
            "http://localhost:5173".parse().expect("valid origin"),
            "http://localhost:8080".parse().expect("valid origin"),
            "http://127.0.0.1:3000".parse().expect("valid origin"),
            "http://127.0.0.1:5173".parse().expect("valid origin"),
            "http://127.0.0.1:8080".parse().expect("valid origin"),
        ];
        CorsLayer::new()
            .allow_origin(localhost_origins)
            .allow_methods(Any)
            .allow_headers(Any)
            .expose_headers(Any)
            .max_age(Duration::from_secs(86400))
    } else {
        // Production with no explicit origins: deny cross-origin.
        tracing::warn!(
            "DDB_CORS_ORIGINS not set in production mode, denying cross-origin requests"
        );
        CorsLayer::new()
            .allow_methods(Any)
            .allow_headers(Any)
            .expose_headers(Any)
            .max_age(Duration::from_secs(86400))
    };

    // -- Count existing triples for startup log --------------------------------
    let triple_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM triples")
        .fetch_one(&pool)
        .await
        .unwrap_or((0,));
    tracing::info!(triples = triple_count.0, "triple store stats");

    // -- Shared state for legacy health endpoints -----------------------------
    let server_started_at = Instant::now();
    let health_pool_ready = pool.clone();
    let health_ws_sessions_ready = sync_sessions.clone();
    let legacy_health_pool = pool.clone();
    let legacy_ws_sessions = sync_sessions.clone();
    let legacy_pool_stats = app_state.pool_stats.clone();

    // -- Router Assembly ------------------------------------------------------
    let api_router = build_router(app_state);

    // Phase 10 observability router: /health, /ready, /live — mounted BEFORE
    // auth middleware so load balancers and orchestrators can probe without a
    // token. Rendered by `ddb_server::observability::health`.
    let health_state = ddb_server::observability::HealthState::new(pool.clone());
    let phase10_health = ddb_server::observability::health_router(health_state);

    // Phase 10 metrics router: /metrics (Prometheus text exposition, IP
    // allow-list enforced via DDB_METRICS_ALLOWED_IPS).
    let phase10_metrics = ddb_server::observability::metrics_router(
        metrics_handle.clone(),
        metrics_allow_list.clone(),
    );

    let app = axum::Router::new()
        // REST API routes under /api
        .nest("/api", api_router)
        // Embedded admin dashboard (Vite SPA baked into the binary via
        // include_dir!). Mounted at the top level so /admin/* serves the
        // dashboard while /api/admin/{schema,functions,...} JSON endpoints
        // remain reachable inside the build_router. — Slice 19/30
        .merge(admin_router())
        // WebSocket route at /ws
        .merge(ws_routes(ws_state))
        // Phase 10: /health /ready /live — mounted before auth middleware.
        .merge(phase10_health)
        // Phase 10: /metrics (Prometheus text exposition).
        .merge(phase10_metrics)
        // Legacy rich health shape at /health/full for dashboards that still
        // want pool/ws/triple counts.
        .route(
            "/health/full",
            axum::routing::get(move || {
                health_check(
                    legacy_health_pool.clone(),
                    legacy_ws_sessions.clone(),
                    server_started_at,
                    legacy_pool_stats.clone(),
                )
            }),
        )
        // Legacy readiness probe for K8s
        .route(
            "/health/ready",
            axum::routing::get(move || {
                readiness_check(health_pool_ready.clone(), health_ws_sessions_ready.clone())
            }),
        )
        // Database pool stats endpoint
        .route(
            "/health/db",
            axum::routing::get({
                let db_pool = pool.clone();
                move || db_pool_health(db_pool.clone())
            }),
        )
        // -- Middleware stack (outermost = runs first) -------------------------
        // Phase 10: HTTP Prometheus counter + histogram observer.
        .layer(axum::middleware::from_fn(
            ddb_server::observability::http_metrics_middleware,
        ))
        // Phase 10: request_id + JSON span per request.
        .layer(axum::middleware::from_fn(
            ddb_server::observability::request_id_middleware,
        ))
        // Structured request logging (legacy)
        .layer(middleware::from_fn(request_logging_middleware))
        // Catch panics in handlers -> 500
        .layer(CatchPanicLayer::custom(handle_panic))
        // 30s request timeout on all routes
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        // CORS
        .layer(cors);

    // -- Start Server ---------------------------------------------------------
    // If DDB_BIND_ADDR is set, use it; else default to 0.0.0.0.
    // Dev mode refuses to bind anything other than loopback so a leaked
    // DDB_DEV=1 env var cannot accidentally expose the bearer-token bypass
    // to a non-local caller.
    let bind_host = std::env::var("DDB_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0".into());
    if dev_mode {
        let is_loopback = matches!(
            bind_host.as_str(),
            "127.0.0.1" | "::1" | "localhost" | "0.0.0.0"
        );
        if !is_loopback {
            return Err(ddb_server::error::DarshJError::Internal(format!(
                "DDB_DEV=1 refuses to bind non-loopback address {bind_host}. \
                 Unset DDB_DEV or set DDB_BIND_ADDR=127.0.0.1."
            )));
        }
        if bind_host == "0.0.0.0" {
            tracing::warn!(
                "DDB_DEV=1 active with bind 0.0.0.0 — dev bypass token only \
                 accepted from loopback sources (requests with proxy headers \
                 are rejected)."
            );
        }
    }
    let bind_ip: std::net::IpAddr = bind_host
        .parse()
        .or_else(|_| "0.0.0.0".parse())
        .expect("default bind parse");
    let addr = SocketAddr::from((bind_ip, port));

    // Optional TLS termination: if DDB_TLS_CERT and DDB_TLS_KEY are set,
    // bind with rustls for native TLS 1.2/1.3 support. Otherwise, plain HTTP.
    let tls_cert = std::env::var("DDB_TLS_CERT");
    let tls_key = std::env::var("DDB_TLS_KEY");

    if let (Ok(cert_path), Ok(key_path)) = (tls_cert, tls_key) {
        tracing::info!("TLS enabled: loading certificate from {cert_path}");

        let rustls_config =
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path)
                .await
                .map_err(|e| {
                    ddb_server::error::DarshJError::Internal(format!(
                        "failed to load TLS certificate/key ({cert_path}, {key_path}): {e}"
                    ))
                })?;

        tracing::info!(%addr, "DarshJDB server listening (TLS enabled)");
        tracing::info!("  REST API:  https://{addr}/api");
        tracing::info!("  WebSocket: wss://{addr}/ws");
        tracing::info!("  Health:    https://{addr}/health");
        tracing::info!("  Ready:     https://{addr}/health/ready");
        tracing::info!("  API Docs:  https://{addr}/api/docs");

        axum_server::bind_rustls(addr, rustls_config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .map_err(|e| ddb_server::error::DarshJError::Internal(format!("server error: {e}")))?;
    } else {
        tracing::info!("TLS disabled (set DDB_TLS_CERT and DDB_TLS_KEY to enable)");

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| ddb_server::error::DarshJError::Internal(format!("bind error: {e}")))?;

        tracing::info!(%addr, "DarshJDB server listening");
        tracing::info!("  REST API:  http://{addr}/api");
        tracing::info!("  WebSocket: ws://{addr}/ws");
        tracing::info!("  Health:    http://{addr}/health");
        tracing::info!("  Ready:     http://{addr}/health/ready");
        tracing::info!("  API Docs:  http://{addr}/api/docs");

        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| ddb_server::error::DarshJError::Internal(format!("server error: {e}")))?;
    }

    tracing::info!("DarshJDB server shut down gracefully");

    Ok(())
}

// =============================================================================
// Structured request logging middleware
// =============================================================================

/// Logs every request with method, path, status, duration_ms, and user_id.
async fn request_logging_middleware(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    // Extract user_id from extensions if auth middleware already ran.
    // We check both the auth context type and a simple string extension.
    let user_id: Option<String> = req
        .extensions()
        .get::<ddb_server::auth::AuthContext>()
        .map(|ctx| ctx.user_id.to_string());

    let start = Instant::now();
    let response = next.run(req).await;
    let duration_ms = start.elapsed().as_millis();
    let status = response.status().as_u16();

    tracing::info!(
        http.method = %method,
        http.path = %path,
        http.status = status,
        duration_ms = duration_ms,
        user_id = user_id.as_deref().unwrap_or("-"),
        "request"
    );

    response
}

// =============================================================================
// Panic handler
// =============================================================================

/// Converts a caught panic into a structured 500 response.
fn handle_panic(err: Box<dyn std::any::Any + Send + 'static>) -> Response {
    let detail = if let Some(s) = err.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = err.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "unknown panic".to_string()
    };

    tracing::error!(panic = %detail, "handler panicked");

    let body = serde_json::json!({
        "error": {
            "code": "INTERNAL",
            "message": "Internal server error",
            "status": 500
        }
    });

    (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(body)).into_response()
}

// =============================================================================
// Health endpoints
// =============================================================================

/// `GET /health` - Comprehensive health check with uptime, pool stats, WS connections.
async fn health_check(
    pool: sqlx::PgPool,
    ws_sessions: Arc<SyncSessionManager>,
    started_at: Instant,
    pool_stats: Arc<ddb_server::api::pool_stats::PoolStats>,
) -> Response {
    let pool_size = pool.size();
    let idle = pool.num_idle();
    let uptime_secs = started_at.elapsed().as_secs();
    let ws_connections = ws_sessions.session_count();

    // Check if Postgres is reachable.
    let db_ok = sqlx::query("SELECT 1").execute(&pool).await.is_ok();

    let triple_count: i64 = if db_ok {
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM triples")
            .fetch_one(&pool)
            .await
            .map(|r| r.0)
            .unwrap_or(-1)
    } else {
        -1
    };

    let status = if db_ok { "ok" } else { "degraded" };
    let http_status = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    // Build pool + latency stats snapshot from the histogram.
    let stats_snapshot = pool_stats.snapshot(&pool);

    let body = serde_json::json!({
        "status": status,
        "service": "darshjdb",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": uptime_secs,
        "pool": {
            "size": pool_size,
            "idle": idle,
            "active": pool_size - idle as u32,
            "max": pool.options().get_max_connections(),
        },
        "pool_stats": stats_snapshot,
        "websockets": {
            "active_connections": ws_connections,
        },
        "triples": triple_count,
        "database": if db_ok { "connected" } else { "disconnected" },
    });

    (http_status, axum::Json(body)).into_response()
}

/// `GET /health/ready` - K8s readiness probe. Returns 200 only when Postgres is connected.
async fn readiness_check(pool: sqlx::PgPool, ws_sessions: Arc<SyncSessionManager>) -> Response {
    match sqlx::query("SELECT 1").execute(&pool).await {
        Ok(_) => {
            let body = serde_json::json!({
                "ready": true,
                "pool_size": pool.size(),
                "pool_idle": pool.num_idle(),
                "ws_connections": ws_sessions.session_count(),
            });
            (StatusCode::OK, axum::Json(body)).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "readiness check failed: database unreachable");
            let body = serde_json::json!({
                "ready": false,
                "error": "database unreachable",
            });
            (StatusCode::SERVICE_UNAVAILABLE, axum::Json(body)).into_response()
        }
    }
}

// =============================================================================
// Database pool health endpoint
// =============================================================================

/// `GET /health/db` - Connection pool statistics (active, idle, size, max).
async fn db_pool_health(pool: sqlx::PgPool) -> Response {
    let size = pool.size();
    let idle = pool.num_idle() as u32;
    let active = size.saturating_sub(idle);
    let max = pool.options().get_max_connections();
    let min = pool.options().get_min_connections();
    let utilization = if max > 0 {
        (active as f64 / max as f64) * 100.0
    } else {
        0.0
    };

    let body = serde_json::json!({
        "active": active,
        "idle": idle,
        "size": size,
        "max": max,
        "min": min,
        "utilization_pct": format!("{:.1}", utilization),
    });

    (StatusCode::OK, axum::Json(body)).into_response()
}

// =============================================================================
// Graceful shutdown
// =============================================================================

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

// =============================================================================
// Helpers
// =============================================================================

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
