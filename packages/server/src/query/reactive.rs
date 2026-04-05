//! Reactive dependency tracking for live queries.
//!
//! When a query is registered, its filter predicates are recorded as
//! dependencies on specific (attribute, optional value-constraint) pairs.
//! When new triples arrive, [`DependencyTracker::get_affected_queries`]
//! returns the set of query IDs whose results may have changed, enabling
//! push-based invalidation or incremental re-evaluation.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::query::{QueryAST, WhereClause};

/// Unique identifier for a registered reactive query.
pub type QueryId = u64;

/// A dependency edge: the query depends on changes to this (attribute, value) pair.
///
/// If `value_constraint` is `None`, the query is affected by *any* change
/// to the attribute (e.g. `ORDER BY created_at` or unfiltered scans).
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// The attribute name this dependency watches.
    pub attribute: String,
    /// Optional exact-value constraint. `None` means "any value".
    pub value_constraint: Option<serde_json::Value>,
}

/// Describes a triple mutation for dependency matching.
#[derive(Debug, Clone)]
pub struct TripleChange {
    /// The attribute that was written or retracted.
    pub attribute: String,
    /// The value that was written (or the retracted value).
    pub value: serde_json::Value,
    /// The entity type of the affected entity (if known).
    pub entity_type: Option<String>,
}

/// Thread-safe tracker that maps dependencies to the queries that hold them.
///
/// Designed for concurrent reads (query execution) with infrequent writes
/// (query registration / deregistration).
pub struct DependencyTracker {
    /// Next query id to assign.
    next_id: RwLock<QueryId>,
    /// Map from query id to its set of dependencies.
    queries: RwLock<HashMap<QueryId, QueryEntry>>,
    /// Inverted index: dependency -> set of query ids.
    index: RwLock<HashMap<Dependency, HashSet<QueryId>>>,
}

/// Internal entry for a registered query.
#[derive(Debug, Clone)]
struct QueryEntry {
    /// The entity type the query targets.
    entity_type: String,
    /// All dependencies extracted from the query.
    dependencies: Vec<Dependency>,
}

