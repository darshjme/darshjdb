//! Streaming CSV export from the EAV triple store.
//!
//! Queries triples by entity type, pivots them into tabular rows, and
//! writes CSV output incrementally — never loading all entities into
//! memory at once.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

use super::ExportResult;

// ── Configuration ─────────────────────────────────────────────────────

/// Configuration for a CSV export job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsvExportConfig {
    /// CSV field delimiter (default: `,`).
    #[serde(default = "default_delimiter")]
    pub delimiter: u8,

    /// Quote character (default: `"`).
    #[serde(default = "default_quote")]
    pub quote_char: u8,
}

fn default_delimiter() -> u8 {
    b','
}
fn default_quote() -> u8 {
    b'"'
}

impl Default for CsvExportConfig {
    fn default() -> Self {
        Self {
            delimiter: b',',
            quote_char: b'"',
        }
    }
}

// ── Export ─────────────────────────────────────────────────────────────

/// Export entities of the given type as CSV bytes.
///
/// The function streams triples from Postgres in pages, pivots them
/// into rows, and writes the CSV incrementally to `output`.
///
/// Multi-valued attributes (multiple triples with the same entity +
/// attribute) are joined with semicolons.
pub async fn export_csv(
    pool: &PgPool,
    entity_type: &str,
    fields: &[String],
    config: &CsvExportConfig,
) -> Result<(Vec<u8>, ExportResult), crate::error::DarshJError> {
    let start = Instant::now();

    // Step 1: Fetch all entity IDs of this type.
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

    // Step 2: Determine columns. If fields are specified use those,
    // otherwise discover all attributes for this entity type.
    let columns: Vec<String> = if fields.is_empty() {
        discover_attributes(pool, entity_type).await?
    } else {
        fields.to_vec()
    };

    // Step 3: Stream entities in pages and write CSV.
    let mut output = Vec::new();
    let mut writer = csv::WriterBuilder::new()
        .delimiter(config.delimiter)
        .quote(config.quote_char)
        .from_writer(&mut output);

    // Write header: id + columns.
    let mut header = vec!["id".to_string()];
    // Strip entity_type prefix from column names for cleaner headers.
    let prefix = format!("{}/", entity_type);
    for col in &columns {
        header.push(col.strip_prefix(&prefix).unwrap_or(col).to_string());
    }
    writer
        .write_record(&header)
        .map_err(|e| crate::error::DarshJError::Internal(format!("CSV write error: {e}")))?;

    let page_size = 500;
    let mut entities_exported = 0usize;

    for page_start in (0..entity_ids.len()).step_by(page_size) {
        let page_end = (page_start + page_size).min(entity_ids.len());
        let page_ids: Vec<uuid::Uuid> = entity_ids[page_start..page_end]
            .iter()
            .map(|(id,)| *id)
            .collect();

        // Fetch all non-retracted triples for this page of entities.
        let triples: Vec<(uuid::Uuid, String, Value)> = sqlx::query_as(
            r#"
            SELECT entity_id, attribute, value
            FROM triples
            WHERE entity_id = ANY($1)
              AND NOT retracted
              AND attribute != ':db/type'
            ORDER BY entity_id, attribute
            "#,
        )
        .bind(&page_ids)
        .fetch_all(pool)
        .await?;

        // Pivot triples into per-entity rows.
        let mut entity_map: BTreeMap<uuid::Uuid, BTreeMap<String, Vec<String>>> = BTreeMap::new();
        for (eid, attr, val) in &triples {
            entity_map
                .entry(*eid)
                .or_default()
                .entry(attr.clone())
                .or_default()
                .push(value_to_string(val));
        }

        // Write rows in entity_id order.
        for eid in &page_ids {
            let mut row = vec![eid.to_string()];
            if let Some(attrs) = entity_map.get(eid) {
                for col in &columns {
                    let cell = attrs
                        .get(col)
                        .map(|vals| vals.join(";"))
                        .unwrap_or_default();
                    row.push(cell);
                }
            } else {
                // Entity exists but has no non-type triples.
                for _ in &columns {
                    row.push(String::new());
                }
            }
            writer
                .write_record(&row)
                .map_err(|e| crate::error::DarshJError::Internal(format!("CSV write error: {e}")))?;
            entities_exported += 1;
        }
    }

    writer
        .flush()
        .map_err(|e| crate::error::DarshJError::Internal(format!("CSV flush error: {e}")))?;
    drop(writer);

    let duration_ms = start.elapsed().as_millis() as u64;

    Ok((
        output,
        ExportResult {
            entities_exported,
            duration_ms,
        },
    ))
}

/// Discover all distinct attributes for an entity type (excluding `:db/type`).
async fn discover_attributes(
    pool: &PgPool,
    entity_type: &str,
) -> Result<Vec<String>, crate::error::DarshJError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT t2.attribute
        FROM triples t1
        JOIN triples t2 ON t1.entity_id = t2.entity_id
        WHERE t1.attribute = ':db/type'
          AND t1.value = $1::jsonb
          AND NOT t1.retracted
          AND NOT t2.retracted
          AND t2.attribute != ':db/type'
        ORDER BY t2.attribute
        "#,
    )
    .bind(Value::String(entity_type.to_string()))
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(a,)| a).collect())
}

/// Convert a JSON value to a display string for CSV output.
fn value_to_string(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        // For objects/arrays, serialize as compact JSON.
        other => other.to_string(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_to_string_variants() {
        assert_eq!(value_to_string(&Value::String("hello".into())), "hello");
        assert_eq!(value_to_string(&Value::Bool(true)), "true");
        assert_eq!(value_to_string(&Value::Number(42.into())), "42");
        assert_eq!(value_to_string(&Value::Null), "");
        assert_eq!(
            value_to_string(&serde_json::json!({"a": 1})),
            r#"{"a":1}"#
        );
    }
}
