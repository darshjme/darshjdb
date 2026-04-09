//! Data storage engine selector for DarshJDB.
//!
//! Reads the `DDB_STORAGE` environment variable and instantiates the
//! appropriate [`DataBackend`] implementation. Wraps it in an `Arc`
//! so the backend can be shared across Axum handlers and background
//! tasks.
//!
//! # Configuration
//!
//! | `DDB_STORAGE` value | Backend            | Notes                              |
//! |---------------------|--------------------|------------------------------------|
//! | `memory`            | [`MemoryBackend`]  | Ephemeral, no persistence          |
//! | `postgres` (default)| [`PostgresBackend`]| Requires `DATABASE_URL`            |
//! | `file`              | Reserved           | RocksDB — not yet implemented      |

use std::sync::Arc;

use sqlx::PgPool;

use super::memory::MemoryBackend;
use super::postgres::PostgresBackend;
use super::traits::DataBackend;
use crate::error::{DarshJError, Result};

// ---------------------------------------------------------------------------
// Engine kind
// ---------------------------------------------------------------------------

/// Supported storage engine kinds, parsed from the `DDB_STORAGE` env var.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    /// In-memory backend (no persistence).
    Memory,
    /// PostgreSQL backend (production default).
    Postgres,
    /// File-based backend (RocksDB) — reserved for future use.
    File,
}

impl EngineKind {
    /// Parse the engine kind from a string. Falls back to [`EngineKind::Postgres`]
    /// for unrecognized values.
    pub fn from_env_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "memory" | "mem" | "inmemory" | "in-memory" => Self::Memory,
            "postgres" | "pg" | "postgresql" => Self::Postgres,
            "file" | "rocksdb" | "rocks" => Self::File,
            other => {
                tracing::warn!(
                    value = other,
                    "unknown DDB_STORAGE value, defaulting to postgres"
                );
                Self::Postgres
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Engine selector
// ---------------------------------------------------------------------------

/// The initialized data engine, holding an `Arc<dyn DataBackend>` that
/// can be cloned and shared freely.
#[derive(Clone)]
pub struct DataEngine {
    backend: Arc<dyn DataBackend>,
    kind: EngineKind,
}

impl DataEngine {
    /// Read `DDB_STORAGE` from the environment and build the matching backend.
    ///
    /// For the `postgres` backend, an existing [`PgPool`] must be provided.
    /// For `memory`, the pool is ignored (pass any pool or `None`).
    ///
    /// Calls [`DataBackend::init`] automatically after construction.
    pub async fn from_env(pool: Option<PgPool>) -> Result<Self> {
        let kind = EngineKind::from_env_str(
            &std::env::var("DDB_STORAGE").unwrap_or_else(|_| "postgres".to_string()),
        );

        Self::new(kind, pool).await
    }

    /// Build a [`DataEngine`] for a specific [`EngineKind`].
    pub async fn new(kind: EngineKind, pool: Option<PgPool>) -> Result<Self> {
        let backend: Arc<dyn DataBackend> = match kind {
            EngineKind::Memory => {
                tracing::info!("data engine: using in-memory backend (ephemeral)");
                Arc::new(MemoryBackend::new())
            }
            EngineKind::Postgres => {
                let pool = pool.ok_or_else(|| {
                    DarshJError::Internal(
                        "PostgreSQL backend requires a PgPool but none was provided".into(),
                    )
                })?;
                tracing::info!("data engine: using PostgreSQL backend");
                Arc::new(PostgresBackend::new(pool))
            }
            EngineKind::File => {
                return Err(DarshJError::Internal(
                    "file-based (RocksDB) backend is not yet implemented — \
                     set DDB_STORAGE=memory or DDB_STORAGE=postgres"
                        .into(),
                ));
            }
        };

        // Run one-time initialization (schema creation, etc.).
        backend.init().await?;

        tracing::info!(backend = backend.backend_name(), "data engine initialized");

        Ok(Self { backend, kind })
    }

    /// Return a reference to the underlying backend.
    pub fn backend(&self) -> &dyn DataBackend {
        self.backend.as_ref()
    }

    /// Return a cloneable `Arc` handle to the backend.
    pub fn backend_arc(&self) -> Arc<dyn DataBackend> {
        Arc::clone(&self.backend)
    }

    /// Which engine kind is active.
    pub fn kind(&self) -> EngineKind {
        self.kind
    }

    /// Gracefully shut down the backend.
    pub async fn shutdown(&self) -> Result<()> {
        self.backend.shutdown().await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_memory_engine_roundtrip() {
        let engine = DataEngine::new(EngineKind::Memory, None).await.unwrap();
        assert_eq!(engine.kind(), EngineKind::Memory);

        let b = engine.backend();
        b.set("test", "k1", json!({"hello": "world"}))
            .await
            .unwrap();

        let val = b.get("test", "k1").await.unwrap();
        assert_eq!(val, Some(json!({"hello": "world"})));

        let deleted = b.delete("test", "k1").await.unwrap();
        assert!(deleted);

        let val = b.get("test", "k1").await.unwrap();
        assert_eq!(val, None);
    }

    #[tokio::test]
    async fn test_engine_kind_parsing() {
        assert_eq!(EngineKind::from_env_str("memory"), EngineKind::Memory);
        assert_eq!(EngineKind::from_env_str("mem"), EngineKind::Memory);
        assert_eq!(EngineKind::from_env_str("in-memory"), EngineKind::Memory);
        assert_eq!(EngineKind::from_env_str("postgres"), EngineKind::Postgres);
        assert_eq!(EngineKind::from_env_str("pg"), EngineKind::Postgres);
        assert_eq!(EngineKind::from_env_str("postgresql"), EngineKind::Postgres);
        assert_eq!(EngineKind::from_env_str("file"), EngineKind::File);
        assert_eq!(EngineKind::from_env_str("rocksdb"), EngineKind::File);
        // Unknown defaults to postgres.
        assert_eq!(EngineKind::from_env_str("banana"), EngineKind::Postgres);
    }

    #[tokio::test]
    async fn test_postgres_engine_requires_pool() {
        let result = DataEngine::new(EngineKind::Postgres, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_engine_not_implemented() {
        let result = DataEngine::new(EngineKind::File, None).await;
        assert!(result.is_err());
    }
}
