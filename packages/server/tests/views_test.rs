//! Integration tests for the DarshJDB view system.
//!
//! Tests create, query-through, update, and delete operations on views
//! backed by a real Postgres triple store. Each test uses isolated entity
//! namespaces and cleans up after itself.
//!
//! ```sh
//! DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshjdb_test \
//!     cargo test --test views_test
//! ```

use ddb_server::triple_store::{PgTripleStore, TripleInput, TripleStore};
use ddb_server::views::{
    CreateViewRequest, FilterClause, FilterOp, PgViewStore, SortClause, SortDir, ViewKind,
    ViewStore, ViewUpdate,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn setup() -> Option<(PgPool, PgTripleStore, PgViewStore)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.ok()?;
    let store = PgTripleStore::new(pool.clone()).await.ok()?;
    let view_store = PgViewStore::new(store.clone());
    Some((pool, store, view_store))
}

async fn cleanup_entities(pool: &PgPool, ids: &[Uuid]) {
    if ids.is_empty() {
        return;
    }
    sqlx::query("DELETE FROM triples WHERE entity_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await
        .ok();
}

/// Clean up all triples belonging to a view by finding its entity via the
/// stored `view/id` attribute, then deleting all triples for that entity.
async fn cleanup_view_triples(pool: &PgPool, view_id: Uuid) {
    let view_id_str = view_id.to_string();
    sqlx::query(
        "DELETE FROM triples WHERE entity_id IN (
            SELECT entity_id FROM triples
            WHERE attribute = 'view/id' AND value = $1::jsonb
        )",
    )
    .bind(serde_json::Value::String(view_id_str))
    .execute(pool)
    .await
    .ok();
}

/// Seed some Task entities so views have data to query.
async fn seed_tasks(store: &PgTripleStore) -> (Vec<Uuid>, Vec<i64>) {
    let mut eids = Vec::new();
    let mut txs = Vec::new();

    let statuses = ["active", "active", "archived", "active", "done"];
    let priorities = [1, 3, 2, 5, 4];
    let titles = [
        "Fix login",
        "Add search",
        "Old feature",
        "Redesign",
        "Write docs",
    ];

    for i in 0..5 {
        let eid = Uuid::new_v4();
        let tx = store
            .set_triples(&[
                TripleInput {
                    entity_id: eid,
                    attribute: ":db/type".into(),
                    value: json!("Task"),
                    value_type: 0,
                    ttl_seconds: None,
                },
                TripleInput {
                    entity_id: eid,
                    attribute: "task/title".into(),
                    value: json!(titles[i]),
                    value_type: 0,
                    ttl_seconds: None,
                },
                TripleInput {
                    entity_id: eid,
                    attribute: "task/status".into(),
                    value: json!(statuses[i]),
                    value_type: 0,
                    ttl_seconds: None,
                },
                TripleInput {
                    entity_id: eid,
                    attribute: "task/priority".into(),
                    value: json!(priorities[i]),
                    value_type: 1,
                    ttl_seconds: None,
                },
            ])
            .await
            .expect("seed task");
        eids.push(eid);
        txs.push(tx);
    }

    (eids, txs)
}

// ===========================================================================
// 1. CREATE A VIEW
// ===========================================================================

#[tokio::test]
async fn test_view_create_roundtrip() {
    let Some((pool, _store, view_store)) = setup().await else {
        return;
    };

    let req = CreateViewRequest {
        name: "Active Tasks".into(),
        kind: ViewKind::Grid,
        table_entity_type: "Task".into(),
        filters: vec![FilterClause {
            field: "task/status".into(),
            op: FilterOp::Eq,
            value: json!("active"),
        }],
        sorts: vec![SortClause {
            field: "task/priority".into(),
            direction: SortDir::Asc,
        }],
        field_order: vec!["task/title".into(), "task/status".into()],
        hidden_fields: vec![],
        group_by: None,
        kanban_field: None,
        calendar_field: None,
        color_field: None,
        row_height: None,
    };

    let user_id = Uuid::new_v4();
    let view = view_store
        .create_view(req, user_id)
        .await
        .expect("create view");

    assert_eq!(view.name, "Active Tasks");
    assert_eq!(view.kind, ViewKind::Grid);
    assert_eq!(view.table_entity_type, "Task");
    assert_eq!(view.filters.len(), 1);
    assert_eq!(view.sorts.len(), 1);
    assert_eq!(view.created_by, user_id);

    // Verify we can get it back.
    let fetched = view_store.get_view(view.id).await.expect("get view");
    assert_eq!(fetched.name, "Active Tasks");
    assert_eq!(fetched.filters.len(), 1);
    assert_eq!(fetched.filters[0].field, "task/status");
    assert_eq!(fetched.sorts[0].field, "task/priority");

    // Cleanup: delete the view then physically remove triples.
    let _ = view_store.delete_view(view.id).await;
    cleanup_view_triples(&pool, view.id).await;
}

