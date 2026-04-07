//! SQL generation for aggregation queries over the EAV triple store.
//!
//! Generates CTE-based parameterized SQL that:
//! 1. Finds all entity IDs of the requested type
//! 2. Filters entities by WHERE clauses
//! 3. Pivots EAV triples into columnar form
//! 4. Applies GROUP BY and aggregate functions
//! 5. Applies HAVING clause on aggregated values
//!
//! All user-supplied values are parameterized ($1, $2, ...) to prevent
//! SQL injection. Attribute names are sanitized to alphanumeric + underscore.

use serde_json::Value;

use super::engine::{AggFn, AggregateQuery};

/// Sanitize an attribute name to prevent SQL injection.
/// Allows alphanumeric, underscore, forward slash, colon, hyphen, and dot.
fn sanitize_attr(attr: &str) -> String {
    attr.chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '/' || *c == ':' || *c == '-' || *c == '.')
        .collect()
}

/// Generate a safe SQL column alias from an attribute name.
fn attr_to_alias(attr: &str) -> String {
    attr.replace(['/', ':', '-', '.'], "_")
}

/// Build the SQL expression for an aggregate function.
///
/// `col_expr` is the SQL expression referencing the pivoted column value.
fn agg_fn_sql(func: &AggFn, col_expr: &str) -> String {
    match func {
        AggFn::Count => format!("COUNT({col_expr})"),
        AggFn::CountDistinct => format!("COUNT(DISTINCT {col_expr})"),
        AggFn::Sum => format!("SUM(({col_expr}#>>'{{{{}}}}')::numeric)"),
        AggFn::Avg => format!("AVG(({col_expr}#>>'{{{{}}}}')::numeric)"),
        AggFn::Min => format!("MIN(({col_expr}#>>'{{{{}}}}')::numeric)"),
        AggFn::Max => format!("MAX(({col_expr}#>>'{{{{}}}}')::numeric)"),
        AggFn::StdDev => format!("STDDEV_POP(({col_expr}#>>'{{{{}}}}')::numeric)"),
        AggFn::Variance => format!("VAR_POP(({col_expr}#>>'{{{{}}}}')::numeric)"),
        AggFn::Median => {
            format!("PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY ({col_expr}#>>'{{{{}}}}')::numeric)")
        }
        AggFn::Percentile(p) => {
            format!(
                "PERCENTILE_CONT({p}) WITHIN GROUP (ORDER BY ({col_expr}#>>'{{{{}}}}')::numeric)"
            )
        }
        AggFn::First => {
            format!("(ARRAY_AGG({col_expr} ORDER BY entity_id))[1]")
        }
        AggFn::Last => {
            format!("(ARRAY_AGG({col_expr} ORDER BY entity_id DESC))[1]")
        }
        AggFn::ArrayAgg => format!("JSONB_AGG({col_expr})"),
        AggFn::StringAgg(sep) => {
            // Separator is embedded safely since it comes from the validated query struct.
            let escaped_sep = sep.replace('\'', "''");
            format!("STRING_AGG({col_expr}#>>'{{{{}}}}', '{escaped_sep}')")
        }
        AggFn::CountEmpty => {
            format!(
                "COUNT(*) FILTER (WHERE {col_expr} IS NULL OR {col_expr} = 'null'::jsonb OR {col_expr} = '\"\"'::jsonb)"
            )
        }
        AggFn::CountFilled => {
            format!(
                "COUNT(*) FILTER (WHERE {col_expr} IS NOT NULL AND {col_expr} != 'null'::jsonb AND {col_expr} != '\"\"'::jsonb)"
            )
        }
        AggFn::PercentEmpty => {
            format!(
                "ROUND(100.0 * COUNT(*) FILTER (WHERE {col_expr} IS NULL OR {col_expr} = 'null'::jsonb OR {col_expr} = '\"\"'::jsonb) / GREATEST(COUNT(*), 1), 2)"
            )
        }
        AggFn::PercentFilled => {
            format!(
                "ROUND(100.0 * COUNT(*) FILTER (WHERE {col_expr} IS NOT NULL AND {col_expr} != 'null'::jsonb AND {col_expr} != '\"\"'::jsonb) / GREATEST(COUNT(*), 1), 2)"
            )
        }
    }
}

