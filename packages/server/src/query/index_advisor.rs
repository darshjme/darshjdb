//! Index advisor for DarshJDB.
//!
//! Analyzes query patterns and slow queries from `pg_stat_statements`
//! to suggest missing indexes. Can optionally auto-create indexes for
//! frequently-filtered attributes when enabled via configuration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{debug, info, warn};

use super::{QueryAST, WhereOp};
use crate::query::optimizer::IndexType;

// ── Index Suggestion ───────────────────────────────────────────────

/// A suggested index that would improve query performance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSuggestion {
    /// Target table name.
    pub table: String,
    /// Columns or expressions to index.
    pub columns: Vec<String>,
    /// Recommended index type.
    pub index_type: IndexType,
    /// Why this index is suggested.
    pub reason: String,
    /// Estimated improvement factor (e.g. 2.0 = 2x faster).
    pub estimated_improvement: f64,
}

impl IndexSuggestion {
    /// Generate the DDL statement to create this index.
    pub fn to_ddl(&self) -> String {
        let idx_type_str = match self.index_type {
            IndexType::BTree => "btree",
            IndexType::GIN => "gin",
            IndexType::HNSW => "hnsw",
            IndexType::GiST => "gist",
        };
        let col_str = self.columns.join(", ");
        let idx_name = format!(
            "idx_{}_{}_{}",
            self.table,
            self.columns.first().unwrap_or(&"expr".to_string()).replace(['(', ')', ' ', ',', '\''], "_"),
            idx_type_str
        );
        format!(
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS {idx_name} ON {} USING {idx_type_str} ({col_str})",
            self.table
        )
    }
}

// ── Slow Query Analysis ────────────────────────────────────────────

/// Row from `pg_stat_statements` that we care about.
#[derive(Debug)]
struct SlowQuery {
    query: String,
    calls: i64,
    mean_time_ms: f64,
    total_time_ms: f64,
}

/// Analyze slow queries from `pg_stat_statements` and suggest indexes.
///
/// Requires the `pg_stat_statements` extension to be loaded. If it is
/// not available, returns an empty list with a warning.
pub async fn analyze_slow_queries(pool: &PgPool) -> Vec<IndexSuggestion> {
    // Check if pg_stat_statements is available.
    let ext_check = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements')",
    )
    .fetch_one(pool)
    .await;

    match ext_check {
        Ok(true) => {}
        Ok(false) => {
            warn!("pg_stat_statements extension not installed, skipping slow query analysis");
            return Vec::new();
        }
        Err(e) => {
            warn!(error = %e, "failed to check pg_stat_statements availability");
            return Vec::new();
        }
    }

    // Fetch slow queries touching the triples table.
    let rows = sqlx::query_as::<_, (String, i64, f64, f64)>(
        r#"
        SELECT query, calls, mean_exec_time, total_exec_time
        FROM pg_stat_statements
        WHERE query ILIKE '%triples%'
          AND mean_exec_time > 10.0
        ORDER BY total_exec_time DESC
        LIMIT 50
        "#,
    )
    .fetch_all(pool)
    .await;

    let slow_queries: Vec<SlowQuery> = match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|(query, calls, mean_time_ms, total_time_ms)| SlowQuery {
                query,
                calls,
                mean_time_ms,
                total_time_ms,
            })
            .collect(),
        Err(e) => {
            warn!(error = %e, "failed to query pg_stat_statements");
            return Vec::new();
        }
    };

    let mut suggestions = Vec::new();

    for sq in &slow_queries {
        // Detect sequential scans on attribute value (no index on value column).
        if sq.query.contains("t0.value")
            && !sq.query.contains("@>")
            && sq.mean_time_ms > 50.0
        {
            suggestions.push(IndexSuggestion {
                table: "triples".to_string(),
                columns: vec!["attribute".to_string(), "value".to_string()],
                index_type: IndexType::BTree,
                reason: format!(
                    "slow query ({:.1}ms mean, {} calls): equality filter on value without B-tree index",
                    sq.mean_time_ms, sq.calls
                ),
                estimated_improvement: (sq.mean_time_ms / 5.0).min(100.0),
            });
        }

        // Detect JSONB containment without GIN.
        if sq.query.contains("@>") && sq.mean_time_ms > 20.0 {
            suggestions.push(IndexSuggestion {
                table: "triples".to_string(),
                columns: vec!["value jsonb_path_ops".to_string()],
                index_type: IndexType::GIN,
                reason: format!(
                    "slow JSONB containment ({:.1}ms mean, {} calls): GIN index on value column recommended",
                    sq.mean_time_ms, sq.calls
                ),
                estimated_improvement: (sq.mean_time_ms / 3.0).min(50.0),
            });
        }

        // Detect full-text search without index.
        if sq.query.contains("to_tsvector") && sq.mean_time_ms > 30.0 {
            suggestions.push(IndexSuggestion {
                table: "triples".to_string(),
                columns: vec!["to_tsvector('english', value #>> '{}')".to_string()],
                index_type: IndexType::GIN,
                reason: format!(
                    "slow FTS query ({:.1}ms mean, {} calls): GIN index on tsvector expression recommended",
                    sq.mean_time_ms, sq.calls
                ),
                estimated_improvement: (sq.mean_time_ms / 2.0).min(80.0),
            });
        }
    }

    // Deduplicate by (table, columns, index_type).
    suggestions.dedup_by(|a, b| {
        a.table == b.table && a.columns == b.columns && a.index_type == b.index_type
    });

    debug!(count = suggestions.len(), "index suggestions from slow query analysis");
    suggestions
}

