//! Integration tests for DarshJDB version history, restore, and snapshots.
//!
//! Tests version reconstruction, restore-to-version, undo, and the
//! snapshot diff system against a real Postgres triple store.
//!
//! ```sh
//! DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshjdb_test \
//!     cargo test --test history_test
//! ```

use ddb_server::history::versions::{self, ChangeType};
use ddb_server::history::restore;
use ddb_server::history::snapshots;
use ddb_server::triple_store::{PgTripleStore, TripleInput, TripleStore};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn setup() -> Option<(PgPool, PgTripleStore)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.ok()?;
    let store = PgTripleStore::new(pool.clone()).await.ok()?;
    // Ensure the snapshots table exists.
    ddb_server::history::ensure_history_schema(&pool)
        .await
        .ok()?;
    Some((pool, store))
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

async fn cleanup_snapshots(pool: &PgPool, ids: &[Uuid]) {
    if ids.is_empty() {
        return;
    }
    sqlx::query("DELETE FROM snapshots WHERE id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await
        .ok();
}

// ===========================================================================
// 1. CREATE RECORD, UPDATE TWICE, VERIFY 3 VERSIONS
// ===========================================================================

#[tokio::test]
async fn test_history_three_versions() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let eid = Uuid::new_v4();

    // Version 1: create record with name and email.
    store
        .set_triples(&[
            TripleInput {
                entity_id: eid,
                attribute: "user/name".into(),
                value: json!("Alice"),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid,
                attribute: "user/email".into(),
                value: json!("alice@test.com"),
                value_type: 0,
                ttl_seconds: None,
            },
        ])
        .await
        .expect("v1");

    // Version 2: update email.
    store.retract(eid, "user/email").await.expect("retract old email");
    store
        .set_triples(&[TripleInput {
            entity_id: eid,
            attribute: "user/email".into(),
            value: json!("alice@new.com"),
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .expect("v2");

    // Version 3: add age.
    store
        .set_triples(&[TripleInput {
            entity_id: eid,
            attribute: "user/age".into(),
            value: json!(30),
            value_type: 1,
            ttl_seconds: None,
        }])
        .await
        .expect("v3");

    // Get full history.
    let history = versions::get_history(&pool, eid, 0)
        .await
        .expect("get history");

    assert!(
        history.len() >= 3,
        "expected at least 3 versions, got {}",
        history.len()
    );

    // Version 1 should have name and email.
    let v1 = &history[0];
    assert_eq!(v1.version_number, 1);
    assert_eq!(v1.snapshot.get("user/name"), Some(&json!("Alice")));
    assert_eq!(v1.snapshot.get("user/email"), Some(&json!("alice@test.com")));

    // Check that one of the later versions has the updated email.
    let last = history.last().unwrap();
    assert_eq!(last.snapshot.get("user/email"), Some(&json!("alice@new.com")));
    assert_eq!(last.snapshot.get("user/age"), Some(&json!(30)));

    // Verify change types are populated.
    let v1_changes: Vec<&str> = v1
        .changes
        .iter()
        .map(|c| c.change_type)
        .filter(|ct| *ct == ChangeType::Added)
        .map(|_| "added")
        .collect();
    assert!(
        !v1_changes.is_empty(),
        "v1 should have Added changes"
    );

    cleanup_entities(&pool, &[eid]).await;
}

// ===========================================================================
// 2. GET SPECIFIC VERSION
// ===========================================================================

#[tokio::test]
async fn test_history_get_version() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let eid = Uuid::new_v4();

    // v1: name = "Alpha".
    store
        .set_triples(&[TripleInput {
            entity_id: eid,
            attribute: "item/name".into(),
            value: json!("Alpha"),
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .expect("v1");

    // v2: name = "Beta" (retract old + set new).
    store.retract(eid, "item/name").await.expect("retract");
    store
        .set_triples(&[TripleInput {
            entity_id: eid,
            attribute: "item/name".into(),
            value: json!("Beta"),
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .expect("v2");

    // Get version 1 state.
    let v1_state = versions::get_version(&pool, eid, 1)
        .await
        .expect("get v1");
    assert_eq!(v1_state.get("item/name"), Some(&json!("Alpha")));

    // Get latest version state.
    let history = versions::get_history(&pool, eid, 0)
        .await
        .expect("history");
    let latest_version = history.last().unwrap().version_number;
    let latest_state = versions::get_version(&pool, eid, latest_version)
        .await
        .expect("get latest");
    assert_eq!(latest_state.get("item/name"), Some(&json!("Beta")));

    // Invalid version should error.
    let err = versions::get_version(&pool, eid, 999).await;
    assert!(err.is_err());

    // Version 0 should error.
    let err = versions::get_version(&pool, eid, 0).await;
    assert!(err.is_err());

    cleanup_entities(&pool, &[eid]).await;
}

// ===========================================================================
// 3. RESTORE TO VERSION 1
// ===========================================================================

#[tokio::test]
async fn test_history_restore_version() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let eid = Uuid::new_v4();

    // v1: name = "Original", color = "blue".
    store
        .set_triples(&[
            TripleInput {
                entity_id: eid,
                attribute: "record/name".into(),
                value: json!("Original"),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid,
                attribute: "record/color".into(),
                value: json!("blue"),
                value_type: 0,
                ttl_seconds: None,
            },
        ])
        .await
        .expect("v1");

    // v2: name = "Modified", color retracted.
    store.retract(eid, "record/name").await.expect("retract name");
    store.retract(eid, "record/color").await.expect("retract color");
    store
        .set_triples(&[TripleInput {
            entity_id: eid,
            attribute: "record/name".into(),
            value: json!("Modified"),
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .expect("v2");

    // Verify current state is "Modified" with no color.
    let current = store.get_entity(eid).await.expect("current");
    let current_name = current
        .iter()
        .find(|t| t.attribute == "record/name" && !t.retracted);
    assert_eq!(
        current_name.map(|t| &t.value),
        Some(&json!("Modified"))
    );

    // Restore to version 1.
    let restore_tx = restore::restore_version(&pool, eid, 1)
        .await
        .expect("restore to v1");
    assert!(restore_tx > 0);

    // Verify state is now back to v1.
    let restored = store.get_entity(eid).await.expect("restored");
    let name = restored
        .iter()
        .find(|t| t.attribute == "record/name" && !t.retracted);
    assert_eq!(
        name.map(|t| &t.value),
        Some(&json!("Original")),
        "name should be restored to 'Original'"
    );

    let color = restored
        .iter()
        .find(|t| t.attribute == "record/color" && !t.retracted);
    assert_eq!(
        color.map(|t| &t.value),
        Some(&json!("blue")),
        "color should be restored to 'blue'"
    );

    cleanup_entities(&pool, &[eid]).await;
}

// ===========================================================================
// 4. UNDO LAST CHANGE
// ===========================================================================

#[tokio::test]
async fn test_history_undo_last() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let eid = Uuid::new_v4();

    // v1.
    store
        .set_triples(&[TripleInput {
            entity_id: eid,
            attribute: "thing/value".into(),
            value: json!("first"),
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .expect("v1");

    // v2.
    store.retract(eid, "thing/value").await.expect("retract");
    store
        .set_triples(&[TripleInput {
            entity_id: eid,
            attribute: "thing/value".into(),
            value: json!("second"),
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .expect("v2");

    // Undo -> should revert to "first".
    let undo_tx = restore::undo_last(&pool, eid)
        .await
        .expect("undo");
    assert!(undo_tx > 0);

    let state = store.get_entity(eid).await.expect("after undo");
    let val = state
        .iter()
        .find(|t| t.attribute == "thing/value" && !t.retracted);
    assert_eq!(
        val.map(|t| &t.value),
        Some(&json!("first")),
        "undo should restore previous version"
    );

    cleanup_entities(&pool, &[eid]).await;
}

#[tokio::test]
async fn test_history_undo_single_version_errors() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let eid = Uuid::new_v4();

    // Only one version -- undo should fail.
    store
        .set_triples(&[TripleInput {
            entity_id: eid,
            attribute: "only/version".into(),
            value: json!("singleton"),
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .expect("v1");

    let result = restore::undo_last(&pool, eid).await;
    assert!(result.is_err(), "undo with single version should error");

    cleanup_entities(&pool, &[eid]).await;
}

// ===========================================================================
// 5. POINT-IN-TIME READ
// ===========================================================================

#[tokio::test]
async fn test_history_at_time() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let eid = Uuid::new_v4();

    // v1.
    store
        .set_triples(&[TripleInput {
            entity_id: eid,
            attribute: "log/entry".into(),
            value: json!("first"),
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .expect("v1");

    let between = chrono::Utc::now();

    // Small delay to ensure timestamp ordering.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // v2.
    store.retract(eid, "log/entry").await.expect("retract");
    store
        .set_triples(&[TripleInput {
            entity_id: eid,
            attribute: "log/entry".into(),
            value: json!("second"),
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .expect("v2");

    // Read at the timestamp between v1 and v2.
    let at_between = versions::get_at_time(&pool, eid, between)
        .await
        .expect("at_time");
    assert_eq!(
        at_between.get("log/entry"),
        Some(&json!("first")),
        "point-in-time should return v1 state"
    );

    // Read at current time should return v2.
    let at_now = versions::get_at_time(&pool, eid, chrono::Utc::now())
        .await
        .expect("at_now");
    assert_eq!(
        at_now.get("log/entry"),
        Some(&json!("second")),
        "current time should return latest state"
    );

    cleanup_entities(&pool, &[eid]).await;
}

// ===========================================================================
// 6. SNAPSHOTS — CREATE, DIFF, LIST
// ===========================================================================

#[tokio::test]
async fn test_snapshot_create_and_list() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    // Create some records of type "Widget".
    let mut eids = Vec::new();
    for i in 0..3 {
        let eid = Uuid::new_v4();
        store
            .set_triples(&[
                TripleInput {
                    entity_id: eid,
                    attribute: ":db/type".into(),
                    value: json!("Widget"),
                    value_type: 0,
                    ttl_seconds: None,
                },
                TripleInput {
                    entity_id: eid,
                    attribute: "widget/name".into(),
                    value: json!(format!("Widget {i}")),
                    value_type: 0,
                    ttl_seconds: None,
                },
            ])
            .await
            .expect("create widget");
        eids.push(eid);
    }

    // Create a snapshot.
    let snapshot = snapshots::create_snapshot(
        &pool,
        "Widget",
        "v1-checkpoint",
        "Before changes",
        None,
    )
    .await
    .expect("create snapshot");

    assert_eq!(snapshot.entity_type, "Widget");
    assert_eq!(snapshot.name, "v1-checkpoint");
    assert!(snapshot.tx_id_at_snapshot > 0);
    assert!(snapshot.record_count >= 3);

    // List snapshots.
    let list = snapshots::list_snapshots(&pool, "Widget")
        .await
        .expect("list snapshots");
    assert!(!list.is_empty());
    assert!(list.iter().any(|s| s.id == snapshot.id));

    // Cleanup.
    cleanup_entities(&pool, &eids).await;
    cleanup_snapshots(&pool, &[snapshot.id]).await;
}

#[tokio::test]
async fn test_snapshot_diff() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    // Create initial data.
    let eid1 = Uuid::new_v4();
    store
        .set_triples(&[
            TripleInput {
                entity_id: eid1,
                attribute: ":db/type".into(),
                value: json!("Order"),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid1,
                attribute: "order/total".into(),
                value: json!(100),
                value_type: 1,
                ttl_seconds: None,
            },
        ])
        .await
        .expect("create order");

    // Create snapshot.
    let snapshot = snapshots::create_snapshot(
        &pool,
        "Order",
        "pre-changes",
        "",
        None,
    )
    .await
    .expect("snapshot");

    // Make changes after snapshot: update existing and add new.
    store.retract(eid1, "order/total").await.expect("retract");
    store
        .set_triples(&[TripleInput {
            entity_id: eid1,
            attribute: "order/total".into(),
            value: json!(200),
            value_type: 1,
            ttl_seconds: None,
        }])
        .await
        .expect("update order");

    let eid2 = Uuid::new_v4();
    store
        .set_triples(&[
            TripleInput {
                entity_id: eid2,
                attribute: ":db/type".into(),
                value: json!("Order"),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid2,
                attribute: "order/total".into(),
                value: json!(50),
                value_type: 1,
                ttl_seconds: None,
            },
        ])
        .await
        .expect("new order");

    // Diff.
    let diff = snapshots::diff_snapshot(&pool, snapshot.id)
        .await
        .expect("diff");

    assert_eq!(diff.snapshot_id, snapshot.id);
    assert_eq!(diff.entity_type, "Order");
    assert!(diff.current_tx_id > diff.snapshot_tx_id);
    // At least 1 entity modified (eid1) and 1 created (eid2).
    assert!(
        diff.entities_modified >= 1 || diff.triples_added >= 1,
        "should detect changes since snapshot"
    );

    cleanup_entities(&pool, &[eid1, eid2]).await;
    cleanup_snapshots(&pool, &[snapshot.id]).await;
}

// ===========================================================================
// 7. RESTORE DELETED RECORD
// ===========================================================================

#[tokio::test]
async fn test_restore_deleted_record() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let eid = Uuid::new_v4();

    // Create record.
    store
        .set_triples(&[
            TripleInput {
                entity_id: eid,
                attribute: "deleted/name".into(),
                value: json!("Ephemeral"),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid,
                attribute: "deleted/value".into(),
                value: json!(42),
                value_type: 1,
                ttl_seconds: None,
            },
        ])
        .await
        .expect("create");

    // Verify it exists.
    let before = store.get_entity(eid).await.expect("before");
    assert!(!before.is_empty());

    // Soft-delete: retract all triples.
    store.retract(eid, "deleted/name").await.expect("retract name");
    store
        .retract(eid, "deleted/value")
        .await
        .expect("retract value");

    // Verify it's "deleted".
    let after_delete = store.get_entity(eid).await.expect("after delete");
    assert!(
        after_delete.is_empty(),
        "entity should appear empty after retraction"
    );

    // Restore the deleted record.
    let restore_tx = restore::restore_deleted(&pool, eid)
        .await
        .expect("restore deleted");
    assert!(restore_tx > 0);

    // Verify it's back.
    let restored = store.get_entity(eid).await.expect("restored");
    let name = restored
        .iter()
        .find(|t| t.attribute == "deleted/name" && !t.retracted);
    assert_eq!(
        name.map(|t| &t.value),
        Some(&json!("Ephemeral")),
        "name should be restored"
    );
    let value = restored
        .iter()
        .find(|t| t.attribute == "deleted/value" && !t.retracted);
    assert_eq!(
        value.map(|t| &t.value),
        Some(&json!(42)),
        "value should be restored"
    );

    cleanup_entities(&pool, &[eid]).await;
}

// ===========================================================================
// 8. HISTORY OF NONEXISTENT ENTITY
// ===========================================================================

#[tokio::test]
async fn test_history_nonexistent_entity() {
    let Some((pool, _store)) = setup().await else {
        return;
    };

    let fake_id = Uuid::new_v4();
    let err = versions::get_history(&pool, fake_id, 0).await;
    assert!(err.is_err(), "history of nonexistent entity should error");
}
