//! Time-series and chart data aggregation.
//!
//! Generates bucketed time-series data for dashboard charts by using
//! PostgreSQL's `date_trunc` to group EAV triples by time intervals.
//! Supports multiple series via an optional `group_by` field.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

use crate::error::{DarshJError, Result};
use crate::query::WhereClause;

// ── Types ──────────────────────────────────────────────────────────

/// Time bucketing interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeBucket {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl TimeBucket {
    /// Return the PostgreSQL `date_trunc` interval string.
    pub fn pg_interval(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Quarter => "quarter",
            Self::Year => "year",
        }
    }
}

/// A time-series chart query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartQuery {
    /// Entity type to chart.
    pub entity_type: String,
    /// The attribute that holds the date/timestamp value.
    pub date_field: String,
    /// The attribute whose values are aggregated.
    pub value_field: String,
    /// Aggregate function to apply.
    pub function: ChartAggFn,
    /// Time bucketing interval.
    pub bucket: TimeBucket,
    /// Optional group-by attribute for multiple series.
    #[serde(default)]
    pub group_by: Option<String>,
    /// Pre-aggregation filters.
    #[serde(default)]
    pub filters: Vec<WhereClause>,
}

/// Simplified aggregate functions for chart queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartAggFn {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl ChartAggFn {
    /// Return the SQL aggregate expression for a numeric column.
    fn sql_expr(self, col: &str) -> String {
        match self {
            Self::Count => format!("COUNT({col})"),
            Self::Sum => format!("SUM(({col}#>>'{{{{}}}}')::numeric)"),
            Self::Avg => format!("AVG(({col}#>>'{{{{}}}}')::numeric)"),
            Self::Min => format!("MIN(({col}#>>'{{{{}}}}')::numeric)"),
            Self::Max => format!("MAX(({col}#>>'{{{{}}}}')::numeric)"),
        }
    }
}

impl ChartQuery {
    /// Validate the chart query.
    pub fn validate(&self) -> Result<()> {
        if self.entity_type.is_empty() {
            return Err(DarshJError::InvalidQuery(
                "entity_type must not be empty".into(),
            ));
        }
        if self.date_field.is_empty() {
            return Err(DarshJError::InvalidQuery(
                "date_field must not be empty".into(),
            ));
        }
        if self.value_field.is_empty() {
            return Err(DarshJError::InvalidQuery(
                "value_field must not be empty".into(),
            ));
        }
        Ok(())
    }
}

/// A single time bucket in the chart result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartBucket {
    /// Human-readable label (e.g. "2025-01", "2025-W03").
    pub label: String,
    /// Bucket start timestamp.
    pub start: DateTime<Utc>,
    /// Bucket end timestamp (exclusive).
    pub end: DateTime<Utc>,
    /// Aggregated value for this bucket.
    pub value: Value,
    /// Optional series name (when group_by is used).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
}

/// Chart aggregation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartResult {
    /// Bucketed data points.
    pub buckets: Vec<ChartBucket>,
}

// ── Engine ─────────────────────────────────────────────────────────

/// Execute a chart query and return bucketed time-series data.
pub async fn execute_chart_query(pool: &PgPool, query: &ChartQuery) -> Result<ChartResult> {
    query.validate()?;

    let (sql, params) = build_chart_sql(query);
    tracing::debug!(sql = %sql, "chart SQL");

    let mut db_query = sqlx::query_as::<_, RawChartRow>(&sql);
    for param in &params {
        db_query = bind_chart_param(db_query, param);
    }

    let rows: Vec<RawChartRow> = db_query.fetch_all(pool).await?;

    let buckets = rows
        .into_iter()
        .map(|row| ChartBucket {
            label: row.bucket_label,
            start: row.bucket_start,
            end: row.bucket_end,
            value: row.agg_value,
            series: row.series_name,
        })
        .collect();

    Ok(ChartResult { buckets })
}

// ── SQL generation ─────────────────────────────────────────────────

/// Raw row from chart SQL.
#[derive(Debug, sqlx::FromRow)]
struct RawChartRow {
    bucket_label: String,
    bucket_start: DateTime<Utc>,
    bucket_end: DateTime<Utc>,
    agg_value: Value,
    #[sqlx(default)]
    series_name: Option<String>,
}

/// Sanitize an attribute name (same as sql_builder).
fn sanitize_attr(attr: &str) -> String {
    attr.chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '/' || *c == ':' || *c == '-' || *c == '.')
        .collect()
}

