//! Integration tests for the DarshJDB automation engine.
//!
//! Tests trigger evaluation, event bus publishing, and the full
//! mutation -> trigger -> action pipeline against a real Postgres
//! triple store.
//!
//! ```sh
//! DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshjdb_test \
//!     cargo test --test automation_test
//! ```

use std::collections::HashMap;

use ddb_server::automations::event_bus::{DdbEvent, EventBus};
use ddb_server::automations::trigger::{TriggerConfig, TriggerEvaluator, TriggerKind};
use ddb_server::automations::action::{ActionConfig, ActionKind};
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

// ===========================================================================
// 1. CREATE AUTOMATION WITH TRIGGER + ACTIONS
// ===========================================================================

#[tokio::test]
async fn test_trigger_config_create() {
    // Trigger configs can be created without a database.
    let trigger = TriggerConfig::new(TriggerKind::OnRecordCreate, "Task");

    assert!(trigger.enabled);
    assert_eq!(trigger.table_entity_type, "Task");
    assert_eq!(trigger.kind, TriggerKind::OnRecordCreate);
    assert!(trigger.condition.is_none());
}

#[tokio::test]
async fn test_action_config_create() {
    let action = ActionConfig::new(
        ActionKind::CreateRecord,
        json!({
            "entity_type": "AuditLog",
            "data": {
                "action": "task_created",
                "timestamp": "2026-04-07T00:00:00Z"
            }
        }),
    );

    assert_eq!(action.kind, ActionKind::CreateRecord);
    assert_eq!(action.timeout_ms, 30_000);
    assert!(action.config["entity_type"] == "AuditLog");
}

#[tokio::test]
async fn test_trigger_serde_roundtrip() {
    let trigger = TriggerConfig::new(TriggerKind::OnRecordUpdate, "Contact");

    let json = serde_json::to_string(&trigger).expect("serialize");
    let restored: TriggerConfig =
        serde_json::from_str(&json).expect("deserialize");

    assert_eq!(restored.kind, TriggerKind::OnRecordUpdate);
    assert_eq!(restored.table_entity_type, "Contact");
    assert!(restored.enabled);
}

// ===========================================================================
// 2. TRIGGER EVALUATOR — MATCH EVENTS AGAINST TRIGGERS
// ===========================================================================

#[tokio::test]
async fn test_trigger_evaluator_record_create() {
    let trigger = TriggerConfig::new(TriggerKind::OnRecordCreate, "Task");
    let evaluator = TriggerEvaluator::new(vec![trigger.clone()]);

    // Matching event: record created for "Task".
    let event = DdbEvent::RecordCreated {
        entity_id: Uuid::new_v4(),
        entity_type: "Task".into(),
        attributes: HashMap::new(),
        tx_id: 1,
    };

    let matched = evaluator.evaluate(&event);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0], trigger.id);
}

#[tokio::test]
async fn test_trigger_evaluator_no_match_wrong_type() {
    let trigger = TriggerConfig::new(TriggerKind::OnRecordCreate, "Task");
    let evaluator = TriggerEvaluator::new(vec![trigger]);

    // Non-matching event: record created for "Contact" (not "Task").
    let event = DdbEvent::RecordCreated {
        entity_id: Uuid::new_v4(),
        entity_type: "Contact".into(),
        attributes: HashMap::new(),
        tx_id: 1,
    };

    let matched = evaluator.evaluate(&event);
    assert!(matched.is_empty(), "should not match different entity type");
}

#[tokio::test]
async fn test_trigger_evaluator_record_update() {
    let trigger = TriggerConfig::new(TriggerKind::OnRecordUpdate, "Task");
    let evaluator = TriggerEvaluator::new(vec![trigger.clone()]);

    let event = DdbEvent::RecordUpdated {
        entity_id: Uuid::new_v4(),
        entity_type: "Task".into(),
        attributes: HashMap::new(),
        changed_fields: vec!["task/status".into()],
        tx_id: 2,
    };

    let matched = evaluator.evaluate(&event);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0], trigger.id);
}

