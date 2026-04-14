// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//
//! Postgres adapter for the [`Store`] trait.
//!
//! This is a thin wrapper around the existing [`PgTripleStore`]. Every
//! method delegates — nothing is rewritten. The point of the adapter
//! is to expose the Postgres-specific store through an object-safe
//! dynamic-dispatch interface so the rest of the codebase can hold
//! `Arc<dyn Store>` and the v0.4 portable query engine has a real
//! boundary to target.

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::query::{QueryPlan, execute_query};
use crate::triple_store::{PgTripleStore, Triple, TripleInput, TripleStore};
use crate::triple_store::schema::Schema;

use super::{Store, StoreTx};

/// Postgres [`Store`] backend. Wraps a [`PgTripleStore`].
///
/// Cloning is cheap — internally both the pool and the adapter are
/// reference-counted.
#[derive(Clone)]
pub struct PgStore {
    inner: PgTripleStore,
}

impl PgStore {
    /// Wrap an existing [`PgTripleStore`]. Does NOT run migrations;
    /// the caller is expected to have already constructed the
    /// underlying store via [`PgTripleStore::new`].
    pub fn new(inner: PgTripleStore) -> Self {
        Self { inner }
    }

    /// Return a reference to the wrapped Postgres triple store.
    pub fn inner(&self) -> &PgTripleStore {
        &self.inner
    }
}

/// Transaction handle for the Postgres backend.
///
/// NOTE: the v0.3.1 `Store` trait intentionally does NOT expose
/// multi-operation transactions through the dynamic dispatch surface.
/// `sqlx::Transaction<'a, Postgres>` is lifetime-bound to the pool
/// borrow, which does not round-trip cleanly through `Box<dyn StoreTx>`
/// without unsafe lifetime surgery. The v0.3.2 milestone tracks a
/// richer transaction API using sqlx's forthcoming owned-connection
/// primitive.
///
/// For now this handle is a stateless marker. Callers that genuinely
/// need a multi-statement transaction must reach through
/// [`PgStore::inner`] to the concrete [`PgTripleStore`] and use
/// [`PgTripleStore::begin_tx`] directly.
pub struct PgStoreTx {
    _private: (),
}

#[async_trait]
impl StoreTx for PgStoreTx {
    async fn commit(self: Box<Self>) -> Result<()> {
        // No-op: marker-only until v0.3.3 multi-statement StoreTx.
        // Matches SqliteStoreTx so behaviour is symmetric across backends.
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> Result<()> {
        // No-op: see `commit` — marker-only until v0.3.3.
        Ok(())
    }
}

#[async_trait]
impl Store for PgStore {
    fn backend_name(&self) -> &'static str {
        "postgres"
    }

    async fn set_triples(&self, tx_id: i64, triples: &[TripleInput]) -> Result<()> {
        // Delegate to PgTripleStore::set_triples_in_tx, which wants a
        // `sqlx::Transaction`. We start and commit one right here so
        // the call is self-contained — callers that want to batch
        // writes across multiple operations should use `begin_tx` +
        // raw PgTripleStore helpers until a richer StoreTx surface
        // lands in v0.3.2.
        if triples.is_empty() {
            return Ok(());
        }
        let mut tx = self
            .inner
            .begin_tx()
            .await
            .map_err(|e| match e {
                DarshJError::Database(err) => DarshJError::Database(err),
                other => other,
            })?;
        PgTripleStore::set_triples_in_tx(&mut tx, triples, tx_id).await?;
        tx.commit().await.map_err(DarshJError::Database)?;
        Ok(())
    }

    async fn get_entity(&self, entity_id: Uuid) -> Result<Vec<Triple>> {
        TripleStore::get_entity(&self.inner, entity_id).await
    }

    async fn retract(&self, entity_id: Uuid, attribute: &str) -> Result<()> {
        TripleStore::retract(&self.inner, entity_id, attribute).await
    }

    async fn query(&self, plan: &QueryPlan) -> Result<Vec<serde_json::Value>> {
        let rows = execute_query(self.inner.pool(), plan).await?;
        // Convert QueryResultRow into a flat JSON value per row so the
        // Store trait signature stays backend-neutral. Downstream
        // callers that need the typed structure can still use
        // `execute_query` directly.
        let out = rows
            .into_iter()
            .map(|r| {
                serde_json::to_value(&r).map_err(DarshJError::Serialization)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(out)
    }

    async fn get_schema(&self) -> Result<Schema> {
        TripleStore::get_schema(&self.inner).await
    }

    async fn next_tx_id(&self) -> Result<i64> {
        let row: (i64,) = sqlx::query_as("SELECT nextval('darshan_tx_seq')")
            .fetch_one(self.inner.pool())
            .await
            .map_err(DarshJError::Database)?;
        Ok(row.0)
    }

    async fn begin_tx(&self) -> Result<Box<dyn StoreTx + Send>> {
        // v0.3.1: return the marker handle. The real multi-statement
        // transaction API is pending v0.3.2 (see `PgStoreTx` docs).
        // We still round-trip to the pool here so connectivity errors
        // surface immediately instead of at the first mutating call.
        let _probe = self.inner.pool().acquire().await.map_err(DarshJError::Database)?;
        drop(_probe);
        Ok(Box::new(PgStoreTx { _private: () }))
    }
}
