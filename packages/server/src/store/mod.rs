// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//
//! Backend-agnostic storage trait for the triple store surface.
//!
//! The core triple store ([`crate::triple_store::PgTripleStore`]) is
//! Postgres-specific: it uses UUID casting, JSONB operators, text[]
//! arrays, `pg_notify`, and TimescaleDB hypertables under the hood.
//! That is perfect for production but makes the rest of the codebase
//! impossible to compile against any other backend.
//!
//! This module defines a minimal [`Store`] trait that abstracts the
//! operations the query engine, history, reactive tracker and HTTP
//! layer actually need — no more, no less. A Postgres adapter
//! ([`pg::PgStore`]) wraps the existing `PgTripleStore` by delegation,
//! so nothing is rewritten. A second adapter ([`sqlite::SqliteStore`],
//! behind the `sqlite-store` feature) provides a compile-time stub
//! against `rusqlite` so the trait boundary is validated against two
//! real backends.
//!
//! # Why a new trait?
//!
//! The existing [`crate::triple_store::TripleStore`] trait uses
//! `impl Future` return types, which makes it *not* object-safe. You
//! cannot hold an `Arc<dyn TripleStore>`. That is fine for a concrete
//! Postgres codebase, but for a pluggable backend system we need a
//! dynamic dispatch surface.
//!
//! This [`Store`] trait uses `async_trait` to preserve object safety,
//! and intentionally has a *narrower* surface than `TripleStore`. It
//! exposes only the methods that are likely portable across SQL,
//! embedded KV, and in-memory backends.
//!
//! # Roadmap
//!
//! - v0.3.1 — trait + Postgres adapter + SQLite stub (this commit).
//! - v0.3.2 — real SQLite backend + in-memory test backend.
//! - v0.4.0 — cross-backend DarshanQL planner emitting portable IR.
//!
//! See [`docs/STORAGE_BACKENDS.md`](../../../../docs/STORAGE_BACKENDS.md).

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::Result;
use crate::query::QueryPlan;
use crate::triple_store::schema::Schema;
use crate::triple_store::{Triple, TripleInput};

pub mod pg;

#[cfg(feature = "sqlite-store")]
pub mod sqlite;

/// Transaction handle returned by [`Store::begin_tx`].
///
/// Callers must call either [`StoreTx::commit`] or [`StoreTx::rollback`]
/// before the handle is dropped. Dropping without committing is a
/// rollback but will log a warning.
#[async_trait]
pub trait StoreTx: Send {
    /// Commit the transaction.
    async fn commit(self: Box<Self>) -> Result<()>;
    /// Roll back the transaction.
    async fn rollback(self: Box<Self>) -> Result<()>;
}

/// Backend-agnostic triple store surface.
///
/// All methods are object-safe (`async_trait`) and `Send + Sync`, so
/// `Arc<dyn Store>` is a valid runtime handle.
#[async_trait]
pub trait Store: Send + Sync {
    /// Human-readable backend identifier (e.g. `"postgres"`, `"sqlite"`).
    ///
    /// Used for structured logging, `/debug/pprof/backend` endpoints,
    /// and the `ddb_backend` Prometheus label.
    fn backend_name(&self) -> &'static str;

    /// Write a batch of triples under the given transaction id.
    ///
    /// Each triple is validated via [`TripleInput::validate`] before
    /// the insert is issued. Implementations SHOULD use a bulk insert
    /// path (e.g. `UNNEST` on Postgres, prepared multi-row on SQLite).
    async fn set_triples(&self, tx_id: i64, triples: &[TripleInput]) -> Result<()>;

    /// Retrieve all active (non-retracted, non-expired) triples for an entity.
    async fn get_entity(&self, entity_id: Uuid) -> Result<Vec<Triple>>;

    /// Retract every active triple matching `(entity_id, attribute)`.
    async fn retract(&self, entity_id: Uuid, attribute: &str) -> Result<()>;

    /// Execute a planned query and return rows as JSON values.
    ///
    /// This is where backend portability breaks down most acutely —
    /// DarshanQL currently emits Postgres-specific SQL, so non-Postgres
    /// implementations may return an `InvalidQuery` error for anything
    /// beyond trivial plans. See the v0.4 roadmap for the portable IR.
    async fn query(&self, plan: &QueryPlan) -> Result<Vec<serde_json::Value>>;

    /// Infer the current schema from the triple data.
    async fn get_schema(&self) -> Result<Schema>;

    /// Allocate the next monotonic transaction id from the backend's
    /// sequence source.
    async fn next_tx_id(&self) -> Result<i64>;

    /// Begin a new backend transaction. The returned handle must be
    /// committed or rolled back explicitly.
    async fn begin_tx(&self) -> Result<Box<dyn StoreTx + Send>>;
}
