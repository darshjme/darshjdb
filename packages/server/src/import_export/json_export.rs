//! Streaming JSON export from the EAV triple store.
//!
//! Queries triples by entity type, reassembles them into JSON objects,
//! and writes output incrementally as a JSON array or NDJSON stream.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

use super::ExportResult;

// ── Configuration ─────────────────────────────────────────────────────

/// Output format for JSON export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum JsonExportFormat {
    /// Standard JSON array `[{...}, {...}]`.
    #[default]
    Array,
    /// Newline-delimited JSON.
    Ndjson,
}

/// Configuration for a JSON export job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonExportConfig {
    /// Output format (default: array).
    #[serde(default)]
    pub format: JsonExportFormat,

    /// Whether to pretty-print the JSON output (default: `false`).
    #[serde(default)]
    pub pretty: bool,
}

impl Default for JsonExportConfig {
    fn default() -> Self {
        Self {
            format: JsonExportFormat::Array,
            pretty: false,
        }
    }
}

// ── Export ─────────────────────────────────────────────────────────────

/// Export entities of the given type as JSON bytes.
///
/// Streams triples from Postgres in pages, reassembles them into JSON
/// objects, and writes output incrementally. Each entity becomes a JSON
/// object with `id`, `created_at`, and all attribute values.
pub async fn export_json(
    pool: &PgPool,
    entity_type: &str,
    config: &JsonExportConfig,
) -> Result<(Vec<u8>, ExportResult), crate::error::DarshJError> {
    let start = Instant::now();

    // Fetch all entity IDs of this type.
    let entity_ids: Vec<(uuid::Uuid,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT entity_id
        FROM triples
        WHERE attribute = ':db/type'
          AND value = $1::jsonb
          AND NOT retracted
        ORDER BY entity_id
        "#,
    )
    .bind(Value::String(entity_type.to_string()))
    .fetch_all(pool)
    .await?;

    let page_size = 500;
    let mut entities_exported = 0usize;
    let mut output = Vec::new();

    // Write opening bracket for array format.
    if config.format == JsonExportFormat::Array {
        output.extend_from_slice(b"[\n");
    }

    let mut first_entity = true;

    for page_start in (0..entity_ids.len()).step_by(page_size) {
        let page_end = (page_start + page_size).min(entity_ids.len());
        let page_ids: Vec<uuid::Uuid> = entity_ids[page_start..page_end]
            .iter()
            .map(|(id,)| *id)
            .collect();

        // Fetch all non-retracted triples for this page.
        let triples: Vec<(uuid::Uuid, String, Value, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as(
                r#"
            SELECT entity_id, attribute, value, created_at
            FROM triples
            WHERE entity_id = ANY($1)
              AND NOT retracted
            ORDER BY entity_id, attribute, created_at
            "#,
            )
            .bind(&page_ids)
            .fetch_all(pool)
            .await?;

        // Group triples by entity.
        let mut entity_map: BTreeMap<
            uuid::Uuid,
            (BTreeMap<String, Vec<Value>>, chrono::DateTime<chrono::Utc>),
        > = BTreeMap::new();

        for (eid, attr, val, created_at) in &triples {
            let entry = entity_map
                .entry(*eid)
                .or_insert_with(|| (BTreeMap::new(), *created_at));
            // Track the earliest created_at.
            if *created_at < entry.1 {
                entry.1 = *created_at;
            }
            entry.0.entry(attr.clone()).or_default().push(val.clone());
        }

        // Build JSON objects.
        let prefix = format!("{}/", entity_type);
        for eid in &page_ids {
            if let Some((attrs, created_at)) = entity_map.get(eid) {
                let mut obj = serde_json::Map::new();
                obj.insert("id".into(), Value::String(eid.to_string()));
                obj.insert("created_at".into(), Value::String(created_at.to_rfc3339()));

                for (attr, values) in attrs {
                    if attr == ":db/type" {
                        continue;
                    }
                    // Strip entity_type prefix for cleaner keys.
                    let key = attr.strip_prefix(&prefix).unwrap_or(attr);

                    if values.len() == 1 {
                        obj.insert(key.into(), values[0].clone());
                    } else {
                        obj.insert(key.into(), Value::Array(values.clone()));
                    }
                }

                // Serialize and write.
                if config.format == JsonExportFormat::Array {
                    if !first_entity {
                        output.extend_from_slice(b",\n");
                    }
                    first_entity = false;
                }

                let json_bytes = if config.pretty {
                    serde_json::to_vec_pretty(&Value::Object(obj))
                } else {
                    serde_json::to_vec(&Value::Object(obj))
                }
                .map_err(|e| {
                    crate::error::DarshJError::Internal(format!("JSON serialize error: {e}"))
                })?;

                output.extend_from_slice(&json_bytes);

                if config.format == JsonExportFormat::Ndjson {
                    output.push(b'\n');
                }

                entities_exported += 1;
            }
        }
    }

    // Write closing bracket for array format.
    if config.format == JsonExportFormat::Array {
        output.extend_from_slice(b"\n]");
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    Ok((
        output,
        ExportResult {
            entities_exported,
            duration_ms,
        },
    ))
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = JsonExportConfig::default();
        assert_eq!(config.format, JsonExportFormat::Array);
        assert!(!config.pretty);
    }
}