/// Build chart aggregation SQL.
fn build_chart_sql(query: &ChartQuery) -> (String, Vec<Value>) {
    let mut params: Vec<Value> = Vec::new();
    let mut param_idx = 1u32;
    let mut sql = String::with_capacity(1024);

    let interval = query.bucket.pg_interval();
    let date_attr = sanitize_attr(&query.date_field);
    let value_attr = sanitize_attr(&query.value_field);

    // CTE 1: Find entity IDs of the requested type.
    sql.push_str("WITH entity_ids AS (\n");
    sql.push_str("  SELECT DISTINCT entity_id\n");
    sql.push_str("  FROM triples\n");
    sql.push_str("  WHERE attribute = ':db/type'\n");
    sql.push_str(&format!("    AND value = to_jsonb(${param_idx}::text)\n"));
    params.push(Value::String(query.entity_type.clone()));
    param_idx += 1;
    sql.push_str("    AND NOT retracted\n");

    // Apply filters.
    for filter in &query.filters {
        let safe_attr = sanitize_attr(&filter.attribute);
        let op = super::sql_builder::where_op_sql_pub(&filter.op);
        sql.push_str(&format!(
            "    AND entity_id IN (\n\
             \x20     SELECT entity_id FROM triples\n\
             \x20     WHERE attribute = '{safe_attr}'\n\
             \x20       AND NOT retracted\n\
             \x20       AND value {op} to_jsonb(${param_idx}::text)\n\
             \x20   )\n"
        ));
        params.push(filter.value.clone());
        param_idx += 1;
    }

    sql.push_str("),\n");

    // CTE 2: Get date and value triples for matching entities.
    sql.push_str("dated AS (\n");
    sql.push_str("  SELECT\n");
    sql.push_str("    e.entity_id,\n");
    sql.push_str(&format!(
        "    d.value AS date_val,\n\
         \x20   v.value AS val\n"
    ));

    // Optional series column.
    if let Some(ref group_attr) = query.group_by {
        let safe_group = sanitize_attr(group_attr);
        sql.push_str(&format!("    , s.value AS series_val\n"));
        let _ = safe_group; // used in join below
    }

    sql.push_str("  FROM entity_ids e\n");

    // Join date field.
    sql.push_str(&format!(
        "  INNER JOIN triples d ON d.entity_id = e.entity_id\n\
         \x20   AND d.attribute = '{date_attr}'\n\
         \x20   AND NOT d.retracted\n"
    ));

    // Join value field.
    sql.push_str(&format!(
        "  INNER JOIN triples v ON v.entity_id = e.entity_id\n\
         \x20   AND v.attribute = '{value_attr}'\n\
         \x20   AND NOT v.retracted\n"
    ));

    // Optional series join.
    if let Some(ref group_attr) = query.group_by {
        let safe_group = sanitize_attr(group_attr);
        sql.push_str(&format!(
            "  LEFT JOIN triples s ON s.entity_id = e.entity_id\n\
             \x20   AND s.attribute = '{safe_group}'\n\
             \x20   AND NOT s.retracted\n"
        ));
    }

    sql.push_str(")\n");

    // Main SELECT with date_trunc bucketing.
    sql.push_str("SELECT\n");
    sql.push_str(&format!(
        "  TO_CHAR(date_trunc('{interval}', (date_val#>>'{{{{}}}}')::timestamptz), 'YYYY-MM-DD') AS bucket_label,\n"
    ));
    sql.push_str(&format!(
        "  date_trunc('{interval}', (date_val#>>'{{{{}}}}')::timestamptz) AS bucket_start,\n"
    ));
    sql.push_str(&format!(
        "  date_trunc('{interval}', (date_val#>>'{{{{}}}}')::timestamptz) + INTERVAL '1 {interval}' AS bucket_end,\n"
    ));

    let agg_expr = query.function.sql_expr("val");
    sql.push_str(&format!("  to_jsonb({agg_expr}) AS agg_value"));

    if query.group_by.is_some() {
        sql.push_str(",\n  series_val#>>'{}' AS series_name\n");
    } else {
        sql.push_str(",\n  NULL::text AS series_name\n");
    }

    sql.push_str("FROM dated\n");
    sql.push_str("WHERE date_val IS NOT NULL\n");

    // GROUP BY.
    sql.push_str(&format!(
        "GROUP BY bucket_label, bucket_start, bucket_end"
    ));
    if query.group_by.is_some() {
        sql.push_str(", series_name");
    }
    sql.push('\n');

    sql.push_str("ORDER BY bucket_start ASC");
    if query.group_by.is_some() {
        sql.push_str(", series_name ASC");
    }
    sql.push('\n');

    let _ = param_idx;

    (sql, params)
}