#[tokio::test]
async fn test_trigger_evaluator_record_delete() {
    let trigger = TriggerConfig::new(TriggerKind::OnRecordDelete, "Task");
    let evaluator = TriggerEvaluator::new(vec![trigger.clone()]);

    let event = DdbEvent::RecordDeleted {
        entity_id: Uuid::new_v4(),
        entity_type: "Task".into(),
        tx_id: 3,
    };

    let matched = evaluator.evaluate(&event);
    assert_eq!(matched.len(), 1);
}

#[tokio::test]
async fn test_trigger_evaluator_field_change() {
    let trigger = TriggerConfig {
        kind: TriggerKind::OnFieldChange {
            field: "task/status".into(),
        },
        ..TriggerConfig::new(TriggerKind::OnRecordCreate, "Task")
    };
    let evaluator = TriggerEvaluator::new(vec![trigger.clone()]);

    // Matching: field "task/status" changed.
    let event = DdbEvent::FieldChanged {
        entity_id: Uuid::new_v4(),
        entity_type: "Task".into(),
        field_name: "task/status".into(),
        old_value: Some(json!("open")),
        new_value: json!("closed"),
        attributes: HashMap::new(),
        tx_id: 4,
    };

    let matched = evaluator.evaluate(&event);
    assert_eq!(matched.len(), 1);

    // Non-matching: different field changed.
    let event_other = DdbEvent::FieldChanged {
        entity_id: Uuid::new_v4(),
        entity_type: "Task".into(),
        field_name: "task/title".into(),
        old_value: Some(json!("Old")),
        new_value: json!("New"),
        attributes: HashMap::new(),
        tx_id: 5,
    };

    let matched_other = evaluator.evaluate(&event_other);
    assert!(
        matched_other.is_empty(),
        "should not match different field name"
    );
}

#[tokio::test]
async fn test_trigger_evaluator_disabled() {
    let mut trigger = TriggerConfig::new(TriggerKind::OnRecordCreate, "Task");
    trigger.enabled = false;
    let evaluator = TriggerEvaluator::new(vec![trigger]);

    let event = DdbEvent::RecordCreated {
        entity_id: Uuid::new_v4(),
        entity_type: "Task".into(),
        attributes: HashMap::new(),
        tx_id: 1,
    };

    let matched = evaluator.evaluate(&event);
    assert!(matched.is_empty(), "disabled trigger should not fire");
}

#[tokio::test]
async fn test_trigger_evaluator_multiple_triggers() {
    let t1 = TriggerConfig::new(TriggerKind::OnRecordCreate, "Task");
    let t2 = TriggerConfig::new(TriggerKind::OnRecordCreate, "Task");
    let t3 = TriggerConfig::new(TriggerKind::OnRecordCreate, "Contact");
    let evaluator = TriggerEvaluator::new(vec![t1.clone(), t2.clone(), t3]);

    let event = DdbEvent::RecordCreated {
        entity_id: Uuid::new_v4(),
        entity_type: "Task".into(),
        attributes: HashMap::new(),
        tx_id: 1,
    };

    let matched = evaluator.evaluate(&event);
    // t1 and t2 should match, t3 should not.
    assert_eq!(matched.len(), 2);
    assert!(matched.contains(&t1.id));
    assert!(matched.contains(&t2.id));
}

// ===========================================================================
// 3. TRIGGER EVALUATOR — ADD/REMOVE
// ===========================================================================

#[tokio::test]
async fn test_trigger_evaluator_add_remove() {
    let mut evaluator = TriggerEvaluator::new(vec![]);

    let trigger = TriggerConfig::new(TriggerKind::OnRecordCreate, "Task");
    let tid = trigger.id;

    // Initially no triggers.
    let event = DdbEvent::RecordCreated {
        entity_id: Uuid::new_v4(),
        entity_type: "Task".into(),
        attributes: HashMap::new(),
        tx_id: 1,
    };
    assert!(evaluator.evaluate(&event).is_empty());

    // Add trigger.
    evaluator.add_trigger(trigger);
    assert_eq!(evaluator.evaluate(&event).len(), 1);

    // Remove trigger.
    evaluator.remove_trigger(tid);
    assert!(evaluator.evaluate(&event).is_empty());
}

// ===========================================================================
// 4. EVENT BUS — PUBLISH AND SUBSCRIBE
// ===========================================================================

