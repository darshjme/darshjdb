//! Core aggregation engine: query types, execution, and result assembly.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

use crate::error::{DarshJError, Result};
use crate::query::WhereClause;

use super::sql_builder;

// ── Aggregate function enum ────────────────────────────────────────

/// Statistical / aggregate function to apply to a field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "fn", content = "arg")]
pub enum AggFn {
    /// Count of non-null values.
    Count,
    /// Count of distinct non-null values.
    CountDistinct,
    /// Numeric sum.
    Sum,
    /// Arithmetic mean.
    Avg,
    /// Minimum value (numeric or lexicographic).
    Min,
    /// Maximum value (numeric or lexicographic).
    Max,
    /// Population standard deviation.
    StdDev,
    /// Population variance.
    Variance,
    /// Median (50th percentile).
    Median,
    /// Arbitrary percentile (0.0..=1.0).
    Percentile(f64),
    /// First value encountered (by tx_id ordering).
    First,
    /// Last value encountered (by tx_id ordering).
    Last,
    /// Collect all values into a JSON array.
    ArrayAgg,
    /// Concatenate string values with a separator.
    StringAgg(String),
    /// Count of null / empty values.
    CountEmpty,
    /// Count of non-null / non-empty values.
    CountFilled,
    /// Percentage of null / empty values.
    PercentEmpty,
    /// Percentage of non-null / non-empty values.
    PercentFilled,
}

impl AggFn {
    /// Validate the function's parameters.
    pub fn validate(&self) -> Result<()> {
        match self {
            AggFn::Percentile(p) => {
                if !(0.0..=1.0).contains(p) {
                    return Err(DarshJError::InvalidQuery(format!(
                        "percentile must be between 0.0 and 1.0, got {p}"
                    )));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

// ── Query types ────────────────────────────────────────────────────

/// A single aggregation to compute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Aggregation {
    /// The attribute to aggregate over.
    pub field: String,
    /// The aggregate function to apply.
    pub function: AggFn,
    /// Output alias for the result column.
    pub alias: String,
}

/// Comparison operator for HAVING clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HavingOp {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
}

impl HavingOp {
    /// Return the SQL operator string.
    pub fn sql_op(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Neq => "!=",
            Self::Gt => ">",
            Self::Gte => ">=",
            Self::Lt => "<",
            Self::Lte => "<=",
        }
    }
}

/// Filter on aggregated values (SQL HAVING).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HavingClause {
    /// Alias of the aggregated column to filter on.
    pub alias: String,
    /// Comparison operator.
    pub op: HavingOp,
    /// Value to compare against.
    pub value: Value,
}

/// Top-level aggregation query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateQuery {
    /// Entity type to aggregate (e.g. `"Invoice"`, `"Order"`).
    pub entity_type: String,
    /// Attributes to group by.
    #[serde(default)]
    pub group_by: Vec<String>,
    /// Aggregations to compute per group.
    pub aggregations: Vec<Aggregation>,
    /// Pre-aggregation filters (WHERE).
    #[serde(default)]
    pub filters: Vec<WhereClause>,
    /// Post-aggregation filter (HAVING).
    #[serde(default)]
    pub having: Option<HavingClause>,
}

impl AggregateQuery {
    /// Validate the query before execution.
    pub fn validate(&self) -> Result<()> {
        if self.entity_type.is_empty() {
            return Err(DarshJError::InvalidQuery(
                "entity_type must not be empty".into(),
            ));
        }
        if self.aggregations.is_empty() {
            return Err(DarshJError::InvalidQuery(
                "at least one aggregation is required".into(),
            ));
        }
        for agg in &self.aggregations {
            agg.function.validate()?;
            if agg.alias.is_empty() {
                return Err(DarshJError::InvalidQuery(
                    "aggregation alias must not be empty".into(),
                ));
            }
            if agg.field.is_empty() {
                return Err(DarshJError::InvalidQuery(
                    "aggregation field must not be empty".into(),
                ));
            }
        }
        if let Some(having) = &self.having {
            // Verify the having alias references an actual aggregation.
            let valid = self.aggregations.iter().any(|a| a.alias == having.alias);
            if !valid {
                return Err(DarshJError::InvalidQuery(format!(
                    "HAVING alias '{}' does not match any aggregation alias",
                    having.alias
                )));
            }
        }
        Ok(())
    }
}

// ── Result types ───────────────────────────────────────────────────

/// A single group in the aggregation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggGroup {
    /// Group key: attribute name -> value.
    pub key: HashMap<String, Value>,
    /// Computed aggregation values: alias -> value.
    pub values: HashMap<String, Value>,
    /// Number of entities in this group.
    pub count: u64,
}

