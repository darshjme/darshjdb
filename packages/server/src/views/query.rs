//! View-aware query execution.
//!
//! Transforms a user's DarshJQL query by layering on the view's built-in
//! filters, sorts, field ordering, and hidden-field projection. The view
//! acts as a reusable lens: users can add additional filters on top, but
//! the view's base constraints are always enforced.

use crate::query::{OrderClause, QueryAST, QueryResultRow, SortDirection, WhereClause, WhereOp};
use crate::views::{FilterClause, FilterOp, SortDir, ViewConfig};

// ── Filter merging ─────────────────────────────────────────────────

/// Convert a view [`FilterOp`] to a query engine [`WhereOp`].
fn filter_op_to_where_op(op: FilterOp) -> Option<WhereOp> {
    match op {
        FilterOp::Eq => Some(WhereOp::Eq),
        FilterOp::Neq => Some(WhereOp::Neq),
        FilterOp::Gt => Some(WhereOp::Gt),
        FilterOp::Gte => Some(WhereOp::Gte),
        FilterOp::Lt => Some(WhereOp::Lt),
        FilterOp::Lte => Some(WhereOp::Lte),
        FilterOp::Contains => Some(WhereOp::Contains),
        // IsEmpty / IsNotEmpty are handled specially — they become
        // equality checks against null.
        FilterOp::IsEmpty | FilterOp::IsNotEmpty => None,
    }
}

/// Convert a view [`FilterClause`] into one or more query [`WhereClause`]s.
fn filter_to_where(filter: &FilterClause) -> Vec<WhereClause> {
    match filter.op {
        FilterOp::IsEmpty => {
            vec![WhereClause {
                attribute: filter.field.clone(),
                op: WhereOp::Eq,
                value: serde_json::Value::Null,
            }]
        }
        FilterOp::IsNotEmpty => {
            vec![WhereClause {
                attribute: filter.field.clone(),
                op: WhereOp::Neq,
                value: serde_json::Value::Null,
            }]
        }
        _ => {
            if let Some(op) = filter_op_to_where_op(filter.op) {
                vec![WhereClause {
                    attribute: filter.field.clone(),
                    op,
                    value: filter.value.clone(),
                }]
            } else {
                vec![]
            }
        }
    }
}

/// Merge the view's built-in filters into a user query AST.
///
/// View filters are prepended so they act as a mandatory base constraint.
/// User-supplied filters are appended afterwards and further narrow results.
pub fn apply_view_filters(ast: &mut QueryAST, view: &ViewConfig) {
    let mut view_wheres: Vec<WhereClause> = view.filters.iter().flat_map(filter_to_where).collect();

    // Prepend view filters, then user filters follow.
    view_wheres.append(&mut ast.where_clauses);
    ast.where_clauses = view_wheres;
}

// ── Sort merging ───────────────────────────────────────────────────

/// Convert a view [`SortDir`] to a query engine [`SortDirection`].
fn sort_dir_to_direction(dir: SortDir) -> SortDirection {
    match dir {
        SortDir::Asc => SortDirection::Asc,
        SortDir::Desc => SortDirection::Desc,
    }
}

/// Apply the view's default sort order.
///
/// If the user query already specifies its own `$order`, the user's sort
/// takes priority and the view sort is ignored. Otherwise the view's
/// sort order becomes the query's ordering.
pub fn apply_view_sorts(ast: &mut QueryAST, view: &ViewConfig) {
    if !ast.order.is_empty() {
        // User-specified sort takes precedence.
        return;
    }

    ast.order = view
        .sorts
        .iter()
        .map(|s| OrderClause {
            attribute: s.field.clone(),
            direction: sort_dir_to_direction(s.direction),
        })
        .collect();
}

// ── Field projection ───────────────────────────────────────────────

/// Project query results to only include fields visible in the view.
///
/// Respects `hidden_fields` (removed) and `field_order` (reordered).
/// If `field_order` is empty, all non-hidden fields are returned in
/// their natural order.
pub fn project_fields(rows: &mut Vec<QueryResultRow>, view: &ViewConfig) {
    let has_hidden = !view.hidden_fields.is_empty();
    let has_order = !view.field_order.is_empty();

    if !has_hidden && !has_order {
        return;
    }

    for row in rows.iter_mut() {
        // Remove hidden fields.
        if has_hidden {
            for hidden in &view.hidden_fields {
                row.attributes.remove(hidden);
            }
        }

        // Reorder fields according to field_order.
        if has_order {
            let mut ordered = serde_json::Map::new();

            // First add fields in the specified order.
            for field in &view.field_order {
                if let Some(val) = row.attributes.remove(field) {
                    ordered.insert(field.clone(), val);
                }
            }

            // Then append any remaining fields not in the explicit order
            // (but not hidden).
            for (key, val) in row.attributes.iter() {
                if !view.hidden_fields.contains(key) {
                    ordered.insert(key.clone(), val.clone());
                }
            }

            row.attributes = ordered;
        }
    }
}

// ── Convenience: apply all view transforms ─────────────────────────

