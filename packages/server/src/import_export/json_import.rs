//! Streaming JSON import into the EAV triple store.
//!
//! Supports two formats:
//! - **JSON array** — `[{...}, {...}, ...]` parsed all at once.
//! - **NDJSON** — one JSON object per line, parsed line-by-line for
//!   true streaming with constant memory usage.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::triple_store::{PgTripleStore, TripleInput};

use super::{ImportError, ImportResult};

// ── Configuration ─────────────────────────────────────────────────────

/// Input format discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum JsonFormat {
    /// Standard JSON array `[{...}, {...}]`.
    Array,
    /// Newline-delimited JSON (one object per line).
    Ndjson,
    /// Auto-detect: try array first, fall back to NDJSON.
    #[default]
    Auto,
}

/// Configuration for a JSON import job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonImportConfig {
    /// Entity type name to assign to all imported records.
    pub entity_type: String,

    /// Map from JSON key to EAV attribute name.
    /// When empty, keys are mapped as `<entity_type>/<key>`.
    #[serde(default)]
    pub field_mapping: HashMap<String, String>,

    /// Input format (default: auto-detect).
    #[serde(default)]
    pub format: JsonFormat,

    /// Whether to skip objects that fail parsing (default: `false`).
    #[serde(default)]
    pub skip_errors: bool,

    /// Number of objects per transaction batch (default: 1000).
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

fn default_batch_size() -> usize {
    1000
}

impl Default for JsonImportConfig {
    fn default() -> Self {
        Self {
            entity_type: String::new(),
            field_mapping: HashMap::new(),
            format: JsonFormat::Auto,
            skip_errors: false,
            batch_size: 1000,
        }
    }
}

// ── Import ────────────────────────────────────────────────────────────

/// Import JSON data from raw bytes into the triple store.
pub async fn import_json(
    pool: &PgPool,
    data: &[u8],
    config: &JsonImportConfig,
) -> Result<ImportResult, crate::error::DarshJError> {
    let start = Instant::now();

    // Determine format.
    let format = match config.format {
        JsonFormat::Auto => detect_format(data),
        other => other,
    };

    match format {
        JsonFormat::Array => import_json_array(pool, data, config, start).await,
        JsonFormat::Ndjson | JsonFormat::Auto => import_ndjson(pool, data, config, start).await,
    }
}

/// Detect whether the input is a JSON array or NDJSON.
fn detect_format(data: &[u8]) -> JsonFormat {
    // Skip leading whitespace.
    for &b in data {
        match b {
            b' ' | b'\t' | b'\n' | b'\r' => continue,
            b'[' => return JsonFormat::Array,
            _ => return JsonFormat::Ndjson,
        }
    }
    JsonFormat::Ndjson
}

