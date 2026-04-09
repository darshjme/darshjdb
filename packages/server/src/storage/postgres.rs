//! PostgreSQL data backend for DarshJDB.
//!
//! Wraps the existing `sqlx::PgPool` to implement [`DataBackend`],
//! providing a key-value interface over a `ddb_data` table while
//! retaining full SQL query capability through the `query()` method.
//!
//! This backend stores records as JSONB values keyed by `(table, id)`,
//! giving you both structured key-value access *and* the power of
//! PostgreSQL's JSONB operators, indexes, and full SQL.

use serde_json::Value;
use sqlx::PgPool;

use super::traits::DataBackend;
use crate::error::{DarshJError, Result};

// ---------------------------------------------------------------------------
// PostgreSQL backend
// ---------------------------------------------------------------------------

/// Production data backend backed by PostgreSQL via `sqlx`.
///
/// Records are stored in a single `ddb_data` table with columns:
///
/// | Column     | Type   | Description                    |
/// |------------|--------|--------------------------------|
/// | `tbl`      | TEXT   | Logical table / namespace      |
/// | `id`       | TEXT   | Primary key within the table   |
/// | `data`     | JSONB  | The record payload             |
/// | `updated_at` | TIMESTAMPTZ | Last write timestamp     |
///
/// The composite primary key is `(tbl, id)`.
#[derive(Clone)]
pub struct PostgresBackend {
    pool: PgPool,
}

impl PostgresBackend {
    /// Create a new PostgreSQL backend from an existing connection pool.
    ///
    /// Call [`DataBackend::init`] after construction to ensure the
    /// `ddb_data` table exists.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Return a reference to the underlying connection pool.
    ///
    /// Useful for code that still needs raw `sqlx` access (e.g.
    /// the triple store, auth tables).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait::async_trait]
impl DataBackend for PostgresBackend {
    async fn get(&self, table: &str, id: &str) -> Result<Option<Value>> {
        let row: Option<(Value,)> =
            sqlx::query_as("SELECT data FROM ddb_data WHERE tbl = $1 AND id = $2")
                .bind(table)
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(DarshJError::Database)?;

        Ok(row.map(|(data,)| data))
    }

    async fn set(&self, table: &str, id: &str, value: Value) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO ddb_data (tbl, id, data, updated_at)
            VALUES ($1, $2, $3, now())
            ON CONFLICT (tbl, id)
            DO UPDATE SET data = EXCLUDED.data, updated_at = now()
            "#,
        )
        .bind(table)
        .bind(id)
        .bind(&value)
        .execute(&self.pool)
        .await
        .map_err(DarshJError::Database)?;

        Ok(())
    }

    async fn delete(&self, table: &str, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM ddb_data WHERE tbl = $1 AND id = $2")
            .bind(table)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(DarshJError::Database)?;

        Ok(result.rows_affected() > 0)
    }

    async fn scan(
        &self,
        table: &str,
        prefix: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Value)>> {
        let rows: Vec<(String, Value)> = match prefix {
            Some(pfx) => {
                // Use a prefix range scan: id >= prefix AND id < prefix+1
                // (where prefix+1 is the prefix with its last byte incremented).
                let upper = prefix_upper_bound(pfx);
                sqlx::query_as(
                    r#"
                    SELECT id, data FROM ddb_data
                    WHERE tbl = $1 AND id >= $2 AND id < $3
                    ORDER BY id
                    LIMIT $4
                    "#,
                )
                .bind(table)
                .bind(pfx)
                .bind(&upper)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(DarshJError::Database)?
            }
            None => sqlx::query_as(
                r#"
                    SELECT id, data FROM ddb_data
                    WHERE tbl = $1
                    ORDER BY id
                    LIMIT $2
                    "#,
            )
            .bind(table)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(DarshJError::Database)?,
        };

        Ok(rows)
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<Value>> {
        // Build a raw query with positional bind parameters.
        // Each param is bound as JSONB — the caller can cast in SQL as needed.
        let mut q = sqlx::query_as::<_, (Value,)>(sql);

        for param in params {
            q = q.bind(param);
        }

        let rows = q
            .fetch_all(&self.pool)
            .await
            .map_err(DarshJError::Database)?;

        Ok(rows.into_iter().map(|(v,)| v).collect())
    }

    fn backend_name(&self) -> &'static str {
        "postgres"
    }

    async fn init(&self) -> Result<()> {
        sqlx::raw_sql(
            r#"
            CREATE TABLE IF NOT EXISTS ddb_data (
                tbl         TEXT        NOT NULL,
                id          TEXT        NOT NULL,
                data        JSONB       NOT NULL DEFAULT '{}',
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (tbl, id)
            );

            -- Index for prefix scans within a table.
            CREATE INDEX IF NOT EXISTS idx_ddb_data_tbl_id
                ON ddb_data (tbl, id);

            -- GIN index for JSONB queries on the data column.
            CREATE INDEX IF NOT EXISTS idx_ddb_data_gin
                ON ddb_data USING gin (data);
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(DarshJError::Database)?;

        tracing::info!("PostgreSQL data backend: ddb_data table ensured");
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        // The pool is shared and managed externally; nothing to do here.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the exclusive upper bound for a prefix scan.
///
/// Given `"user:"`, returns `"user;"` (the byte after `:` is `;`).
/// This allows an efficient `id >= prefix AND id < upper` range query
/// that uses the B-tree index.
fn prefix_upper_bound(prefix: &str) -> String {
    let mut bytes = prefix.as_bytes().to_vec();
    // Increment the last byte. If it overflows (0xFF), pop and try the
    // previous byte, etc. If the entire prefix is 0xFF bytes, fall back
    // to an empty string (which effectively means "no upper bound" —
    // the caller should handle this, but in practice keys are never
    // all-0xFF).
    while let Some(last) = bytes.last_mut() {
        if *last < 0xFF {
            *last += 1;
            return String::from_utf8_lossy(&bytes).into_owned();
        }
        bytes.pop();
    }
    // Fallback: return a string that sorts after everything.
    "\u{FFFF}".to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefix_upper_bound() {
        assert_eq!(prefix_upper_bound("user:"), "user;");
        assert_eq!(prefix_upper_bound("a"), "b");
        assert_eq!(prefix_upper_bound("abc"), "abd");
    }
}