/// Apply all view transformations (filters + sorts) to a query AST.
///
/// Call this before planning/executing. After execution, call
/// [`project_fields`] on the result rows.
pub fn apply_view_to_query(ast: &mut QueryAST, view: &ViewConfig) {
    apply_view_filters(ast, view);
    apply_view_sorts(ast, view);
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::{FilterOp, SortClause, SortDir, ViewKind};
    use chrono::Utc;
    use uuid::Uuid;

    fn make_view() -> ViewConfig {
        ViewConfig {
            id: Uuid::new_v4(),
            name: "Test".into(),
            kind: ViewKind::Grid,
            table_entity_type: "Task".into(),
            filters: vec![
                FilterClause {
                    field: "status".into(),
                    op: FilterOp::Neq,
                    value: serde_json::json!("archived"),
                },
                FilterClause {
                    field: "assigned".into(),
                    op: FilterOp::IsNotEmpty,
                    value: serde_json::Value::Null,
                },
            ],
            sorts: vec![SortClause {
                field: "priority".into(),
                direction: SortDir::Asc,
            }],
            field_order: vec!["title".into(), "status".into(), "priority".into()],
            hidden_fields: vec!["internal_id".into()],
            group_by: None,
            kanban_field: None,
            calendar_field: None,
            color_field: None,
            row_height: None,
            created_by: Uuid::nil(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn apply_filters_prepends_view_filters() {
        let view = make_view();
        let mut ast = QueryAST {
            entity_type: "Task".into(),
            where_clauses: vec![WhereClause {
                attribute: "title".into(),
                op: WhereOp::Contains,
                value: serde_json::json!("urgent"),
            }],
            order: vec![],
            limit: None,
            offset: None,
            search: None,
            semantic: None,
            hybrid: None,
            nested: vec![],
        };

        apply_view_filters(&mut ast, &view);

        // View has 2 filters + user has 1 => 3 total.
        assert_eq!(ast.where_clauses.len(), 3);
        // First two are from the view.
        assert_eq!(ast.where_clauses[0].attribute, "status");
        assert_eq!(ast.where_clauses[1].attribute, "assigned");
        // Last one is from the user.
        assert_eq!(ast.where_clauses[2].attribute, "title");
    }

    #[test]
    fn apply_sorts_sets_view_sort_when_user_has_none() {
        let view = make_view();
        let mut ast = QueryAST {
            entity_type: "Task".into(),
            where_clauses: vec![],
            order: vec![],
            limit: None,
            offset: None,
            search: None,
            semantic: None,
            hybrid: None,
            nested: vec![],
        };

        apply_view_sorts(&mut ast, &view);
        assert_eq!(ast.order.len(), 1);
        assert_eq!(ast.order[0].attribute, "priority");
        assert_eq!(ast.order[0].direction, SortDirection::Asc);
    }

    #[test]
    fn apply_sorts_preserves_user_sort() {
        let view = make_view();
        let mut ast = QueryAST {
            entity_type: "Task".into(),
            where_clauses: vec![],
            order: vec![OrderClause {
                attribute: "created_at".into(),
                direction: SortDirection::Desc,
            }],
            limit: None,
            offset: None,
            search: None,
            semantic: None,
            hybrid: None,
            nested: vec![],
        };

        apply_view_sorts(&mut ast, &view);
        // User sort takes priority; view sort is NOT applied.
        assert_eq!(ast.order.len(), 1);
        assert_eq!(ast.order[0].attribute, "created_at");
    }

    #[test]
    fn project_fields_hides_and_reorders() {
        let view = make_view();
        let mut rows = vec![QueryResultRow {
            entity_id: Uuid::new_v4(),
            attributes: {
                let mut m = serde_json::Map::new();
                m.insert("title".into(), serde_json::json!("Fix bug"));
                m.insert("status".into(), serde_json::json!("open"));
                m.insert("priority".into(), serde_json::json!(1));
                m.insert("internal_id".into(), serde_json::json!("abc123"));
                m.insert("extra".into(), serde_json::json!("data"));
                m
            },
            nested: serde_json::Map::new(),
        }];

        project_fields(&mut rows, &view);

        let keys: Vec<String> = rows[0].attributes.keys().cloned().collect();
        // internal_id should be removed (hidden).
        assert!(!keys.contains(&"internal_id".to_string()));
        // First three should be in field_order.
        assert_eq!(keys[0], "title");
        assert_eq!(keys[1], "status");
        assert_eq!(keys[2], "priority");
        // Extra field appended after ordered ones.
        assert!(keys.contains(&"extra".to_string()));
    }

    #[test]
    fn project_fields_noop_when_no_config() {
        let mut view = make_view();
        view.field_order.clear();
        view.hidden_fields.clear();

        let mut rows = vec![QueryResultRow {
            entity_id: Uuid::new_v4(),
            attributes: {
                let mut m = serde_json::Map::new();
                m.insert("a".into(), serde_json::json!(1));
                m.insert("b".into(), serde_json::json!(2));
                m
            },
            nested: serde_json::Map::new(),
        }];

        let original_len = rows[0].attributes.len();
        project_fields(&mut rows, &view);
        assert_eq!(rows[0].attributes.len(), original_len);
    }

    #[test]
    fn is_empty_filter_produces_null_eq() {
        let filter = FilterClause {
            field: "notes".into(),
            op: FilterOp::IsEmpty,
            value: serde_json::Value::Null,
        };
        let wheres = filter_to_where(&filter);
        assert_eq!(wheres.len(), 1);
        assert_eq!(wheres[0].op, WhereOp::Eq);
        assert!(wheres[0].value.is_null());
    }

    #[test]
    fn is_not_empty_filter_produces_null_neq() {
        let filter = FilterClause {
            field: "notes".into(),
            op: FilterOp::IsNotEmpty,
            value: serde_json::Value::Null,
        };
        let wheres = filter_to_where(&filter);
        assert_eq!(wheres.len(), 1);
        assert_eq!(wheres[0].op, WhereOp::Neq);
        assert!(wheres[0].value.is_null());
    }
}
