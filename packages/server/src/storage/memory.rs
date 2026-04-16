//! In-memory data backend for DarshJDB.
//!
//! Uses [`DashMap`] for concurrent, lock-free access — ideal for
//! development, testing, and ephemeral workloads where persistence
//! is not required. All data lives in process memory and is lost
//! on restart.

use std::collections::BTreeMap;

use dashmap::DashMap;
use serde_json::Value;

use super::traits::DataBackend;
use crate::error::{DarshJError, Result};

// ---------------------------------------------------------------------------
// In-memory backend
// ---------------------------------------------------------------------------

/// A fully concurrent in-memory data backend.
///
/// Data is organized as `table -> (id -> value)` using nested
/// [`DashMap`] + [`BTreeMap`] for ordered scans.
pub struct MemoryBackend {
    /// Outer map: table name -> inner map of id -> value.
    /// We use DashMap<String, DashMap<...>> for concurrent table access
    /// and BTreeMap inside a parking_lot RwLock for ordered key scans.
    tables: DashMap<String, BTreeMap<String, Value>>,
}

impl MemoryBackend {
    /// Create a new empty in-memory backend.
    pub fn new() -> Self {
        Self {
            tables: DashMap::new(),
        }
    }

    /// Return the number of tables currently stored.
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    /// Return the total number of records across all tables.
    pub fn record_count(&self) -> usize {
        self.tables.iter().map(|t| t.value().len()).sum()
    }
}

impl Default for MemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl DataBackend for MemoryBackend {
    async fn get(&self, table: &str, id: &str) -> Result<Option<Value>> {
        Ok(self.tables.get(table).and_then(|t| t.get(id).cloned()))
    }

    async fn set(&self, table: &str, id: &str, value: Value) -> Result<()> {
        self.tables
            .entry(table.to_string())
            .or_default()
            .insert(id.to_string(), value);
        Ok(())
    }

    async fn delete(&self, table: &str, id: &str) -> Result<bool> {
        if let Some(mut table_map) = self.tables.get_mut(table) {
            Ok(table_map.remove(id).is_some())
        } else {
            Ok(false)
        }
    }

    async fn scan(
        &self,
        table: &str,
        prefix: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Value)>> {
        let Some(table_map) = self.tables.get(table) else {
            return Ok(Vec::new());
        };

        let results: Vec<(String, Value)> = match prefix {
            Some(pfx) => table_map
                .range(pfx.to_string()..)
                .take_while(|(k, _)| k.starts_with(pfx))
                .take(limit)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            None => table_map
                .iter()
                .take(limit)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };

        Ok(results)
    }

    async fn query(&self, sql: &str, _params: &[Value]) -> Result<Vec<Value>> {
        // The in-memory backend supports a minimal filter DSL:
        //   "SELECT * FROM <table>"                 — return all records
        //   "SELECT * FROM <table> WHERE id = '<id>'" — single lookup
        //
        // For anything more complex, use the PostgreSQL backend.
        let sql_lower = sql.trim().to_lowercase();

        if !sql_lower.starts_with("select") {
            return Err(DarshJError::InvalidQuery(
                "in-memory backend only supports SELECT queries".into(),
            ));
        }

        // Extract table name from "SELECT * FROM <table> ..."
        let parts: Vec<&str> = sql.split_whitespace().collect();
        let table_name = parts
            .iter()
            .position(|p| p.eq_ignore_ascii_case("from"))
            .and_then(|i| parts.get(i + 1))
            .ok_or_else(|| {
                DarshJError::InvalidQuery("could not parse table name from query".into())
            })?;

        let table_name = table_name.trim_end_matches(';');

        let Some(table_map) = self.tables.get(table_name) else {
            return Ok(Vec::new());
        };

        // Check for WHERE id = '<value>'
        if let Some(where_pos) = parts.iter().position(|p| p.eq_ignore_ascii_case("where"))
            && parts
                .get(where_pos + 1)
                .map(|s| s.eq_ignore_ascii_case("id"))
                == Some(true)
            && parts.get(where_pos + 2).map(|s| *s == "=") == Some(true)
            && let Some(id_val) = parts.get(where_pos + 3)
        {
            let id = id_val
                .trim_matches('\'')
                .trim_matches('"')
                .trim_end_matches(';');
            return Ok(table_map.get(id).into_iter().cloned().collect());
        }

        // No WHERE clause — return all records.
        Ok(table_map.values().cloned().collect())
    }

    fn backend_name(&self) -> &'static str {
        "memory"
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
    async fn test_get_set_delete() {
        let backend = MemoryBackend::new();

        // Initially empty.
        assert_eq!(backend.get("users", "1").await.unwrap(), None);

        // Set a value.
        backend
            .set("users", "1", json!({"name": "Darsh"}))
            .await
            .unwrap();
        assert_eq!(
            backend.get("users", "1").await.unwrap(),
            Some(json!({"name": "Darsh"}))
        );

        // Delete it.
        assert!(backend.delete("users", "1").await.unwrap());
        assert_eq!(backend.get("users", "1").await.unwrap(), None);

        // Double-delete returns false.
        assert!(!backend.delete("users", "1").await.unwrap());
    }

    #[tokio::test]
    async fn test_scan_with_prefix() {
        let backend = MemoryBackend::new();

        backend.set("kv", "user:1", json!("a")).await.unwrap();
        backend.set("kv", "user:2", json!("b")).await.unwrap();
        backend.set("kv", "order:1", json!("c")).await.unwrap();

        let users = backend.scan("kv", Some("user:"), 10).await.unwrap();
        assert_eq!(users.len(), 2);
        assert!(users.iter().all(|(k, _)| k.starts_with("user:")));

        let all = backend.scan("kv", None, 10).await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_scan_limit() {
        let backend = MemoryBackend::new();

        for i in 0..20 {
            backend
                .set("items", &format!("item:{i:03}"), json!(i))
                .await
                .unwrap();
        }

        let limited = backend.scan("items", None, 5).await.unwrap();
        assert_eq!(limited.len(), 5);
    }

    #[tokio::test]
    async fn test_query_select_all() {
        let backend = MemoryBackend::new();
        backend.set("docs", "a", json!({"x": 1})).await.unwrap();
        backend.set("docs", "b", json!({"x": 2})).await.unwrap();

        let results = backend.query("SELECT * FROM docs", &[]).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_query_select_by_id() {
        let backend = MemoryBackend::new();
        backend
            .set("docs", "myid", json!({"val": 42}))
            .await
            .unwrap();

        let results = backend
            .query("SELECT * FROM docs WHERE id = 'myid'", &[])
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], json!({"val": 42}));
    }

    #[tokio::test]
    async fn test_query_rejects_non_select() {
        let backend = MemoryBackend::new();
        assert!(backend.query("DELETE FROM docs", &[]).await.is_err());
    }

    #[tokio::test]
    async fn test_backend_name() {
        let backend = MemoryBackend::new();
        assert_eq!(backend.backend_name(), "memory");
    }
}
