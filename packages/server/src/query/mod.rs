//! DarshanQL query engine: parse, plan, and execute queries over the triple store.
//!
//! Queries are expressed as JSON objects using a declarative syntax inspired
//! by Datomic pull and GraphQL. The engine converts these into SQL plans
//! that join across the `triples` table, caching plan shapes in an LRU.

pub mod parallel;
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
    /// Semantic / vector search clause (pgvector cosine similarity).
    pub semantic: Option<SemanticQuery>,
    /// Hybrid search clause (combines tsvector + pgvector via RRF).
    pub hybrid: Option<HybridQuery>,
    /// Nested entity references to resolve inline.
    #[serde(default)]
    pub nested: Vec<NestedQuery>,
}

/// Semantic (vector) search clause for `$semantic`.
///
/// Accepts either a pre-computed `vector` or a `query` text string.
/// When `query` is supplied without `vector`, the engine logs a warning
/// that an embedding API must be configured to convert text to vectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticQuery {
    /// Pre-computed embedding vector for similarity search.
    #[serde(default)]
    pub vector: Option<Vec<f32>>,
    /// Text query (requires embedding API to convert to vector).
    #[serde(default)]
    pub query: Option<String>,
    /// Maximum number of results to return from the vector search.
    #[serde(default = "default_semantic_limit")]
    pub limit: u32,
}

/// Hybrid search clause for `$hybrid` — Reciprocal Rank Fusion of
/// full-text (tsvector) and vector (pgvector) results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridQuery {
    /// Text query for full-text search component.
    pub text: String,
    /// Pre-computed embedding vector for vector search component.
    pub vector: Vec<f32>,
    /// Weight for text search results in RRF (0.0..=1.0).
    #[serde(default = "default_text_weight")]
    pub text_weight: f32,
    /// Weight for vector search results in RRF (0.0..=1.0).
    #[serde(default = "default_vector_weight")]
    pub vector_weight: f32,
    /// Maximum number of results to return.
    #[serde(default = "default_semantic_limit")]
    pub limit: u32,
}

fn default_semantic_limit() -> u32 {
    10
}

fn default_text_weight() -> f32 {
    0.3
}

fn default_vector_weight() -> f32 {
    0.7
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
    let semantic: Option<SemanticQuery> = match obj.get("$semantic") {
        Some(v) if v.is_null() => None,
        Some(v) if v.is_string() => {
            // Legacy string form: { "$semantic": "meaning of life" }
            Some(SemanticQuery {
                vector: None,
                query: v.as_str().map(String::from),
                limit: default_semantic_limit(),
            })
        }
        Some(v) if v.is_object() => {
            // Rich form: { "$semantic": { "vector": [...], "limit": 10 } }
            Some(
                serde_json::from_value(v.clone())
                    .map_err(|e| DarshanError::InvalidQuery(format!("invalid $semantic: {e}")))?,
            )
        }
        Some(_) => {
            return Err(DarshanError::InvalidQuery(
                "$semantic must be a string, object, or null".into(),
            ));
        }
        None => None,
    };

    let hybrid: Option<HybridQuery> = match obj.get("$hybrid") {
        Some(v) if v.is_null() => None,
        Some(v) => Some(
            serde_json::from_value(v.clone())
                .map_err(|e| DarshanError::InvalidQuery(format!("invalid $hybrid: {e}")))?,
        ),
        None => None,
    };

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
        hybrid,
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
    /// Sub-nested plans for multi-level resolution (e.g. todos -> owner -> org).
    pub sub_nested: Vec<NestedPlan>,
}

/// Maximum nesting depth to prevent query explosion.
const MAX_NESTING_DEPTH: usize = 3;

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

    // Full-text search clause using PostgreSQL tsvector/tsquery.
    // Uses the GIN index on to_tsvector('english', value #>> '{}') for
    // efficient ranked full-text matching instead of brute-force ILIKE.
    if let Some(ref term) = ast.search {
        sql.push_str("INNER JOIN triples t_search ON t_search.entity_id = t0.entity_id\n");
        sql.push_str("  AND NOT t_search.retracted\n");
        sql.push_str(&format!(
            "  AND to_tsvector('english', t_search.value #>> '{{}}') @@ plainto_tsquery('english', ${param_idx})\n"
        ));
        params.push(serde_json::Value::String(term.clone()));
        param_idx += 1;
    }

    // Semantic (vector) search: join the embeddings table and use pgvector's
    // cosine distance operator (<=>) for nearest-neighbour ordering.
    if let Some(ref sem) = ast.semantic {
        if let Some(ref vec) = sem.vector {
            // Pre-computed vector supplied — wire directly to pgvector.
            let vec_literal = format_vector_literal(vec);
            sql.push_str("INNER JOIN embeddings t_emb ON t_emb.entity_id = t0.entity_id\n");
            sql.push_str(&format!(
                "  AND t_emb.embedding <=> '{vec_literal}'::vector < 2.0\n"
            ));
            // We store the vector literal as a param for the ORDER BY clause later.
        } else if sem.query.is_some() {
            tracing::warn!(
                "$semantic.query requires an embedding API to convert text to vectors; \
                 pass a pre-computed vector via $semantic.vector instead"
            );
        }
    }

    // Hybrid search uses a CTE-based approach, so it is handled in
    // plan_hybrid_query() rather than here.
    if ast.hybrid.is_some() && ast.semantic.is_none() {
        // Hybrid is handled separately; this branch catches the case where
        // someone passes $hybrid without $semantic.
    }

    sql.push_str("WHERE NOT t0.retracted\n");

    // Ordering: when semantic search is active with a vector, order by
    // cosine distance (ascending = most similar first). Explicit $order
    // clauses are appended after the distance sort as tiebreakers.
    let has_semantic_vector = ast.semantic.as_ref().is_some_and(|s| s.vector.is_some());

    if has_semantic_vector || !ast.order.is_empty() {
        sql.push_str("ORDER BY ");
        let mut first = true;

        // Vector distance sort (most similar first).
        if let Some(ref sem) = ast.semantic
            && let Some(ref vec) = sem.vector
        {
            let vec_literal = format_vector_literal(vec);
            sql.push_str(&format!("t_emb.embedding <=> '{vec_literal}'::vector ASC"));
            first = false;
        }

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

    // Pagination — parameterised to keep plan shapes reusable and to
    // follow the same bind-everything policy as the rest of the query.
    // For semantic queries, use the semantic limit if no explicit $limit.
    let effective_limit = ast.limit.or_else(|| ast.semantic.as_ref().map(|s| s.limit));
    if let Some(limit) = effective_limit {
        sql.push_str(&format!("LIMIT ${param_idx}\n"));
        params.push(serde_json::json!(limit));
        param_idx += 1;
    }
    if let Some(offset) = ast.offset {
        sql.push_str(&format!("OFFSET ${param_idx}\n"));
        params.push(serde_json::json!(offset));
        #[allow(unused_assignments)]
        {
            param_idx += 1;
        }
    }

    // Nested plans (with recursive sub-nesting up to MAX_NESTING_DEPTH)
    let nested_plans = build_nested_plans(&ast.nested, 1);

    Ok(QueryPlan {
        sql,
        params,
        nested_plans,
    })
}