/// Bind a JSON param for chart queries.
fn bind_chart_param<'q>(
    query: sqlx::query::QueryAs<'q, sqlx::Postgres, RawChartRow, sqlx::postgres::PgArguments>,
    value: &'q Value,
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, RawChartRow, sqlx::postgres::PgArguments> {
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

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_empty_entity_type() {
        let q = ChartQuery {
            entity_type: String::new(),
            date_field: "created_at".into(),
            value_field: "amount".into(),
            function: ChartAggFn::Sum,
            bucket: TimeBucket::Month,
            group_by: None,
            filters: vec![],
        };
        assert!(q.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_date_field() {
        let q = ChartQuery {
            entity_type: "Order".into(),
            date_field: String::new(),
            value_field: "amount".into(),
            function: ChartAggFn::Sum,
            bucket: TimeBucket::Month,
            group_by: None,
            filters: vec![],
        };
        assert!(q.validate().is_err());
    }

    #[test]
    fn validate_accepts_valid_query() {
        let q = ChartQuery {
            entity_type: "Order".into(),
            date_field: "created_at".into(),
            value_field: "amount".into(),
            function: ChartAggFn::Sum,
            bucket: TimeBucket::Month,
            group_by: Some("region".into()),
            filters: vec![],
        };
        assert!(q.validate().is_ok());
    }

    #[test]
    fn time_bucket_pg_intervals() {
        assert_eq!(TimeBucket::Day.pg_interval(), "day");
        assert_eq!(TimeBucket::Week.pg_interval(), "week");
        assert_eq!(TimeBucket::Month.pg_interval(), "month");
        assert_eq!(TimeBucket::Quarter.pg_interval(), "quarter");
        assert_eq!(TimeBucket::Year.pg_interval(), "year");
    }

    #[test]
    fn chart_sql_has_date_trunc() {
        let q = ChartQuery {
            entity_type: "Order".into(),
            date_field: "created_at".into(),
            value_field: "amount".into(),
            function: ChartAggFn::Sum,
            bucket: TimeBucket::Month,
            group_by: None,
            filters: vec![],
        };
        let (sql, params) = build_chart_sql(&q);

        assert!(sql.contains("date_trunc('month'"));
        assert!(sql.contains("entity_ids AS"));
        assert!(sql.contains("dated AS"));
        assert!(sql.contains("SUM"));
        assert!(sql.contains("bucket_start"));
        assert!(sql.contains("bucket_end"));
        assert!(sql.contains("ORDER BY bucket_start ASC"));
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn chart_sql_with_group_by_includes_series() {
        let q = ChartQuery {
            entity_type: "Order".into(),
            date_field: "created_at".into(),
            value_field: "amount".into(),
            function: ChartAggFn::Count,
            bucket: TimeBucket::Week,
            group_by: Some("region".into()),
            filters: vec![],
        };
        let (sql, _) = build_chart_sql(&q);

        assert!(sql.contains("series_name"));
        assert!(sql.contains("series_val"));
        assert!(sql.contains("GROUP BY bucket_label, bucket_start, bucket_end, series_name"));
    }

    #[test]
    fn chart_sql_with_filters() {
        let q = ChartQuery {
            entity_type: "Order".into(),
            date_field: "created_at".into(),
            value_field: "amount".into(),
            function: ChartAggFn::Avg,
            bucket: TimeBucket::Day,
            group_by: None,
            filters: vec![crate::query::WhereClause {
                attribute: "status".into(),
                op: crate::query::WhereOp::Eq,
                value: Value::String("completed".into()),
            }],
        };
        let (sql, params) = build_chart_sql(&q);

        assert!(sql.contains("entity_id IN ("));
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn chart_agg_fn_sql_expressions() {
        assert!(ChartAggFn::Count.sql_expr("v").contains("COUNT"));
        assert!(ChartAggFn::Sum.sql_expr("v").contains("SUM"));
        assert!(ChartAggFn::Avg.sql_expr("v").contains("AVG"));
        assert!(ChartAggFn::Min.sql_expr("v").contains("MIN"));
        assert!(ChartAggFn::Max.sql_expr("v").contains("MAX"));
    }

    #[test]
    fn chart_bucket_serde_roundtrip() {
        let bucket = ChartBucket {
            label: "2025-01-01".into(),
            start: Utc::now(),
            end: Utc::now(),
            value: Value::Number(42.into()),
            series: Some("US".into()),
        };
        let json = serde_json::to_string(&bucket).unwrap();
        let back: ChartBucket = serde_json::from_str(&json).unwrap();
        assert_eq!(back.label, "2025-01-01");
        assert_eq!(back.value, Value::Number(42.into()));
        assert_eq!(back.series, Some("US".into()));
    }

    #[test]
    fn chart_bucket_without_series_skips_field() {
        let bucket = ChartBucket {
            label: "2025-01".into(),
            start: Utc::now(),
            end: Utc::now(),
            value: Value::Number(100.into()),
            series: None,
        };
        let json = serde_json::to_string(&bucket).unwrap();
        assert!(!json.contains("series"));
    }
}