#[tokio::test]
async fn test_view_create_empty_name_rejected() {
    let Some((_pool, _store, view_store)) = setup().await else {
        return;
    };

    let req = CreateViewRequest {
        name: "  ".into(),
        kind: ViewKind::Grid,
        table_entity_type: "Task".into(),
        filters: vec![],
        sorts: vec![],
        field_order: vec![],
        hidden_fields: vec![],
        group_by: None,
        kanban_field: None,
        calendar_field: None,
        color_field: None,
        row_height: None,
    };

    let result = view_store.create_view(req, Uuid::new_v4()).await;
    assert!(result.is_err(), "empty name should be rejected");
}

// ===========================================================================
// 2. LIST VIEWS FOR A TABLE
// ===========================================================================

#[tokio::test]
async fn test_view_list_by_table() {
    let Some((pool, _store, view_store)) = setup().await else {
        return;
    };

    let user_id = Uuid::new_v4();
    let mut view_ids = Vec::new();

    for name in ["Grid View", "Kanban View"] {
        let req = CreateViewRequest {
            name: name.into(),
            kind: ViewKind::Grid,
            table_entity_type: "Contact".into(),
            filters: vec![],
            sorts: vec![],
            field_order: vec![],
            hidden_fields: vec![],
            group_by: None,
            kanban_field: None,
            calendar_field: None,
            color_field: None,
            row_height: None,
        };
        let v = view_store
            .create_view(req, user_id)
            .await
            .expect("create view");
        view_ids.push(v.id);
    }

    let views = view_store.list_views("Contact").await.expect("list views");
    assert!(views.len() >= 2);

    let our_names: Vec<&str> = views
        .iter()
        .filter(|v| view_ids.contains(&v.id))
        .map(|v| v.name.as_str())
        .collect();
    assert!(our_names.contains(&"Grid View"));
    assert!(our_names.contains(&"Kanban View"));

    // Cleanup.
    for id in &view_ids {
        let _ = view_store.delete_view(*id).await;
        cleanup_view_triples(&pool, *id).await;
    }
}

// ===========================================================================
// 3. QUERY THROUGH A VIEW (filter application)
// ===========================================================================

