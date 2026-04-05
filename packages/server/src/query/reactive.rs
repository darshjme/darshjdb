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

            // Changes to `:db/type` affect queries watching that entity type.
            if change.attribute == ":db/type" {
                // Any query whose entity_type matches the value
                for (&id, entry) in queries.iter() {
                    if let Some(type_str) = change.value.as_str() {
                        if entry.entity_type == type_str {
                            affected.insert(id);
                        }
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
    if ast.search.is_some() {
        // Wildcard — but we scope it by registering a broad dependency
        // that will be matched via the `:db/type` check.
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
    use crate::query::{OrderClause, QueryAST, SortDirection, WhereClause, WhereOp};

    fn make_tracker() -> Arc<DependencyTracker> {
        DependencyTracker::new()
    }

    #[test]
    fn register_and_deregister() {
        let tracker = make_tracker();
        let ast = QueryAST {
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

        let id = tracker.register(&ast);
        assert_eq!(tracker.query_count(), 1);

        tracker.deregister(id);
        assert_eq!(tracker.query_count(), 0);
    }

    #[test]
    fn exact_match_triggers_affected() {
        let tracker = make_tracker();
        let ast = QueryAST {
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

        let id = tracker.register(&ast);

        // Change to email = "a@b.com" on a User entity should match.
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

        let id = tracker.register(&ast);

        // Different value should NOT match the exact dependency.
        let changes = vec![TripleChange {
            attribute: "email".into(),
            value: serde_json::json!("other@b.com"),
            entity_type: Some("User".into()),
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(!affected.contains(&id));
    }

    #[test]
    fn order_by_creates_wildcard_dep() {
        let tracker = make_tracker();
        let ast = QueryAST {
            entity_type: "User".into(),
            where_clauses: vec![],
            order: vec![OrderClause {
                attribute: "created_at".into(),
                direction: SortDirection::Desc,
            }],
            limit: None,
            offset: None,
            search: None,
            semantic: None,
            nested: vec![],
        };

        let id = tracker.register(&ast);

        // Any change to created_at should trigger.
        let changes = vec![TripleChange {
            attribute: "created_at".into(),
            value: serde_json::json!("2026-01-01T00:00:00Z"),
            entity_type: Some("User".into()),
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(affected.contains(&id));
    }

    #[test]
    fn wrong_entity_type_no_match() {
        let tracker = make_tracker();
        let ast = QueryAST {
            entity_type: "User".into(),
            where_clauses: vec![WhereClause {
                attribute: "name".into(),
                op: WhereOp::Eq,
                value: serde_json::json!("Alice"),
            }],
            order: vec![],
            limit: None,
            offset: None,
            search: None,
            semantic: None,
            nested: vec![],
        };

        let id = tracker.register(&ast);

        // Change on a "Post" entity should NOT match a "User" query.
        let changes = vec![TripleChange {
            attribute: "name".into(),
            value: serde_json::json!("Alice"),
            entity_type: Some("Post".into()),
        }];

        let affected = tracker.get_affected_queries(&changes);
        assert!(!affected.contains(&id));
    }
}