impl DependencyTracker {
    /// Create a new empty tracker.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            next_id: RwLock::new(1),
            queries: RwLock::new(HashMap::new()),
            index: RwLock::new(HashMap::new()),
        })
    }

    /// Register a query and return its unique id.
    ///
    /// Extracts dependencies from the AST's where-clauses, order-by,
    /// and search terms.
    pub fn register(&self, ast: &QueryAST) -> QueryId {
        let deps = extract_dependencies(ast);
        let id = {
            let mut next = self.next_id.write().expect("next_id lock poisoned");
            let id = *next;
            *next += 1;
            id
        };

        let entry = QueryEntry {
            entity_type: ast.entity_type.clone(),
            dependencies: deps.clone(),
        };

        {
            let mut queries = self.queries.write().expect("queries lock poisoned");
            queries.insert(id, entry);
        }
        {
            let mut index = self.index.write().expect("index lock poisoned");
            for dep in deps {
                index.entry(dep).or_default().insert(id);
            }
        }

        id
    }

    /// Remove a query from tracking.
    pub fn deregister(&self, query_id: QueryId) {
        let entry = {
            let mut queries = self.queries.write().expect("queries lock poisoned");
            queries.remove(&query_id)
        };
        if let Some(entry) = entry {
            let mut index = self.index.write().expect("index lock poisoned");
            for dep in &entry.dependencies {
                if let Some(set) = index.get_mut(dep) {
                    set.remove(&query_id);
                    if set.is_empty() {
                        index.remove(dep);
                    }
                }
            }
        }
    }

    /// Given a batch of triple changes, return all query IDs whose results
    /// may be affected.
    ///
    /// A query is affected if:
    /// 1. The change's attribute matches a dependency attribute, AND
    /// 2. Either the dependency has no value constraint, OR the value matches.
    /// 3. If the change carries an entity_type, it must match the query's type
    ///    (or the query watches all types).
    pub fn get_affected_queries(&self, changes: &[TripleChange]) -> HashSet<QueryId> {
        let mut affected = HashSet::new();

        let index = self.index.read().expect("index lock poisoned");
        let queries = self.queries.read().expect("queries lock poisoned");

        for change in changes {
            // Check exact (attribute, value) match.
            let exact_dep = Dependency {
                attribute: change.attribute.clone(),
                value_constraint: Some(change.value.clone()),
            };
            if let Some(ids) = index.get(&exact_dep) {
                for &id in ids {
                    if matches_entity_type(&queries, id, change) {
                        affected.insert(id);
                    }
                }
            }

            // Check wildcard (attribute, None) match.
            let wild_dep = Dependency {
                attribute: change.attribute.clone(),
                value_constraint: None,
            };
            if let Some(ids) = index.get(&wild_dep) {
                for &id in ids {
                    if matches_entity_type(&queries, id, change) {
                        affected.insert(id);
                    }
                }
            }

            // Wildcard dependencies (from `$search` queries) match any attribute.
            let star_dep = Dependency {
                attribute: "*".to_string(),
                value_constraint: None,
            };
            if let Some(ids) = index.get(&star_dep) {
                for &id in ids {
                    if matches_entity_type(&queries, id, change) {
                        affected.insert(id);
                    }
                }
            }

            // Changes to `:db/type` affect queries watching that entity type.
            if change.attribute == ":db/type" {
                // Any query whose entity_type matches the value
                for (&id, entry) in queries.iter() {
                    if let Some(type_str) = change.value.as_str()
                        && entry.entity_type == type_str
                    {
                        affected.insert(id);
                    }
                }
            }
        }

        affected
    }

    /// Return the number of currently tracked queries.
    pub fn query_count(&self) -> usize {
        self.queries.read().expect("queries lock poisoned").len()
    }

    /// Return the number of distinct dependency edges in the index.
    pub fn dependency_count(&self) -> usize {
        self.index.read().expect("index lock poisoned").len()
    }
}

impl Default for DependencyTracker {
    fn default() -> Self {
        Self {
            next_id: RwLock::new(1),
            queries: RwLock::new(HashMap::new()),
            index: RwLock::new(HashMap::new()),
        }
    }
}

/// Check whether a change's entity type matches the query's target type.
fn matches_entity_type(
    queries: &HashMap<QueryId, QueryEntry>,
    query_id: QueryId,
    change: &TripleChange,
) -> bool {
    match (&change.entity_type, queries.get(&query_id)) {
        (Some(ct), Some(entry)) => entry.entity_type == *ct,
        // If entity type is unknown, conservatively assume it matches.
        (None, _) => true,
        _ => false,
    }
}

/// Extract dependency edges from a query AST.
fn extract_dependencies(ast: &QueryAST) -> Vec<Dependency> {
    let mut deps = Vec::new();

    // The query inherently depends on the `:db/type` attribute.
    deps.push(Dependency {
        attribute: ":db/type".to_string(),
        value_constraint: Some(serde_json::Value::String(ast.entity_type.clone())),
    });

    // Where-clause predicates create precise dependencies.
    for wc in &ast.where_clauses {
        deps.push(dependency_from_where(wc));
    }

    // Order-by attributes: any change could reorder results.
    for oc in &ast.order {
        deps.push(Dependency {
            attribute: oc.attribute.clone(),
            value_constraint: None,
        });
    }

    // Full-text search: any attribute change could match.
    // We use the sentinel attribute `"*"` which is matched specially
    // inside `get_affected_queries` to trigger on every attribute.
    if ast.search.is_some() {
        deps.push(Dependency {
            attribute: "*".to_string(),
            value_constraint: None,
        });
    }

    // Nested references: changes to the referenced entity's attributes
    // affect results. We track the reference attribute itself.
    for n in &ast.nested {
        deps.push(Dependency {
            attribute: n.via_attribute.clone(),
            value_constraint: None,
        });
    }

    deps
}