/// Full aggregation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateResult {
    /// Per-group results.
    pub groups: Vec<AggGroup>,
    /// Grand totals (ungrouped aggregation over the entire result set).
    pub totals: HashMap<String, Value>,
}

// ── Engine ─────────────────────────────────────────────────────────

/// The aggregation engine. Holds a reference to the database pool
/// and executes [`AggregateQuery`] instances.
#[derive(Clone)]
pub struct AggregationEngine {
    pool: PgPool,
}

impl AggregationEngine {
    /// Create a new engine backed by the given Postgres pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Execute an aggregation query and return grouped results with totals.
    pub async fn execute(&self, query: &AggregateQuery) -> Result<AggregateResult> {
        query.validate()?;

        let (sql, params) = sql_builder::build_aggregate_sql(query);
        tracing::debug!(sql = %sql, params = ?params, "aggregation SQL");

        let mut db_query = sqlx::query_as::<_, RawAggRow>(&sql);
        for param in &params {
            db_query = bind_json_param(db_query, param);
        }

        let rows: Vec<RawAggRow> = db_query.fetch_all(&self.pool).await?;

        let groups = assemble_groups(&query.group_by, &query.aggregations, &rows);

        // Compute grand totals by running a totals query (no GROUP BY).
        let totals = self.compute_totals(query).await?;

        Ok(AggregateResult { groups, totals })
    }

