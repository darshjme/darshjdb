//! Query optimizer for DarshJQL.
//!
//! Analyzes a [`QueryAST`] against schema statistics to produce an
//! optimized [`OptimizedPlan`] with cost estimates, index selection,
//! and parallel-safety detection. Replaces naive sequential scanning
//! with intelligent plan selection.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{QueryAST, WhereOp};

// ── Plan Steps ─────────────────────────────────────────────────────

/// A single step in the optimized query plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanStep {
    /// Full sequential scan of the triples table.
    SeqScan,
    /// Use a named index for lookup.
    IndexScan(String),
    /// Post-scan filter applied in Rust.
    FilterStep,
    /// Sort results by given attribute and direction.
    SortStep,
    /// Limit result count.
    LimitStep,
    /// Join for nested entity resolution.
    JoinStep,
    /// Aggregation step (count, sum, etc.).
    AggStep,
}

impl std::fmt::Display for PlanStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SeqScan => write!(f, "SeqScan"),
            Self::IndexScan(idx) => write!(f, "IndexScan({idx})"),
            Self::FilterStep => write!(f, "Filter"),
            Self::SortStep => write!(f, "Sort"),
            Self::LimitStep => write!(f, "Limit"),
            Self::JoinStep => write!(f, "Join"),
            Self::AggStep => write!(f, "Agg"),
        }
    }
}

/// An optimized query execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizedPlan {
    /// Ordered execution steps.
    pub steps: Vec<PlanStep>,
    /// Estimated cost (lower is better, unitless relative measure).
    pub estimated_cost: f64,
    /// Whether any index will be used.
    pub use_index: bool,
    /// Whether this plan is safe to execute in parallel batches.
    pub parallel_safe: bool,
    /// Optimization notes for debugging / EXPLAIN output.
    pub notes: Vec<String>,
}

// ── Schema Statistics ──────────────────────────────────────────────

/// Index type available on a column or expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IndexType {
    BTree,
    GIN,
    HNSW,
    GiST,
}

impl std::fmt::Display for IndexType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Metadata about a known index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexInfo {
    /// Name of the index.
    pub name: String,
    /// What type of index.
    pub index_type: IndexType,
    /// Which table/column(s) it covers.
    pub columns: Vec<String>,
    /// Which attribute(s) it is relevant for (for triple-store attribute indexes).
    pub attributes: Vec<String>,
}

/// Statistics about the schema used for cost estimation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaStats {
    /// Approximate entity count per entity type.
    pub entity_count_by_type: HashMap<String, u64>,
    /// Approximate distinct value count per attribute (cardinality).
    pub attribute_cardinality: HashMap<String, u64>,
    /// Sample value distribution per attribute (for histogram estimation).
    pub value_distribution: HashMap<String, ValueDistribution>,
    /// Known indexes on the database.
    pub index_info: Vec<IndexInfo>,
}

/// Value distribution statistics for an attribute.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValueDistribution {
    /// Number of distinct values.
    pub n_distinct: u64,
    /// Number of null values.
    pub n_null: u64,
    /// Total row count for this attribute.
    pub total: u64,
    /// Most common values (top-N).
    pub most_common: Vec<(String, u64)>,
}

// ── Cost Constants ─────────────────────────────────────────────────

const SEQ_SCAN_COST_PER_ROW: f64 = 1.0;
const INDEX_SCAN_COST_PER_ROW: f64 = 0.1;
const FILTER_COST: f64 = 0.5;
const SORT_COST_FACTOR: f64 = 2.0; // n * log(n) approximation multiplier
const JOIN_COST_FACTOR: f64 = 5.0;

// ── Optimizer ──────────────────────────────────────────────────────