/// Build a parameterized aggregation SQL query from an [`AggregateQuery`].
///
/// Returns `(sql, params)` where params are ordered bind values.
pub fn build_aggregate_sql(query: &AggregateQuery) -> (String, Vec<Value>) {
    let mut params: Vec<Value> = Vec::new();
    let mut param_idx = 1u32;
    let mut sql = String::with_capacity(1024);

    // Collect all unique attributes we need to pivot.
    let mut pivot_attrs: Vec<String> = Vec::new();
    for attr in &query.group_by {
        if !pivot_attrs.contains(attr) {
            pivot_attrs.push(attr.clone());
        }
    }
    for agg in &query.aggregations {
        if !pivot_attrs.contains(&agg.field) {
            pivot_attrs.push(agg.field.clone());
        }
    }

    // CTE 1: Find entity IDs of the requested type.
    sql.push_str("WITH entity_ids AS (\n");
    sql.push_str("  SELECT DISTINCT entity_id\n");
    sql.push_str("  FROM triples\n");
    sql.push_str("  WHERE attribute = ':db/type'\n");
    sql.push_str(&format!("    AND value = to_jsonb(${param_idx}::text)\n"));
    params.push(Value::String(query.entity_type.clone()));
    param_idx += 1;
    sql.push_str("    AND NOT retracted\n");

    // Apply pre-aggregation WHERE filters at the entity level.
    // Each filter requires a correlated subquery to check the attribute value.
    for filter in &query.filters {
        let safe_attr = sanitize_attr(&filter.attribute);
        let op_sql = where_op_sql(&filter.op);
        sql.push_str(&format!(
            "    AND entity_id IN (\n\
             \x20     SELECT entity_id FROM triples\n\
             \x20     WHERE attribute = '{safe_attr}'\n\
             \x20       AND NOT retracted\n\
             \x20       AND value {op_sql} to_jsonb(${param_idx}::text)\n\
             \x20   )\n"
        ));
        params.push(filter.value.clone());
        param_idx += 1;
    }

    sql.push_str("),\n");

    // CTE 2: Pivot EAV into columns.
    // For each needed attribute, LEFT JOIN a subquery to get its value.
    sql.push_str("pivoted AS (\n");
    sql.push_str("  SELECT e.entity_id");

    for attr in &pivot_attrs {
        let alias = attr_to_alias(attr);
        let safe_attr = sanitize_attr(attr);
        sql.push_str(&format!(
            ",\n    {alias}.value AS {alias}_val"
        ));
        // We'll add the joins below; just declare the select columns here.
        let _ = safe_attr; // used in the FROM clause below
    }

    sql.push_str("\n  FROM entity_ids e\n");

    for attr in &pivot_attrs {
        let alias = attr_to_alias(attr);
        let safe_attr = sanitize_attr(attr);
        sql.push_str(&format!(
            "  LEFT JOIN LATERAL (\n\
             \x20   SELECT value FROM triples\n\
             \x20   WHERE entity_id = e.entity_id\n\
             \x20     AND attribute = '{safe_attr}'\n\
             \x20     AND NOT retracted\n\
             \x20   ORDER BY tx_id DESC LIMIT 1\n\
             \x20 ) {alias} ON true\n"
        ));
    }

    sql.push_str(")\n");

    // Main SELECT: build aggregation expressions.
    sql.push_str("SELECT\n");

    // Group key expression.
    if query.group_by.is_empty() {
        sql.push_str("  '_all' AS group_key,\n");
    } else if query.group_by.len() == 1 {
        let alias = attr_to_alias(&query.group_by[0]);
        sql.push_str(&format!(
            "  COALESCE({alias}_val::text, 'null') AS group_key,\n"
        ));
    } else {
        // Multi-column group key: concatenate with ||| separator.
        let parts: Vec<String> = query
            .group_by
            .iter()
            .map(|attr| {
                let alias = attr_to_alias(attr);
                format!("COALESCE({alias}_val::text, 'null')")
            })
            .collect();
        sql.push_str(&format!(
            "  {} AS group_key,\n",
            parts.join(" || '|||' || ")
        ));
    }

    // Aggregation values as a JSONB object.
    sql.push_str("  jsonb_build_object(\n");
    let agg_parts: Vec<String> = query
        .aggregations
        .iter()
        .map(|agg| {
            let col = format!("{}_val", attr_to_alias(&agg.field));
            let expr = agg_fn_sql(&agg.function, &col);
            format!("    '{}', {}", agg.alias, expr)
        })
        .collect();
    sql.push_str(&agg_parts.join(",\n"));
    sql.push_str("\n  ) AS agg_values,\n");

    // Group count.
    sql.push_str("  COUNT(*) AS group_count\n");

    sql.push_str("FROM pivoted\n");

    // GROUP BY clause.
    if !query.group_by.is_empty() {
        let group_cols: Vec<String> = query
            .group_by
            .iter()
            .map(|attr| format!("{}_val", attr_to_alias(attr)))
            .collect();
        sql.push_str(&format!("GROUP BY {}\n", group_cols.join(", ")));
    }

    // HAVING clause.
    if let Some(having) = &query.having {
        // Find the aggregation that matches the having alias.
        if let Some(agg) = query.aggregations.iter().find(|a| a.alias == having.alias) {
            let col = format!("{}_val", attr_to_alias(&agg.field));
            let agg_expr = agg_fn_sql(&agg.function, &col);
            let op = having.op.sql_op();

            // Extract numeric comparison value.
            let cmp_val = match &having.value {
                Value::Number(n) => n.to_string(),
                Value::String(s) => {
                    // Parameterize string comparisons.
                    let placeholder = format!("${param_idx}");
                    params.push(Value::String(s.clone()));
                    param_idx += 1;
                    placeholder
                }
                other => other.to_string(),
            };

            sql.push_str(&format!("HAVING {agg_expr} {op} {cmp_val}\n"));
        }
    }

    // Order by group count descending by default.
    sql.push_str("ORDER BY group_count DESC\n");

    // Suppress the "unused" warning for the final param_idx increment.
    let _ = param_idx;

    (sql, params)
}

