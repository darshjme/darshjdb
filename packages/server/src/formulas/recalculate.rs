//! Batch recalculation engine: propagates field changes through the dependency
//! graph and persists updated formula values within a single Postgres transaction.

use std::collections::HashMap;
use std::time::Instant;

use sqlx::PgPool;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::formulas::evaluator::{RecordContext, evaluate};
use crate::formulas::graph::DependencyGraph;

/// Metrics captured during a recalculation pass.
#[derive(Debug, Clone)]
pub struct RecalcMetrics {
    /// Total wall-clock time for the recalculation batch.
    pub duration_ms: u64,
    /// Number of formula fields recalculated.
    pub fields_recalculated: usize,
    /// Number of SQL updates executed.
    pub updates_executed: usize,
}

/// Recalculate all formula fields affected by a mutation on a single entity.
///
/// 1. Determines the topological recalculation order from the [`DependencyGraph`].
/// 2. Fetches current field values for the entity from the triple store.
/// 3. Evaluates each affected formula in dependency order, accumulating results.
/// 4. Writes all updated values back in a single SQL transaction.
///
/// # Arguments
///
/// * `pool` — Postgres connection pool.
/// * `entity_id` — The entity whose fields changed.
/// * `changed_attributes` — The raw (non-formula) attributes that were mutated.
/// * `graph` — The formula dependency graph.
/// * `tx_id` — Transaction id to stamp on new triples.
pub async fn recalculate_affected(
    pool: &PgPool,
    entity_id: Uuid,
    changed_attributes: &[String],
    graph: &DependencyGraph,
    tx_id: i64,
) -> Result<RecalcMetrics> {
    let start = Instant::now();

    // 1. Determine recalculation order
    let order = graph.calculation_order(changed_attributes);
    if order.is_empty() {
        return Ok(RecalcMetrics {
            duration_ms: 0,
            fields_recalculated: 0,
            updates_executed: 0,
        });
    }

    debug!(
        entity_id = %entity_id,
        changed = ?changed_attributes,
        order = ?order,
        "recalculating {} formula fields",
        order.len()
    );

    // 2. Fetch current field values for this entity
    let rows = sqlx::query_as::<_, (String, serde_json::Value)>(
        r#"
        SELECT attribute, value
        FROM triples
        WHERE entity_id = $1
          AND retracted = false
          AND (expires_at IS NULL OR expires_at > NOW())
        ORDER BY tx_id DESC
        "#,
    )
    .bind(entity_id)
    .fetch_all(pool)
    .await
    .map_err(DarshJError::Database)?;

    // Build field map (latest value per attribute)
    let mut field_values: HashMap<String, serde_json::Value> = HashMap::new();
    for (attr, val) in rows {
        field_values.entry(attr).or_insert(val);
    }

    // 3. Evaluate each formula in topological order
    let mut updates: Vec<(String, serde_json::Value)> = Vec::new();

    for field_id in &order {
        let expr = match graph.get_expr(field_id) {
            Some(e) => e,
            None => {
                warn!(field = %field_id, "formula field in order but no expression found");
                continue;
            }
        };

        let ctx = RecordContext {
            field_values: field_values.clone(),
            record_id: Some(entity_id.to_string()),
        };

        match evaluate(expr, &ctx) {
            Ok(value) => {
                // Update the field_values map so downstream formulas see this result
                field_values.insert(field_id.clone(), value.clone());
                updates.push((field_id.clone(), value));
            }
            Err(e) => {
                warn!(
                    field = %field_id,
                    error = %e,
                    "formula evaluation failed, storing #ERROR"
                );
                let error_val = serde_json::Value::String("#ERROR".into());
                field_values.insert(field_id.clone(), error_val.clone());
                updates.push((field_id.clone(), error_val));
            }
        }
    }

    // 4. Batch write within a transaction
    let mut tx = pool.begin().await.map_err(DarshJError::Database)?;
    let mut update_count = 0usize;

    for (attribute, value) in &updates {
        // Retract the old value
        sqlx::query(
            r#"
            UPDATE triples
            SET retracted = true
            WHERE entity_id = $1
              AND attribute = $2
              AND retracted = false
            "#,
        )
        .bind(entity_id)
        .bind(attribute)
        .execute(&mut *tx)
        .await
        .map_err(DarshJError::Database)?;

        // Determine value_type discriminator
        let value_type: i16 = match value {
            serde_json::Value::String(_) => 0,
            serde_json::Value::Number(n) => {
                if n.is_f64() {
                    2
                } else {
                    1
                }
            }
            serde_json::Value::Bool(_) => 3,
            serde_json::Value::Null => 0,
            _ => 6, // JSON
        };

        // Insert new triple
        sqlx::query(
            r#"
            INSERT INTO triples (entity_id, attribute, value, value_type, tx_id, retracted)
            VALUES ($1, $2, $3, $4, $5, false)
            "#,
        )
        .bind(entity_id)
        .bind(attribute)
        .bind(value)
        .bind(value_type)
        .bind(tx_id)
        .execute(&mut *tx)
        .await
        .map_err(DarshJError::Database)?;

        update_count += 1;
    }

    tx.commit().await.map_err(DarshJError::Database)?;

    let elapsed = start.elapsed();
    let metrics = RecalcMetrics {
        duration_ms: elapsed.as_millis() as u64,
        fields_recalculated: order.len(),
        updates_executed: update_count,
    };

    info!(
        entity_id = %entity_id,
        fields = metrics.fields_recalculated,
        updates = metrics.updates_executed,
        duration_ms = metrics.duration_ms,
        "recalculation complete"
    );

    Ok(metrics)
}

/// Batch recalculate formula fields across multiple entities.
///
/// Useful when a schema change (e.g. formula definition update) requires
/// recomputing every record.
pub async fn recalculate_all_entities(
    pool: &PgPool,
    entity_type: &str,
    graph: &DependencyGraph,
    tx_id: i64,
) -> Result<Vec<RecalcMetrics>> {
    // Find all distinct entity_ids of this type
    let entity_ids: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT entity_id
        FROM triples
        WHERE attribute = '__type'
          AND value = $1
          AND retracted = false
        "#,
    )
    .bind(serde_json::Value::String(entity_type.to_string()))
    .fetch_all(pool)
    .await
    .map_err(DarshJError::Database)?;

    let all_formula_fields = graph.formula_fields();
    let mut results = Vec::new();

    for (eid,) in entity_ids {
        let metrics = recalculate_affected(pool, eid, &all_formula_fields, graph, tx_id).await?;
        results.push(metrics);
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration tests require a live PG — unit-test the metrics struct
    // and the graph interaction here; full integration tests belong in
    // tests/ with a test database.

    #[test]
    fn test_recalc_metrics_default() {
        let m = RecalcMetrics {
            duration_ms: 0,
            fields_recalculated: 0,
            updates_executed: 0,
        };
        assert_eq!(m.fields_recalculated, 0);
    }

    #[tokio::test]
    async fn test_empty_recalculation_order_returns_early() {
        // With no graph entries, recalculate should be a no-op.
        // We can't call recalculate_affected without a pool, but we can
        // verify the graph returns empty order.
        let graph = DependencyGraph::new();
        let order = graph.calculation_order(&["Anything".into()]);
        assert!(order.is_empty());
    }
}