    /// Quick summary: count, sum, avg for all numeric fields of an entity type.
    pub async fn summary(&self, entity_type: &str) -> Result<AggregateResult> {
        let sql = format!(
            r#"
            WITH entity_ids AS (
                SELECT DISTINCT entity_id
                FROM triples
                WHERE attribute = ':db/type'
                  AND value = to_jsonb($1::text)
                  AND NOT retracted
            ),
            vals AS (
                SELECT t.attribute,
                       t.value,
                       t.value_type
                FROM triples t
                INNER JOIN entity_ids e ON e.entity_id = t.entity_id
                WHERE NOT t.retracted
                  AND t.attribute != ':db/type'
            )
            SELECT
                attribute AS group_key,
                jsonb_build_object(
                    'count', COUNT(*),
                    'count_distinct', COUNT(DISTINCT value),
                    'count_empty', COUNT(*) FILTER (WHERE value = 'null'::jsonb OR value = '""'::jsonb),
                    'count_filled', COUNT(*) FILTER (WHERE value != 'null'::jsonb AND value != '""'::jsonb),
                    'sum', CASE WHEN bool_and(value_type IN (1,2)) THEN SUM((value#>>'{{}}')::numeric) ELSE NULL END,
                    'avg', CASE WHEN bool_and(value_type IN (1,2)) THEN AVG((value#>>'{{}}')::numeric) ELSE NULL END,
                    'min', CASE WHEN bool_and(value_type IN (1,2)) THEN MIN((value#>>'{{}}')::numeric) ELSE NULL END,
                    'max', CASE WHEN bool_and(value_type IN (1,2)) THEN MAX((value#>>'{{}}')::numeric) ELSE NULL END
                ) AS agg_values,
                COUNT(*) AS group_count
            FROM vals
            GROUP BY attribute
            ORDER BY attribute
            "#
        );

        let rows: Vec<RawAggRow> = sqlx::query_as(&sql)
            .bind(entity_type)
            .fetch_all(&self.pool)
            .await?;

        let groups = rows
            .iter()
            .map(|row| {
                let mut key = HashMap::new();
                key.insert("attribute".to_string(), Value::String(row.group_key.clone()));
                let values: HashMap<String, Value> = match &row.agg_values {
                    Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                    _ => HashMap::new(),
                };
                AggGroup {
                    key,
                    values,
                    count: row.group_count as u64,
                }
            })
            .collect();

        Ok(AggregateResult {
            groups,
            totals: HashMap::new(),
        })
    }

    /// Compute grand totals (aggregations without GROUP BY).
    async fn compute_totals(
        &self,
        query: &AggregateQuery,
    ) -> Result<HashMap<String, Value>> {
        let mut totals_query = query.clone();
        totals_query.group_by.clear();
        totals_query.having = None;

        let (sql, params) = sql_builder::build_aggregate_sql(&totals_query);

        let mut db_query = sqlx::query_as::<_, RawAggRow>(&sql);
        for param in &params {
            db_query = bind_json_param(db_query, param);
        }

        let rows: Vec<RawAggRow> = db_query.fetch_all(&self.pool).await?;

        if let Some(row) = rows.first() {
            match &row.agg_values {
                Value::Object(map) => Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                _ => Ok(HashMap::new()),
            }
        } else {
            Ok(HashMap::new())
        }
    }
}

// ── Internal types ─────────────────────────────────────────────────

/// Raw row returned from aggregation SQL.
#[derive(Debug, sqlx::FromRow)]
struct RawAggRow {
    /// Serialized group key (comma-separated for multi-column groups).
    group_key: String,
    /// JSONB object of aggregated values.
    agg_values: Value,
    /// Entity count in this group.
    group_count: i64,
}

/// Bind a `serde_json::Value` parameter to a sqlx query.
fn bind_json_param<'q>(
    query: sqlx::query::QueryAs<'q, sqlx::Postgres, RawAggRow, sqlx::postgres::PgArguments>,
    value: &'q Value,
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, RawAggRow, sqlx::postgres::PgArguments> {
    match value {
        Value::String(s) => query.bind(s.as_str()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                query.bind(i)
            } else if let Some(f) = n.as_f64() {
                query.bind(f)
            } else {
                query.bind(n.to_string())
            }
        }
        Value::Bool(b) => query.bind(*b),
        _ => query.bind(value.to_string()),
    }
}

/// Assemble `RawAggRow` into structured `AggGroup` instances.
fn assemble_groups(
    group_by: &[String],
    aggregations: &[Aggregation],
    rows: &[RawAggRow],
) -> Vec<AggGroup> {
    rows.iter()
        .map(|row| {
            // Parse group key. For multi-column groups the key is
            // stored as "val1|||val2|||val3" separated by |||.
            let key_parts: Vec<&str> = row.group_key.split("|||").collect();
            let mut key = HashMap::new();
            for (i, col) in group_by.iter().enumerate() {
                let val = key_parts
                    .get(i)
                    .map(|s| {
                        // Try to parse as JSON value, fallback to string.
                        serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
                    })
                    .unwrap_or(Value::Null);
                key.insert(col.clone(), val);
            }
            // If no group_by, set a single "all" key.
            if group_by.is_empty() {
                key.insert("_all".to_string(), Value::Bool(true));
            }

            let values: HashMap<String, Value> = match &row.agg_values {
                Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                _ => {
                    // Fallback: try to map aggregation aliases.
                    let mut m = HashMap::new();
                    for agg in aggregations {
                        m.insert(agg.alias.clone(), Value::Null);
                    }
                    m
                }
            };

            AggGroup {
                key,
                values,
                count: row.group_count as u64,
            }
        })
        .collect()
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_empty_entity_type() {
        let q = AggregateQuery {
            entity_type: String::new(),
            group_by: vec![],
            aggregations: vec![Aggregation {
                field: "amount".into(),
                function: AggFn::Sum,
                alias: "total".into(),
            }],
            filters: vec![],
            having: None,
        };
        assert!(q.validate().is_err());
    }

    #[test]
    fn validate_rejects_no_aggregations() {
        let q = AggregateQuery {
            entity_type: "Order".into(),
            group_by: vec![],
            aggregations: vec![],
            filters: vec![],
            having: None,
        };
        assert!(q.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_alias() {
        let q = AggregateQuery {
            entity_type: "Order".into(),
            group_by: vec![],
            aggregations: vec![Aggregation {
                field: "amount".into(),
                function: AggFn::Sum,
                alias: String::new(),
            }],
            filters: vec![],
            having: None,
        };
        assert!(q.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_field() {
        let q = AggregateQuery {
            entity_type: "Order".into(),
            group_by: vec![],
            aggregations: vec![Aggregation {
                field: String::new(),
                function: AggFn::Sum,
                alias: "total".into(),
            }],
            filters: vec![],
            having: None,
        };
        assert!(q.validate().is_err());
    }

    #[test]
    fn validate_rejects_invalid_percentile() {
        let q = AggregateQuery {
            entity_type: "Order".into(),
            group_by: vec![],
            aggregations: vec![Aggregation {
                field: "amount".into(),
                function: AggFn::Percentile(1.5),
                alias: "p150".into(),
            }],
            filters: vec![],
            having: None,
        };
        assert!(q.validate().is_err());
    }

    #[test]
    fn validate_rejects_having_on_missing_alias() {
        let q = AggregateQuery {
            entity_type: "Order".into(),
            group_by: vec!["status".into()],
            aggregations: vec![Aggregation {
                field: "amount".into(),
                function: AggFn::Sum,
                alias: "total".into(),
            }],
            filters: vec![],
            having: Some(HavingClause {
                alias: "nonexistent".into(),
                op: HavingOp::Gt,
                value: Value::Number(100.into()),
            }),
        };
        assert!(q.validate().is_err());
    }

    #[test]
    fn validate_accepts_valid_query() {
        let q = AggregateQuery {
            entity_type: "Order".into(),
            group_by: vec!["status".into()],
            aggregations: vec![
                Aggregation {
                    field: "amount".into(),
                    function: AggFn::Sum,
                    alias: "total_amount".into(),
                },
                Aggregation {
                    field: "amount".into(),
                    function: AggFn::Avg,
                    alias: "avg_amount".into(),
                },
                Aggregation {
                    field: "id".into(),
                    function: AggFn::Count,
                    alias: "order_count".into(),
                },
            ],
            filters: vec![],
            having: Some(HavingClause {
                alias: "total_amount".into(),
                op: HavingOp::Gt,
                value: Value::Number(1000.into()),
            }),
        };
        assert!(q.validate().is_ok());
    }

    #[test]
    fn validate_accepts_valid_percentile() {
        let q = AggregateQuery {
            entity_type: "Metric".into(),
            group_by: vec![],
            aggregations: vec![Aggregation {
                field: "latency".into(),
                function: AggFn::Percentile(0.95),
                alias: "p95".into(),
            }],
            filters: vec![],
            having: None,
        };
        assert!(q.validate().is_ok());
    }

    #[test]
    fn agg_fn_serde_roundtrip() {
        let fns = vec![
            AggFn::Count,
            AggFn::CountDistinct,
            AggFn::Sum,
            AggFn::Avg,
            AggFn::Min,
            AggFn::Max,
            AggFn::StdDev,
            AggFn::Variance,
            AggFn::Median,
            AggFn::Percentile(0.99),
            AggFn::First,
            AggFn::Last,
            AggFn::ArrayAgg,
            AggFn::StringAgg(", ".into()),
            AggFn::CountEmpty,
            AggFn::CountFilled,
            AggFn::PercentEmpty,
            AggFn::PercentFilled,
        ];

        for f in &fns {
            let json = serde_json::to_string(f).expect("serialize");
            let back: AggFn = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&back, f);
        }
    }

    #[test]
    fn having_op_sql() {
        assert_eq!(HavingOp::Eq.sql_op(), "=");
        assert_eq!(HavingOp::Neq.sql_op(), "!=");
        assert_eq!(HavingOp::Gt.sql_op(), ">");
        assert_eq!(HavingOp::Gte.sql_op(), ">=");
        assert_eq!(HavingOp::Lt.sql_op(), "<");
        assert_eq!(HavingOp::Lte.sql_op(), "<=");
    }

    #[test]
    fn assemble_groups_single_group() {
        let group_by = vec!["status".to_string()];
        let aggregations = vec![Aggregation {
            field: "amount".into(),
            function: AggFn::Sum,
            alias: "total".into(),
        }];
        let rows = vec![
            RawAggRow {
                group_key: "\"active\"".into(),
                agg_values: serde_json::json!({"total": 500}),
                group_count: 10,
            },
            RawAggRow {
                group_key: "\"closed\"".into(),
                agg_values: serde_json::json!({"total": 300}),
                group_count: 5,
            },
        ];

        let groups = assemble_groups(&group_by, &aggregations, &rows);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].key["status"], Value::String("active".into()));
        assert_eq!(groups[0].values["total"], serde_json::json!(500));
        assert_eq!(groups[0].count, 10);
        assert_eq!(groups[1].key["status"], Value::String("closed".into()));
        assert_eq!(groups[1].count, 5);
    }

    #[test]
    fn assemble_groups_multi_key() {
        let group_by = vec!["region".to_string(), "status".to_string()];
        let aggregations = vec![Aggregation {
            field: "amount".into(),
            function: AggFn::Count,
            alias: "cnt".into(),
        }];
        let rows = vec![RawAggRow {
            group_key: "\"US\"|||\"active\"".into(),
            agg_values: serde_json::json!({"cnt": 42}),
            group_count: 42,
        }];

        let groups = assemble_groups(&group_by, &aggregations, &rows);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key["region"], Value::String("US".into()));
        assert_eq!(groups[0].key["status"], Value::String("active".into()));
    }

    #[test]
    fn assemble_groups_no_group_by() {
        let group_by: Vec<String> = vec![];
        let aggregations = vec![Aggregation {
            field: "amount".into(),
            function: AggFn::Sum,
            alias: "total".into(),
        }];
        let rows = vec![RawAggRow {
            group_key: "_all".into(),
            agg_values: serde_json::json!({"total": 9999}),
            group_count: 100,
        }];

        let groups = assemble_groups(&group_by, &aggregations, &rows);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].key.contains_key("_all"));
        assert_eq!(groups[0].values["total"], serde_json::json!(9999));
    }
}