/// Map a [`WhereOp`] to its SQL string (public for chart module).
pub(crate) fn where_op_sql_pub(op: &crate::query::WhereOp) -> &'static str {
    where_op_sql(op)
}

/// Map a [`WhereOp`] to its SQL string.
fn where_op_sql(op: &crate::query::WhereOp) -> &'static str {
    match op {
        crate::query::WhereOp::Eq => "=",
        crate::query::WhereOp::Neq => "!=",
        crate::query::WhereOp::Gt => ">",
        crate::query::WhereOp::Gte => ">=",
        crate::query::WhereOp::Lt => "<",
        crate::query::WhereOp::Lte => "<=",
        crate::query::WhereOp::Contains => "@>",
        crate::query::WhereOp::Like => "LIKE",
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::WhereOp;

    fn make_query(
        entity_type: &str,
        group_by: Vec<&str>,
        aggregations: Vec<(&str, AggFn, &str)>,
    ) -> AggregateQuery {
        AggregateQuery {
            entity_type: entity_type.into(),
            group_by: group_by.into_iter().map(String::from).collect(),
            aggregations: aggregations
                .into_iter()
                .map(|(field, function, alias)| Aggregation {
                    field: field.into(),
                    function,
                    alias: alias.into(),
                })
                .collect(),
            filters: vec![],
            having: None,
        }
    }

    #[test]
    fn basic_sql_has_cte_structure() {
        let q = make_query("Order", vec!["status"], vec![("amount", AggFn::Sum, "total")]);
        let (sql, params) = build_aggregate_sql(&q);

        assert!(sql.contains("WITH entity_ids AS"));
        assert!(sql.contains("pivoted AS"));
        assert!(sql.contains("GROUP BY"));
        assert!(sql.contains("to_jsonb($1::text)"));
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], Value::String("Order".into()));
    }

    #[test]
    fn no_group_by_uses_all_key() {
        let q = make_query("Order", vec![], vec![("amount", AggFn::Sum, "total")]);
        let (sql, _) = build_aggregate_sql(&q);

        assert!(sql.contains("'_all' AS group_key"));
        assert!(!sql.contains("GROUP BY"));
    }

    #[test]
    fn multi_group_by_uses_separator() {
        let q = make_query(
            "Order",
            vec!["region", "status"],
            vec![("amount", AggFn::Count, "cnt")],
        );
        let (sql, _) = build_aggregate_sql(&q);

        assert!(sql.contains("|||"));
        assert!(sql.contains("region_val"));
        assert!(sql.contains("status_val"));
        assert!(sql.contains("GROUP BY region_val, status_val"));
    }

    #[test]
    fn where_filters_add_subqueries() {
        let mut q = make_query("Order", vec![], vec![("amount", AggFn::Sum, "total")]);
        q.filters.push(WhereClause {
            attribute: "status".into(),
            op: WhereOp::Eq,
            value: Value::String("active".into()),
        });

        let (sql, params) = build_aggregate_sql(&q);

        assert!(sql.contains("entity_id IN ("));
        assert!(sql.contains("attribute = 'status'"));
        assert_eq!(params.len(), 2); // entity_type + filter value
    }

    #[test]
    fn having_clause_generates_sql() {
        let mut q = make_query("Order", vec!["status"], vec![("amount", AggFn::Sum, "total")]);
        q.having = Some(super::super::engine::HavingClause {
            alias: "total".into(),
            op: super::super::engine::HavingOp::Gt,
            value: Value::Number(1000.into()),
        });

        let (sql, _) = build_aggregate_sql(&q);

        assert!(sql.contains("HAVING"));
        assert!(sql.contains("> 1000"));
    }

    #[test]
    fn sanitize_attr_strips_dangerous_chars() {
        assert_eq!(sanitize_attr("user/email"), "user/email");
        assert_eq!(sanitize_attr(":db/type"), ":db/type");
        assert_eq!(sanitize_attr("name; DROP TABLE--"), "nameDROPTABLE--");
        assert_eq!(sanitize_attr("amount' OR '1'='1"), "amountOR11");
    }

    #[test]
    fn attr_to_alias_replaces_special_chars() {
        assert_eq!(attr_to_alias("user/email"), "user_email");
        assert_eq!(attr_to_alias(":db/type"), "_db_type");
        assert_eq!(attr_to_alias("order.total"), "order_total");
    }

    #[test]
    fn all_agg_fns_produce_valid_sql() {
        let fns: Vec<AggFn> = vec![
            AggFn::Count,
            AggFn::CountDistinct,
            AggFn::Sum,
            AggFn::Avg,
            AggFn::Min,
            AggFn::Max,
            AggFn::StdDev,
            AggFn::Variance,
            AggFn::Median,
            AggFn::Percentile(0.95),
            AggFn::First,
            AggFn::Last,
            AggFn::ArrayAgg,
            AggFn::StringAgg(", ".into()),
            AggFn::CountEmpty,
            AggFn::CountFilled,
            AggFn::PercentEmpty,
            AggFn::PercentFilled,
        ];

        for func in &fns {
            let sql = agg_fn_sql(func, "col_val");
            assert!(!sql.is_empty(), "AggFn {:?} produced empty SQL", func);
            // Must reference the column expression.
            assert!(
                sql.contains("col_val") || matches!(func, AggFn::CountEmpty | AggFn::PercentEmpty),
                "AggFn {:?} does not reference column",
                func
            );
        }
    }

    #[test]
    fn string_agg_escapes_single_quotes() {
        let sql = agg_fn_sql(&AggFn::StringAgg("it's".into()), "col");
        assert!(sql.contains("it''s"), "Single quotes not escaped: {sql}");
    }

    #[test]
    fn pivot_uses_lateral_join() {
        let q = make_query(
            "Invoice",
            vec!["customer"],
            vec![("total", AggFn::Sum, "sum_total")],
        );
        let (sql, _) = build_aggregate_sql(&q);

        assert!(sql.contains("LEFT JOIN LATERAL"));
        assert!(sql.contains("ORDER BY tx_id DESC LIMIT 1"));
    }

    #[test]
    fn params_are_ordered_correctly() {
        let mut q = make_query("Order", vec![], vec![("amount", AggFn::Sum, "total")]);
        q.filters.push(WhereClause {
            attribute: "status".into(),
            op: WhereOp::Eq,
            value: Value::String("active".into()),
        });
        q.filters.push(WhereClause {
            attribute: "region".into(),
            op: WhereOp::Eq,
            value: Value::String("US".into()),
        });

        let (sql, params) = build_aggregate_sql(&q);

        assert_eq!(params.len(), 3);
        assert_eq!(params[0], Value::String("Order".into()));
        assert_eq!(params[1], Value::String("active".into()));
        assert_eq!(params[2], Value::String("US".into()));
        assert!(sql.contains("$1"));
        assert!(sql.contains("$2"));
        assert!(sql.contains("$3"));
    }
}
