//! DarshanQL query engine: parse, plan, and execute queries over the triple store.
//!
//! Queries are expressed as JSON objects using a declarative syntax inspired
//! by Datomic pull and GraphQL. The engine converts these into SQL plans
//! that join across the `triples` table, caching plan shapes in an LRU.

pub mod reactive;

use lru::LruCache;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::num::NonZeroUsize;
use std::sync::Mutex;

use crate::error::{DarshanError, Result};

// ── AST ─────────────────────────────────────────────────────────────

/// Top-level query AST produced by [`parse_darshan_ql`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryAST {
    /// Entity type to query (e.g. `"User"`).
    pub entity_type: String,
    /// Where-clause predicates.
    #[serde(default)]
    pub where_clauses: Vec<WhereClause>,
    /// Ordering specification.
    #[serde(default)]
    pub order: Vec<OrderClause>,
    /// Maximum rows to return.
    pub limit: Option<u32>,
    /// Offset for pagination.
    pub offset: Option<u32>,
    /// Full-text search term.
    pub search: Option<String>,
    /// Semantic / vector search term (placeholder for future embeddings).
    pub semantic: Option<String>,
    /// Nested entity references to resolve inline.
    #[serde(default)]
    pub nested: Vec<NestedQuery>,
}

/// A single predicate in a `$where` clause.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhereClause {
    /// Attribute name to filter on.
    pub attribute: String,
    /// Comparison operator.
    pub op: WhereOp,
    /// Value to compare against.
    pub value: serde_json::Value,
}

/// Supported comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhereOp {
    /// Exact equality (`=`).
    Eq,
    /// Not equal (`!=`).
    Neq,
    /// Greater than (`>`).
    Gt,
    /// Greater than or equal (`>=`).
    Gte,
    /// Less than (`<`).
    Lt,
    /// Less than or equal (`<=`).
    Lte,
    /// JSON containment (`@>`).
    Contains,
    /// LIKE / ILIKE prefix match.
    Like,
}

/// Ordering direction for result sets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderClause {
    /// Attribute to order by.
    pub attribute: String,
    /// Sort direction.
    pub direction: SortDirection,
}

/// Ascending or descending sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    /// Ascending order (default).
    Asc,
    /// Descending order.
    Desc,
}

/// A nested entity query (resolves references inline in results).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NestedQuery {
    /// The reference attribute that holds the foreign entity UUID.
    pub via_attribute: String,
    /// Optional sub-query to apply on the nested entity.
    pub sub_query: Option<Box<QueryAST>>,
}

// ── Parsing ─────────────────────────────────────────────────────────

/// Parse a DarshanQL JSON value into a [`QueryAST`].
///
/// # Expected format
///
/// ```json
/// {
///   "type": "User",
///   "$where": [{ "attribute": "email", "op": "Eq", "value": "a@b.com" }],
///   "$order": [{ "attribute": "created_at", "direction": "Desc" }],
///   "$limit": 50,
///   "$offset": 0,
///   "$search": "alice",
///   "$semantic": null,
///   "$nested": [{ "via_attribute": "org_id" }]
/// }
/// ```
pub fn parse_darshan_ql(input: &serde_json::Value) -> Result<QueryAST> {
    let obj = input
        .as_object()
        .ok_or_else(|| DarshanError::InvalidQuery("query must be a JSON object".into()))?;

    let entity_type = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DarshanError::InvalidQuery("missing 'type' field".into()))?
        .to_string();

    let where_clauses: Vec<WhereClause> = match obj.get("$where") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| DarshanError::InvalidQuery(format!("invalid $where: {e}")))?,
        None => Vec::new(),
    };

    let order: Vec<OrderClause> = match obj.get("$order") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| DarshanError::InvalidQuery(format!("invalid $order: {e}")))?,
        None => Vec::new(),
    };

    let limit = obj.get("$limit").and_then(|v| v.as_u64()).map(|v| v as u32);
    let offset = obj
        .get("$offset")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let search = obj
        .get("$search")
        .and_then(|v| v.as_str())
        .map(String::from);
    let semantic = obj
        .get("$semantic")
        .and_then(|v| v.as_str())
        .map(String::from);

    let nested: Vec<NestedQuery> = match obj.get("$nested") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| DarshanError::InvalidQuery(format!("invalid $nested: {e}")))?,
        None => Vec::new(),
    };

    Ok(QueryAST {
        entity_type,
        where_clauses,
        order,
        limit,
        offset,
        search,
        semantic,
        nested,
    })
}

