//! Rollup field: aggregate values from linked records.
//!
//! A rollup follows a link, collects a target attribute's values, and
//! applies an aggregation function. Where possible the aggregation is
//! pushed down to SQL for efficiency; complex functions fall back to
//! Rust-side computation.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;
use crate::triple_store::schema::ValueType;

use super::link;

// ── Types ──────────────────────────────────────────────────────────

/// Aggregation functions available for rollup fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollupFn {
    /// Count of non-null linked values.
    Count,
    /// Numeric sum.
    Sum,
    /// Arithmetic mean.
    Average,
    /// Minimum value.
    Min,
    /// Maximum value.
    Max,
    /// Count of ALL linked records (regardless of whether the field exists).
    CountAll,
    /// Count of linked records where the field has a value.
    CountValues,
    /// Count of linked records where the field has NO value.
    CountEmpty,
    /// Join values with a separator string.
    ArrayJoin(String),
    /// Concatenate values with no separator.
    Concatenate,
}

impl RollupFn {
    /// Whether this function can be pushed down to SQL aggregation.
    pub fn is_sql_pushable(&self) -> bool {
        matches!(
            self,
            RollupFn::Count
                | RollupFn::Sum
                | RollupFn::Average
                | RollupFn::Min
                | RollupFn::Max
                | RollupFn::CountAll
                | RollupFn::CountValues
        )
    }
}

/// Configuration for a rollup field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollupConfig {
    /// The link attribute to follow.
    pub link_field: String,
    /// The attribute to aggregate from linked entities.
    pub rollup_field: String,
    /// Aggregation function to apply.
    pub function: RollupFn,
}

// ── Computation ────────────────────────────────────────────────────

/// Compute a rollup value for a given entity.
///
/// Follows the link, then either pushes the aggregation to SQL or
/// performs it in Rust depending on the function type.
pub async fn compute_rollup(
    pool: &PgPool,
    entity_id: Uuid,
    config: &RollupConfig,
) -> Result<serde_json::Value> {
    let linked_ids = link::get_linked(pool, entity_id, &config.link_field).await?;

    if linked_ids.is_empty() {
        return Ok(empty_result(&config.function));
    }

    if config.function.is_sql_pushable() {
        compute_sql_rollup(pool, &linked_ids, &config.rollup_field, &config.function).await
    } else {
        compute_rust_rollup(pool, &linked_ids, &config.rollup_field, &config.function).await
    }
}

/// SQL-pushed aggregation for standard numeric functions.
async fn compute_sql_rollup(
    pool: &PgPool,
    target_ids: &[Uuid],
    rollup_field: &str,
    function: &RollupFn,
) -> Result<serde_json::Value> {
    match function {
        RollupFn::CountAll => {
            // Count of linked records, regardless of field value.
            Ok(serde_json::json!(target_ids.len()))
        }

        RollupFn::Count | RollupFn::CountValues => {
            let row: (i64,) = sqlx::query_as(
                r#"
                SELECT COUNT(*)
                FROM triples
                WHERE entity_id = ANY($1)
                  AND attribute = $2
                  AND NOT retracted
                "#,
            )
            .bind(target_ids)
            .bind(rollup_field)
            .fetch_one(pool)
            .await?;
            Ok(serde_json::json!(row.0))
        }

        RollupFn::CountEmpty => {
            // Count of targets that do NOT have the field.
            let row: (i64,) = sqlx::query_as(
                r#"
                SELECT COUNT(*)
                FROM triples
                WHERE entity_id = ANY($1)
                  AND attribute = $2
                  AND NOT retracted
                "#,
            )
            .bind(target_ids)
            .bind(rollup_field)
            .fetch_one(pool)
            .await?;
            let with_value = row.0 as usize;
            let empty = target_ids.len().saturating_sub(with_value);
            Ok(serde_json::json!(empty))
        }

        RollupFn::Sum => {
            let row: (Option<f64>,) = sqlx::query_as(
                r#"
                SELECT SUM((value::text)::numeric)::float8
                FROM triples
                WHERE entity_id = ANY($1)
                  AND attribute = $2
                  AND value_type IN ($3, $4)
                  AND NOT retracted
                "#,
            )
            .bind(target_ids)
            .bind(rollup_field)
            .bind(ValueType::Integer as i16)
            .bind(ValueType::Float as i16)
            .fetch_one(pool)
            .await?;
            Ok(serde_json::json!(row.0.unwrap_or(0.0)))
        }

        RollupFn::Average => {
            let row: (Option<f64>,) = sqlx::query_as(
                r#"
                SELECT AVG((value::text)::numeric)::float8
                FROM triples
                WHERE entity_id = ANY($1)
                  AND attribute = $2
                  AND value_type IN ($3, $4)
                  AND NOT retracted
                "#,
            )
            .bind(target_ids)
            .bind(rollup_field)
            .bind(ValueType::Integer as i16)
            .bind(ValueType::Float as i16)
            .fetch_one(pool)
            .await?;
            Ok(serde_json::json!(row.0))
        }

        RollupFn::Min => {
            let row: (Option<serde_json::Value>,) = sqlx::query_as(
                r#"
                SELECT MIN(value)
                FROM triples
                WHERE entity_id = ANY($1)
                  AND attribute = $2
                  AND NOT retracted
                "#,
            )
            .bind(target_ids)
            .bind(rollup_field)
            .fetch_one(pool)
            .await?;
            Ok(row.0.unwrap_or(serde_json::Value::Null))
        }

        RollupFn::Max => {
            let row: (Option<serde_json::Value>,) = sqlx::query_as(
                r#"
                SELECT MAX(value)
                FROM triples
                WHERE entity_id = ANY($1)
                  AND attribute = $2
                  AND NOT retracted
                "#,
            )
            .bind(target_ids)
            .bind(rollup_field)
            .fetch_one(pool)
            .await?;
            Ok(row.0.unwrap_or(serde_json::Value::Null))
        }

        // ArrayJoin and Concatenate are not SQL-pushable.
        _ => unreachable!("non-SQL-pushable function routed to SQL path"),
    }
}