#[tokio::test]
async fn test_event_bus_publish_subscribe() {
    let bus = EventBus::new(64, 100);
    let mut subscriber = bus.subscribe();

    let event = DdbEvent::RecordCreated {
        entity_id: Uuid::new_v4(),
        entity_type: "Task".into(),
        attributes: HashMap::new(),
        tx_id: 42,
    };

    bus.emit(event.clone()).await;

    // Subscriber should receive the event.
    let received = subscriber.recv().await.expect("should receive event");
    assert_eq!(received.entity_type(), Some("Task"));
    assert_eq!(received.tx_id(), Some(42));
}

#[tokio::test]
async fn test_event_bus_log() {
    let bus = EventBus::new(64, 100);

    // Emit several events.
    for i in 0..5 {
        let event = DdbEvent::RecordCreated {
            entity_id: Uuid::new_v4(),
            entity_type: "Task".into(),
            attributes: HashMap::new(),
            tx_id: i,
        };
        bus.emit(event).await;
    }

    // Check the log.
    let recent = bus.recent_events(3).await;
    assert_eq!(recent.len(), 3);
    // Most recent should be last.
    assert_eq!(recent[2].event.tx_id(), Some(4));
}

// ===========================================================================
// 5. SIMULATE MUTATION THAT FIRES TRIGGER
// ===========================================================================

#[tokio::test]
async fn test_mutation_fires_trigger() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    // Set up a trigger that fires on Task creation.
    let trigger = TriggerConfig::new(TriggerKind::OnRecordCreate, "Task");
    let evaluator = TriggerEvaluator::new(vec![trigger.clone()]);

    // Set up event bus.
    let bus = EventBus::new(64, 100);
    let mut subscriber = bus.subscribe();

    // Simulate a record creation (write triples + emit event).
    let eid = Uuid::new_v4();
    let tx_id = store
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
                value: json!("Automated task"),
                value_type: 0,
                ttl_seconds: None,
            },
        ])
        .await
        .expect("create record");

    // Emit the event that would normally be emitted by the data layer.
    let mut attrs = HashMap::new();
    attrs.insert("task/title".into(), json!("Automated task"));
    let event = DdbEvent::RecordCreated {
        entity_id: eid,
        entity_type: "Task".into(),
        attributes: attrs,
        tx_id,
    };

    bus.emit(event.clone()).await;

    // Evaluate the trigger.
    let matched = evaluator.evaluate(&event);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0], trigger.id);

    // Subscriber should have received the event.
    let received = subscriber.recv().await.expect("should receive");
    assert_eq!(received.entity_id(), Some(eid));

    cleanup_entities(&pool, &[eid]).await;
}

// ===========================================================================
// 6. ACTION KIND SERIALIZATION
// ===========================================================================

#[tokio::test]
async fn test_action_kind_serde() {
    let kinds = [
        ActionKind::CreateRecord,
        ActionKind::UpdateRecord,
        ActionKind::DeleteRecord,
        ActionKind::SendWebhook,
        ActionKind::SendEmail,
        ActionKind::RunFunction,
        ActionKind::SetFieldValue,
        ActionKind::AddToView,
        ActionKind::Notify,
        ActionKind::Custom {
            name: "my_action".into(),
        },
    ];

    for kind in &kinds {
        let json = serde_json::to_value(kind).expect("serialize");
        let restored: ActionKind =
            serde_json::from_value(json).expect("deserialize");
        assert_eq!(&restored, kind);
    }
}

// ===========================================================================
// 7. EVENT ENTITY HELPERS
// ===========================================================================

#[tokio::test]
async fn test_ddb_event_accessors() {
    let eid = Uuid::new_v4();

    let event = DdbEvent::RecordUpdated {
        entity_id: eid,
        entity_type: "Contact".into(),
        attributes: HashMap::new(),
        changed_fields: vec!["email".into()],
        tx_id: 99,
    };

    assert_eq!(event.entity_type(), Some("Contact"));
    assert_eq!(event.entity_id(), Some(eid));
    assert_eq!(event.tx_id(), Some(99));

    // Auth events have no entity.
    let auth = DdbEvent::AuthEvent {
        user_id: "u123".into(),
        action: "login".into(),
        metadata: HashMap::new(),
    };
    assert!(auth.entity_type().is_none());
    assert!(auth.entity_id().is_none());
    assert!(auth.tx_id().is_none());
}