/// Import a JSON array `[{...}, {...}]`.
async fn import_json_array(
    pool: &PgPool,
    data: &[u8],
    config: &JsonImportConfig,
    start: Instant,
) -> Result<ImportResult, crate::error::DarshJError> {
    let objects: Vec<Value> = serde_json::from_slice(data)
        .map_err(|e| crate::error::DarshJError::InvalidQuery(format!("JSON parse error: {e}")))?;

    let store = PgTripleStore::new_lazy(pool.clone());
    let mut rows_processed = 0usize;
    let mut rows_imported = 0usize;
    let mut rows_skipped = 0usize;
    let mut triples_written = 0usize;
    let mut errors: Vec<ImportError> = Vec::new();
    let mut batch: Vec<TripleInput> = Vec::with_capacity(config.batch_size * 4);
    let mut batch_row_count = 0usize;

    for (idx, obj) in objects.into_iter().enumerate() {
        rows_processed += 1;

        match convert_object_to_triples(&obj, config, idx) {
            Ok(triples) => {
                batch.extend(triples);
                rows_imported += 1;
                batch_row_count += 1;
            }
            Err(e) => {
                errors.push(e);
                rows_skipped += 1;
                if !config.skip_errors {
                    return Err(crate::error::DarshJError::InvalidQuery(format!(
                        "JSON import error at object {idx}"
                    )));
                }
                continue;
            }
        }

        if batch_row_count >= config.batch_size {
            let count = batch.len();
            store.bulk_load(std::mem::take(&mut batch)).await?;
            triples_written += count;
            batch_row_count = 0;
            batch = Vec::with_capacity(config.batch_size * 4);
        }
    }

    if !batch.is_empty() {
        let count = batch.len();
        store.bulk_load(batch).await?;
        triples_written += count;
    }

    Ok(ImportResult {
        rows_processed,
        rows_imported,
        rows_skipped,
        errors,
        triples_written,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Import NDJSON (one JSON object per line).
async fn import_ndjson(
    pool: &PgPool,
    data: &[u8],
    config: &JsonImportConfig,
    start: Instant,
) -> Result<ImportResult, crate::error::DarshJError> {
    let store = PgTripleStore::new_lazy(pool.clone());
    let mut rows_processed = 0usize;
    let mut rows_imported = 0usize;
    let mut rows_skipped = 0usize;
    let mut triples_written = 0usize;
    let mut errors: Vec<ImportError> = Vec::new();
    let mut batch: Vec<TripleInput> = Vec::with_capacity(config.batch_size * 4);
    let mut batch_row_count = 0usize;

    let text = std::str::from_utf8(data)
        .map_err(|e| crate::error::DarshJError::InvalidQuery(format!("Invalid UTF-8: {e}")))?;

    for (line_idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        rows_processed += 1;

        let obj: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                errors.push(ImportError {
                    row: line_idx,
                    message: format!("JSON parse error: {e}"),
                });
                rows_skipped += 1;
                if !config.skip_errors {
                    return Err(crate::error::DarshJError::InvalidQuery(format!(
                        "NDJSON parse error at line {line_idx}: {e}"
                    )));
                }
                continue;
            }
        };

        match convert_object_to_triples(&obj, config, line_idx) {
            Ok(triples) => {
                batch.extend(triples);
                rows_imported += 1;
                batch_row_count += 1;
            }
            Err(e) => {
                errors.push(e);
                rows_skipped += 1;
                if !config.skip_errors {
                    return Err(crate::error::DarshJError::InvalidQuery(format!(
                        "JSON import error at line {line_idx}"
                    )));
                }
                continue;
            }
        }

        if batch_row_count >= config.batch_size {
            let count = batch.len();
            store.bulk_load(std::mem::take(&mut batch)).await?;
            triples_written += count;
            batch_row_count = 0;
            batch = Vec::with_capacity(config.batch_size * 4);
        }
    }

    if !batch.is_empty() {
        let count = batch.len();
        store.bulk_load(batch).await?;
        triples_written += count;
    }

    Ok(ImportResult {
        rows_processed,
        rows_imported,
        rows_skipped,
        errors,
        triples_written,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Convert a single JSON object into a list of [`TripleInput`] values.
fn convert_object_to_triples(
    obj: &Value,
    config: &JsonImportConfig,
    row_idx: usize,
) -> Result<Vec<TripleInput>, ImportError> {
    let map = match obj.as_object() {
        Some(m) => m,
        None => {
            return Err(ImportError {
                row: row_idx,
                message: "Expected a JSON object".to_string(),
            });
        }
    };

    // Use `id` field from the object if present, otherwise generate a new UUID.
    let entity_id = map
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(Uuid::new_v4);

    let mut triples = Vec::with_capacity(map.len() + 1);

    // :db/type triple.
    triples.push(TripleInput {
        entity_id,
        attribute: ":db/type".to_string(),
        value: Value::String(config.entity_type.clone()),
        value_type: 0,
        ttl_seconds: None,
    });

    for (key, value) in map {
        // Skip the `id` field since it's used as entity_id.
        if key == "id" {
            continue;
        }

        let attr = if let Some(mapped) = config.field_mapping.get(key) {
            mapped.clone()
        } else {
            format!("{}/{}", config.entity_type, key)
        };

        let value_type = infer_json_value_type(value);

        triples.push(TripleInput {
            entity_id,
            attribute: attr,
            value: value.clone(),
            value_type,
            ttl_seconds: None,
        });
    }

    Ok(triples)
}

/// Infer the EAV value type tag from a JSON value.
fn infer_json_value_type(value: &Value) -> i16 {
    match value {
        Value::String(s) => {
            // Check if it looks like a UUID (reference).
            if s.len() == 36 && Uuid::parse_str(s).is_ok() {
                5 // Reference
            } else if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
                4 // Timestamp
            } else {
                0 // String
            }
        }
        Value::Number(n) => {
            if n.is_f64() && !n.is_i64() && !n.is_u64() {
                2 // Float
            } else {
                1 // Integer
            }
        }
        Value::Bool(_) => 3,
        Value::Object(_) | Value::Array(_) => 6, // Json
        Value::Null => 0,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_format_array() {
        assert_eq!(detect_format(b"  [{\"a\":1}]"), JsonFormat::Array);
    }

    #[test]
    fn detect_format_ndjson() {
        assert_eq!(detect_format(b"{\"a\":1}\n{\"b\":2}"), JsonFormat::Ndjson);
    }

    #[test]
    fn convert_object_basic() {
        let config = JsonImportConfig {
            entity_type: "user".into(),
            ..Default::default()
        };
        let obj = serde_json::json!({"name": "Alice", "age": 30});
        let triples = convert_object_to_triples(&obj, &config, 0).unwrap();

        // :db/type + name + age = 3 triples.
        assert_eq!(triples.len(), 3);
        assert_eq!(triples[0].attribute, ":db/type");

        let attrs: Vec<&str> = triples.iter().map(|t| t.attribute.as_str()).collect();
        assert!(attrs.contains(&"user/name"));
        assert!(attrs.contains(&"user/age"));
    }

    #[test]
    fn convert_object_with_id() {
        let config = JsonImportConfig {
            entity_type: "user".into(),
            ..Default::default()
        };
        let id = Uuid::new_v4();
        let obj = serde_json::json!({"id": id.to_string(), "name": "Bob"});
        let triples = convert_object_to_triples(&obj, &config, 0).unwrap();

        // Should use the provided id, and not create a triple for "id".
        assert_eq!(triples[0].entity_id, id);
        assert!(!triples.iter().any(|t| t.attribute == "user/id"));
    }

    #[test]
    fn convert_object_with_field_mapping() {
        let mut mapping = HashMap::new();
        mapping.insert("name".into(), "person/full_name".into());
        let config = JsonImportConfig {
            entity_type: "user".into(),
            field_mapping: mapping,
            ..Default::default()
        };
        let obj = serde_json::json!({"name": "Carol", "age": 25});
        let triples = convert_object_to_triples(&obj, &config, 0).unwrap();

        let attrs: Vec<&str> = triples.iter().map(|t| t.attribute.as_str()).collect();
        assert!(attrs.contains(&"person/full_name"));
        assert!(attrs.contains(&"user/age"));
    }

    #[test]
    fn infer_json_types() {
        assert_eq!(infer_json_value_type(&Value::String("hello".into())), 0);
        assert_eq!(infer_json_value_type(&Value::Number(42.into())), 1);
        assert_eq!(infer_json_value_type(&Value::Bool(true)), 3);
        assert_eq!(
            infer_json_value_type(&serde_json::json!({"nested": true})),
            6
        );
    }

    #[test]
    fn convert_non_object_errors() {
        let config = JsonImportConfig {
            entity_type: "user".into(),
            ..Default::default()
        };
        let result = convert_object_to_triples(&Value::Array(vec![]), &config, 0);
        assert!(result.is_err());
    }
}