/// Rust-side aggregation for complex functions (ArrayJoin, Concatenate).
async fn compute_rust_rollup(
    pool: &PgPool,
    target_ids: &[Uuid],
    rollup_field: &str,
    function: &RollupFn,
) -> Result<serde_json::Value> {
    // Fetch all values for the rollup field across linked entities.
    let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
        r#"
        SELECT value
        FROM triples
        WHERE entity_id = ANY($1)
          AND attribute = $2
          AND NOT retracted
        ORDER BY created_at ASC
        "#,
    )
    .bind(target_ids)
    .bind(rollup_field)
    .fetch_all(pool)
    .await?;

    let values: Vec<serde_json::Value> = rows.into_iter().map(|(v,)| v).collect();

    match function {
        RollupFn::ArrayJoin(separator) => {
            let strings: Vec<String> = values.iter().map(value_to_string).collect();
            Ok(serde_json::json!(strings.join(separator)))
        }

        RollupFn::Concatenate => {
            let strings: Vec<String> = values.iter().map(value_to_string).collect();
            Ok(serde_json::json!(strings.concat()))
        }

        // Fallback for any function that somehow got routed here.
        RollupFn::Count | RollupFn::CountValues => Ok(serde_json::json!(values.len())),

        RollupFn::CountAll => Ok(serde_json::json!(target_ids.len())),

        RollupFn::CountEmpty => {
            let empty = target_ids.len().saturating_sub(values.len());
            Ok(serde_json::json!(empty))
        }

        RollupFn::Sum => {
            let sum: f64 = values.iter().filter_map(value_to_f64).sum();
            Ok(serde_json::json!(sum))
        }

        RollupFn::Average => {
            let nums: Vec<f64> = values.iter().filter_map(value_to_f64).collect();
            if nums.is_empty() {
                Ok(serde_json::Value::Null)
            } else {
                let avg = nums.iter().sum::<f64>() / nums.len() as f64;
                Ok(serde_json::json!(avg))
            }
        }

        RollupFn::Min => {
            let min = values
                .iter()
                .filter_map(value_to_f64)
                .fold(f64::INFINITY, f64::min);
            if min == f64::INFINITY {
                Ok(serde_json::Value::Null)
            } else {
                Ok(serde_json::json!(min))
            }
        }

        RollupFn::Max => {
            let max = values
                .iter()
                .filter_map(value_to_f64)
                .fold(f64::NEG_INFINITY, f64::max);
            if max == f64::NEG_INFINITY {
                Ok(serde_json::Value::Null)
            } else {
                Ok(serde_json::json!(max))
            }
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────

/// Return the "zero" value for a rollup function with no inputs.
fn empty_result(function: &RollupFn) -> serde_json::Value {
    match function {
        RollupFn::Count | RollupFn::CountAll | RollupFn::CountValues | RollupFn::CountEmpty => {
            serde_json::json!(0)
        }
        RollupFn::Sum => serde_json::json!(0.0),
        RollupFn::Average | RollupFn::Min | RollupFn::Max => serde_json::Value::Null,
        RollupFn::ArrayJoin(_) | RollupFn::Concatenate => serde_json::json!(""),
    }
}

/// Convert a JSON value to its string representation for joining.
fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Try to extract a numeric value from a JSON value.
fn value_to_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollup_fn_serialization_roundtrip() {
        let variants: Vec<RollupFn> = vec![
            RollupFn::Count,
            RollupFn::Sum,
            RollupFn::Average,
            RollupFn::Min,
            RollupFn::Max,
            RollupFn::CountAll,
            RollupFn::CountValues,
            RollupFn::CountEmpty,
            RollupFn::ArrayJoin(", ".into()),
            RollupFn::Concatenate,
        ];
        for rf in &variants {
            let json = serde_json::to_string(rf).unwrap();
            let back: RollupFn = serde_json::from_str(&json).unwrap();
            assert_eq!(*rf, back, "roundtrip failed for {rf:?}");
        }
    }

    #[test]
    fn rollup_fn_sql_pushable() {
        assert!(RollupFn::Count.is_sql_pushable());
        assert!(RollupFn::Sum.is_sql_pushable());
        assert!(RollupFn::Average.is_sql_pushable());
        assert!(RollupFn::Min.is_sql_pushable());
        assert!(RollupFn::Max.is_sql_pushable());
        assert!(RollupFn::CountAll.is_sql_pushable());
        assert!(RollupFn::CountValues.is_sql_pushable());

        assert!(!RollupFn::CountEmpty.is_sql_pushable());
        assert!(!RollupFn::ArrayJoin(", ".into()).is_sql_pushable());
        assert!(!RollupFn::Concatenate.is_sql_pushable());
    }

    #[test]
    fn rollup_config_serialization() {
        let config = RollupConfig {
            link_field: "tasks".into(),
            rollup_field: "hours".into(),
            function: RollupFn::Sum,
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["link_field"], "tasks");
        assert_eq!(json["rollup_field"], "hours");
        assert_eq!(json["function"], "sum");

        let back: RollupConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back.function, RollupFn::Sum);
    }

    #[test]
    fn rollup_config_array_join_serialization() {
        let config = RollupConfig {
            link_field: "tags".into(),
            rollup_field: "name".into(),
            function: RollupFn::ArrayJoin(" | ".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: RollupConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.function, RollupFn::ArrayJoin(" | ".into()));
    }

    #[test]
    fn empty_result_values() {
        assert_eq!(empty_result(&RollupFn::Count), serde_json::json!(0));
        assert_eq!(empty_result(&RollupFn::CountAll), serde_json::json!(0));
        assert_eq!(empty_result(&RollupFn::Sum), serde_json::json!(0.0));
        assert_eq!(empty_result(&RollupFn::Average), serde_json::Value::Null);
        assert_eq!(empty_result(&RollupFn::Min), serde_json::Value::Null);
        assert_eq!(empty_result(&RollupFn::Max), serde_json::Value::Null);
        assert_eq!(
            empty_result(&RollupFn::ArrayJoin(", ".into())),
            serde_json::json!("")
        );
        assert_eq!(empty_result(&RollupFn::Concatenate), serde_json::json!(""));
    }

    #[test]
    fn value_to_string_variants() {
        assert_eq!(value_to_string(&serde_json::json!("hello")), "hello");
        assert_eq!(value_to_string(&serde_json::json!(42)), "42");
        assert_eq!(value_to_string(&serde_json::json!(3.25)), "3.25");
        assert_eq!(value_to_string(&serde_json::json!(true)), "true");
        assert_eq!(value_to_string(&serde_json::Value::Null), "");
        assert_eq!(value_to_string(&serde_json::json!({"a": 1})), r#"{"a":1}"#);
    }

    #[test]
    fn value_to_f64_variants() {
        assert_eq!(value_to_f64(&serde_json::json!(42)), Some(42.0));
        assert_eq!(value_to_f64(&serde_json::json!(3.25)), Some(3.25));
        assert_eq!(value_to_f64(&serde_json::json!("99")), Some(99.0));
        assert_eq!(value_to_f64(&serde_json::json!("not_a_number")), None);
        assert_eq!(value_to_f64(&serde_json::json!(true)), None);
        assert_eq!(value_to_f64(&serde_json::Value::Null), None);
    }

    #[test]
    fn rust_fallback_aggregations() {
        // Test the Rust-side aggregation helpers used in compute_rust_rollup.
        let values = [
            serde_json::json!(10),
            serde_json::json!(20),
            serde_json::json!(30),
        ];

        // Sum
        let sum: f64 = values.iter().filter_map(value_to_f64).sum();
        assert_eq!(sum, 60.0);

        // Average
        let nums: Vec<f64> = values.iter().filter_map(value_to_f64).collect();
        let avg = nums.iter().sum::<f64>() / nums.len() as f64;
        assert_eq!(avg, 20.0);

        // Min
        let min = values
            .iter()
            .filter_map(value_to_f64)
            .fold(f64::INFINITY, f64::min);
        assert_eq!(min, 10.0);

        // Max
        let max = values
            .iter()
            .filter_map(value_to_f64)
            .fold(f64::NEG_INFINITY, f64::max);
        assert_eq!(max, 30.0);
    }

    #[test]
    fn array_join_with_separator() {
        let values = [
            serde_json::json!("alpha"),
            serde_json::json!("beta"),
            serde_json::json!("gamma"),
        ];
        let strings: Vec<String> = values.iter().map(value_to_string).collect();
        assert_eq!(strings.join(", "), "alpha, beta, gamma");
        assert_eq!(strings.concat(), "alphabetagamma");
    }
}