// ── Pattern-Based Suggestions ──────────────────────────────────────

/// Suggest indexes based on query AST patterns (offline analysis).
///
/// Counts which attributes are filtered on and recommends indexes for
/// the most frequently used ones.
pub fn suggest_indexes(query_patterns: &[QueryAST]) -> Vec<IndexSuggestion> {
    if query_patterns.is_empty() {
        return Vec::new();
    }

    // Count filter usage per (attribute, op_class).
    let mut attr_ops: HashMap<(String, OpClass), usize> = HashMap::new();
    let mut search_count = 0usize;
    let mut semantic_count = 0usize;

    for ast in query_patterns {
        for wc in &ast.where_clauses {
            let op_class = classify_op(&wc.op);
            *attr_ops.entry((wc.attribute.clone(), op_class)).or_default() += 1;
        }
        if ast.search.is_some() {
            search_count += 1;
        }
        if ast.semantic.is_some() {
            semantic_count += 1;
        }
    }

    let mut suggestions = Vec::new();
    let threshold = (query_patterns.len() as f64 * 0.05).max(2.0) as usize;

    for ((attr, op_class), count) in &attr_ops {
        if *count < threshold {
            continue;
        }

        let (index_type, reason) = match op_class {
            OpClass::Equality | OpClass::Range => (
                IndexType::BTree,
                format!(
                    "attribute '{}' filtered {} times ({:?}), B-tree recommended",
                    attr, count, op_class
                ),
            ),
            OpClass::Containment => (
                IndexType::GIN,
                format!(
                    "attribute '{}' uses @> containment {} times, GIN recommended",
                    attr, count
                ),
            ),
            OpClass::Pattern => (
                IndexType::BTree,
                format!(
                    "attribute '{}' uses LIKE {} times, B-tree (text_pattern_ops) recommended",
                    attr, count
                ),
            ),
        };

        let improvement = (*count as f64 / query_patterns.len() as f64 * 10.0).min(20.0);

        suggestions.push(IndexSuggestion {
            table: "triples".to_string(),
            columns: vec![format!("attribute, value -- for '{attr}'")],
            index_type,
            reason,
            estimated_improvement: improvement,
        });
    }

    if search_count >= threshold {
        suggestions.push(IndexSuggestion {
            table: "triples".to_string(),
            columns: vec!["to_tsvector('english', value #>> '{}')".to_string()],
            index_type: IndexType::GIN,
            reason: format!("$search used in {search_count} queries, FTS GIN index recommended"),
            estimated_improvement: (search_count as f64 * 2.0).min(50.0),
        });
    }

    if semantic_count >= threshold {
        suggestions.push(IndexSuggestion {
            table: "embeddings".to_string(),
            columns: vec!["embedding vector_cosine_ops".to_string()],
            index_type: IndexType::HNSW,
            reason: format!("$semantic used in {semantic_count} queries, HNSW index recommended"),
            estimated_improvement: (semantic_count as f64 * 3.0).min(50.0),
        });
    }

    suggestions
}

/// Auto-create suggested indexes (guarded by config flag).
pub async fn auto_create_indexes(
    pool: &PgPool,
    suggestions: &[IndexSuggestion],
    enabled: bool,
) -> Vec<String> {
    if !enabled {
        info!(
            count = suggestions.len(),
            "auto-index creation disabled, skipping {} suggestion(s)",
            suggestions.len()
        );
        return Vec::new();
    }

    let mut created = Vec::new();

    for suggestion in suggestions {
        let ddl = suggestion.to_ddl();
        info!(ddl = %ddl, reason = %suggestion.reason, "auto-creating index");

        match sqlx::query(&ddl).execute(pool).await {
            Ok(_) => {
                created.push(ddl);
            }
            Err(e) => {
                warn!(error = %e, ddl = %ddl, "failed to auto-create index");
            }
        }
    }

    info!(count = created.len(), "auto-created indexes");
    created
}