#[tokio::test]
async fn test_view_query_applies_filters() {
    let Some((pool, store, _view_store)) = setup().await else {
        return;
    };

    use ddb_server::views::ViewConfig;
    use ddb_server::views::query::{apply_view_filters, apply_view_sorts, project_fields};

    let (task_eids, _task_txs) = seed_tasks(&store).await;

    // Build a ViewConfig that filters to status == "active".
    let view = ViewConfig {
        id: Uuid::new_v4(),
        name: "Active Only".into(),
        kind: ViewKind::Grid,
        table_entity_type: "Task".into(),
        filters: vec![FilterClause {
            field: "task/status".into(),
            op: FilterOp::Eq,
            value: json!("active"),
        }],
        sorts: vec![SortClause {
            field: "task/priority".into(),
            direction: SortDir::Asc,
        }],
        field_order: vec!["task/title".into(), "task/status".into()],
        hidden_fields: vec!["task/priority".into()],
        group_by: None,
        kanban_field: None,
        calendar_field: None,
        color_field: None,
        row_height: None,
        created_by: Uuid::nil(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    // Build a minimal query AST for Task entity type.
    let mut ast = ddb_server::query::QueryAST {
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

    // Apply the view's filters and sorts to the AST.
    apply_view_filters(&mut ast, &view);
    apply_view_sorts(&mut ast, &view);

    // Verify the AST now has the view's filter.
    assert_eq!(ast.where_clauses.len(), 1);
    assert_eq!(ast.where_clauses[0].attribute, "task/status");

    // Verify sort was applied (no user sort, so view sort should win).
    assert_eq!(ast.order.len(), 1);
    assert_eq!(ast.order[0].attribute, "task/priority");

    // Execute the query and project fields.
    let plan = ddb_server::query::plan_query(&ast).expect("plan");
    let mut rows = ddb_server::query::execute_query(&pool, &plan)
        .await
        .expect("exec");

    // Project removes hidden fields and reorders.
    project_fields(&mut rows, &view);

    // All returned rows should have status == "active".
    for row in &rows {
        if let Some(status) = row.attributes.get("task/status") {
            assert_eq!(status, "active", "view filter should exclude non-active");
        }
        // task/priority should be hidden.
        assert!(
            !row.attributes.contains_key("task/priority"),
            "priority should be hidden"
        );
    }

    cleanup_entities(&pool, &task_eids).await;
}

// ===========================================================================
// 4. UPDATE VIEW CONFIG
// ===========================================================================

#[tokio::test]
async fn test_view_update_config() {
    let Some((pool, _store, view_store)) = setup().await else {
        return;
    };

    let user_id = Uuid::new_v4();
    let req = CreateViewRequest {
        name: "Original".into(),
        kind: ViewKind::Grid,
        table_entity_type: "Task".into(),
        filters: vec![],
        sorts: vec![],
        field_order: vec![],
        hidden_fields: vec![],
        group_by: None,
        kanban_field: None,
        calendar_field: None,
        color_field: None,
        row_height: Some(32),
    };
    let view = view_store.create_view(req, user_id).await.expect("create");

    // Update name and add a filter.
    let update = ViewUpdate {
        name: Some("Renamed View".into()),
        kind: Some(ViewKind::Kanban),
        filters: Some(vec![FilterClause {
            field: "task/status".into(),
            op: FilterOp::Neq,
            value: json!("archived"),
        }]),
        kanban_field: Some(Some("task/status".into())),
        row_height: Some(Some(48)),
        ..Default::default()
    };

    let updated = view_store
        .update_view(view.id, update)
        .await
        .expect("update");

    assert_eq!(updated.name, "Renamed View");
    assert_eq!(updated.kind, ViewKind::Kanban);
    assert_eq!(updated.filters.len(), 1);
    assert_eq!(updated.kanban_field, Some("task/status".into()));
    assert_eq!(updated.row_height, Some(48));
    assert!(updated.updated_at > view.updated_at);

    // Verify persisted correctly.
    let fetched = view_store.get_view(view.id).await.expect("get");
    assert_eq!(fetched.name, "Renamed View");
    assert_eq!(fetched.kind, ViewKind::Kanban);

    let _ = view_store.delete_view(view.id).await;
    cleanup_view_triples(&pool, view.id).await;
}

#[tokio::test]
async fn test_view_update_empty_name_rejected() {
    let Some((pool, _store, view_store)) = setup().await else {
        return;
    };

    let view = view_store
        .create_view(
            CreateViewRequest {
                name: "Valid".into(),
                kind: ViewKind::Grid,
                table_entity_type: "Task".into(),
                filters: vec![],
                sorts: vec![],
                field_order: vec![],
                hidden_fields: vec![],
                group_by: None,
                kanban_field: None,
                calendar_field: None,
                color_field: None,
                row_height: None,
            },
            Uuid::new_v4(),
        )
        .await
        .expect("create");

    let result = view_store
        .update_view(
            view.id,
            ViewUpdate {
                name: Some("".into()),
                ..Default::default()
            },
        )
        .await;

    assert!(result.is_err(), "empty name update should be rejected");

    let _ = view_store.delete_view(view.id).await;
    cleanup_view_triples(&pool, view.id).await;
}

// ===========================================================================
// 5. DELETE VIEW
// ===========================================================================

#[tokio::test]
async fn test_view_delete() {
    let Some((pool, _store, view_store)) = setup().await else {
        return;
    };

    let view = view_store
        .create_view(
            CreateViewRequest {
                name: "Temporary".into(),
                kind: ViewKind::Gallery,
                table_entity_type: "Photo".into(),
                filters: vec![],
                sorts: vec![],
                field_order: vec![],
                hidden_fields: vec![],
                group_by: None,
                kanban_field: None,
                calendar_field: None,
                color_field: None,
                row_height: None,
            },
            Uuid::new_v4(),
        )
        .await
        .expect("create");

    // Delete the view.
    view_store
        .delete_view(view.id)
        .await
        .expect("delete should succeed");

    // Verify it's gone.
    let result = view_store.get_view(view.id).await;
    assert!(result.is_err(), "view should not exist after delete");

    cleanup_view_triples(&pool, view.id).await;
}

#[tokio::test]
async fn test_view_delete_nonexistent() {
    let Some((_pool, _store, view_store)) = setup().await else {
        return;
    };

    let result = view_store.delete_view(Uuid::new_v4()).await;
    assert!(result.is_err(), "deleting nonexistent view should error");
}