// ── Query Plan ──────────────────────────────────────────────────────

/// A compiled query plan ready for execution.
#[derive(Debug, Clone)]
pub struct QueryPlan {
    /// The generated SQL statement.
    pub sql: String,
    /// Ordered bind parameters (as JSON values for sqlx binding).
    pub params: Vec<serde_json::Value>,
    /// Nested plans to execute after the root results are fetched.
    pub nested_plans: Vec<NestedPlan>,
}

/// Plan for resolving a nested reference.
#[derive(Debug, Clone)]
pub struct NestedPlan {
    /// The attribute on the parent whose value is a UUID reference.
    pub via_attribute: String,
    /// SQL to fetch the nested entity's triples.
    pub sql: String,
}

/// Convert a [`QueryAST`] into an executable [`QueryPlan`].
///
/// The planner generates SQL that joins the `triples` table once per
/// attribute mentioned in the query (where-clauses, ordering, etc.)
/// and applies pagination server-side.
pub fn plan_query(ast: &QueryAST) -> Result<QueryPlan> {
    let mut sql = String::with_capacity(512);
    let mut params: Vec<serde_json::Value> = Vec::new();
    let mut param_idx = 1u32;

    // Base: find entity_ids that have :db/type = entity_type
    sql.push_str(
        "SELECT DISTINCT t0.entity_id, t0.attribute, t0.value, t0.value_type, t0.tx_id, t0.created_at\n",
    );
    sql.push_str("FROM triples t0\n");

    // Join for type filter
    sql.push_str("INNER JOIN triples t_type ON t_type.entity_id = t0.entity_id\n");
    sql.push_str("  AND t_type.attribute = ':db/type'\n");
    sql.push_str("  AND NOT t_type.retracted\n");
    sql.push_str(&format!("  AND t_type.value = ${param_idx}::jsonb\n"));
    params.push(serde_json::Value::String(ast.entity_type.clone()));
    param_idx += 1;

    // Joins for where-clause attributes
    for (i, wc) in ast.where_clauses.iter().enumerate() {
        let alias = format!("tw{i}");
        sql.push_str(&format!(
            "INNER JOIN triples {alias} ON {alias}.entity_id = t0.entity_id\n"
        ));
        sql.push_str(&format!("  AND {alias}.attribute = ${param_idx}\n"));
        params.push(serde_json::Value::String(wc.attribute.clone()));
        param_idx += 1;

        sql.push_str(&format!("  AND NOT {alias}.retracted\n"));

        let op_sql = match wc.op {
            WhereOp::Eq => format!("  AND {alias}.value = ${param_idx}::jsonb\n"),
            WhereOp::Neq => format!("  AND {alias}.value != ${param_idx}::jsonb\n"),
            WhereOp::Gt => format!("  AND {alias}.value > ${param_idx}::jsonb\n"),
            WhereOp::Gte => format!("  AND {alias}.value >= ${param_idx}::jsonb\n"),
            WhereOp::Lt => format!("  AND {alias}.value < ${param_idx}::jsonb\n"),
            WhereOp::Lte => format!("  AND {alias}.value <= ${param_idx}::jsonb\n"),
            WhereOp::Contains => format!("  AND {alias}.value @> ${param_idx}::jsonb\n"),
            WhereOp::Like => format!("  AND {alias}.value #>> '{{}}' ILIKE ${param_idx}\n"),
        };
        sql.push_str(&op_sql);
        params.push(wc.value.clone());
        param_idx += 1;
    }

    // Full-text search clause (simple ILIKE across all values)
    if let Some(ref term) = ast.search {
        sql.push_str(&format!(
            "INNER JOIN triples t_search ON t_search.entity_id = t0.entity_id\n"
        ));
        sql.push_str(&format!(
            "  AND NOT t_search.retracted\n  AND t_search.value #>> '{{}}' ILIKE ${param_idx}\n"
        ));
        params.push(serde_json::Value::String(format!("%{term}%")));
        param_idx += 1;
    }

    sql.push_str("WHERE NOT t0.retracted\n");

    // Ordering: join the order attributes and sort
    if !ast.order.is_empty() {
        sql.push_str("ORDER BY ");
        let mut first = true;
        for (i, oc) in ast.order.iter().enumerate() {
            if !first {
                sql.push_str(", ");
            }
            first = false;
            let alias = format!("to{i}");
            // We need a sub-select or lateral join for ordering;
            // for simplicity, sort by entity_id-scoped value.
            // The ORDER BY references a correlated subquery.
            sql.push_str(&format!(
                "(SELECT {alias}.value FROM triples {alias} WHERE {alias}.entity_id = t0.entity_id AND {alias}.attribute = ${param_idx} AND NOT {alias}.retracted ORDER BY {alias}.tx_id DESC LIMIT 1)",
            ));
            params.push(serde_json::Value::String(oc.attribute.clone()));
            param_idx += 1;

            match oc.direction {
                SortDirection::Asc => sql.push_str(" ASC"),
                SortDirection::Desc => sql.push_str(" DESC"),
            }
        }
        sql.push('\n');
    }

    // Pagination
    if let Some(limit) = ast.limit {
        sql.push_str(&format!("LIMIT {limit}\n"));
    }
    if let Some(offset) = ast.offset {
        sql.push_str(&format!("OFFSET {offset}\n"));
    }

    // Nested plans
    let nested_plans: Vec<NestedPlan> = ast
        .nested
        .iter()
        .map(|n| NestedPlan {
            via_attribute: n.via_attribute.clone(),
            sql: format!(
                "SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted \
                 FROM triples WHERE entity_id = $1 AND NOT retracted ORDER BY attribute, tx_id DESC"
            ),
        })
        .collect();

    Ok(QueryPlan {
        sql,
        params,
        nested_plans,
    })
}