/// Recursively build nested plans from the AST's nested queries,
/// respecting [`MAX_NESTING_DEPTH`] to prevent query explosion.
fn build_nested_plans(nested: &[NestedQuery], depth: usize) -> Vec<NestedPlan> {
    if depth > MAX_NESTING_DEPTH {
        return Vec::new();
    }
    nested
        .iter()
        .map(|n| {
            let sub_nested = match &n.sub_query {
                Some(sub) => build_nested_plans(&sub.nested, depth + 1),
                None => Vec::new(),
            };
            NestedPlan {
                via_attribute: n.via_attribute.clone(),
                sql: "SELECT attribute, value FROM triples \
                     WHERE entity_id = ANY($1::uuid[]) AND NOT retracted \
                     ORDER BY entity_id, attribute, tx_id DESC"
                    .to_string(),
                sub_nested,
            }
        })
        .collect()
}

/// Format a vector of f32 values as a pgvector literal string: `[0.1,0.2,0.3]`.
fn format_vector_literal(vec: &[f32]) -> String {
    let mut s = String::with_capacity(vec.len() * 8 + 2);
    s.push('[');
    for (i, v) in vec.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&v.to_string());
    }
    s.push(']');
    s
}

/// Build a hybrid search query plan that combines tsvector full-text search
/// with pgvector cosine similarity using Reciprocal Rank Fusion (RRF).
///
/// The plan executes two CTEs — one for text ranking, one for vector ranking —
/// then merges them with weighted RRF scores: `weight / (k + rank)`.
pub fn plan_hybrid_query(ast: &QueryAST) -> Result<QueryPlan> {
    let hybrid = ast
        .hybrid
        .as_ref()
        .ok_or_else(|| DarshanError::InvalidQuery("$hybrid clause is required".into()))?;

    let vec_literal = format_vector_literal(&hybrid.vector);
    let text_w = hybrid.text_weight;
    let vector_w = hybrid.vector_weight;
    let limit = hybrid.limit;
    let k = 60; // RRF constant (standard value from the literature)

    // The SQL uses two CTEs:
    //   text_ranked: full-text search results ranked by ts_rank_cd
    //   vector_ranked: cosine similarity results ranked by distance
    // Then a FULL OUTER JOIN with RRF scoring to merge both lists.
    let sql = format!(
        r#"WITH type_entities AS (
    SELECT DISTINCT entity_id
    FROM triples
    WHERE attribute = ':db/type'
      AND value = $1::jsonb
      AND NOT retracted
),
text_ranked AS (
    SELECT t.entity_id,
           ROW_NUMBER() OVER (ORDER BY ts_rank_cd(
               to_tsvector('english', t.value #>> '{{}}'),
               plainto_tsquery('english', $2)
           ) DESC) AS rank
    FROM triples t
    INNER JOIN type_entities te ON te.entity_id = t.entity_id
    WHERE NOT t.retracted
      AND to_tsvector('english', t.value #>> '{{}}') @@ plainto_tsquery('english', $2)
    LIMIT {limit_inner}
),
vector_ranked AS (
    SELECT e.entity_id,
           ROW_NUMBER() OVER (ORDER BY e.embedding <=> '{vec_literal}'::vector) AS rank,
           (e.embedding <=> '{vec_literal}'::vector) AS distance
    FROM embeddings e
    INNER JOIN type_entities te ON te.entity_id = e.entity_id
    ORDER BY e.embedding <=> '{vec_literal}'::vector
    LIMIT {limit_inner}
),
rrf_merged AS (
    SELECT COALESCE(tr.entity_id, vr.entity_id) AS entity_id,
           COALESCE({text_w} / ({k} + tr.rank), 0.0) +
           COALESCE({vector_w} / ({k} + vr.rank), 0.0) AS rrf_score,
           vr.distance
    FROM text_ranked tr
    FULL OUTER JOIN vector_ranked vr ON tr.entity_id = vr.entity_id
    ORDER BY rrf_score DESC
    LIMIT {limit}
)
SELECT t0.entity_id, t0.attribute, t0.value, t0.value_type, t0.tx_id, t0.created_at
FROM triples t0
INNER JOIN rrf_merged rm ON rm.entity_id = t0.entity_id
WHERE NOT t0.retracted
ORDER BY rm.rrf_score DESC
"#,
        vec_literal = vec_literal,
        text_w = text_w,
        vector_w = vector_w,
        k = k,
        limit = limit,
        limit_inner = limit * 3, // Oversample for better RRF fusion
    );

    let params = vec![
        serde_json::Value::String(ast.entity_type.clone()),
        serde_json::Value::String(hybrid.text.clone()),
    ];

    // Nested plans reuse the standard approach.
    let nested_plans = build_nested_plans(&ast.nested, 1);

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

    // Resolve nested references using batched fetching.
    // Instead of N queries (one per parent entity per nested plan), we collect
    // all referenced UUIDs and fetch them in a single WHERE entity_id = ANY($1)
    // query per nested plan. This turns N+1 into 1+P where P = number of nested
    // plans (typically 1-3), regardless of how many parent entities exist.
    let nested_maps = batch_resolve_nested(pool, &entities, &plan.nested_plans).await?;

    let mut results: Vec<QueryResultRow> = Vec::with_capacity(entities.len());
    for (entity_id, attributes) in &entities {
        let mut nested = serde_json::Map::new();

        for (np_idx, np) in plan.nested_plans.iter().enumerate() {
            if let Some(ref_value) = attributes.get(&np.via_attribute)
                && let Some(ref_str) = ref_value.as_str()
                && let Ok(ref_uuid) = ref_str.parse::<uuid::Uuid>()
                && let Some(nested_entity) = nested_maps[np_idx].get(&ref_uuid)
            {
                nested.insert(
                    np.via_attribute.clone(),
                    serde_json::Value::Object(nested_entity.clone()),
                );
            }
        }

        results.push(QueryResultRow {
            entity_id: *entity_id,
            attributes: attributes.clone(),
            nested,
        });
    }

    Ok(results)
}

/// Batch-fetch nested entities for all parent entities at once.
///
/// For each `NestedPlan`, collects all referenced UUIDs from the parent
/// entity attributes, fetches their triples in a single
/// `WHERE entity_id = ANY($1::uuid[])` query, groups results by entity_id,
/// and recursively resolves sub-nested references (up to [`MAX_NESTING_DEPTH`]).
///
/// Returns one `HashMap<Uuid, Map>` per nested plan, in the same order as
/// `nested_plans`, where each map entry is `referenced_uuid -> attributes`.
#[allow(clippy::type_complexity)]
fn batch_resolve_nested<'a>(
    pool: &'a PgPool,
    parent_entities: &'a std::collections::HashMap<
        uuid::Uuid,
        serde_json::Map<String, serde_json::Value>,
    >,
    nested_plans: &'a [NestedPlan],
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<
                    Vec<
                        std::collections::HashMap<
                            uuid::Uuid,
                            serde_json::Map<String, serde_json::Value>,
                        >,
                    >,
                >,
            > + Send
            + 'a,
    >,
> {
    Box::pin(async move {
        let mut all_nested_maps = Vec::with_capacity(nested_plans.len());

        for np in nested_plans {
            // Step 1: Collect all referenced UUIDs from parent entities for this attribute.
            let ref_uuids: Vec<uuid::Uuid> = parent_entities
                .values()
                .filter_map(|attrs| {
                    attrs
                        .get(&np.via_attribute)
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<uuid::Uuid>().ok())
                })
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            if ref_uuids.is_empty() {
                all_nested_maps.push(std::collections::HashMap::new());
                continue;
            }

            // Step 2: Batch-fetch all referenced entities in one query.
            let rows = sqlx::query_as::<_, (uuid::Uuid, String, serde_json::Value)>(
                "SELECT entity_id, attribute, value FROM triples \
             WHERE entity_id = ANY($1::uuid[]) AND NOT retracted \
             ORDER BY entity_id, attribute, tx_id DESC",
            )
            .bind(&ref_uuids)
            .fetch_all(pool)
            .await?;

            // Step 3: Group fetched triples by entity_id.
            let mut grouped: std::collections::HashMap<
                uuid::Uuid,
                serde_json::Map<String, serde_json::Value>,
            > = std::collections::HashMap::new();

            for (eid, attr, val) in rows {
                let entry = grouped.entry(eid).or_default();
                // First attribute value wins (rows ordered by tx_id DESC).
                entry.entry(attr).or_insert(val);
            }

            // Step 4: Recursively resolve sub-nested references if any.
            if !np.sub_nested.is_empty() {
                let sub_maps = batch_resolve_nested(pool, &grouped, &np.sub_nested).await?;

                // Attach sub-nested results to each grouped entity.
                for (eid, attrs) in grouped.iter_mut() {
                    for (sub_idx, sub_np) in np.sub_nested.iter().enumerate() {
                        if let Some(ref_value) = attrs.get(&sub_np.via_attribute)
                            && let Some(ref_str) = ref_value.as_str()
                            && let Ok(ref_uuid) = ref_str.parse::<uuid::Uuid>()
                            && let Some(sub_entity) = sub_maps[sub_idx].get(&ref_uuid)
                        {
                            // Store under a _nested key to avoid attribute collision.
                            let nested_key = format!("_nested:{}", sub_np.via_attribute);
                            attrs.insert(nested_key, serde_json::Value::Object(sub_entity.clone()));
                        }
                        let _ = eid; // suppress unused warning in the non-sub_nested path
                    }
                }
            }

            all_nested_maps.push(grouped);
        }

        Ok(all_nested_maps)
    })
}