/// Produce an optimized plan for a DarshJQL query.
///
/// # Optimization Rules
///
/// 1. Push filters to SQL WHERE (avoid post-filtering in Rust).
/// 2. Use GIN index for JSONB containment (`@>`) queries.
/// 3. Use B-tree index for range queries on known attributes.
/// 4. Use FTS index for `$search` queries.
/// 5. Batch nested queries with IN clause (avoid N+1).
/// 6. Detect parallel-safe plans for batch operations.
pub fn optimize(ast: &QueryAST, stats: &SchemaStats) -> OptimizedPlan {
    let mut steps = Vec::new();
    let mut notes = Vec::new();
    let mut use_index = false;
    let mut estimated_cost = 0.0;
    let mut parallel_safe = true;

    let entity_count = stats
        .entity_count_by_type
        .get(&ast.entity_type)
        .copied()
        .unwrap_or(1000) as f64;

    // Step 1: Determine scan strategy based on where clauses.
    if ast.where_clauses.is_empty() && ast.search.is_none() && ast.semantic.is_none() {
        // No filters — sequential scan.
        steps.push(PlanStep::SeqScan);
        estimated_cost += entity_count * SEQ_SCAN_COST_PER_ROW;
        notes.push("no filters, using sequential scan".to_string());
    } else {
        // Check each where clause for index applicability.
        let mut best_index: Option<(&IndexInfo, f64)> = None;

        for wc in &ast.where_clauses {
            match wc.op {
                // Rule 2: GIN for containment.
                WhereOp::Contains => {
                    if let Some(idx) = find_index_for_attr(stats, &wc.attribute, &IndexType::GIN) {
                        let selectivity = estimate_selectivity(stats, &wc.attribute, &wc.op);
                        let cost = entity_count * selectivity * INDEX_SCAN_COST_PER_ROW;
                        if best_index.as_ref().is_none_or(|(_, c)| cost < *c) {
                            best_index = Some((idx, cost));
                        }
                        notes.push(format!(
                            "GIN index '{}' available for @> on '{}'",
                            idx.name, wc.attribute
                        ));
                    }
                }
                // Rule 3: B-tree for range and equality.
                WhereOp::Eq | WhereOp::Gt | WhereOp::Gte | WhereOp::Lt | WhereOp::Lte | WhereOp::Neq => {
                    if let Some(idx) = find_index_for_attr(stats, &wc.attribute, &IndexType::BTree) {
                        let selectivity = estimate_selectivity(stats, &wc.attribute, &wc.op);
                        let cost = entity_count * selectivity * INDEX_SCAN_COST_PER_ROW;
                        if best_index.as_ref().is_none_or(|(_, c)| cost < *c) {
                            best_index = Some((idx, cost));
                        }
                        notes.push(format!(
                            "B-tree index '{}' available for {:?} on '{}'",
                            idx.name, wc.op, wc.attribute
                        ));
                    }
                }
                WhereOp::Like => {
                    // LIKE can use B-tree for prefix patterns only.
                    if let Some(idx) = find_index_for_attr(stats, &wc.attribute, &IndexType::BTree) {
                        let cost = entity_count * 0.3 * INDEX_SCAN_COST_PER_ROW;
                        if best_index.as_ref().is_none_or(|(_, c)| cost < *c) {
                            best_index = Some((idx, cost));
                        }
                        notes.push(format!(
                            "B-tree index '{}' may help LIKE prefix on '{}'",
                            idx.name, wc.attribute
                        ));
                    }
                }
            }
        }

        // Rule 4: FTS index for $search.
        if ast.search.is_some() {
            if let Some(idx) = stats
                .index_info
                .iter()
                .find(|i| i.index_type == IndexType::GIN && i.columns.iter().any(|c| c.contains("tsvector")))
            {
                let cost = entity_count * 0.05 * INDEX_SCAN_COST_PER_ROW;
                if best_index.as_ref().is_none_or(|(_, c)| cost < *c) {
                    best_index = Some((idx, cost));
                }
                notes.push(format!("FTS GIN index '{}' used for $search", idx.name));
            }
        }

        // HNSW for semantic search.
        if ast.semantic.as_ref().is_some_and(|s| s.vector.is_some()) {
            if let Some(idx) = stats
                .index_info
                .iter()
                .find(|i| i.index_type == IndexType::HNSW)
            {
                let cost = (ast.semantic.as_ref().map(|s| s.limit).unwrap_or(10) as f64) * INDEX_SCAN_COST_PER_ROW;
                if best_index.as_ref().is_none_or(|(_, c)| cost < *c) {
                    best_index = Some((idx, cost));
                }
                notes.push(format!("HNSW index '{}' used for $semantic", idx.name));
            }
        }

        if let Some((idx, cost)) = best_index {
            steps.push(PlanStep::IndexScan(idx.name.clone()));
            estimated_cost += cost;
            use_index = true;
        } else {
            // No applicable index — fall back to seq scan with filters pushed to SQL.
            steps.push(PlanStep::SeqScan);
            estimated_cost += entity_count * SEQ_SCAN_COST_PER_ROW;
            notes.push("no matching index, falling back to sequential scan".to_string());
        }

        // Rule 1: Filters are always pushed to SQL WHERE (this is just annotation).
        if !ast.where_clauses.is_empty() {
            steps.push(PlanStep::FilterStep);
            estimated_cost += ast.where_clauses.len() as f64 * FILTER_COST;
            notes.push(format!(
                "{} filter(s) pushed to SQL WHERE",
                ast.where_clauses.len()
            ));
        }
    }

    // Sort step.
    if !ast.order.is_empty() || ast.semantic.as_ref().is_some_and(|s| s.vector.is_some()) {
        steps.push(PlanStep::SortStep);
        let sort_rows = if use_index {
            (entity_count * 0.1).max(10.0)
        } else {
            entity_count
        };
        estimated_cost += sort_rows * SORT_COST_FACTOR * (sort_rows.max(1.0).ln());
        notes.push("sort step added".to_string());
    }

    // Limit step.
    if ast.limit.is_some() || ast.offset.is_some() {
        steps.push(PlanStep::LimitStep);
        notes.push("limit/offset applied".to_string());
    }

    // Rule 5: Nested queries use JOIN (batch resolution).
    if !ast.nested.is_empty() {
        steps.push(PlanStep::JoinStep);
        estimated_cost += ast.nested.len() as f64 * JOIN_COST_FACTOR;
        notes.push(format!(
            "{} nested resolution(s) via batched IN clause (avoids N+1)",
            ast.nested.len()
        ));
        // Nested queries with sub-queries are not parallel-safe.
        if ast.nested.iter().any(|n| n.sub_query.is_some()) {
            parallel_safe = false;
            notes.push("nested sub-queries prevent parallel execution".to_string());
        }
    }

    // Semantic and hybrid searches are not parallel-safe (they use global state).
    if ast.semantic.is_some() || ast.hybrid.is_some() {
        parallel_safe = false;
        notes.push("vector/hybrid search prevents parallel execution".to_string());
    }

    OptimizedPlan {
        steps,
        estimated_cost,
        use_index,
        parallel_safe,
        notes,
    }
}