// ── Execution ───────────────────────────────────────────────────────

/// Result row from a DarshanQL query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResultRow {
    /// The entity UUID.
    pub entity_id: uuid::Uuid,
    /// Attribute-value map for the entity.
    pub attributes: serde_json::Map<String, serde_json::Value>,
    /// Resolved nested entities keyed by reference attribute.
    #[serde(default)]
    pub nested: serde_json::Map<String, serde_json::Value>,
}

/// Execute a [`QueryPlan`] against the database and resolve nested references.
///
/// Returns a list of entity result rows with their attributes merged
/// and nested entities resolved inline.
pub async fn execute_query(pool: &PgPool, plan: &QueryPlan) -> Result<Vec<QueryResultRow>> {
    // Build the query with dynamic binds.
    let mut query = sqlx::query_as::<
        _,
        (
            uuid::Uuid,
            String,
            serde_json::Value,
            i16,
            i64,
            chrono::DateTime<chrono::Utc>,
        ),
    >(&plan.sql);

    for p in &plan.params {
        query = bind_json_param(query, p);
    }

    let rows = query.fetch_all(pool).await?;

    // Group by entity_id.
    let mut entities: std::collections::HashMap<
        uuid::Uuid,
        serde_json::Map<String, serde_json::Value>,
    > = std::collections::HashMap::new();

    for (entity_id, attribute, value, _value_type, _tx_id, _created_at) in &rows {
        let entry = entities.entry(*entity_id).or_default();
        // Latest tx wins (rows are ordered by tx_id DESC within grouping).
        entry
            .entry(attribute.clone())
            .or_insert_with(|| value.clone());
    }

    // Resolve nested references.
    let mut results: Vec<QueryResultRow> = Vec::with_capacity(entities.len());
    for (entity_id, attributes) in entities {
        let mut nested = serde_json::Map::new();

        for np in &plan.nested_plans {
            if let Some(ref_value) = attributes.get(&np.via_attribute) {
                if let Some(ref_str) = ref_value.as_str() {
                    if let Ok(ref_uuid) = ref_str.parse::<uuid::Uuid>() {
                        let nested_rows = sqlx::query_as::<_, (String, serde_json::Value)>(
                            "SELECT attribute, value FROM triples \
                             WHERE entity_id = $1 AND NOT retracted \
                             ORDER BY attribute, tx_id DESC",
                        )
                        .bind(ref_uuid)
                        .fetch_all(pool)
                        .await?;

                        let mut nested_attrs = serde_json::Map::new();
                        for (attr, val) in nested_rows {
                            nested_attrs.entry(attr).or_insert(val);
                        }
                        nested.insert(
                            np.via_attribute.clone(),
                            serde_json::Value::Object(nested_attrs),
                        );
                    }
                }
            }
        }

        results.push(QueryResultRow {
            entity_id,
            attributes,
            nested,
        });
    }

    Ok(results)
}

