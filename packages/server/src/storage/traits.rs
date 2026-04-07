//! Data storage backend trait for DarshJDB.
//!
//! Defines the [`DataBackend`] trait — a pluggable interface for key-value
//! and query operations against different storage engines (in-memory,
//! PostgreSQL, file-based). This is *separate* from the file/object
//! [`StorageBackend`] in the parent module; `DataBackend` handles
//! structured application data (documents, rows, triples).

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;

// ---------------------------------------------------------------------------
// Data backend trait
// ---------------------------------------------------------------------------

/// Pluggable data storage backend.
///
/// Implementations handle structured data I/O against different engines:
/// in-memory hashmaps, PostgreSQL, RocksDB (file-based), etc. The trait
/// uses `#[async_trait]` so it can be used behind `Arc<dyn DataBackend>`.
///
/// # Table semantics
///
/// `table` is a logical namespace (e.g. `"users"`, `"triples"`). Backends
/// may map this to a database table, a HashMap key prefix, a column family,
/// or a directory — whatever is natural for the engine.
#[async_trait]
pub trait DataBackend: Send + Sync {
    /// Retrieve a single record by primary key.
    ///
    /// Returns `None` if the key does not exist in the table.
    async fn get(&self, table: &str, id: &str) -> Result<Option<Value>>;

    /// Upsert a record. If a record with the same `id` already exists in
    /// `table`, it is replaced.
    async fn set(&self, table: &str, id: &str, value: Value) -> Result<()>;

    /// Delete a record by primary key. Returns `true` if the record existed
    /// and was removed, `false` if it was not found.
    async fn delete(&self, table: &str, id: &str) -> Result<bool>;

    /// Scan records in a table, optionally filtering by key prefix.
    ///
    /// Results are ordered by key and limited to at most `limit` entries.
    /// Each entry is a `(key, value)` pair.
    async fn scan(
        &self,
        table: &str,
        prefix: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Value)>>;

    /// Execute a raw query string with positional parameters.
    ///
    /// This is backend-specific: PostgreSQL backends accept SQL, the
    /// in-memory backend accepts a minimal filter DSL, etc.
    ///
    /// Returns a vector of result rows, each serialized as a JSON value.
    async fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<Value>>;

    /// Return a human-readable name for this backend (e.g. `"memory"`,
    /// `"postgres"`, `"file"`).
    fn backend_name(&self) -> &'static str;

    /// Perform any one-time initialization (schema creation, directory
    /// setup, etc.). Called once at startup.
    async fn init(&self) -> Result<()> {
        Ok(())
    }

    /// Graceful shutdown — flush buffers, close connections, etc.
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