/// Find an index applicable to a given attribute and type.
fn find_index_for_attr<'a>(
    stats: &'a SchemaStats,
    attribute: &str,
    index_type: &IndexType,
) -> Option<&'a IndexInfo> {
    stats.index_info.iter().find(|idx| {
        idx.index_type == *index_type
            && (idx.attributes.iter().any(|a| a == attribute)
                || idx.columns.iter().any(|c| c.contains("value")))
    })
}

/// Estimate selectivity for a filter operation (fraction of rows that pass).
fn estimate_selectivity(stats: &SchemaStats, attribute: &str, op: &WhereOp) -> f64 {
    let cardinality = stats
        .attribute_cardinality
        .get(attribute)
        .copied()
        .unwrap_or(100);

    if cardinality == 0 {
        return 1.0;
    }

    match op {
        WhereOp::Eq => 1.0 / cardinality as f64,
        WhereOp::Neq => 1.0 - (1.0 / cardinality as f64),
        WhereOp::Gt | WhereOp::Gte | WhereOp::Lt | WhereOp::Lte => 0.3,
        WhereOp::Contains => 0.1,
        WhereOp::Like => 0.2,
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{NestedQuery, OrderClause, SemanticQuery, SortDirection, WhereClause};

    fn empty_stats() -> SchemaStats {
        SchemaStats::default()
    }

    fn stats_with_indexes() -> SchemaStats {
        SchemaStats {
            entity_count_by_type: [("User".to_string(), 10_000)].into_iter().collect(),
            attribute_cardinality: [("email".to_string(), 10_000), ("status".to_string(), 5)]
                .into_iter()
                .collect(),
            index_info: vec![
                IndexInfo {
                    name: "idx_triples_value_btree".to_string(),
                    index_type: IndexType::BTree,
                    columns: vec!["value".to_string()],
                    attributes: vec!["email".to_string(), "status".to_string()],
                },
                IndexInfo {
                    name: "idx_triples_value_gin".to_string(),
                    index_type: IndexType::GIN,
                    columns: vec!["value".to_string()],
                    attributes: vec!["tags".to_string()],
                },
                IndexInfo {
                    name: "idx_fts_gin".to_string(),
                    index_type: IndexType::GIN,
                    columns: vec!["tsvector_value".to_string()],
                    attributes: vec![],
                },
                IndexInfo {
                    name: "idx_embeddings_hnsw".to_string(),
                    index_type: IndexType::HNSW,
                    columns: vec!["embedding".to_string()],
                    attributes: vec![],
                },
            ],
            ..Default::default()
        }
    }

    fn simple_query(entity_type: &str) -> QueryAST {
        QueryAST {
            entity_type: entity_type.to_string(),
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

    #[test]
    fn no_filters_uses_seq_scan() {
        let ast = simple_query("User");
        let plan = optimize(&ast, &empty_stats());

        assert!(plan.steps.contains(&PlanStep::SeqScan));
        assert!(!plan.use_index);
        assert!(plan.parallel_safe);
    }

    #[test]
    fn eq_filter_with_btree_uses_index_scan() {
        let mut ast = simple_query("User");
        ast.where_clauses.push(WhereClause {
            attribute: "email".to_string(),
            op: WhereOp::Eq,
            value: serde_json::json!("test@example.com"),
        });

        let plan = optimize(&ast, &stats_with_indexes());

        assert!(plan.use_index);
        assert!(
            plan.steps
                .iter()
                .any(|s| matches!(s, PlanStep::IndexScan(_))),
            "should use index scan"
        );
        assert!(plan.steps.contains(&PlanStep::FilterStep));
    }

    #[test]
    fn contains_filter_uses_gin_index() {
        let mut ast = simple_query("User");
        ast.where_clauses.push(WhereClause {
            attribute: "tags".to_string(),
            op: WhereOp::Contains,
            value: serde_json::json!(["rust"]),
        });

        let plan = optimize(&ast, &stats_with_indexes());
        assert!(plan.use_index);

        let idx_name = plan.steps.iter().find_map(|s| match s {
            PlanStep::IndexScan(name) => Some(name.as_str()),
            _ => None,
        });
        assert_eq!(idx_name, Some("idx_triples_value_gin"));
    }

    #[test]
    fn search_uses_fts_index() {
        let mut ast = simple_query("User");
        ast.search = Some("alice".to_string());

        let plan = optimize(&ast, &stats_with_indexes());
        assert!(plan.use_index);
        assert!(plan.notes.iter().any(|n| n.contains("FTS")));
    }

    #[test]
    fn semantic_uses_hnsw_index() {
        let mut ast = simple_query("User");
        ast.semantic = Some(SemanticQuery {
            vector: Some(vec![0.1, 0.2, 0.3]),
            query: None,
            limit: 10,
        });

        let plan = optimize(&ast, &stats_with_indexes());
        assert!(plan.use_index);
        assert!(!plan.parallel_safe, "semantic search is not parallel-safe");
        assert!(plan.notes.iter().any(|n| n.contains("HNSW")));
    }

    #[test]
    fn nested_queries_add_join_step() {
        let mut ast = simple_query("User");
        ast.nested.push(NestedQuery {
            via_attribute: "org_id".to_string(),
            sub_query: None,
        });

        let plan = optimize(&ast, &empty_stats());
        assert!(plan.steps.contains(&PlanStep::JoinStep));
        assert!(plan.notes.iter().any(|n| n.contains("N+1")));
    }

    #[test]
    fn nested_with_subquery_not_parallel_safe() {
        let mut ast = simple_query("User");
        ast.nested.push(NestedQuery {
            via_attribute: "org_id".to_string(),
            sub_query: Some(Box::new(simple_query("Org"))),
        });

        let plan = optimize(&ast, &empty_stats());
        assert!(!plan.parallel_safe);
    }

    #[test]
    fn order_and_limit_add_steps() {
        let mut ast = simple_query("User");
        ast.order.push(OrderClause {
            attribute: "created_at".to_string(),
            direction: SortDirection::Desc,
        });
        ast.limit = Some(20);

        let plan = optimize(&ast, &empty_stats());
        assert!(plan.steps.contains(&PlanStep::SortStep));
        assert!(plan.steps.contains(&PlanStep::LimitStep));
    }

    #[test]
    fn index_scan_cheaper_than_seq_scan() {
        let mut ast = simple_query("User");
        ast.where_clauses.push(WhereClause {
            attribute: "email".to_string(),
            op: WhereOp::Eq,
            value: serde_json::json!("test@example.com"),
        });

        let plan_with_index = optimize(&ast, &stats_with_indexes());
        let plan_without = optimize(&ast, &empty_stats());

        assert!(
            plan_with_index.estimated_cost < plan_without.estimated_cost,
            "index plan ({:.1}) should be cheaper than seq scan ({:.1})",
            plan_with_index.estimated_cost,
            plan_without.estimated_cost
        );
    }

    #[test]
    fn selectivity_estimation() {
        let stats = stats_with_indexes();

        // High-cardinality attribute (email) should have very low selectivity for Eq.
        let sel_email = estimate_selectivity(&stats, "email", &WhereOp::Eq);
        assert!(sel_email < 0.01);

        // Low-cardinality attribute (status) should have higher selectivity.
        let sel_status = estimate_selectivity(&stats, "status", &WhereOp::Eq);
        assert!(sel_status > sel_email);
        assert!((sel_status - 0.2).abs() < 0.01);
    }

    #[test]
    fn plan_display_formatting() {
        let plan = OptimizedPlan {
            steps: vec![
                PlanStep::IndexScan("idx_email".to_string()),
                PlanStep::FilterStep,
                PlanStep::SortStep,
                PlanStep::LimitStep,
            ],
            estimated_cost: 42.5,
            use_index: true,
            parallel_safe: true,
            notes: vec!["test".to_string()],
        };

        assert_eq!(format!("{}", plan.steps[0]), "IndexScan(idx_email)");
        assert_eq!(format!("{}", plan.steps[1]), "Filter");
    }
}