/// Bind a `serde_json::Value` as the appropriate sqlx parameter type.
fn bind_json_param<'q>(
    query: sqlx::query::QueryAs<
        'q,
        sqlx::Postgres,
        (
            uuid::Uuid,
            String,
            serde_json::Value,
            i16,
            i64,
            chrono::DateTime<chrono::Utc>,
        ),
        sqlx::postgres::PgArguments,
    >,
    param: &'q serde_json::Value,
) -> sqlx::query::QueryAs<
    'q,
    sqlx::Postgres,
    (
        uuid::Uuid,
        String,
        serde_json::Value,
        i16,
        i64,
        chrono::DateTime<chrono::Utc>,
    ),
    sqlx::postgres::PgArguments,
> {
    // For JSONB comparisons we bind as serde_json::Value;
    // for ILIKE we bind as String.
    match param {
        serde_json::Value::String(s) if s.contains('%') => query.bind(s.as_str()),
        _ => query.bind(param),
    }
}

// ── Plan Cache ──────────────────────────────────────────────────────

/// Thread-safe LRU cache for query plans, keyed by a SHA-256 hash
/// of the query shape (entity type + where attributes + order + nested).
pub struct PlanCache {
    inner: Mutex<LruCache<[u8; 32], QueryPlan>>,
}

impl PlanCache {
    /// Create a new cache with the given capacity.
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(256).expect("256 > 0"));
        Self {
            inner: Mutex::new(LruCache::new(cap)),
        }
    }

    /// Compute the shape hash for a query AST.
    ///
    /// The shape ignores concrete values so that queries differing only
    /// in filter values share the same plan.
    pub fn shape_key(ast: &QueryAST) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(ast.entity_type.as_bytes());

        for wc in &ast.where_clauses {
            hasher.update(wc.attribute.as_bytes());
            hasher.update(&[wc.op as u8]);
        }
        for oc in &ast.order {
            hasher.update(oc.attribute.as_bytes());
            hasher.update(&[oc.direction as u8]);
        }
        if ast.limit.is_some() {
            hasher.update(b"L");
        }
        if ast.offset.is_some() {
            hasher.update(b"O");
        }
        if ast.search.is_some() {
            hasher.update(b"S");
        }
        for n in &ast.nested {
            hasher.update(n.via_attribute.as_bytes());
        }

        hasher.finalize().into()
    }

    /// Look up a cached plan by AST shape.
    pub fn get(&self, ast: &QueryAST) -> Option<QueryPlan> {
        let key = Self::shape_key(ast);
        let mut guard = self.inner.lock().ok()?;
        guard.get(&key).cloned()
    }

    /// Insert a plan into the cache.
    pub fn insert(&self, ast: &QueryAST, plan: QueryPlan) {
        let key = Self::shape_key(ast);
        if let Ok(mut guard) = self.inner.lock() {
            guard.put(key, plan);
        }
    }
}