// ── Helpers ────────────────────────────────────────────────────────

/// Classify a `WhereOp` into a broader category for index recommendation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum OpClass {
    Equality,
    Range,
    Containment,
    Pattern,
}

fn classify_op(op: &WhereOp) -> OpClass {
    match op {
        WhereOp::Eq | WhereOp::Neq => OpClass::Equality,
        WhereOp::Gt | WhereOp::Gte | WhereOp::Lt | WhereOp::Lte => OpClass::Range,
        WhereOp::Contains => OpClass::Containment,
        WhereOp::Like => OpClass::Pattern,
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::WhereClause;

    fn make_query(entity_type: &str, filters: Vec<(&str, WhereOp)>) -> QueryAST {
        QueryAST {
            entity_type: entity_type.to_string(),
            where_clauses: filters
                .into_iter()
                .map(|(attr, op)| WhereClause {
                    attribute: attr.to_string(),
                    op,
                    value: serde_json::json!("test"),
                })
                .collect(),
            order: vec![],
            limit: None,
            offset: None,
            search: None,
            semantic: None,
            hybrid: None,
            nested: vec![],
        }
    }

    #[test]
    fn suggest_btree_for_frequent_eq_filter() {
        let queries: Vec<QueryAST> = (0..20)
            .map(|_| make_query("User", vec![("email", WhereOp::Eq)]))
            .collect();

        let suggestions = suggest_indexes(&queries);
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.index_type == IndexType::BTree
            && s.reason.contains("email")));
    }

    #[test]
    fn suggest_gin_for_frequent_containment() {
        let queries: Vec<QueryAST> = (0..20)
            .map(|_| make_query("Post", vec![("tags", WhereOp::Contains)]))
            .collect();

        let suggestions = suggest_indexes(&queries);
        assert!(suggestions.iter().any(|s| s.index_type == IndexType::GIN
            && s.reason.contains("tags")));
    }

    #[test]
    fn suggest_fts_for_frequent_search() {
        let queries: Vec<QueryAST> = (0..20)
            .map(|_| {
                let mut q = make_query("Post", vec![]);
                q.search = Some("keyword".to_string());
                q
            })
            .collect();

        let suggestions = suggest_indexes(&queries);
        assert!(suggestions.iter().any(|s| s.index_type == IndexType::GIN
            && s.reason.contains("$search")));
    }

    #[test]
    fn suggest_hnsw_for_frequent_semantic() {
        let queries: Vec<QueryAST> = (0..20)
            .map(|_| {
                let mut q = make_query("Doc", vec![]);
                q.semantic = Some(crate::query::SemanticQuery {
                    vector: Some(vec![0.1, 0.2]),
                    query: None,
                    limit: 10,
                });
                q
            })
            .collect();

        let suggestions = suggest_indexes(&queries);
        assert!(suggestions.iter().any(|s| s.index_type == IndexType::HNSW
            && s.reason.contains("$semantic")));
    }

    #[test]
    fn no_suggestions_for_infrequent_filters() {
        let queries: Vec<QueryAST> = vec![
            make_query("User", vec![("email", WhereOp::Eq)]),
        ];

        let suggestions = suggest_indexes(&queries);
        // With only 1 query pattern, nothing should exceed the threshold.
        assert!(suggestions.is_empty());
    }

    #[test]
    fn ddl_generation() {
        let suggestion = IndexSuggestion {
            table: "triples".to_string(),
            columns: vec!["value jsonb_path_ops".to_string()],
            index_type: IndexType::GIN,
            reason: "test".to_string(),
            estimated_improvement: 5.0,
        };

        let ddl = suggestion.to_ddl();
        assert!(ddl.contains("CREATE INDEX CONCURRENTLY IF NOT EXISTS"));
        assert!(ddl.contains("USING gin"));
        assert!(ddl.contains("jsonb_path_ops"));
    }

    #[test]
    fn empty_patterns_empty_suggestions() {
        let suggestions = suggest_indexes(&[]);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn multiple_op_classes_produce_separate_suggestions() {
        let mut queries = Vec::new();
        for _ in 0..20 {
            queries.push(make_query("User", vec![("email", WhereOp::Eq)]));
            queries.push(make_query("User", vec![("tags", WhereOp::Contains)]));
        }

        let suggestions = suggest_indexes(&queries);
        let btree_count = suggestions.iter().filter(|s| s.index_type == IndexType::BTree).count();
        let gin_count = suggestions.iter().filter(|s| s.index_type == IndexType::GIN).count();

        assert!(btree_count >= 1, "should suggest B-tree for email");
        assert!(gin_count >= 1, "should suggest GIN for tags");
    }
}