/// Convert a single where-clause into a dependency edge.
fn dependency_from_where(wc: &WhereClause) -> Dependency {
    use crate::query::WhereOp;

    match wc.op {
        // Exact equality: precise constraint.
        WhereOp::Eq => Dependency {
            attribute: wc.attribute.clone(),
            value_constraint: Some(wc.value.clone()),
        },
        // All other operators: any change to the attribute is relevant.
        _ => Dependency {
            attribute: wc.attribute.clone(),
            value_constraint: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{NestedQuery, OrderClause, QueryAST, SortDirection, WhereClause, WhereOp};

    fn make_tracker() -> Arc<DependencyTracker> {
        DependencyTracker::new()
    }

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

    // ── Registration / deregistration ───────────────────────────────

    #[test]
    fn register_and_deregister() {
        let tracker = make_tracker();
        let ast = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "email".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("a@b.com"),
            }],
            ..bare_ast("User")
        };

        let id = tracker.register(&ast);
        assert_eq!(tracker.query_count(), 1);
        assert!(tracker.dependency_count() > 0);

        tracker.deregister(id);
        assert_eq!(tracker.query_count(), 0);
        // All dependency edges for this query should be cleaned up.
        assert_eq!(tracker.dependency_count(), 0);
    }

    #[test]
    fn register_assigns_unique_ids() {
        let tracker = make_tracker();
        let ast = bare_ast("User");
        let id1 = tracker.register(&ast);
        let id2 = tracker.register(&ast);
        assert_ne!(id1, id2);
        assert_eq!(tracker.query_count(), 2);
    }

    #[test]
    fn deregister_nonexistent_is_noop() {
        let tracker = make_tracker();
        tracker.deregister(9999); // Should not panic
        assert_eq!(tracker.query_count(), 0);
    }

    #[test]
    fn deregister_preserves_other_queries_deps() {
        let tracker = make_tracker();
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
                value: serde_json::json!("b@b.com"),
            }],
            ..bare_ast("User")
        };
        let id1 = tracker.register(&ast1);
        let id2 = tracker.register(&ast2);

        tracker.deregister(id1);
        assert_eq!(tracker.query_count(), 1);

        // id2 should still be matchable
        let changes = vec![TripleChange {
            attribute: "email".into(),
            value: serde_json::json!("b@b.com"),
            entity_type: Some("User".into()),
        }];
        let affected = tracker.get_affected_queries(&changes);
        assert!(affected.contains(&id2));
    }

    // ── Exact match (Eq operator) ───────────────────────────────────

    #[test]
    fn exact_match_triggers_affected() {
        let tracker = make_tracker();
        let ast = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "email".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("a@b.com"),
            }],
            ..bare_ast("User")
        };

        let id = tracker.register(&ast);

        let changes = vec![TripleChange {
            attribute: "email".into(),
            value: serde_json::json!("a@b.com"),
            entity_type: Some("User".into()),
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(affected.contains(&id));
    }

    #[test]
    fn different_value_no_match() {
        let tracker = make_tracker();
        let ast = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "email".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("a@b.com"),
            }],
            ..bare_ast("User")
        };

        let id = tracker.register(&ast);

        let changes = vec![TripleChange {
            attribute: "email".into(),
            value: serde_json::json!("other@b.com"),
            entity_type: Some("User".into()),
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(!affected.contains(&id));
    }

    // ── Wildcard dependencies (non-Eq operators) ────────────────────

    #[test]
    fn order_by_creates_wildcard_dep() {
        let tracker = make_tracker();
        let ast = QueryAST {
            order: vec![OrderClause {
                attribute: "created_at".into(),
                direction: SortDirection::Desc,
            }],
            ..bare_ast("User")
        };

        let id = tracker.register(&ast);

        let changes = vec![TripleChange {
            attribute: "created_at".into(),
            value: serde_json::json!("2026-01-01T00:00:00Z"),
            entity_type: Some("User".into()),
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(affected.contains(&id));
    }

    #[test]
    fn range_operator_creates_wildcard_dep() {
        let tracker = make_tracker();
        // Gt, Gte, Lt, Lte, Neq, Contains, Like all produce wildcard deps
        for op in [
            WhereOp::Gt,
            WhereOp::Gte,
            WhereOp::Lt,
            WhereOp::Lte,
            WhereOp::Neq,
            WhereOp::Contains,
            WhereOp::Like,
        ] {
            let ast = QueryAST {
                where_clauses: vec![WhereClause {
                    attribute: "score".into(),
                    op,
                    value: serde_json::json!(50),
                }],
                ..bare_ast("T")
            };
            let id = tracker.register(&ast);

            // ANY value change to "score" should trigger (wildcard)
            let changes = vec![TripleChange {
                attribute: "score".into(),
                value: serde_json::json!(999),
                entity_type: Some("T".into()),
            }];
            let affected = tracker.get_affected_queries(&changes);
            assert!(
                affected.contains(&id),
                "op {op:?} should create wildcard dep"
            );
        }
    }

    // ── Entity type filtering ───────────────────────────────────────

    #[test]
    fn wrong_entity_type_no_match() {
        let tracker = make_tracker();
        let ast = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "name".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("Alice"),
            }],
            ..bare_ast("User")
        };

        let id = tracker.register(&ast);

        let changes = vec![TripleChange {
            attribute: "name".into(),
            value: serde_json::json!("Alice"),
            entity_type: Some("Post".into()),
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(!affected.contains(&id));
    }

    #[test]
    fn unknown_entity_type_conservatively_matches() {
        let tracker = make_tracker();
        let ast = QueryAST {
            order: vec![OrderClause {
                attribute: "score".into(),
                direction: SortDirection::Asc,
            }],
            ..bare_ast("User")
        };

        let id = tracker.register(&ast);

        // entity_type = None means "unknown" — should conservatively match
        let changes = vec![TripleChange {
            attribute: "score".into(),
            value: serde_json::json!(42),
            entity_type: None,
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(
            affected.contains(&id),
            "unknown entity type should conservatively match"
        );
    }

    // ── :db/type changes ────────────────────────────────────────────

    #[test]
    fn db_type_change_triggers_matching_queries() {
        let tracker = make_tracker();
        let ast = bare_ast("User");
        let id = tracker.register(&ast);

        // A new entity gets :db/type = "User"
        let changes = vec![TripleChange {
            attribute: ":db/type".into(),
            value: serde_json::json!("User"),
            entity_type: None,
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(affected.contains(&id));
    }

    #[test]
    fn db_type_change_different_type_no_match() {
        let tracker = make_tracker();
        let ast = bare_ast("User");
        let id = tracker.register(&ast);

        let changes = vec![TripleChange {
            attribute: ":db/type".into(),
            value: serde_json::json!("Post"),
            entity_type: None,
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(!affected.contains(&id));
    }

    // ── Search (wildcard *) dependency ──────────────────────────────

    #[test]
    fn search_query_affected_by_any_attribute_change() {
        let tracker = make_tracker();
        let ast = QueryAST {
            search: Some("alice".into()),
            ..bare_ast("User")
        };

        let id = tracker.register(&ast);

        // Any attribute change on a User entity should trigger
        let changes = vec![TripleChange {
            attribute: "bio".into(),
            value: serde_json::json!("something about alice"),
            entity_type: Some("User".into()),
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(
            affected.contains(&id),
            "search query should match any attribute change"
        );
    }

    #[test]
    fn search_query_not_affected_by_wrong_entity_type() {
        let tracker = make_tracker();
        let ast = QueryAST {
            search: Some("alice".into()),
            ..bare_ast("User")
        };

        let id = tracker.register(&ast);

        let changes = vec![TripleChange {
            attribute: "title".into(),
            value: serde_json::json!("alice's post"),
            entity_type: Some("Post".into()),
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(
            !affected.contains(&id),
            "wrong entity type should not match"
        );
    }

    // ── Nested reference dependencies ───────────────────────────────

    #[test]
    fn nested_ref_change_triggers_query() {
        let tracker = make_tracker();
        let ast = QueryAST {
            nested: vec![NestedQuery {
                via_attribute: "org_id".into(),
                sub_query: None,
            }],
            ..bare_ast("User")
        };

        let id = tracker.register(&ast);

        let changes = vec![TripleChange {
            attribute: "org_id".into(),
            value: serde_json::json!("some-uuid"),
            entity_type: Some("User".into()),
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(affected.contains(&id));
    }

    // ── Multiple queries, batch changes ─────────────────────────────

    #[test]
    fn batch_changes_match_multiple_queries() {
        let tracker = make_tracker();

        let ast1 = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "email".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("a@b.com"),
            }],
            ..bare_ast("User")
        };
        let ast2 = QueryAST {
            order: vec![OrderClause {
                attribute: "score".into(),
                direction: SortDirection::Desc,
            }],
            ..bare_ast("User")
        };

        let id1 = tracker.register(&ast1);
        let id2 = tracker.register(&ast2);

        let changes = vec![
            TripleChange {
                attribute: "email".into(),
                value: serde_json::json!("a@b.com"),
                entity_type: Some("User".into()),
            },
            TripleChange {
                attribute: "score".into(),
                value: serde_json::json!(100),
                entity_type: Some("User".into()),
            },
        ];

        let affected = tracker.get_affected_queries(&changes);
        assert!(affected.contains(&id1));
        assert!(affected.contains(&id2));
    }

    #[test]
    fn empty_changes_returns_empty() {
        let tracker = make_tracker();
        let ast = bare_ast("User");
        tracker.register(&ast);

        let affected = tracker.get_affected_queries(&[]);
        assert!(affected.is_empty());
    }

    // ── Dependency extraction correctness ───────────────────────────

    #[test]
    fn extract_dependencies_includes_db_type() {
        let ast = bare_ast("User");
        let deps = extract_dependencies(&ast);
        assert!(
            deps.iter().any(|d| d.attribute == ":db/type"
                && d.value_constraint == Some(serde_json::json!("User")))
        );
    }

    #[test]
    fn extract_dependencies_eq_produces_exact_constraint() {
        let ast = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "status".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("active"),
            }],
            ..bare_ast("T")
        };
        let deps = extract_dependencies(&ast);
        assert!(
            deps.iter().any(|d| d.attribute == "status"
                && d.value_constraint == Some(serde_json::json!("active")))
        );
    }

    #[test]
    fn extract_dependencies_gt_produces_wildcard() {
        let ast = QueryAST {
            where_clauses: vec![WhereClause {
                attribute: "age".into(),
                op: WhereOp::Gt,
                value: serde_json::json!(18),
            }],
            ..bare_ast("T")
        };
        let deps = extract_dependencies(&ast);
        assert!(
            deps.iter()
                .any(|d| d.attribute == "age" && d.value_constraint.is_none())
        );
    }

    #[test]
    fn extract_dependencies_order_produces_wildcard() {
        let ast = QueryAST {
            order: vec![OrderClause {
                attribute: "ts".into(),
                direction: SortDirection::Asc,
            }],
            ..bare_ast("T")
        };
        let deps = extract_dependencies(&ast);
        assert!(
            deps.iter()
                .any(|d| d.attribute == "ts" && d.value_constraint.is_none())
        );
    }

    #[test]
    fn extract_dependencies_search_produces_star() {
        let ast = QueryAST {
            search: Some("x".into()),
            ..bare_ast("T")
        };
        let deps = extract_dependencies(&ast);
        assert!(
            deps.iter()
                .any(|d| d.attribute == "*" && d.value_constraint.is_none())
        );
    }

    #[test]
    fn extract_dependencies_nested_produces_wildcard() {
        let ast = QueryAST {
            nested: vec![NestedQuery {
                via_attribute: "ref".into(),
                sub_query: None,
            }],
            ..bare_ast("T")
        };
        let deps = extract_dependencies(&ast);
        assert!(
            deps.iter()
                .any(|d| d.attribute == "ref" && d.value_constraint.is_none())
        );
    }
}