/// Parse, plan, and execute a DarshanQL query using the plan cache.
///
/// This is the main entry point for query execution. It checks the
/// cache first, falling back to [`plan_query`] on a miss.
pub async fn run_query(
    pool: &PgPool,
    cache: &PlanCache,
    input: &serde_json::Value,
) -> Result<Vec<QueryResultRow>> {
    let ast = parse_darshan_ql(input)?;

    let plan = match cache.get(&ast) {
        Some(cached) => {
            tracing::debug!("plan cache hit for type={}", ast.entity_type);
            // Re-plan to get fresh params (cache stores the shape, not values).
            let mut fresh = plan_query(&ast)?;
            fresh.sql = cached.sql;
            fresh
        }
        None => {
            tracing::debug!("plan cache miss for type={}", ast.entity_type);
            let plan = plan_query(&ast)?;
            cache.insert(&ast, plan.clone());
            plan
        }
    };

    execute_query(pool, &plan).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_query() {
        let input = serde_json::json!({
            "type": "User"
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert_eq!(ast.entity_type, "User");
        assert!(ast.where_clauses.is_empty());
        assert!(ast.order.is_empty());
    }

    #[test]
    fn parse_full_query() {
        let input = serde_json::json!({
            "type": "User",
            "$where": [
                { "attribute": "email", "op": "Eq", "value": "a@b.com" }
            ],
            "$order": [
                { "attribute": "created_at", "direction": "Desc" }
            ],
            "$limit": 10,
            "$offset": 5,
            "$search": "alice",
            "$nested": [
                { "via_attribute": "org_id" }
            ]
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert_eq!(ast.entity_type, "User");
        assert_eq!(ast.where_clauses.len(), 1);
        assert_eq!(ast.where_clauses[0].op, WhereOp::Eq);
        assert_eq!(ast.order.len(), 1);
        assert_eq!(ast.limit, Some(10));
        assert_eq!(ast.offset, Some(5));
        assert_eq!(ast.search.as_deref(), Some("alice"));
        assert_eq!(ast.nested.len(), 1);
    }

    #[test]
    fn plan_cache_hit() {
        let cache = PlanCache::new(16);
        let ast = QueryAST {
            entity_type: "User".into(),
            where_clauses: vec![],
            order: vec![],
            limit: None,
            offset: None,
            search: None,
            semantic: None,
            nested: vec![],
        };
        let plan = plan_query(&ast).expect("should plan");
        cache.insert(&ast, plan);
        assert!(cache.get(&ast).is_some());
    }

    #[test]
    fn shape_key_ignores_values() {
        let ast1 = QueryAST {
            entity_type: "User".into(),
            where_clauses: vec![WhereClause {
                attribute: "email".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("a@b.com"),
            }],
            order: vec![],
            limit: None,
            offset: None,
            search: None,
            semantic: None,
            nested: vec![],
        };
        let ast2 = QueryAST {
            entity_type: "User".into(),
            where_clauses: vec![WhereClause {
                attribute: "email".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("other@example.com"),
            }],
            order: vec![],
            limit: None,
            offset: None,
            search: None,
            semantic: None,
            nested: vec![],
        };
        assert_eq!(PlanCache::shape_key(&ast1), PlanCache::shape_key(&ast2));
    }

    #[test]
    fn reject_non_object_query() {
        let input = serde_json::json!("not an object");
        assert!(parse_darshan_ql(&input).is_err());
    }
}