/// Row type returned by triple queries.
type TripleRow = (
    uuid::Uuid,
    String,
    serde_json::Value,
    i16,
    i64,
    chrono::DateTime<chrono::Utc>,
);

/// Bind a `serde_json::Value` as the appropriate sqlx parameter type.
fn bind_json_param<'q>(
    query: sqlx::query::QueryAs<'q, sqlx::Postgres, TripleRow, sqlx::postgres::PgArguments>,
    param: &'q serde_json::Value,
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, TripleRow, sqlx::postgres::PgArguments> {
    // Bind strings as text (SQL casts to ::jsonb where needed for
    // WHERE-clause comparisons; plainto_tsquery and ILIKE expect text).
    // Non-string JSON values bind as serde_json::Value for JSONB ops.
    match param {
        serde_json::Value::String(s) => query.bind(s.as_str()),
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
            hasher.update([wc.op as u8]);
        }
        for oc in &ast.order {
            hasher.update(oc.attribute.as_bytes());
            hasher.update([oc.direction as u8]);
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
        if ast.semantic.is_some() {
            hasher.update(b"V"); // V for vector/semantic
        }
        if ast.hybrid.is_some() {
            hasher.update(b"H"); // H for hybrid
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

    // Route hybrid queries to the dedicated RRF planner.
    let plan_fn: fn(&QueryAST) -> Result<QueryPlan> = if ast.hybrid.is_some() {
        plan_hybrid_query
    } else {
        plan_query
    };

    let plan = match cache.get(&ast) {
        Some(cached) => {
            tracing::debug!("plan cache hit for type={}", ast.entity_type);
            // Re-plan to get fresh params (cache stores the shape, not values).
            let mut fresh = plan_fn(&ast)?;
            fresh.sql = cached.sql;
            fresh
        }
        None => {
            tracing::debug!("plan cache miss for type={}", ast.entity_type);
            let plan = plan_fn(&ast)?;
            cache.insert(&ast, plan.clone());
            plan
        }
    };

    execute_query(pool, &plan).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper ──────────────────────────────────────────────────────

    fn bare_ast(entity_type: &str) -> QueryAST {
        QueryAST {
            entity_type: entity_type.into(),
            where_clauses: vec![],
            order: vec![],
            limit: None,
            offset: None,
            search: None,
            semantic: None,
            hybrid: None,
            nested: vec![],
        }
    }

    // ── Parsing: every DarshanQL operator ───────────────────────────

    #[test]
    fn parse_minimal_query() {
        let input = serde_json::json!({ "type": "User" });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert_eq!(ast.entity_type, "User");
        assert!(ast.where_clauses.is_empty());
        assert!(ast.order.is_empty());
        assert!(ast.limit.is_none());
        assert!(ast.offset.is_none());
        assert!(ast.search.is_none());
        assert!(ast.semantic.is_none());
        assert!(ast.nested.is_empty());
    }

    #[test]
    fn parse_where_all_operators() {
        for (op_str, expected) in [
            ("Eq", WhereOp::Eq),
            ("Neq", WhereOp::Neq),
            ("Gt", WhereOp::Gt),
            ("Gte", WhereOp::Gte),
            ("Lt", WhereOp::Lt),
            ("Lte", WhereOp::Lte),
            ("Contains", WhereOp::Contains),
            ("Like", WhereOp::Like),
        ] {
            let input = serde_json::json!({
                "type": "Item",
                "$where": [{ "attribute": "x", "op": op_str, "value": 1 }]
            });
            let ast = parse_darshan_ql(&input)
                .unwrap_or_else(|e| panic!("failed to parse op {op_str}: {e}"));
            assert_eq!(
                ast.where_clauses[0].op, expected,
                "op mismatch for {op_str}"
            );
        }
    }

    #[test]
    fn parse_order_asc_desc() {
        let input = serde_json::json!({
            "type": "Post",
            "$order": [
                { "attribute": "created_at", "direction": "Asc" },
                { "attribute": "score", "direction": "Desc" }
            ]
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert_eq!(ast.order.len(), 2);
        assert_eq!(ast.order[0].direction, SortDirection::Asc);
        assert_eq!(ast.order[1].direction, SortDirection::Desc);
    }

    #[test]
    fn parse_limit_offset() {
        let input = serde_json::json!({
            "type": "T",
            "$limit": 100,
            "$offset": 20
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert_eq!(ast.limit, Some(100));
        assert_eq!(ast.offset, Some(20));
    }

    #[test]
    fn parse_search() {
        let input = serde_json::json!({
            "type": "Doc",
            "$search": "hello world"
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert_eq!(ast.search.as_deref(), Some("hello world"));
    }

    #[test]
    fn parse_semantic_legacy_string() {
        let input = serde_json::json!({
            "type": "Doc",
            "$semantic": "meaning of life"
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        let sem = ast.semantic.as_ref().expect("semantic should be Some");
        assert_eq!(sem.query.as_deref(), Some("meaning of life"));
        assert!(sem.vector.is_none());
        assert_eq!(sem.limit, 10); // default
    }

    #[test]
    fn parse_semantic_with_vector() {
        let input = serde_json::json!({
            "type": "Doc",
            "$semantic": { "vector": [0.1, 0.2, 0.3], "limit": 5 }
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        let sem = ast.semantic.as_ref().expect("semantic should be Some");
        assert_eq!(sem.vector.as_ref().unwrap().len(), 3);
        assert_eq!(sem.limit, 5);
    }

    #[test]
    fn parse_semantic_null_is_none() {
        let input = serde_json::json!({
            "type": "Doc",
            "$semantic": null
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert!(ast.semantic.is_none());
    }

    #[test]
    fn parse_hybrid() {
        let input = serde_json::json!({
            "type": "Article",
            "$hybrid": {
                "text": "machine learning",
                "vector": [0.1, 0.2, 0.3],
                "text_weight": 0.4,
                "vector_weight": 0.6,
                "limit": 20
            }
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        let hyb = ast.hybrid.as_ref().expect("hybrid should be Some");
        assert_eq!(hyb.text, "machine learning");
        assert_eq!(hyb.vector.len(), 3);
        assert!((hyb.text_weight - 0.4).abs() < f32::EPSILON);
        assert!((hyb.vector_weight - 0.6).abs() < f32::EPSILON);
        assert_eq!(hyb.limit, 20);
    }

    #[test]
    fn parse_nested() {
        let input = serde_json::json!({
            "type": "User",
            "$nested": [
                { "via_attribute": "org_id" },
                { "via_attribute": "team_id" }
            ]
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert_eq!(ast.nested.len(), 2);
        assert_eq!(ast.nested[0].via_attribute, "org_id");
        assert_eq!(ast.nested[1].via_attribute, "team_id");
    }

    #[test]
    fn parse_full_query() {
        let input = serde_json::json!({
            "type": "User",
            "$where": [{ "attribute": "email", "op": "Eq", "value": "a@b.com" }],
            "$order": [{ "attribute": "created_at", "direction": "Desc" }],
            "$limit": 10,
            "$offset": 5,
            "$search": "alice",
            "$nested": [{ "via_attribute": "org_id" }]
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

    // ── Parsing: nested queries with forward/backward references ────

    #[test]
    fn parse_nested_with_sub_query() {
        // Note: sub_query is deserialized via serde, so it uses struct field
        // names (entity_type, where_clauses) rather than the DarshanQL JSON
        // operators ($where, $order). Only the top-level parse_darshan_ql
        // translates the $ operators.
        let input = serde_json::json!({
            "type": "Order",
            "$nested": [{
                "via_attribute": "customer_id",
                "sub_query": {
                    "entity_type": "Customer",
                    "where_clauses": [{ "attribute": "active", "op": "Eq", "value": true }],
                    "nested": [{
                        "via_attribute": "billing_address_id",
                        "sub_query": {
                            "entity_type": "Address",
                            "limit": 1
                        }
                    }]
                }
            }]
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert_eq!(ast.nested.len(), 1);

        let sub = ast.nested[0].sub_query.as_ref().expect("sub_query");
        assert_eq!(sub.entity_type, "Customer");
        assert_eq!(sub.where_clauses.len(), 1);

        let deep = sub.nested[0].sub_query.as_ref().expect("deep sub_query");
        assert_eq!(deep.entity_type, "Address");
        assert_eq!(deep.limit, Some(1));
    }

    #[test]
    fn parse_multiple_nested_forward_and_backward() {
        // Forward ref: Order -> Customer, backward ref: Order -> LineItems
        let input = serde_json::json!({
            "type": "Order",
            "$nested": [
                { "via_attribute": "customer_id" },
                { "via_attribute": "line_items_ref" }
            ]
        });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert_eq!(ast.nested.len(), 2);
        assert_eq!(ast.nested[0].via_attribute, "customer_id");
        assert_eq!(ast.nested[1].via_attribute, "line_items_ref");
    }

    // ── Parsing: edge cases ─────────────────────────────────────────

    #[test]
    fn reject_non_object_query() {
        let input = serde_json::json!("not an object");
        assert!(parse_darshan_ql(&input).is_err());
    }

    #[test]
    fn reject_array_query() {
        let input = serde_json::json!([1, 2, 3]);
        assert!(parse_darshan_ql(&input).is_err());
    }

    #[test]
    fn reject_missing_type() {
        let input = serde_json::json!({ "$limit": 10 });
        assert!(parse_darshan_ql(&input).is_err());
    }

    #[test]
    fn reject_null_type() {
        let input = serde_json::json!({ "type": null });
        assert!(parse_darshan_ql(&input).is_err());
    }

    #[test]
    fn reject_numeric_type() {
        let input = serde_json::json!({ "type": 42 });
        assert!(parse_darshan_ql(&input).is_err());
    }

    #[test]
    fn reject_invalid_where_shape() {
        let input = serde_json::json!({
            "type": "T",
            "$where": "not an array"
        });
        assert!(parse_darshan_ql(&input).is_err());
    }

    #[test]
    fn reject_unknown_operator_in_where() {
        let input = serde_json::json!({
            "type": "T",
            "$where": [{ "attribute": "x", "op": "Regex", "value": ".*" }]
        });
        assert!(parse_darshan_ql(&input).is_err());
    }

    #[test]
    fn empty_where_is_ok() {
        let input = serde_json::json!({ "type": "T", "$where": [] });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert!(ast.where_clauses.is_empty());
    }

    #[test]
    fn empty_order_is_ok() {
        let input = serde_json::json!({ "type": "T", "$order": [] });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert!(ast.order.is_empty());
    }

    #[test]
    fn empty_nested_is_ok() {
        let input = serde_json::json!({ "type": "T", "$nested": [] });
        let ast = parse_darshan_ql(&input).expect("should parse");
        assert!(ast.nested.is_empty());
    }

    #[test]
    fn deeply_nested_query() {
        // Build 5 levels of nesting. Sub-queries use serde struct field
        // names (entity_type, nested) because they are deserialized, not
        // parsed via parse_darshan_ql. Only the outermost level uses the
        // DarshanQL JSON keys (type, $nested).
        let mut inner = serde_json::json!({ "entity_type": "Leaf" });
        for depth in (0..5).rev() {
            let nested_arr = serde_json::json!([{
                "via_attribute": format!("ref_{depth}"),
                "sub_query": inner
            }]);
            if depth == 0 {
                // Top level: use DarshanQL syntax
                inner = serde_json::json!({
                    "type": format!("Level{depth}"),
                    "$nested": nested_arr
                });
            } else {
                // Inner levels: use serde struct field names
                inner = serde_json::json!({
                    "entity_type": format!("Level{depth}"),
                    "nested": nested_arr
                });
            }
        }
        let ast = parse_darshan_ql(&inner).expect("should parse deeply nested");
        assert_eq!(ast.entity_type, "Level0");

        // Walk to the leaf
        let mut current = &ast;
        for depth in 0..5 {
            assert_eq!(current.nested.len(), 1, "depth {depth}");
            current = current.nested[0]
                .sub_query
                .as_ref()
                .expect("sub_query at depth");
        }
        assert_eq!(current.entity_type, "Leaf");
    }

    // ── Plan generation ─────────────────────────────────────────────

    #[test]
    fn plan_basic_generates_valid_sql() {
        let ast = bare_ast("User");
        let plan = plan_query(&ast).expect("should plan");
        assert!(plan.sql.contains("triples"));
        assert!(plan.sql.contains(":db/type"));
        assert!(!plan.sql.contains("LIMIT"));
        assert!(!plan.sql.contains("OFFSET"));
    }

    #[test]
    fn plan_with_where_creates_joins() {
        let ast = QueryAST {
            where_clauses: vec![
                WhereClause {
                    attribute: "a".into(),
                    op: WhereOp::Eq,
                    value: serde_json::json!(1),
                },
                WhereClause {
                    attribute: "b".into(),
                    op: WhereOp::Gt,
                    value: serde_json::json!(2),
                },
            ],
            ..bare_ast("T")
        };
        let plan = plan_query(&ast).expect("should plan");
        assert!(plan.sql.contains("tw0"), "should have alias tw0");
        assert!(plan.sql.contains("tw1"), "should have alias tw1");
    }

    #[test]
    fn plan_all_operators_produce_correct_sql_op() {
        let ops = [
            (WhereOp::Eq, "="),
            (WhereOp::Neq, "!="),
            (WhereOp::Gt, ">"),
            (WhereOp::Gte, ">="),
            (WhereOp::Lt, "<"),
            (WhereOp::Lte, "<="),
            (WhereOp::Contains, "@>"),
            (WhereOp::Like, "ILIKE"),
        ];
        for (op, expected_sql) in ops {
            let ast = QueryAST {
                where_clauses: vec![WhereClause {
                    attribute: "x".into(),
                    op,
                    value: serde_json::json!("v"),
                }],
                ..bare_ast("T")
            };
            let plan = plan_query(&ast).unwrap();
            assert!(
                plan.sql.contains(expected_sql),
                "op {op:?} should produce '{expected_sql}' in SQL, got:\n{}",
                plan.sql
            );
        }
    }

    #[test]
    fn plan_limit_offset_parameterised() {
        let ast = QueryAST {
            limit: Some(50),
            offset: Some(10),
            ..bare_ast("T")
        };
        let plan = plan_query(&ast).expect("should plan");
        // Limit and offset should appear as $N placeholders, not literals
        assert!(
            plan.sql.contains("LIMIT $"),
            "LIMIT should be parameterised"
        );
        assert!(
            plan.sql.contains("OFFSET $"),
            "OFFSET should be parameterised"
        );
        // Values should be in the params vec
        assert!(plan.params.iter().any(|p| p == &serde_json::json!(50)));
        assert!(plan.params.iter().any(|p| p == &serde_json::json!(10)));
    }

    #[test]
    fn plan_search_uses_tsvector_tsquery() {
        let ast = QueryAST {
            search: Some("hello world".into()),
            ..bare_ast("T")
        };
        let plan = plan_query(&ast).expect("should plan");

        // SQL should use tsvector/tsquery, not ILIKE.
        assert!(
            plan.sql.contains("to_tsvector"),
            "should use to_tsvector: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("plainto_tsquery"),
            "should use plainto_tsquery: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("@@"),
            "should use @@ match operator: {}",
            plan.sql
        );
        assert!(
            !plan.sql.contains("ILIKE"),
            "should NOT use ILIKE: {}",
            plan.sql
        );

        // The search term should be passed as-is (no LIKE wildcards).
        let search_param = plan
            .params
            .iter()
            .find(|p| p.as_str().is_some_and(|s| s.contains("hello")))
            .expect("search param missing");
        assert_eq!(
            search_param.as_str().unwrap(),
            "hello world",
            "search term should be passed verbatim to plainto_tsquery"
        );
    }

    #[test]
    fn plan_search_passes_special_chars_verbatim() {
        // plainto_tsquery handles sanitization internally, so special
        // characters like % and _ should be passed through unchanged.
        let ast = QueryAST {
            search: Some("%_dangerous\\".into()),
            ..bare_ast("T")
        };
        let plan = plan_query(&ast).expect("should plan");
        let search_param = plan
            .params
            .iter()
            .find(|p| p.as_str().is_some_and(|s| s.contains("dangerous")))
            .expect("search param missing");
        assert_eq!(
            search_param.as_str().unwrap(),
            "%_dangerous\\",
            "special chars passed verbatim to plainto_tsquery"
        );
    }

    #[test]
    fn plan_nested_creates_plans() {
        let ast = QueryAST {
            nested: vec![
                NestedQuery {
                    via_attribute: "org_id".into(),
                    sub_query: None,
                },
                NestedQuery {
                    via_attribute: "team_id".into(),
                    sub_query: None,
                },
            ],
            ..bare_ast("User")
        };
        let plan = plan_query(&ast).expect("should plan");
        assert_eq!(plan.nested_plans.len(), 2);
        assert_eq!(plan.nested_plans[0].via_attribute, "org_id");
        assert_eq!(plan.nested_plans[1].via_attribute, "team_id");
    }

    #[test]
    fn plan_order_by_generates_subqueries() {
        let ast = QueryAST {
            order: vec![OrderClause {
                attribute: "score".into(),
                direction: SortDirection::Desc,
            }],
            ..bare_ast("T")
        };
        let plan = plan_query(&ast).expect("should plan");
        assert!(plan.sql.contains("ORDER BY"));
        assert!(plan.sql.contains("DESC"));
    }

    // ── Plan cache ──────────────────────────────────────────────────

    #[test]
    fn plan_cache_hit() {
        let cache = PlanCache::new(16);
        let ast = bare_ast("User");
        let plan = plan_query(&ast).expect("should plan");
        cache.insert(&ast, plan);
        assert!(cache.get(&ast).is_some());
    }

    #[test]
    fn plan_cache_miss_on_different_entity_type() {
        let cache = PlanCache::new(16);
        let ast1 = bare_ast("User");
        let plan = plan_query(&ast1).expect("should plan");
        cache.insert(&ast1, plan);

        let ast2 = bare_ast("Post");
        assert!(cache.get(&ast2).is_none());
    }

    #[test]
    fn plan_cache_miss_on_different_operator() {
        let cache = PlanCache::new(16);
        let ast1 = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "x".into(),
                op: WhereOp::Eq,
                value: serde_json::json!(1),
            }],
            ..bare_ast("T")
        };
        cache.insert(&ast1, plan_query(&ast1).unwrap());

        let ast2 = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "x".into(),
                op: WhereOp::Gt,
                value: serde_json::json!(1),
            }],
            ..bare_ast("T")
        };
        assert!(cache.get(&ast2).is_none(), "different op should miss");
    }

    #[test]
    fn plan_cache_miss_on_different_attribute() {
        let cache = PlanCache::new(16);
        let ast1 = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "email".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("a"),
            }],
            ..bare_ast("T")
        };
        cache.insert(&ast1, plan_query(&ast1).unwrap());

        let ast2 = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "name".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("a"),
            }],
            ..bare_ast("T")
        };
        assert!(
            cache.get(&ast2).is_none(),
            "different attribute should miss"
        );
    }

    #[test]
    fn shape_key_ignores_values() {
        let ast1 = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "email".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("a@b.com"),
            }],
            ..bare_ast("User")
        };
        let ast2 = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "email".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("other@example.com"),
            }],
            ..bare_ast("User")
        };
        assert_eq!(PlanCache::shape_key(&ast1), PlanCache::shape_key(&ast2));
    }

    #[test]
    fn shape_key_ignores_limit_offset_values() {
        let ast1 = QueryAST {
            limit: Some(10),
            offset: Some(0),
            ..bare_ast("T")
        };
        let ast2 = QueryAST {
            limit: Some(999),
            offset: Some(500),
            ..bare_ast("T")
        };
        assert_eq!(
            PlanCache::shape_key(&ast1),
            PlanCache::shape_key(&ast2),
            "different limit/offset values should share a shape"
        );
    }

    #[test]
    fn shape_key_differs_with_without_limit() {
        let with = QueryAST {
            limit: Some(10),
            ..bare_ast("T")
        };
        let without = bare_ast("T");
        assert_ne!(
            PlanCache::shape_key(&with),
            PlanCache::shape_key(&without),
            "limit presence vs absence should differ"
        );
    }

    #[test]
    fn shape_key_differs_with_without_search() {
        let with = QueryAST {
            search: Some("x".into()),
            ..bare_ast("T")
        };
        let without = bare_ast("T");
        assert_ne!(PlanCache::shape_key(&with), PlanCache::shape_key(&without),);
    }

    #[test]
    fn shape_key_differs_with_without_semantic() {
        let with = QueryAST {
            semantic: Some(SemanticQuery {
                vector: None,
                query: Some("x".into()),
                limit: 10,
            }),
            ..bare_ast("T")
        };
        let without = bare_ast("T");
        assert_ne!(PlanCache::shape_key(&with), PlanCache::shape_key(&without),);
    }

    #[test]
    fn shape_key_differs_with_without_hybrid() {
        let with = QueryAST {
            hybrid: Some(HybridQuery {
                text: "test".into(),
                vector: vec![0.1, 0.2],
                text_weight: 0.3,
                vector_weight: 0.7,
                limit: 10,
            }),
            ..bare_ast("T")
        };
        let without = bare_ast("T");
        assert_ne!(PlanCache::shape_key(&with), PlanCache::shape_key(&without));
    }

    #[test]
    fn plan_cache_lru_eviction() {
        let cache = PlanCache::new(2);
        let ast_a = bare_ast("A");
        let ast_b = bare_ast("B");
        let ast_c = bare_ast("C");

        cache.insert(&ast_a, plan_query(&ast_a).unwrap());
        cache.insert(&ast_b, plan_query(&ast_b).unwrap());

        // Access B so it becomes most-recently-used; A is now LRU.
        assert!(cache.get(&ast_b).is_some());

        // Insert C — should evict A (the least recently used).
        cache.insert(&ast_c, plan_query(&ast_c).unwrap());
        assert!(cache.get(&ast_a).is_none(), "A should have been evicted");
        assert!(cache.get(&ast_b).is_some(), "B was recently accessed");
        assert!(cache.get(&ast_c).is_some(), "C was just inserted");
    }

    #[test]
    fn plan_cache_zero_capacity_uses_default() {
        // Should not panic; falls back to 256.
        let cache = PlanCache::new(0);
        let ast = bare_ast("T");
        cache.insert(&ast, plan_query(&ast).unwrap());
        assert!(cache.get(&ast).is_some());
    }

    // ── Vector helpers ─────────────────────────────────────────────

    #[test]
    fn format_vector_literal_empty() {
        assert_eq!(format_vector_literal(&[]), "[]");
    }

    #[test]
    fn format_vector_literal_single() {
        assert_eq!(format_vector_literal(&[0.5]), "[0.5]");
    }

    #[test]
    fn format_vector_literal_multiple() {
        let result = format_vector_literal(&[0.1, 0.2, 0.3]);
        assert_eq!(result, "[0.1,0.2,0.3]");
    }

    // ── Semantic plan generation ───────────────────────────────────

    #[test]
    fn plan_semantic_with_vector_joins_embeddings() {
        let ast = QueryAST {
            semantic: Some(SemanticQuery {
                vector: Some(vec![0.1, 0.2, 0.3]),
                query: None,
                limit: 5,
            }),
            ..bare_ast("Doc")
        };
        let plan = plan_query(&ast).expect("should plan");
        assert!(
            plan.sql.contains("embeddings"),
            "should join embeddings table: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("<=>"),
            "should use cosine distance operator: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("ORDER BY"),
            "should order by distance: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("LIMIT"),
            "should apply semantic limit: {}",
            plan.sql
        );
    }

    #[test]
    fn plan_semantic_text_only_no_join() {
        // Text-only semantic queries (no vector) should NOT join embeddings
        // because there is no vector to compare against.
        let ast = QueryAST {
            semantic: Some(SemanticQuery {
                vector: None,
                query: Some("cats".into()),
                limit: 10,
            }),
            ..bare_ast("Doc")
        };
        let plan = plan_query(&ast).expect("should plan");
        assert!(
            !plan.sql.contains("embeddings"),
            "text-only semantic should not join embeddings: {}",
            plan.sql
        );
    }

    // ── Hybrid plan generation ─────────────────────────────────────

    #[test]
    fn plan_hybrid_generates_rrf_ctes() {
        let ast = QueryAST {
            hybrid: Some(HybridQuery {
                text: "machine learning".into(),
                vector: vec![0.1, 0.2, 0.3],
                text_weight: 0.3,
                vector_weight: 0.7,
                limit: 10,
            }),
            ..bare_ast("Article")
        };
        let plan = plan_hybrid_query(&ast).expect("should plan hybrid");

        assert!(
            plan.sql.contains("text_ranked"),
            "should have text_ranked CTE: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("vector_ranked"),
            "should have vector_ranked CTE: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("rrf_merged"),
            "should have rrf_merged CTE: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("rrf_score"),
            "should compute RRF score: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("<=>"),
            "should use cosine distance: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("plainto_tsquery"),
            "should use full-text search: {}",
            plan.sql
        );
    }

    #[test]
    fn plan_hybrid_requires_hybrid_clause() {
        let ast = bare_ast("T");
        assert!(
            plan_hybrid_query(&ast).is_err(),
            "should error without $hybrid clause"
        );
    }

    // ── Batch nested resolution ────────────────────────────────────

    #[test]
    fn nested_plan_sql_uses_any_batch() {
        let ast = QueryAST {
            nested: vec![NestedQuery {
                via_attribute: "owner_id".into(),
                sub_query: None,
            }],
            ..bare_ast("Todo")
        };
        let plan = plan_query(&ast).expect("should plan");
        assert_eq!(plan.nested_plans.len(), 1);
        assert!(
            plan.nested_plans[0].sql.contains("ANY($1::uuid[])"),
            "nested SQL should use batched ANY(): {}",
            plan.nested_plans[0].sql
        );
    }

    #[test]
    fn nested_plan_builds_sub_nested() {
        let ast = QueryAST {
            nested: vec![NestedQuery {
                via_attribute: "customer_id".into(),
                sub_query: Some(Box::new(QueryAST {
                    entity_type: "Customer".into(),
                    nested: vec![NestedQuery {
                        via_attribute: "address_id".into(),
                        sub_query: None,
                    }],
                    ..bare_ast("Customer")
                })),
            }],
            ..bare_ast("Order")
        };
        let plan = plan_query(&ast).expect("should plan");
        assert_eq!(plan.nested_plans.len(), 1);
        assert_eq!(plan.nested_plans[0].via_attribute, "customer_id");
        assert_eq!(plan.nested_plans[0].sub_nested.len(), 1);
        assert_eq!(
            plan.nested_plans[0].sub_nested[0].via_attribute,
            "address_id"
        );
    }

    #[test]
    fn nested_plan_respects_max_depth() {
        // Build nesting 5 levels deep — only 3 should be resolved.
        fn make_nested(depth: usize) -> Vec<NestedQuery> {
            if depth == 0 {
                return vec![];
            }
            vec![NestedQuery {
                via_attribute: format!("ref_{depth}"),
                sub_query: Some(Box::new(QueryAST {
                    entity_type: format!("Level{depth}"),
                    nested: make_nested(depth - 1),
                    ..bare_ast(&format!("Level{depth}"))
                })),
            }]
        }

        let ast = QueryAST {
            nested: make_nested(5),
            ..bare_ast("Root")
        };
        let plan = plan_query(&ast).expect("should plan");

        // Walk the nested plans — should stop at depth 3.
        let mut current = &plan.nested_plans;
        let mut depth = 0;
        while !current.is_empty() {
            depth += 1;
            current = &current[0].sub_nested;
        }
        assert_eq!(depth, MAX_NESTING_DEPTH, "should stop at max nesting depth");
    }
}
