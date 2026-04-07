//! Streaming CSV import into the EAV triple store.
//!
//! Parses CSV rows incrementally, auto-detects field types, and flushes
//! triples in configurable batches using the UNNEST bulk-insert path.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::triple_store::{PgTripleStore, TripleInput};

use super::mapping::infer_value_type_from_str;
use super::{ImportError, ImportResult};

// ── Configuration ─────────────────────────────────────────────────────

/// Configuration for a CSV import job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsvImportConfig {
    /// Entity type name to assign to all imported records (e.g. `"users"`).
    pub entity_type: String,

    /// Map from CSV column name to EAV attribute name.
    /// When empty, columns are mapped as `<entity_type>/<column_name>`.
    #[serde(default)]
    pub field_mapping: HashMap<String, String>,

    /// CSV field delimiter (default: `,`).
    #[serde(default = "default_delimiter")]
    pub delimiter: u8,

    /// Whether the first row is a header row (default: `true`).
    #[serde(default = "default_true")]
    pub has_header: bool,

    /// Whether to skip rows that fail parsing instead of aborting
    /// the entire import (default: `false`).
    #[serde(default)]
    pub skip_errors: bool,

    /// Number of rows per transaction batch (default: 1000).
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

fn default_delimiter() -> u8 {
    b','
}
fn default_true() -> bool {
    true
}
fn default_batch_size() -> usize {
    1000
}

impl Default for CsvImportConfig {
    fn default() -> Self {
        Self {
            entity_type: String::new(),
            field_mapping: HashMap::new(),
            delimiter: b',',
            has_header: true,
            skip_errors: false,
            batch_size: 1000,
        }
    }
}

// ── Import ────────────────────────────────────────────────────────────

/// Import CSV data from raw bytes into the triple store.
///
/// Reads the CSV synchronously from `data` (already buffered from the
/// multipart upload), but writes to Postgres in streaming batches so
/// we never hold more than `batch_size` triples in memory at once.
pub async fn import_csv(
    pool: &PgPool,
    data: &[u8],
    config: &CsvImportConfig,
) -> Result<ImportResult, crate::error::DarshJError> {
    let start = Instant::now();

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(config.delimiter)
        .has_headers(config.has_header)
        .flexible(true)
        .from_reader(data);

    // Resolve headers.
    let headers: Vec<String> = if config.has_header {
        reader
            .headers()
            .map_err(|e| crate::error::DarshJError::InvalidQuery(format!("CSV header error: {e}")))?
            .iter()
            .map(|h| h.to_string())
            .collect()
    } else {
        Vec::new()
    };

    // Build column-index-to-attribute mapping.
    let col_map = build_column_map(&headers, &config.field_mapping, &config.entity_type);

    let store = PgTripleStore::new_lazy(pool.clone());
    let mut rows_processed: usize = 0;
    let mut rows_imported: usize = 0;
    let mut rows_skipped: usize = 0;
    let mut triples_written: usize = 0;
    let mut errors: Vec<ImportError> = Vec::new();
    let mut batch: Vec<TripleInput> = Vec::with_capacity(config.batch_size * 4);
    let mut batch_row_count: usize = 0;

    for result in reader.records() {
        let row_idx = rows_processed;
        rows_processed += 1;

        let record = match result {
            Ok(r) => r,
            Err(e) => {
                errors.push(ImportError {
                    row: row_idx,
                    message: format!("CSV parse error: {e}"),
                });
                rows_skipped += 1;
                if !config.skip_errors {
                    return Err(crate::error::DarshJError::InvalidQuery(format!(
                        "CSV parse error at row {row_idx}: {e}"
                    )));
                }
                continue;
            }
        };

        let entity_id = Uuid::new_v4();

        // Add :db/type triple.
        batch.push(TripleInput {
            entity_id,
            attribute: ":db/type".to_string(),
            value: Value::String(config.entity_type.clone()),
            value_type: 0,
            ttl_seconds: None,
        });

        // Add a triple for each mapped column.
        for (col_idx, attr_name) in &col_map {
            if let Some(raw) = record.get(*col_idx) {
                if raw.is_empty() {
                    continue;
                }
                let (value, value_type) = infer_value_type_from_str(raw);
                batch.push(TripleInput {
                    entity_id,
                    attribute: attr_name.clone(),
                    value,
                    value_type,
                    ttl_seconds: None,
                });
            }
        }

        rows_imported += 1;
        batch_row_count += 1;

        // Flush batch when it reaches the configured size.
        if batch_row_count >= config.batch_size {
            let count = batch.len();
            store.bulk_load(std::mem::take(&mut batch)).await?;
            triples_written += count;
            batch_row_count = 0;
            batch = Vec::with_capacity(config.batch_size * 4);
        }
    }

    // Flush remaining triples.
    if !batch.is_empty() {
        let count = batch.len();
        store.bulk_load(batch).await?;
        triples_written += count;
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    Ok(ImportResult {
        rows_processed,
        rows_imported,
        rows_skipped,
        errors,
        triples_written,
        duration_ms,
    })
}

/// Build a mapping from column index to EAV attribute name.
fn build_column_map(
    headers: &[String],
    field_mapping: &HashMap<String, String>,
    entity_type: &str,
) -> HashMap<usize, String> {
    let mut map = HashMap::new();

    if headers.is_empty() {
        // No headers — columns are mapped positionally.
        // Without explicit mapping we can't do much, but honour any
        // numeric-string keys in field_mapping (e.g. "0" -> "user/name").
        for (key, attr) in field_mapping {
            if let Ok(idx) = key.parse::<usize>() {
                map.insert(idx, attr.clone());
            }
        }
        return map;
    }

    for (idx, header) in headers.iter().enumerate() {
        let header_trimmed = header.trim();
        if header_trimmed.is_empty() {
            continue;
        }

        // Check explicit mapping first.
        if let Some(attr) = field_mapping.get(header_trimmed) {
            map.insert(idx, attr.clone());
        } else {
            // Default: <entity_type>/<column_name>
            let attr = format!("{}/{}", entity_type, header_trimmed);
            map.insert(idx, attr);
        }
    }

    map
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_column_map_default_naming() {
        let headers = vec!["name".into(), "age".into(), "active".into()];
        let mapping = HashMap::new();
        let map = build_column_map(&headers, &mapping, "user");
        assert_eq!(map.get(&0), Some(&"user/name".to_string()));
        assert_eq!(map.get(&1), Some(&"user/age".to_string()));
        assert_eq!(map.get(&2), Some(&"user/active".to_string()));
    }

    #[test]
    fn build_column_map_explicit_override() {
        let headers = vec!["name".into(), "age".into()];
        let mut mapping = HashMap::new();
        mapping.insert("name".into(), "person/full_name".into());
        let map = build_column_map(&headers, &mapping, "user");
        assert_eq!(map.get(&0), Some(&"person/full_name".to_string()));
        assert_eq!(map.get(&1), Some(&"user/age".to_string()));
    }

    #[test]
    fn build_column_map_no_headers() {
        let headers: Vec<String> = vec![];
        let mut mapping = HashMap::new();
        mapping.insert("0".into(), "user/name".into());
        mapping.insert("1".into(), "user/age".into());
        let map = build_column_map(&headers, &mapping, "user");
        assert_eq!(map.get(&0), Some(&"user/name".to_string()));
        assert_eq!(map.get(&1), Some(&"user/age".to_string()));
    }
}
