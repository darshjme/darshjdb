// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//
//! SQLite [`Store`] backend — **compile-time stub**.
//!
//! Full implementation is tracked as the v0.3.2 milestone. The stub
//! compiles against `rusqlite` so the [`Store`] trait surface is
//! validated against two real backends at the type level. Nothing in
//! the production server wires `SqliteStore` up — you must opt in via
//! the `sqlite-store` cargo feature, and even then every method
//! returns an `Internal("not yet implemented")` error.
//!
//! # Why a stub now?
//!
//! The architectural critique landed against v0.3.0 flagged zero
//! portability: every line of the server assumes PostgreSQL. We cannot
//! fix that overnight, but we *can* establish the trait boundary and
//! prove it holds up against a second backend (one that does not have
//! JSONB, does not have UUID casting, and does not have `text[]`).
//! The stub enforces that property.

#![cfg(feature = "sqlite-store")]

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Mutex;
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::query::QueryPlan;
use crate::triple_store::{Triple, TripleInput};
use crate::triple_store::schema::Schema;

use super::{Store, StoreTx};

/// SQLite-backed [`Store`] — not yet implemented.
///
/// Construction opens an in-process `rusqlite::Connection` so the
/// binding chain is exercised by the type checker. Every mutating or
/// query method returns [`DarshJError::Internal`] with a
/// "not yet implemented" message.
pub struct SqliteStore {
    // A single-threaded rusqlite connection guarded by a mutex. The
    // real backend will shard reads across a connection pool and
    // serialise writes through a WAL-mode primary, but for the stub
    // a mutex keeps the Send/Sync bounds honest without pulling in
    // extra crates.
    _conn: Mutex<rusqlite::Connection>,
    _path: PathBuf,
}

impl SqliteStore {
    /// Open a stub SQLite store at the given filesystem path.
    ///
    /// Pass `":memory:"` for an anonymous in-memory database. The
    /// stub does not create any schema — that is reserved for v0.3.2.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let conn = rusqlite::Connection::open(&path)
            .map_err(|e| DarshJError::Internal(format!("sqlite open failed: {e}")))?;
        Ok(Self {
            _conn: Mutex::new(conn),
            _path: path,
        })
    }
}

/// Marker transaction handle for the SQLite stub.
pub struct SqliteStoreTx {
    _private: (),
}

#[async_trait]
impl StoreTx for SqliteStoreTx {
    async fn commit(self: Box<Self>) -> Result<()> {
        Err(DarshJError::Internal(
            "SqliteStore: commit not yet implemented (v0.3.2)".into(),
        ))
    }

    async fn rollback(self: Box<Self>) -> Result<()> {
        Err(DarshJError::Internal(
            "SqliteStore: rollback not yet implemented (v0.3.2)".into(),
        ))
    }
}

fn not_yet(op: &'static str) -> DarshJError {
    DarshJError::Internal(format!(
        "SqliteStore::{op} not yet implemented — tracked as v0.3.2 milestone"
    ))
}

#[async_trait]
impl Store for SqliteStore {
    fn backend_name(&self) -> &'static str {
        "sqlite"
    }

    async fn set_triples(&self, _tx_id: i64, _triples: &[TripleInput]) -> Result<()> {
        Err(not_yet("set_triples"))
    }

    async fn get_entity(&self, _entity_id: Uuid) -> Result<Vec<Triple>> {
        Err(not_yet("get_entity"))
    }

    async fn retract(&self, _entity_id: Uuid, _attribute: &str) -> Result<()> {
        Err(not_yet("retract"))
    }

    async fn query(&self, _plan: &QueryPlan) -> Result<Vec<serde_json::Value>> {
        Err(not_yet("query"))
    }

    async fn get_schema(&self) -> Result<Schema> {
        Err(not_yet("get_schema"))
    }

    async fn next_tx_id(&self) -> Result<i64> {
        Err(not_yet("next_tx_id"))
    }

    async fn begin_tx(&self) -> Result<Box<dyn StoreTx + Send>> {
        Err(not_yet("begin_tx"))
    }
}
