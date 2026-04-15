// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//
//! v0.3.2.1 — end-to-end integration test for the SqliteStore::query
//! path against an in-memory rusqlite database.
//!
//! Validates the full chain:
//!   1. open(":memory:")          — schema migration applies
//!   2. set_triples(...)          — round-trips bind / value encoding
//!   3. plan_query_with_dialect   — emits SqliteDialect-flavoured SQL
//!      (json_quote, ?N placeholders)
//!   4. store.query(plan)         — executes via rusqlite, materialises
//!      results as the same QueryResultRow JSON shape PgStore returns
//!
//! The v0.3.2 ship explicitly deferred this — SqliteStore::query
//! returned InvalidQuery for any plan. v0.3.2.1 closes that gap, and
//! this file is the regression net.

#![cfg(feature = "sqlite-store")]

use ddb_server::query::dialect::SqliteDialect;
use ddb_server::query::{
    OrderClause, QueryAST, SortDirection, WhereClause, WhereOp, plan_query_with_dialect,
};
use ddb_server::store::Store;
use ddb_server::store::sqlite::SqliteStore;
use ddb_server::triple_store::TripleInput;
use ddb_server::triple_store::schema::ValueType;
use uuid::Uuid;

fn triple(entity: Uuid, attr: &str, val: serde_json::Value) -> TripleInput {
    TripleInput {
        entity_id: entity,
        attribute: attr.to_string(),
        value: val,
        value_type: ValueType::String as i16,
        ttl_seconds: None,
    }
}

async fn seed_users(store: &SqliteStore, count: usize) -> Vec<Uuid> {
    let tx = store.next_tx_id().await.expect("next_tx_id");
    let mut entities = Vec::with_capacity(count);
    let mut batch = Vec::with_capacity(count * 3);
    for i in 0..count {
        let e = Uuid::new_v4();
        entities.push(e);
        batch.push(triple(e, ":db/type", serde_json::json!("user")));
        batch.push(triple(
            e,
            "user/email",
            serde_json::json!(format!("user{i}@example.com")),
        ));
        batch.push(triple(
            e,
            "user/name",
            serde_json::json!(format!("User {i}")),
        ));
    }
    store.set_triples(tx, &batch).await.expect("set_triples");
    entities
}

#[tokio::test]
async fn select_all_users_via_dialect_planner_and_sqlite_store() {
    let store = SqliteStore::open(":memory:").expect("open in-memory");
    let _entities = seed_users(&store, 3).await;

    let ast = QueryAST {
        entity_type: "user".to_string(),
        where_clauses: vec![],
        order: vec![],
        limit: None,
        offset: None,
        nested: vec![],
        search: None,
        semantic: None,
        hybrid: None,
    };
    let plan = plan_query_with_dialect(&ast, &SqliteDialect).expect("plan");
    let rows = store.query(&plan).await.expect("query");

    assert_eq!(rows.len(), 3, "three users seeded → three rows");
    for row in &rows {
        let obj = row.as_object().expect("row is JSON object");
        assert!(obj.contains_key("entity_id"), "row exposes entity_id");
        let attrs = obj
            .get("attributes")
            .and_then(|v| v.as_object())
            .expect("attributes object");
        assert!(attrs.contains_key("user/email"));
        assert!(attrs.contains_key("user/name"));
    }
}

#[tokio::test]
async fn select_with_eq_where_filters_to_one_row() {
    let store = SqliteStore::open(":memory:").expect("open");
    let _entities = seed_users(&store, 5).await;

    let ast = QueryAST {
        entity_type: "user".to_string(),
        where_clauses: vec![WhereClause {
            attribute: "user/email".to_string(),
            op: WhereOp::Eq,
            value: serde_json::json!("user2@example.com"),
        }],
        order: vec![],
        limit: None,
        offset: None,
        nested: vec![],
        search: None,
        semantic: None,
        hybrid: None,
    };
    let plan = plan_query_with_dialect(&ast, &SqliteDialect).expect("plan");
    let rows = store.query(&plan).await.expect("query");

    assert_eq!(rows.len(), 1, "exactly user2 matches");
    let attrs = rows[0]
        .get("attributes")
        .and_then(|v| v.as_object())
        .expect("attributes");
    assert_eq!(
        attrs.get("user/email").unwrap(),
        &serde_json::json!("user2@example.com")
    );
}

#[tokio::test]
async fn select_with_limit_offset_paginates_after_grouping() {
    let store = SqliteStore::open(":memory:").expect("open");
    let _entities = seed_users(&store, 10).await;

    // Page 2 of size 3 (offset 3, limit 3) — should return 3 rows.
    let ast = QueryAST {
        entity_type: "user".to_string(),
        where_clauses: vec![],
        order: vec![],
        limit: Some(3),
        offset: Some(3),
        nested: vec![],
        search: None,
        semantic: None,
        hybrid: None,
    };
    let plan = plan_query_with_dialect(&ast, &SqliteDialect).expect("plan");
    let rows = store.query(&plan).await.expect("query");
    assert_eq!(rows.len(), 3, "limit=3 honoured after offset=3");
}

#[tokio::test]
async fn select_with_order_by_attribute_runs_correlated_subquery() {
    let store = SqliteStore::open(":memory:").expect("open");
    let _entities = seed_users(&store, 4).await;

    let ast = QueryAST {
        entity_type: "user".to_string(),
        where_clauses: vec![],
        order: vec![OrderClause {
            attribute: "user/email".to_string(),
            direction: SortDirection::Asc,
        }],
        limit: None,
        offset: None,
        nested: vec![],
        search: None,
        semantic: None,
        hybrid: None,
    };
    let plan = plan_query_with_dialect(&ast, &SqliteDialect).expect("plan");
    let rows = store.query(&plan).await.expect("query");
    assert_eq!(rows.len(), 4, "all rows returned with ORDER BY active");
}

#[tokio::test]
async fn select_with_neq_where_excludes_matching_row() {
    let store = SqliteStore::open(":memory:").expect("open");
    let _entities = seed_users(&store, 3).await;

    let ast = QueryAST {
        entity_type: "user".to_string(),
        where_clauses: vec![WhereClause {
            attribute: "user/email".to_string(),
            op: WhereOp::Neq,
            value: serde_json::json!("user1@example.com"),
        }],
        order: vec![],
        limit: None,
        offset: None,
        nested: vec![],
        search: None,
        semantic: None,
        hybrid: None,
    };
    let plan = plan_query_with_dialect(&ast, &SqliteDialect).expect("plan");
    let rows = store.query(&plan).await.expect("query");
    assert_eq!(rows.len(), 2, "user1 excluded by != filter");
    for r in &rows {
        let attrs = r.get("attributes").and_then(|v| v.as_object()).unwrap();
        let email = attrs.get("user/email").unwrap().as_str().unwrap();
        assert_ne!(email, "user1@example.com");
    }
}

#[tokio::test]
async fn select_returns_empty_when_no_entities_match_type() {
    let store = SqliteStore::open(":memory:").expect("open");
    let _entities = seed_users(&store, 2).await;

    let ast = QueryAST {
        entity_type: "ghost".to_string(), // no triples of this type
        where_clauses: vec![],
        order: vec![],
        limit: None,
        offset: None,
        nested: vec![],
        search: None,
        semantic: None,
        hybrid: None,
    };
    let plan = plan_query_with_dialect(&ast, &SqliteDialect).expect("plan");
    let rows = store.query(&plan).await.expect("query");
    assert!(rows.is_empty(), "no users-of-type-ghost → empty result");
}
